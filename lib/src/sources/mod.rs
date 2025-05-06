/**
 * Module for data sources
 * Mofidied based on: wisense/sensei/lib/src/sources/mod.rs
 * Originally authored by: Fabian Portner
 */

pub mod dummy;
pub mod packet_file;
    
#[cfg(feature = "netlink_source")]
pub mod netlink;
#[cfg(feature = "pcap_source")]
pub mod pcap_file;
#[cfg(feature = "pcap_source")]
pub mod pcap_live;
#[cfg(feature = "tcpstream_source")]
pub mod zen_tcp;
#[cfg(feature = "websocket_source")]
pub mod zen_web;

mod handler;
use std::net::SocketAddr;

pub use handler::{ServiceRequest, SourceHandler, SourceId, Subscriber};

use crate::controller::ControllerParams;
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
    async fn configure(&mut self, params: ControllerParams) -> Result<(), DataSourceError>;
}

/// Enum capturing all source handler requests
/// ------------------------------------------
///
/// The source handler allows for a few different requests. These requests are passed
/// down from the server that deals with clients. While the server API may differ, we
/// want a unified API afterwards.
///
/// Since some sources may be on remote machines and connected through a middle layer
/// of e.g. TCP/Webservice, we need this request here for the RemoteSource trait.
#[cfg_attr(feature = "docs", derive(schemars::JsonSchema))]
#[derive(serde::Deserialize, serde::Serialize, Debug)]
pub enum SourceRequest {
    Subscribe(Subscription),
    Configure(Configuration),
}

#[cfg_attr(feature = "docs", derive(schemars::JsonSchema))]
#[derive(serde::Deserialize, serde::Serialize, Debug)]
pub struct Subscription {
    pub source_id: SourceId,
    pub raw: bool,
}

#[cfg_attr(feature = "docs", derive(schemars::JsonSchema))]
#[derive(serde::Deserialize, serde::Serialize, Debug)]
pub struct Configuration {
    pub source_id: SourceId,
    pub params: crate::controller::ControllerParams,
}

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
pub struct RemoteSourceConfig {
    pub source_id: String,
    pub addr: SocketAddr,
    pub raw: bool,
}

#[derive(serde::Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum DataSourceConfig {
    Dummy(dummy::DummyConfig),
    PacketFile(packet_file::PacketFileConfig),
    #[cfg(feature = "pcap_source")]
    PcapFile(pcap_file::PcapFileConfig),
    #[cfg(feature = "pcap_source")]
    PcapLive(pcap_live::PcapLiveConfig),
    #[cfg(feature = "tcpstream_source")]
    Tcp(RemoteSourceConfig),
    #[cfg(feature = "websocket_source")]
    Websocket(RemoteSourceConfig),
    #[cfg(feature = "netlink_source")]
    Netlink(netlink::NetlinkConfig),
}

pub async fn source_from_config(
    config: DataSourceConfig,
) -> Result<Box<dyn DataSourceT>, DataSourceError> {
    let source: Box<dyn DataSourceT> = match config {
        DataSourceConfig::Dummy(config) => Box::new(dummy::DummySource::new(config)?),
        DataSourceConfig::PacketFile(config) => {
            Box::new(packet_file::PacketFileSource::new(config)?)
        }
        #[cfg(feature = "pcap_source")]
        DataSourceConfig::PcapFile(config) => Box::new(pcap_file::PcapFileSource::new(config)?),
        #[cfg(feature = "pcap_source")]
        DataSourceConfig::PcapLive(config) => Box::new(pcap_live::PcapLiveSource::new(config)?),
        #[cfg(feature = "tcpstream_source")]
        DataSourceConfig::Tcp(config) => Box::new(zen_tcp::ZenTcpSource::new(config).await?),
        #[cfg(feature = "websocket_source")]
        DataSourceConfig::Websocket(config) => Box::new(zen_web::ZenWebSource::new(config)),
        #[cfg(feature = "netlink_source")]
        DataSourceConfig::Netlink(config) => Box::new(netlink::NetlinkSource::new(config)?),
    };

    Ok(source)
}
