use crate::ToConfig;
use crate::errors::{DataSourceError, TaskError};
use crate::network::rpc_message::{DataMsg, RpcMessage, RpcMessageKind};
use crate::network::tcp::client::TcpClient;
use crate::sources::{DataSourceConfig, DataSourceT};
use log::trace;
use std::net::SocketAddr;

/// Configuration for a `TCPSource`.
///
/// Contains the target TCP address to which the data source will connect.
#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
pub struct TCPConfig {
    /// source adress from which to read
    target_addr: SocketAddr,
}

/// TCP-based data source for receiving `DataMsg` payloads over a network.
///
/// Establishes a TCP connection to a remote data provider and reads serialized
/// `RpcMessage` values from the stream.
pub struct TCPSource {
    /// Client from which to read
    client: TcpClient,
    /// Place to where to send
    target_addr: SocketAddr,
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
            target_addr: config.target_addr,
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
        trace!("Connecting to TCP socket at {}", self.target_addr);
        self.client
            .connect(self.target_addr)
            .await
            .map_err(DataSourceError::from)?;
        Ok(())
    }

    /// Stops the data source by closing the TCP connection.
    ///
    /// # Errors
    /// Returns a [`DataSourceError`] if disconnection fails.
    async fn stop(&mut self) -> Result<(), DataSourceError> {
        trace!("Disconnecting from TCP socket at {}", self.target_addr);
        self.client
            .disconnect(self.target_addr)
            .await
            .map_err(DataSourceError::from)?;
        Ok(())
    }

    /// Attempts to read raw data into the given buffer.
    ///
    /// Currently unimplemented for TCP sources. Use [`read`] instead to access
    /// structured messages.
    ///
    /// # Errors
    /// Always returns [`DataSourceError::ReadBuf`] as this method is not supported.
    async fn read_buf(&mut self, buf: &mut [u8]) -> Result<usize, DataSourceError> {
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
            .read_message(self.target_addr)
            .await
            .map_err(DataSourceError::from)?;

        match rpcmsg.msg {
            RpcMessageKind::Ctrl(_ctrl_msg) => {
                // control messages carry no data payload
                Ok(None)
            }
            RpcMessageKind::Data { data_msg, .. } => {
                // return the actual data payload
                Ok(Some(data_msg))
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
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example(netlink_source: &NetlinkSource) -> Result<(), TaskError> {
    /// let config = netlink_source.to_config().await?;
    /// // `config` will be a `DataSourceConfig::Tcp` variant.
    /// # Ok(())
    /// # }
    /// ```
    async fn to_config(&self) -> Result<DataSourceConfig, TaskError> {
        Ok(DataSourceConfig::Tcp(TCPConfig {
            target_addr: self.target_addr,
        }))
    }
}
