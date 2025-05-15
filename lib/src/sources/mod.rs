pub mod netlink;
mod controllers;

use std::net::SocketAddr;

use crate::sources::controllers::Controller;
use crate::errors::DataSourceError;

/// Data Source Trait
/// -----------------
///
/// Sources are anything that provides packetized bytestream data that can be
/// interpreted by CSI adapters. It is up to the user to correct a source
/// sensibly with an adapter.
#[async_trait::async_trait]
pub trait DataSourceT: Send {
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
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, DataSourceError>;

    /// Configure a source
    /// ------------------
    /// Try to configure a source with a given set of control parameters. These are
    /// tool/protocol specific, and sources must decide what they can and can't handle.
    async fn configure(&mut self, params: Box<dyn Controller>) -> Result<(), DataSourceError>;
}


//TODO: Create a way to control the sources with a RPCMessage, Bellow is what Fabian worked on and how he controlled the sources

// /// of e.g. TCP/Webservice, we need this request here for the RemoteSource trait.
// #[cfg_attr(feature = "docs", derive(schemars::JsonSchema))]
// #[derive(serde::Deserialize, serde::Serialize, Debug)]
// pub enum SourceRequest {
//     Subscribe(Subscription),
//     Configure(Configuration),
// }

// #[cfg_attr(feature = "docs", derive(schemars::JsonSchema))]
// #[derive(serde::Deserialize, serde::Serialize, Debug)]
// pub struct Subscription {
//     pub source_id: SourceId,
//     pub raw: bool,
// }

// #[cfg_attr(feature = "docs", derive(schemars::JsonSchema))]
// #[derive(serde::Deserialize, serde::Serialize, Debug)]
// pub struct Configuration {
//     pub source_id: SourceId,
//     pub params: Box<dyn Controller>,
// }

// #[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
// pub struct RemoteSourceConfig {
//     pub source_id: String,
//     pub addr: SocketAddr,
//     pub raw: bool,
// }

// #[derive(serde::Deserialize, Debug, Clone)]
// #[serde(tag = "type")]
// pub enum DataSourceConfig {
//     Netlink(netlink::NetlinkConfig),
// }

// pub async fn source_from_config(
//     config: DataSourceConfig,
// ) -> Result<Box<dyn DataSourceT>, DataSourceError> {
//     let source: Box<dyn DataSourceT> = match config {
//         DataSourceConfig::Netlink(config) => Box::new(netlink::NetlinkSource::new(config)?),
//     };

//     Ok(source)
// }
