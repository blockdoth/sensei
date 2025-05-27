use std::net::SocketAddr;

use async_trait::async_trait;
use log::trace;

use crate::errors::SinkError;
use crate::network::rpc_message::{DataMsg, RpcMessage, RpcMessageKind};
use crate::network::tcp::client::TcpClient;
use crate::sinks::Sink;

/// Configuration for a TCP sink.
///
/// This structure holds the target address and device ID used for sending data
/// over a TCP connection.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
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
    /// Adress to which to send
    target_addr: SocketAddr,
    /// Device id of the sink
    device_id: u64,
}

impl TCPSink {
    /// Create a new `TCPSink` from a given configuration.
    ///
    /// Establishes a new internal TCP client and stores the configuration details.
    pub async fn new(config: TCPConfig) -> Result<Self, SinkError> {
        trace!("Creating new TCPSink for {}", config.target_addr);
        Ok(Self {
            client: TcpClient::new(),
            target_addr: config.target_addr,
            device_id: config.device_id,
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
    async fn open(&mut self, _data: DataMsg) -> Result<(), SinkError> {
        trace!("Connecting to TCP socket at {}", self.target_addr);
        self.client.connect(self.target_addr).await.map_err(SinkError::from)?;
        Ok(())
    }

    /// Close the TCP connection to the target address.
    ///
    /// Attempts to disconnect using the internal TCP client.
    ///
    /// # Errors
    ///
    /// Returns a ['SinkError'] if the operation fails (e.g., I/O failure)
    async fn close(&mut self, _data: DataMsg) -> Result<(), SinkError> {
        trace!("Disconnecting from TCP socket at {}", self.target_addr);
        self.client.disconnect(self.target_addr).await.map_err(SinkError::from)?;
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
            device_id: self.device_id,
        };

        self.client.send_message(self.target_addr, ret).await.map_err(SinkError::from)?;
        Ok(())
    }
}
