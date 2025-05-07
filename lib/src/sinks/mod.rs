//! Module for adapters
//! Mofidied based on: wisense/sensei/lib/src/sinks/mod.rs
//! Originally authored by: Fabian Portner

mod file;
pub use queue;

// #[cfg(feature = "tcpserver")]
// mod tcp;

use crate::csi_types::CsiData;
use crate::errors::SinkError;
use file::File;

pub enum SubscriberData<'a> {
    Raw(&'a [u8]),
    Csi(CsiData),
    Probe,
}

/// Sink configs that can be created from file.
///
/// NOTE: Tcpstream are handled by our webserver.
/// It does not make sense to create them from a config, since they require a peer.
/// The same goes for the queue, which requires someone to handle the other end.
#[derive(serde::Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum SinkConfig {
    File(file::FileConfig),
}

/// Sinks to handle data
/// Wrapped in a value enum for a clean abstraction.
pub enum Sink {
    // #[cfg(feature = "tcpserver")]
    // TcpStream(tcp::TcpStream),
    // Queue(queue::Sender<CsiData>),
    File(File),
}

impl Sink {
    pub async fn provide<'a>(&mut self, data: SubscriberData<'a>) -> Result<(), SinkError> {
        match self {
            // #[cfg(feature = "tcpserver")]
            // Self::TcpStream(socket) => tcp::tcpstream_write(socket, data).await,
            // Self::Queue(queue) => queue::send(queue, data).await,
            Self::File(file) => file::file_write(file, data).await,
        }
    }

    pub async fn from_config(config: SinkConfig) -> Self {
        match config {
            SinkConfig::File(config) => Sink::File(file::create(config).await.unwrap()),
        }
    }
}
