//! Data source definitions and configuration.
//!
//! This module provides the core [`DataSourceT`] trait for consuming packetized
//! byte streams, as well as the [`DataSourceConfig`] enum for instantiating
//! concrete source implementations from configuration.
//!
//! Supported sources (via `DataSourceConfig`):
//! - [`netlink::NetlinkSource`]: Linux-specific netlink packet capture (requires `target_os = "linux"`).
//! - [`esp32::Esp32Source`]: ESP32-based wireless or serial data source.
//! - [`csv`]: Placeholder for Csv-based source (e.g., playback from file).
//! - ['tcp::TCPSource']: Source to receive from other system nodes
//!
//! Each source implementation must be constructed with configuration via the
//! [`FromConfig<DataSourceConfig>`] trait and then activated via the
//! [`DataSourceT::start`] method before reading frames.

pub mod controllers;
#[cfg(feature = "csv")]
pub mod csv;
#[cfg(feature = "esp_tool")]
pub mod esp32;
#[cfg(all(target_os = "linux", feature = "iwl5300"))]
pub mod netlink;
pub mod tcp;

use std::any::Any;

#[cfg(test)]
use mockall::automock;
use serde::{Deserialize, Serialize};

use crate::errors::{DataSourceError, TaskError};
use crate::network::rpc_message::DataMsg;
use crate::{FromConfig, ToConfig};

pub const BUFSIZE: usize = 4096;

/// Data Source Trait
/// -----------------
///
/// Sources are anything that provides packetized bytestream data that can be
/// interpreted by CSI adapters. It is up to the user to correct a source
/// sensibly with an adapter.
#[cfg_attr(test, automock)]
#[async_trait::async_trait]
pub trait DataSourceT: Send + Sync + Any + ToConfig<DataSourceConfig> {
    /// Start collecting data
    /// ---------------------
    /// Must activate the source, such that we can read from it. For example, starting
    /// threads with internal buffering, opening sockets or captures, etc.
    /// Sources should not be active by default, as otherwise we can't control when
    /// data is being collected.
    async fn start(&mut self) -> Result<(), DataSourceError>;

    /// Stop collecting data
    /// --------------------
    /// From after this call on, no further data should be collected by this source,
    /// until start is called again. Internal buffers need however not be cleared,
    /// just not populated any further.
    async fn stop(&mut self) -> Result<(), DataSourceError>;

    /// Read data from source
    /// ---------------------
    /// Copy one "packet" (meaning being source specific) into the buffer and report
    /// its size.
    /// Don't use this method use read instead
    async fn read_buf(&mut self, buf: &mut [u8]) -> Result<usize, DataSourceError>;

    /// Attempts to read a data message from the source.
    ///
    /// This method polls the underlying data source for new data and returns
    /// an optional `DataMsg` if available.
    ///
    /// # Errors
    /// Returns a [`DataSourceError`] if the underlying source encounters an error
    /// during the read operation.
    async fn read(&mut self) -> Result<Option<DataMsg>, DataSourceError>;
}

/// Configuration =for available data source types.
///
/// This enum is tagged using Serde’s `tag = "type"`  Each variant
/// corresponds to a concrete source implementation:
/// - `Netlink`: Linux-only netlink-based capture (requires `target_os = "linux"`)
/// - `Esp32`: ESP32-based data source
/// - 'Tcp': receiving from another node
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum DataSourceConfig {
    /// Linux netlink source (packet capture via netlink sockets).
    #[cfg(all(target_os = "linux", feature = "iwl5300"))]
    Netlink(netlink::NetlinkConfig),
    /// Data source backed by an ESP32 device.
    #[cfg(feature = "esp_tool")]
    Esp32(esp32::Esp32SourceConfig),
    /// TCP receiving from another device.
    Tcp(tcp::TCPConfig),
    /// Csv config
    #[cfg(feature = "csv")]
    Csv(csv::CsvSourceConfig),
}

#[async_trait::async_trait]
impl FromConfig<DataSourceConfig> for dyn DataSourceT {
    /// Instantiate a concrete [`DataSourceT`] from its configuration.
    ///
    /// # Errors
    /// Returns [`TaskError::DataSourceError`] if the underlying source
    /// constructor fails.
    async fn from_config(config: DataSourceConfig) -> Result<Box<Self>, TaskError> {
        let source: Box<dyn DataSourceT> = match config {
            #[cfg(all(target_os = "linux", feature = "iwl5300"))]
            DataSourceConfig::Netlink(cfg) => Box::new(netlink::NetlinkSource::new(cfg).map_err(TaskError::DataSourceError)?),
            #[cfg(feature = "esp_tool")]
            DataSourceConfig::Esp32(cfg) => Box::new(esp32::Esp32Source::new(cfg).map_err(TaskError::DataSourceError)?),
            DataSourceConfig::Tcp(cfg) => Box::new(tcp::TCPSource::new(cfg).map_err(TaskError::DataSourceError)?),
            #[cfg(feature = "csv")]
            DataSourceConfig::Csv(cfg) => Box::new(csv::CsvSource::new(cfg).map_err(TaskError::DataSourceError)?),
        };
        Ok(source)
    }
}
