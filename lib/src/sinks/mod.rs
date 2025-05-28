//! Data sinks for writing to a sink
//!
//! This module defines the [`Sink`] trait, which represents a consumer of [`DataMsg`] messages,
//! and provides and writes or sends the data to somwhere[`SinkConfig`].
//!
//! Sink implementations may write messages to files, send them over the network etc.
//!
//! # Example
//!
//! ```rust,ignore
//! use your_crate::sinks::{Sink, SinkConfig};
//!
//! let config: SinkConfig = load_config();
//! let mut sink = Sink::from_config(config).await?;
//! sink.provide(data_msg).await?;
//! ```

use std::net::SocketAddr;

use async_trait::async_trait;

use crate::FromConfig;
use crate::errors::{SinkError, TaskError};
use crate::network::rpc_message::{DataMsg, RpcMessage, RpcMessageKind};
use crate::network::tcp::client::TcpClient;

pub mod file;
pub mod tcp;

/// A trait representing a data sink that consumes [`DataMsg`] messages.
///
/// Implementing this trait define how data messages are handled after being produced by
/// a device. This could involve writing to a file, streaming to a network endpoint,
/// or storing in a database.
///
/// Implementations must be `Send` to ensure they can be used across asynchronous tasks.
#[async_trait]
pub trait Sink: Send {
    /// Open the connection to the sink
    ///
    /// # Errors
    ///
    /// Returns a ['SinkError'] if the operation fails (e.g., I/O failure)
    async fn open(&mut self, data: DataMsg) -> Result<(), SinkError>;

    /// Closes the connection to the sink
    ///
    /// # Errors
    ///
    /// Returns a ['SinkError'] if the operation fails (e.g., I/O failure)
    async fn close(&mut self, data: DataMsg) -> Result<(), SinkError>;

    /// Consume a DataMsg and process it (e.g., write to file, send over network).
    /// Consume a [`DataMsg`] and perform a sink-specific operation.
    ///
    /// # Errors
    ///
    /// Returns a [`SinkError`] if the operation fails (e.g., I/O failure).
    async fn provide(&mut self, data: DataMsg) -> Result<(), SinkError>;
}

/// Configuration options for available sink types.
///
/// This enum is tagged using Serde's `tag`  meaning the configuration must specify
/// a `type` field (e.g., `{ "type": "File", ... }`). Each variant corresponds to a different
/// kind of sink implementation.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum SinkConfig {
    /// File sink configuration
    File(file::FileConfig),
    /// Tcp configuration
    Tcp(tcp::TCPConfig),
    // add other sink types here
}

/// Constructs a [`Sink`] implementation from a [`SinkConfig`] using the [`FromConfig`] trait.
///
/// TNew sink types can be added by
/// extending [`SinkConfig`] and matching them here.
///
/// # Errors
///
/// Returns a [`TaskError`] if sink instantiation fails.
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
