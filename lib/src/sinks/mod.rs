use crate::FromConfig;
use crate::errors::SinkError;
use crate::errors::TaskError;
use crate::network::rpc_message::{DataMsg, RpcMessage, RpcMessageKind};
use crate::network::tcp::client::TcpClient;
use async_trait::async_trait;
use std::net::SocketAddr;

pub mod file;
pub mod tcp;

/// Sink trait for handling data messages.
#[async_trait]
pub trait Sink: Send {
    async fn open(&mut self, data: DataMsg) -> Result<(), SinkError>;
    async fn close(&mut self, data: DataMsg) -> Result<(), SinkError>;
    /// Consume a DataMsg and process it (e.g., write to file, send over network).
    async fn provide(&mut self, data: DataMsg) -> Result<(), SinkError>;
}

/// Possible sink configurations.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum SinkConfig {
    File(file::FileConfig),
    Tcp(tcp::TCPConfig),
    // add other sink types here
}

#[async_trait::async_trait]
impl FromConfig<SinkConfig> for dyn Sink {
    async fn from_config(config: SinkConfig) -> Result<Box<Self>, TaskError> {
        match config {
            SinkConfig::File(cfg) => {
                let sink = file::FileSink::new(cfg).await?;
                Ok(Box::new(sink))
            }
            SinkConfig::Tcp(cfg) => {
                let sink = tcp::TCPSink::new(cfg).await?;
                Ok(Box::new(sink))
            }
        }
    }
}
