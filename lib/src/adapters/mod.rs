/**
 * Module for adapters
 * Mofidied based on: wisense/sensei/lib/src/adapters/mod.rs
 * Originally authored by: Fabian Portner
 */

//!
//! Data Adapters
//! -------------
//!
//! Brokernet deals with brokering binary data extracted from "Sources", such as files
//! or sockets. Different sources provide different data formats, which must be handled
//! accordingly. This is the dask of Data Adapters.
//!
pub mod iwl;
pub mod nexmon;

use crate::csi_types::CsiData;
use crate::errors::CsiAdapterError;

/// Csi Data Adapter Trait
/// ----------------------
///
/// The data we stream from sources is always in some binary format.
/// Csi Data Adapters take on the task of processing this data into
/// the desired CsiData format.
///
/// NOTE: Adapters may hold data internally because the bytestream may
/// be fragmented over multiple packets. To this end, we split the API
/// into consumption (data from a packet) and reaping (after assembly).
#[async_trait::async_trait]
pub trait CsiDataAdapter: Send {
    /// Consume a packet to parse CSI from.
    ///
    /// NOTE: The packet must be of appropriate size. Adapters are not expected
    /// to handle fragmentation, caching, or anything. This must be done by the
    /// caller.
    async fn consume(&mut self, buf: &[u8]) -> Result<(), CsiAdapterError>;

    /// Reap CSI Data from this adapter.
    /// If there is no data to reap, this function returns None.
    /// If the data is corrupted or the adapter meets an internal problem, it
    /// returns an error.
    async fn reap(&mut self) -> Result<Option<CsiData>, CsiAdapterError>;
}

/// Just a tag for directly deserializing an adapter from a config
#[derive(serde::Deserialize, Debug, Clone, Copy)]
#[serde(tag = "type")]
pub enum DataAdapterTag {
    Nexmon,
    Iwl { scale_csi: bool },
}

impl From<DataAdapterTag> for Box<dyn CsiDataAdapter> {
    fn from(tag: DataAdapterTag) -> Box<dyn CsiDataAdapter> {
        match tag {
            DataAdapterTag::Nexmon => Box::new(nexmon::NexmonDataAdapter::default()),
            DataAdapterTag::Iwl { scale_csi } => Box::new(iwl::IwlAdapter::new(scale_csi)),
        }
    }
}
