use std::net::SocketAddr;

use log::trace;

use crate::ToConfig;
use crate::errors::{DataSourceError, TaskError};
use crate::network::rpc_message::{DataMsg, RpcMessage, RpcMessageKind};
use crate::network::tcp::client::TcpClient;
use crate::sources::{DataSourceConfig, DataSourceT};

/// Configuration for a `TCPSource`.
///
/// Contains the target TCP address to which the data source will connect.
#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
pub struct TCPConfig {
    /// source adress from which to read
    pub target_addr: SocketAddr,

    /// device id to which this is relevant
    pub device_id: u64,
}

/// TCP-based data source for receiving `DataMsg` payloads over a network.
///
/// Establishes a TCP connection to a remote data provider and reads serialized
/// `RpcMessage` values from the stream.
pub struct TCPSource  {
    /// Client from which to read
    client: Box<dyn TcpClientT>,
    /// Place to where to send
    config: TCPConfig,
}

impl TCPSource {
    /// Constructs a new `TCPSource` from the given configuration.
    ///
    /// # Arguments
    /// * `config` - Configuration object containing the target address.
    ///
    /// # Errors
    /// Returns a [`DataSourceError`] if construction fails (e.g., invalid config).
    pub fn new(config: TCPConfig) -> Result<Self, DataSourceError> {
        trace!("Creating new TCPSource for {}", config.target_addr);
        Ok(Self {
            client: Box::new(TcpClient::new()),
            config,
        })
    }
}

#[async_trait::async_trait]
impl ToConfig<DataSourceConfig> for TCPSource {
    /// Starts the data source by connecting to the TCP server.
    ///
    /// # Errors
    /// Returns a [`DataSourceError`] if the connection fails.
    async fn start(&mut self) -> Result<(), DataSourceError> {
        trace!("Connecting to TCP socket at {}", self.config.target_addr);
        self.client.connect(self.config.target_addr).await.map_err(DataSourceError::from)?;
        Ok(())
    }

    /// Stops the data source by closing the TCP connection.
    ///
    /// # Errors
    /// Returns a [`DataSourceError`] if disconnection fails.
    async fn stop(&mut self) -> Result<(), DataSourceError> {
        trace!("Disconnecting from TCP socket at {}", self.config.target_addr);
        self.client.disconnect(self.config.target_addr).await.map_err(DataSourceError::from)?;
        Ok(())
    }

    /// Attempts to read raw data into the given buffer.
    ///
    /// Currently unimplemented for TCP sources. Use [`read`] instead to access
    /// structured messages.
    ///
    /// # Errors
    /// Always returns [`DataSourceError::ReadBuf`] as this method is not supported.
    async fn read_buf(&mut self, _buf: &mut [u8]) -> Result<usize, DataSourceError> {
        // you can either proxy to the client or leave unimplemented
        Err(DataSourceError::ReadBuf)
    }

    /// Reads the next available data message from the TCP connection.
    ///
    /// Receives an `RpcMessage`, extracts the inner `DataMsg` if available,
    /// and discards control messages.
    ///
    /// # Returns
    /// * `Ok(Some(DataMsg))` if a valid data message was received.
    /// * `Ok(None)` if the message was a control message.
    ///
    /// # Errors
    /// Returns a [`DataSourceError`] if the TCP read or deserialization fails.
    async fn read(&mut self) -> Result<Option<DataMsg>, DataSourceError> {
        let rpcmsg: RpcMessage = self.client.read_message(self.config.target_addr).await.map_err(DataSourceError::from)?;

        match rpcmsg.msg {
            RpcMessageKind::Ctrl(_ctrl_msg) => {
                // control messages carry no data payload
                Ok(None)
            }
            RpcMessageKind::Data { data_msg, device_id } => {
                if device_id == self.config.device_id {
                    // return the actual data payload
                    Ok(Some(data_msg))
                } else {
                    Ok(None)
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl ToConfig<DataSourceConfig> for TCPSource {
    /// Converts this `NetlinkSource` instance into a `DataSourceConfig` representing a TCP source.
    ///
    /// **Note:** Although this is implemented on `NetlinkSource`, it returns a `DataSourceConfig::Tcp`
    /// variant constructed from the `target_addr` field. This might be intentional or a design choice
    /// depending on your application logic.
    ///
    /// # Returns
    ///
    /// * `Ok(DataSourceConfig::Tcp)` containing a `TCPConfig` initialized with the cloned target address.
    /// * `Err(TaskError)` if conversion fails (not expected here as cloning should succeed).
    async fn to_config(&self) -> Result<DataSourceConfig, TaskError> {
        Ok(DataSourceConfig::Tcp(TCPConfig {
            target_addr: self.config.target_addr,
            device_id: self.config.device_id,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::rpc_message::{RpcMessage, RpcMessageKind, DataMsg, SourceType};
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use mockall::predicate::*;
    use tokio;

    fn dummy_addr() -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 5555)
    }

    fn dummy_data_msg(device_id: u64) -> RpcMessage {
        RpcMessage {
            msg: RpcMessageKind::Data {
                device_id,
                data_msg: DataMsg::RawFrame {
                    ts: 0.0,
                    bytes: vec![1, 2, 3],
                    source_type: SourceType::IWL5300,
                },
            },
        }
    }

    #[tokio::test]
    async fn test_read_valid_data_msg() {
        let addr = dummy_addr();
        let mut mock = MockTcpClientT::new();
        let expected_msg = dummy_data_msg(42);

        mock.expect_connect()
            .with(eq(addr))
            .returning(|_| Ok(()));

        mock.expect_read_message()
            .with(eq(addr))
            .return_once(move |_| Ok(expected_msg.clone()));

        let config = TCPConfig { target_addr: addr, device_id: 42 };
        let mut source = TCPSource {
            client: mock,
            config,
        };

        let result = source.read().await.unwrap();
        assert!(matches!(result, Some(DataMsg::RawFrame { .. })));
    }

    #[tokio::test]
    async fn test_control_msg_returns_none() {
        let addr = dummy_addr();
        let mut mock = MockTcpClientT::new();
        let msg = RpcMessage {
            msg: RpcMessageKind::Ctrl("noop".into()),
        };

        mock.expect_read_message()
            .with(eq(addr))
            .return_once(move |_| Ok(msg));

        let config = TCPConfig { target_addr: addr, device_id: 1 };
        let mut source = TCPSource { client: mock, config };

        let result = source.read().await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_read_buf_returns_error() {
        let addr = dummy_addr();
        let mock = MockTcpClientT::new();
        let mut source = TCPSource {
            client: mock,
            config: TCPConfig { target_addr: addr, device_id: 1 },
        };

        let mut buf = vec![0; 16];
        let result = source.read_buf(&mut buf).await;
        assert!(matches!(result, Err(DataSourceError::ReadBuf)));
    }

    #[tokio::test]
    async fn test_to_config_roundtrip() {
        let addr = dummy_addr();
        let config = TCPConfig { target_addr: addr, device_id: 999 };
        let client = MockTcpClientT::new();

        let source = TCPSource { client, config: config.clone() };
        let roundtrip = source.to_config().await.unwrap();
        if let DataSourceConfig::Tcp(cfg) = roundtrip {
            assert_eq!(cfg.target_addr, config.target_addr);
            assert_eq!(cfg.device_id, config.device_id);
        } else {
            panic!("Expected DataSourceConfig::Tcp");
        }
    }
}
