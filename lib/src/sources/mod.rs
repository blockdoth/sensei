mod controllers;
pub mod csv;
pub mod netlink;

use crate::FromConfig;
use crate::errors::DataSourceError;
use crate::errors::TaskError;
use crate::sources::controllers::Controller;
use std::net::SocketAddr;

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
}

/// Unified controller parameters
#[derive(serde::Serialize, serde::Deserialize, Debug, schemars::JsonSchema)]
#[serde(tag = "type", content = "params")]
pub enum ControllerParams {
    Netlink(controllers::netlink_controller::NetlinkControllerParams),
    // Extendable
}

#[derive(serde::Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum DataSourceConfig {
    Netlink(netlink::NetlinkConfig),
}

#[async_trait::async_trait]
impl FromConfig<DataSourceConfig> for dyn DataSourceT {
    async fn from_config(config: DataSourceConfig) -> Result<Box<Self>, TaskError> {
        let source: Box<dyn DataSourceT> = match config {
            DataSourceConfig::Netlink(cfg) => Box::new(netlink::NetlinkSource::new(cfg)?),
        };
        Ok(source)
    }
}

// Not sure if I need everything after this yet
//
//

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
pub struct RemoteSourceConfig {
    pub device_id: u64,
    pub addr: SocketAddr,
    pub raw: bool,
}

#[derive(serde::Deserialize, serde::Serialize, Debug, schemars::JsonSchema)]
pub enum SourceRequest {
    Subscribe(Subscription),
    Configure(Configuration),
}

#[derive(serde::Deserialize, serde::Serialize, Debug, schemars::JsonSchema)]
pub struct Subscription {
    pub device_id: u64,
    pub raw: bool,
}

#[derive(serde::Deserialize, serde::Serialize, Debug, schemars::JsonSchema)]
pub struct Configuration {
    pub device_id: u64,
    pub params: ControllerParams,
}
