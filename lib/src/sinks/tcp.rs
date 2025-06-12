use std::net::SocketAddr;

use async_trait::async_trait;
use log::trace;
use mockall_double::double;

use crate::ToConfig;
use crate::errors::{SinkError, TaskError};
use crate::network::rpc_message::{DataMsg, RpcMessageKind};
#[double]
use crate::network::tcp::client::TcpClient;
use crate::sinks::{Sink, SinkConfig};

/// Configuration for a TCP sink.
///
/// This structure holds the target address and device ID used for sending data
/// over a TCP connection.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone,  PartialEq)]
pub struct TCPConfig {
    /// IP address and port of the target server.
    pub target_addr: SocketAddr,
    /// Unique identifier of the device sending the data.
    pub device_id: u64,
}

/// TCP-based data sink implementation.
///
/// This struct manages a TCP client connection and sends data messages
/// to a configured target address.
pub struct TCPSink {
    /// Tcp client that is going to be sending data
    client: TcpClient,
    /// Configuration
    config: TCPConfig,
}

impl TCPSink {
    /// Create a new `TCPSink` from a given configuration.
    ///
    /// Establishes a new internal TCP client and stores the configuration details.
    pub async fn new(config: TCPConfig) -> Result<Self, SinkError> {
        trace!("Creating new TCPSink for {}", config.target_addr);
        Ok(Self {
            client: TcpClient::new(),
            config,
        })
    }
}

#[async_trait]
impl Sink for TCPSink {
    /// Open the TCP connection to the target address.
    ///
    /// Attempts to connect using the internal TCP client.
    ///
    /// # Errors
    ///
    /// Returns a ['SinkError'] if the operation fails (e.g., I/O failure)
    async fn open(&mut self) -> Result<(), SinkError> {
        trace!("Connecting to TCP socket at {}", self.config.target_addr);
        self.client
            .connect(self.config.target_addr)
            .await
            .map_err(|e| SinkError::from(Box::new(e)))?;
        Ok(())
    }

    /// Close the TCP connection to the target address.
    ///
    /// Attempts to disconnect using the internal TCP client.
    ///
    /// # Errors
    ///
    /// Returns a ['SinkError'] if the operation fails (e.g., I/O failure)
    async fn close(&mut self) -> Result<(), SinkError> {
        trace!("Disconnecting from TCP socket at {}", self.config.target_addr);
        self.client
            .disconnect(self.config.target_addr)
            .await
            .map_err(|e| SinkError::from(Box::new(e)))?;
        Ok(())
    }

    /// Send a data message over the TCP connection.
    ///
    /// Wraps the message in an `RpcMessageKind::Data` and sends it to the target address.
    ///
    /// # Errors
    ///
    /// Returns a ['SinkError'] if the operation fails (e.g., I/O failure)
    async fn provide(&mut self, data: DataMsg) -> Result<(), SinkError> {
        let ret = RpcMessageKind::Data {
            data_msg: data,
            device_id: self.config.device_id,
        };

        self.client
            .send_message(self.config.target_addr, ret)
            .await
            .map_err(|e| SinkError::from(Box::new(e)))?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl ToConfig<SinkConfig> for TCPSink {
    /// Converts the current `TCPSink` instance into its configuration representation.
    ///
    /// This allows the runtime `TCPSink` to be serialized or stored as part of a broader configuration,
    /// such as when exporting to YAML or JSON. The method wraps the internal `TCPConfig` in a
    /// `SinkConfig::TCP` variant.
    ///
    /// # Returns
    /// - `Ok(SinkConfig::TCP)` containing the internal configuration of the `TCPSink`.
    /// - `Err(TaskError)` if any failure occurs during the conversion (though this implementation
    ///   does not currently produce an error).
    async fn to_config(&self) -> Result<SinkConfig, TaskError> {
        Ok(SinkConfig::Tcp(self.config.clone()))
    }
}


#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use mockall::predicate::*;

    use super::*;
    use crate::network::rpc_message::{DataMsg, RpcMessageKind, SourceType};

    fn test_addr() -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 23456)
    }

    fn make_config() -> TCPConfig {
        TCPConfig {
            target_addr: test_addr(),
            device_id: 42,
        }
    }

    #[tokio::test]
    async fn test_new_sink() {
        let config = make_config();
        let ctx = TcpClient::new_context();
        ctx.expect().returning(TcpClient::default);

        let sink = TCPSink::new(config.clone()).await.unwrap();
        assert_eq!(sink.config, config);
    }

    #[tokio::test]
    async fn test_open_sink() {
        let config = make_config();
        let mut mock = TcpClient::default();

        mock.expect_connect()
            .with(eq(config.target_addr))
            .returning(|_| Ok(()));

        let mut sink = TCPSink {
            client: mock,
            config,
        };

        let result = sink.open().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_close_sink() {
        let config = make_config();
        let mut mock = TcpClient::default();

        mock.expect_disconnect()
            .with(eq(config.target_addr))
            .returning(|_| Ok(()));

        let mut sink = TCPSink {
            client: mock,
            config,
        };

        let result = sink.close().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_provide_data() {
        let config = make_config();
        let data_msg = DataMsg::RawFrame {
            ts: 1.0,
            bytes: vec![1, 2, 3],
            source_type: SourceType::IWL5300,
        };

        let expected_message = RpcMessageKind::Data {
            data_msg: data_msg.clone(),
            device_id: config.device_id,
        };

        let mut mock = TcpClient::default();
        mock.expect_send_message()
            .with(eq(config.target_addr), eq(expected_message))
            .returning(|_, _| Ok(()));

        let mut sink = TCPSink {
            client: mock,
            config,
        };

        let result = sink.provide(data_msg).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_to_config() {
        let config = make_config();
        let sink = TCPSink {
            client: TcpClient::default(),
            config: config.clone(),
        };

        let conf = sink.to_config().await.unwrap();
        assert_eq!(conf, SinkConfig::Tcp(config));
    }
}
