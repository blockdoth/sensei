//!
//! Data Adapters
//! -------------
//!
//! Brokernet deals with brokering binary data extracted from "Sources", such as files
//! or sockets. Different sources provide different data formats, which must be handled
//! accordingly. This is the dask of Data Adapters.
//!
//! Module for adapters
//! Mofidied based on: wisense/sensei/lib/src/adapters/mod.rs
//! Originally authored by: Fabian Portner

use crate::csi_types::CsiData;
use crate::errors::CsiAdapterError;
use crate::network::rpc_message::DataMsg;
pub mod iwl;

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
#[async_trait::async_trait]
pub trait CsiDataAdapter: Send {
    /// Attempts to consume a slice of bytes and produce a `CsiData` frame.
    ///
    /// # Arguments
    /// * `buf` - A slice of bytes representing a raw data packet from a source.
    ///
    /// # Returns
    /// * `Ok(Some(CsiData))` - When a complete and valid frame is assembled.
    /// * `OK(none) or Err(CsiAdapterError)` - On malformed input or internal parsing error.
    ///
    /// # Notes
    /// - Adapters do not handle packet fragmentation logic. Callers must ensure
    ///   that only complete packets are passed in (unless fragmentation is internally supported).
    async fn produce(&mut self, buf: &[u8]) -> Result<Option<CsiData>, CsiAdapterError>;
}

/// Adapter type tag for configuration-based instantiation.
///
/// This enum allows adapters to be specified via configuration files and deserialized
/// automatically. Each variant contains options specific to the corresponding adapter.
#[derive(serde::Deserialize, Debug, Clone, Copy)]
#[serde(tag = "type")]
pub enum DataAdapterTag {
    Iwl { scale_csi: bool },
}

/// Instantiates a boxed CSI data adapter from a configuration tag.
///
/// This implementation allows you to convert a `DataAdapterTag` into a
/// boxed dynamic adapter instance.
///
///
impl From<DataAdapterTag> for Box<dyn CsiDataAdapter> {
    fn from(tag: DataAdapterTag) -> Box<dyn CsiDataAdapter> {
        match tag {
            DataAdapterTag::Iwl { scale_csi } => Box::new(iwl::IwlAdapter::new(scale_csi)),
        }
    }
}
