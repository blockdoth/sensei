//!
//! Data Adapters
//! -------------
//!
//! Sensei deals with binary data extracted from "Sources", such as files
//! or sockets. Different sources provide different data formats, which must be handled
//! accordingly. This is the task of Data Adapters.
//!
//! Module for adapters
//! Mofidied based on: wisense/sensei/lib/src/adapters/mod.rs
//! Originally authored by: Fabian Portner

#[cfg(test)]
use mockall::automock;

use crate::errors::{CsiAdapterError, TaskError};
use crate::network::rpc_message::DataMsg;
use crate::{FromConfig, ToConfig};
#[cfg(feature = "csv")]
pub mod csv;
#[cfg(feature = "esp_tool")]
pub mod esp32;
#[cfg(feature = "iwl5300")]
pub mod iwl;
pub mod tcp;

/// Csi Data Adapter Trait
/// ----------------------
///
/// The data we stream from sources is always in some binary format.
/// Csi Data Adapters take on the task of processing this data into
/// the desired CsiData format.
///
/// NOTE: Adapters may hold data internally because the bytestream may
/// be fragmented over multiple packets. To this end, we split the API
/// the function only starts reutrning data once the CSIData frame has been collected
/// otherwise returns None
#[cfg_attr(test, automock)]
#[async_trait::async_trait]
pub trait CsiDataAdapter: Send + ToConfig<DataAdapterConfig> {
    /// Attempts to consume a DataMsg and produce a CsiFrame variant.
    ///
    /// # Arguments
    /// * `msg` - A DataMsg enum (either raw bytes or already parsed CSI).
    ///
    /// # Returns
    /// * `Ok(Some(DataMsg::CsiFrame))` - When a CSI frame is ready.
    /// * `Ok(None)` - When more data is needed (e.g. fragmented input).
    /// * `Err(CsiAdapterError)` - On decoding error.
    async fn produce(&mut self, msg: DataMsg) -> Result<Option<DataMsg>, CsiAdapterError>;
}

/// Adapter type tag for configuration-based instantiation.
///
/// This enum allows adapters to be specified via configuration files and deserialized
/// automatically. Each variant contains options specific to the corresponding adapter.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq)]
pub enum DataAdapterConfig {
    #[cfg(feature = "iwl5300")]
    Iwl {
        scale_csi: bool,
    },
    #[cfg(feature = "esp_tool")]
    Esp32 {
        scale_csi: bool,
    },
    Tcp {
        scale_csi: bool,
    },
    #[cfg(feature = "csv")]
    CSV {},
}

/// Instantiates a boxed CSI data adapter from a configuration tag.
///
/// This implementation allows you to convert a `DataAdapterConfig` into a
/// boxed dynamic adapter instance.
///
///
#[async_trait::async_trait]
impl FromConfig<DataAdapterConfig> for dyn CsiDataAdapter {
    async fn from_config(tag: DataAdapterConfig) -> Result<Box<Self>, TaskError> {
        let adapter: Box<dyn CsiDataAdapter> = match tag {
            #[cfg(feature = "iwl5300")]
            DataAdapterConfig::Iwl { scale_csi } => Box::new(iwl::IwlAdapter::new(scale_csi)),
            #[cfg(feature = "esp_tool")]
            DataAdapterConfig::Esp32 { scale_csi } => Box::new(esp32::ESP32Adapter::new(scale_csi)),
            DataAdapterConfig::Tcp { scale_csi } => Box::new(tcp::TCPAdapter::new(scale_csi)),
            #[cfg(feature = "csv")]
            DataAdapterConfig::CSV {} => Box::new(csv::CSVAdapter::default()),
        };
        Ok(adapter)
    }
}
