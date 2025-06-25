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
            DataSourceConfig::Tcp(cfg) => Box::new(tcp::TCPSource::new(cfg).await.map_err(TaskError::DataSourceError)?),
            #[cfg(feature = "csv")]
            DataSourceConfig::Csv(cfg) => Box::new(csv::CsvSource::new(cfg).map_err(TaskError::DataSourceError)?),
        };
        Ok(source)
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn test_data_source_config_debug() {
        let tcp_config = DataSourceConfig::Tcp(crate::sources::tcp::TCPConfig {
            target_addr: "127.0.0.1:8080".parse::<SocketAddr>().unwrap(),
            device_id: 123,
        });
        let debug_str = format!("{tcp_config:?}");
        assert!(debug_str.contains("Tcp"));
        assert!(debug_str.contains("127.0.0.1:8080"));
        assert!(debug_str.contains("123"));
    }

    #[test]
    fn test_data_source_config_clone() {
        let tcp_config = DataSourceConfig::Tcp(crate::sources::tcp::TCPConfig {
            target_addr: "127.0.0.1:8080".parse::<SocketAddr>().unwrap(),
            device_id: 123,
        });
        let cloned_config = tcp_config.clone();
        assert_eq!(tcp_config, cloned_config);
    }

    #[test]
    fn test_data_source_config_partial_eq() {
        let tcp_config1 = DataSourceConfig::Tcp(crate::sources::tcp::TCPConfig {
            target_addr: "127.0.0.1:8080".parse::<SocketAddr>().unwrap(),
            device_id: 123,
        });
        let tcp_config2 = DataSourceConfig::Tcp(crate::sources::tcp::TCPConfig {
            target_addr: "127.0.0.1:8080".parse::<SocketAddr>().unwrap(),
            device_id: 123,
        });
        let tcp_config3 = DataSourceConfig::Tcp(crate::sources::tcp::TCPConfig {
            target_addr: "127.0.0.1:8080".parse::<SocketAddr>().unwrap(),
            device_id: 456,
        });

        assert_eq!(tcp_config1, tcp_config2);
        assert_ne!(tcp_config1, tcp_config3);
    }

    #[test]
    fn test_data_source_config_serialization() {
        let tcp_config = DataSourceConfig::Tcp(crate::sources::tcp::TCPConfig {
            target_addr: "127.0.0.1:8080".parse::<SocketAddr>().unwrap(),
            device_id: 123,
        });

        let serialized = serde_json::to_string(&tcp_config).unwrap();
        let deserialized: DataSourceConfig = serde_json::from_str(&serialized).unwrap();

        assert_eq!(tcp_config, deserialized);
    }

    #[cfg(feature = "esp_tool")]
    #[test]
    fn test_data_source_config_esp32() {
        let esp32_config = DataSourceConfig::Esp32(crate::sources::esp32::Esp32SourceConfig {
            port_name: "/dev/ttyUSB0".to_string(),
            baud_rate: 115200,
            csi_buffer_size: 100,
            ack_timeout_ms: 2000,
        });

        let debug_str = format!("{esp32_config:?}");
        assert!(debug_str.contains("Esp32"));
        assert!(debug_str.contains("/dev/ttyUSB0"));
        assert!(debug_str.contains("115200"));
    }

    #[cfg(feature = "csv")]
    #[test]
    fn test_data_source_config_csv() {
        let csv_config = DataSourceConfig::Csv(crate::sources::csv::CsvSourceConfig {
            path: PathBuf::from("/tmp/test.csv"),
            cell_delimiter: Some(b','),
            row_delimiter: Some(b'\n'),
            header: true,
            delay: 100,
        });

        let debug_str = format!("{csv_config:?}");
        assert!(debug_str.contains("Csv"));
        assert!(debug_str.contains("/tmp/test.csv"));
    }

    #[test]
    fn test_tcp_config_creation() {
        // Test that we can create a TCP config without issues
        let tcp_config = DataSourceConfig::Tcp(crate::sources::tcp::TCPConfig {
            target_addr: "127.0.0.1:8080".parse::<SocketAddr>().unwrap(),
            device_id: 123,
        });

        // Verify the config was created correctly
        match tcp_config {
            DataSourceConfig::Tcp(config) => {
                assert_eq!(config.target_addr.to_string(), "127.0.0.1:8080");
                assert_eq!(config.device_id, 123);
            }
            _ => panic!("Expected TCP config"),
        }
    }

    #[cfg(feature = "esp_tool")]
    #[tokio::test]
    async fn test_from_config_esp32() {
        let esp32_config = DataSourceConfig::Esp32(crate::sources::esp32::Esp32SourceConfig {
            port_name: "/dev/ttyUSB0".to_string(),
            baud_rate: 115200,
            csi_buffer_size: 100,
            ack_timeout_ms: 2000,
        });

        let result = <dyn DataSourceT>::from_config(esp32_config).await;
        assert!(result.is_ok());
    }

    #[cfg(feature = "csv")]
    #[tokio::test]
    async fn test_from_config_csv_invalid_path() {
        let csv_config = DataSourceConfig::Csv(crate::sources::csv::CsvSourceConfig {
            path: PathBuf::from("/nonexistent/file.csv"),
            cell_delimiter: Some(b','),
            row_delimiter: Some(b'\n'),
            header: true,
            delay: 100,
        });

        let result = <dyn DataSourceT>::from_config(csv_config).await;
        assert!(result.is_err());
        match result {
            Err(TaskError::DataSourceError(_)) => {}
            _ => panic!("Expected DataSourceError"),
        }
    }

    #[test]
    fn test_data_source_config_different_variants_not_equal() {
        let tcp_config = DataSourceConfig::Tcp(crate::sources::tcp::TCPConfig {
            target_addr: "127.0.0.1:8080".parse::<SocketAddr>().unwrap(),
            device_id: 123,
        });

        #[cfg(feature = "esp_tool")]
        {
            let esp32_config = DataSourceConfig::Esp32(crate::sources::esp32::Esp32SourceConfig::default());
            assert_ne!(tcp_config, esp32_config);
        }

        #[cfg(feature = "csv")]
        {
            let csv_config = DataSourceConfig::Csv(crate::sources::csv::CsvSourceConfig {
                path: PathBuf::from("/tmp/test.csv"),
                cell_delimiter: Some(b','),
                row_delimiter: Some(b'\n'),
                header: true,
                delay: 100,
            });
            assert_ne!(tcp_config, csv_config);
        }
    }

    #[test]
    fn test_data_source_config_serde_roundtrip() {
        let mut configs = vec![DataSourceConfig::Tcp(crate::sources::tcp::TCPConfig {
            target_addr: "192.168.1.100:9000".parse::<SocketAddr>().unwrap(),
            device_id: 999,
        })];

        #[cfg(feature = "esp_tool")]
        {
            configs.push(DataSourceConfig::Esp32(crate::sources::esp32::Esp32SourceConfig {
                port_name: "/dev/ttyACM0".to_string(),
                baud_rate: 3_000_000,
                csi_buffer_size: 50,
                ack_timeout_ms: 1000,
            }));
        }

        #[cfg(feature = "csv")]
        {
            configs.push(DataSourceConfig::Csv(crate::sources::csv::CsvSourceConfig {
                path: PathBuf::from("/data/measurements.csv"),
                cell_delimiter: Some(b';'),
                row_delimiter: Some(b'\r'),
                header: false,
                delay: 500,
            }));
        }

        for config in configs {
            let json = serde_json::to_string(&config).unwrap();
            let deserialized: DataSourceConfig = serde_json::from_str(&json).unwrap();
            assert_eq!(config, deserialized);
        }
    }
}
