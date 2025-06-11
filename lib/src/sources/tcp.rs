use std::net::SocketAddr;

use log::trace;
use mockall_double::double;

use crate::ToConfig;
use crate::errors::{DataSourceError, TaskError};
use crate::network::rpc_message::{DataMsg, RpcMessage, RpcMessageKind};
#[double]
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
pub struct TCPSource {
    /// Client from which to read
    client: TcpClient,
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
            client: TcpClient::new(),
            config,
        })
    }
}

#[async_trait::async_trait]
impl DataSourceT for TCPSource {
    /// Starts the data source by connecting to the TCP server.
    ///
    /// # Errors
    /// Returns a [`DataSourceError`] if the connection fails.
    async fn start(&mut self) -> Result<(), DataSourceError> {
        trace!("Connecting to TCP socket at {}", self.config.target_addr);
        self.client
            .connect(self.config.target_addr)
            .await
            .map_err(|e| DataSourceError::from(Box::new(e)))?;
        Ok(())
    }

    /// Stops the data source by closing the TCP connection.
    ///
    /// # Errors
    /// Returns a [`DataSourceError`] if disconnection fails.
    async fn stop(&mut self) -> Result<(), DataSourceError> {
        trace!("Disconnecting from TCP socket at {}", self.config.target_addr);
        self.client
            .disconnect(self.config.target_addr)
            .await
            .map_err(|e| DataSourceError::from(Box::new(e)))?;
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
        let rpcmsg: RpcMessage = self
            .client
            .read_message(self.config.target_addr)
            .await
            .map_err(|e| DataSourceError::from(Box::new(e)))?;
        match rpcmsg.msg {
            RpcMessageKind::HostCtrl(_) | RpcMessageKind::RegCtrl(_) => Ok(None), // control messages carry no data payload
            RpcMessageKind::Data { data_msg, device_id } => {
                if device_id == self.config.device_id {
                    Ok(Some(data_msg)) // return the actual data payload
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
    use std::net::{IpAddr, Ipv4Addr};

    use mockall::predicate::*;

    use super::*;
    use crate::errors::DataSourceError;
    use crate::network::rpc_message::*;

    fn test_addr() -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 12345)
    }

    fn make_config() -> TCPConfig {
        TCPConfig {
            target_addr: test_addr(),
            device_id: 42,
        }
    }

    #[tokio::test]
    async fn test_new() {
        let config = make_config();
        let ctx = TcpClient::new_context();
        ctx.expect().returning(TcpClient::default);

        let mut source = TCPSource::new(config.clone());
        assert!(source.is_ok());

        let source = source.unwrap();
        assert_eq!(source.config.target_addr, config.target_addr);
        assert_eq!(source.config.device_id, config.device_id);
    }

    #[tokio::test]
    async fn test_start() {
        let mut mock = TcpClient::default();
        let config = make_config();

        mock.expect_connect().with(eq(config.target_addr)).returning(|_| Ok(()));

        let mut source = TCPSource {
            client: mock,
            config: make_config(),
        };
        let ret = source.start().await;
        assert!(ret.is_ok());
    }

    #[tokio::test]
    async fn test_stop() {
        let mut mock = TcpClient::default();
        let config = make_config();

        mock.expect_disconnect().with(eq(config.target_addr)).returning(|_| Ok(()));

        let mut source = TCPSource {
            client: mock,
            config: make_config(),
        };
        let ret = source.stop().await;
        assert!(ret.is_ok());
    }

    #[tokio::test]
    async fn test_read_buf() {
        let mut source = TCPSource {
            client: TcpClient::default(),
            config: make_config(),
        };

        let mut buf = [0u8; 8];
        assert!(matches!(source.read_buf(&mut buf).await.unwrap_err(), DataSourceError::ReadBuf));
    }

    #[tokio::test]
    async fn test_read() {
        let mut mock = TcpClient::default();
        let config = make_config();
        let dt_msg = DataMsg::RawFrame {
            ts: 0f64,
            bytes: vec![0u8, 8],
            source_type: SourceType::IWL5300,
        };

        let message = RpcMessage {
            msg: RpcMessageKind::Data {
                data_msg: dt_msg.clone(),
                device_id: config.device_id,
            },
            src_addr: test_addr(),
            target_addr: test_addr(),
        };

        mock.expect_read_message().with(eq(config.target_addr)).return_once(move |_| Ok(message));

        let mut source = TCPSource { client: mock, config };

        let ret = source.read().await;
        assert_eq!(ret.unwrap(), Some(dt_msg));
    }

    #[tokio::test]
    async fn test_read_wrong_did() {
        let mut mock = TcpClient::default();
        let config = make_config();
        let dt_msg = DataMsg::RawFrame {
            ts: 0f64,
            bytes: vec![0u8, 8],
            source_type: SourceType::IWL5300,
        };

        let message = RpcMessage {
            msg: RpcMessageKind::Data {
                data_msg: dt_msg,
                device_id: config.device_id - 2,
            },
            src_addr: test_addr(),
            target_addr: test_addr(),
        };

        mock.expect_read_message().returning(move |_| Ok(message.clone()));

        let mut source = TCPSource {
            client: mock,
            config: config.clone(),
        };
        let ret = source.read().await;
        assert!(ret.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_read_ctrl_message() {
        let config = make_config();

        let mut mock = TcpClient::default();
        mock.expect_read_message().returning(|_| {
            Ok(RpcMessage {
                msg: RpcMessageKind::HostCtrl(HostCtrl::Connect),
                src_addr: test_addr(),
                target_addr: test_addr(),
            })
        });

        let mut source = TCPSource { client: mock, config };

        let result = source.read().await;
        assert!(result.unwrap().is_none());
    }
}
