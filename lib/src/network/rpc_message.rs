//! Network RPC message types and related data structures for Sensei.
//!
//! This module defines the types used for remote procedure call (RPC) messages exchanged between nodes and orchestrators in the Sensei system. It includes message kinds, control commands, device and host status, and serialization helpers for network communication.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;

use crate::csi_types::CsiData;
use crate::handler::device_handler::DeviceHandlerConfig;
use crate::network::experiment_config::Experiment;
use crate::network::rpc_message::CfgType::{Create, Delete, Edit};

/// The default address used for network communication (localhost:6969).
pub const DEFAULT_ADDRESS: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 6969));

/// Information about how well the host is responding
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Responsiveness {
    Connected,
    Lossy,
    Disconnected,
}

/// Represents a full RPC message, including its kind and source/target addresses.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct RpcMessage {
    /// The kind of RPC message (control, registration, or data).
    pub msg: RpcMessageKind,
    /// The address of the sender.
    pub src_addr: SocketAddr,
    /// The address of the intended recipient.
    pub target_addr: SocketAddr,
}

/// The different kinds of RPC messages that can be sent over the network.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum RpcMessageKind {
    /// Host control message (e.g., connect, disconnect, configure).
    HostCtrl(HostCtrl),
    /// Registration/control message (e.g., poll status, announce presence).
    RegCtrl(RegCtrl),
    /// Data message containing CSI or raw frame data.
    Data { data_msg: DataMsg, device_id: u64 },
}

/// Unique identifier for a host in the network.
pub type HostId = u64;
/// Unique identifier for a device in the network.
pub type DeviceId = u64;

/// Host control commands for managing device connections and subscriptions.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum HostCtrl {
    /// An empty message. Used for signaling that a response cannot be generated.
    Empty,
    /// Simple ping message. Host should respond with Pong.
    Ping,
    /// See ping
    Pong,
    /// Connect to a host.
    Connect,
    /// Disconnect from a host.
    Disconnect,
    /// Configure a device handler.
    Configure { device_id: DeviceId, cfg_type: CfgType },
    /// Subscribe to a device's data stream.
    Subscribe { device_id: DeviceId },
    /// Unsubscribe from a device's data stream.
    Unsubscribe { device_id: DeviceId },
    /// Subscribe to another node's device stream.
    SubscribeTo { target_addr: SocketAddr, device_id: DeviceId },
    /// Unsubscribe from another node's device stream.
    UnsubscribeFrom { target_addr: SocketAddr, device_id: DeviceId },
    /// Sends an experiment configuration
    Experiment { experiment: Experiment },
}

/// Registration and control messages for orchestrator and node communication.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum RegCtrl {
    /// Poll the status of a specific host.
    PollHostStatus { host_id: HostId },
    /// Poll the statuses of all hosts.
    PollHostStatuses,
    /// Announce the presence of a host to the network.
    AnnouncePresence { host_id: HostId, host_address: SocketAddr },
    /// Report the status of a host.
    HostStatus(HostStatus),
    /// Report the statuses of multiple hosts.
    HostStatuses { host_statuses: Vec<HostStatus> },
}

/// Status information for a host, including its devices.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct HostStatus {
    /// The unique ID of the host.
    pub host_id: HostId,
    /// The status of each device managed by the host.
    pub device_statuses: Vec<DeviceStatus>,
    /// How well the host is responding.
    pub responsiveness: Responsiveness,
}

impl From<HostStatus> for RegCtrl {
    fn from(value: HostStatus) -> Self {
        RegCtrl::HostStatus(value)
    }
}

impl From<RegCtrl> for HostStatus {
    fn from(value: RegCtrl) -> Self {
        match value {
            RegCtrl::HostStatus(HostStatus {
                host_id,
                device_statuses,
                responsiveness,
            }) => HostStatus {
                host_id,
                device_statuses,
                responsiveness,
            },
            _ => {
                panic!("Could not convert from this type of CtrlMsg: {value:?}");
            }
        }
    }
}

/// Status information for a device managed by a host.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct DeviceStatus {
    /// The unique ID of the device.
    pub id: DeviceId,
    /// The type of the device/source.
    pub dev_type: SourceType,
}

impl From<&DeviceHandlerConfig> for DeviceStatus {
    fn from(value: &DeviceHandlerConfig) -> Self {
        DeviceStatus {
            id: value.device_id,
            dev_type: value.stype.clone(),
        }
    }
}

/// Data messages exchanged between nodes, containing either raw or parsed CSI data.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum DataMsg {
    /// Raw frame data (requires decoding adapter).
    RawFrame { ts: f64, bytes: Vec<u8>, source_type: SourceType },
    /// Parsed CSI frame data.
    CsiFrame { csi: CsiData },
}

/// Supported source/device types in the Sensei system.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum SourceType {
    ESP32,
    IWL5300,
    AX200,
    AX210,
    AtherosQCA,
    Csv,
    TCP,
    Unknown,
}

/// Modes for CSI adapters.
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub enum AdapterMode {
    RAW,
    SOURCE,
    TARGET,
}

/// Types of configuration operations for device handlers.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq)]
pub enum CfgType {
    /// Create a new device handler with the given config.
    Create { cfg: DeviceHandlerConfig },
    /// Edit an existing device handler with the given config.
    Edit { cfg: DeviceHandlerConfig },
    /// Delete a device handler.
    Delete,
}

impl CfgType {
    /// It is sadly necessary to make my own function, as FromStr can not have two fields nor be async
    /// This requires preprocessing the cfg
    /// Construct a CfgType from a string and a device handler config.
    ///
    /// # Arguments
    /// * `config_type` - The type of configuration operation ("create", "edit", or "delete").
    /// * `cfg` - The device handler configuration.
    pub fn from_string(config_type: Option<&str>, cfg: DeviceHandlerConfig) -> Result<Self, String> {
        match config_type {
            Some("create") => Ok(Create { cfg }),
            Some("edit") => Ok(Edit { cfg }),
            Some("delete") => Ok(Delete),
            _ => Err(format!("Unrecognized config type {config_type:?}")),
        }
    }
}

impl FromStr for AdapterMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "raw" => Ok(AdapterMode::RAW),
            "source" => Ok(AdapterMode::SOURCE),
            "target" => Ok(AdapterMode::TARGET),
            _ => Err(format!("Unrecognised adapter mode '{s}'")),
        }
    }
}

// FromStr implementations for easy cli usage
impl FromStr for HostCtrl {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let lowercase = s.to_lowercase();
        let mut parts = lowercase.split_whitespace();
        let kind = parts.next().unwrap_or("not a valid command");

        match kind {
            "connect" => Ok(HostCtrl::Connect),
            "disconnect" => Ok(HostCtrl::Disconnect),
            "configure" => {
                todo!("support this")
            }
            "subscribe" => {
                let device_id = parts.next().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0); // TODO better id assignment

                Ok(HostCtrl::Subscribe { device_id })
            }
            "unsubscribe" => {
                let device_id = parts.next().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);

                Ok(HostCtrl::Unsubscribe { device_id })
            }
            s => Err(s.to_owned()),
        }
    }
}

// FromStr implementations for easy cli usage
impl FromStr for RegCtrl {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let lowercase = s.to_lowercase();
        let mut parts = lowercase.split_whitespace();
        let kind = parts.next().unwrap_or("not a valid command");

        match kind {
            "pollhoststatus" => {
                let host_id = parts.next().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
                Ok(RegCtrl::PollHostStatus { host_id })
            }
            "pollhoststatuses" => Ok(RegCtrl::PollHostStatuses),
            "hoststatus" => Ok(RegCtrl::HostStatus(HostStatus {
                host_id: 0,
                device_statuses: vec![],
                responsiveness: Responsiveness::Connected,
            })),
            "heartbeat" => Ok(RegCtrl::AnnouncePresence {
                host_id: 0,
                host_address: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 8080)),
            }), // TODO better id assignment
            s => Err(s.to_owned()),
        }
    }
}

/// Helper to construct an RpcMessage from a stream and message kind.
///
/// Takes any type that implements AsRef, such as TcpStream/OwnedReadHalf/OwnedWriteHalf,
/// as refs (&), Arc<>, Box<> and Rc<>.
pub fn make_msg<S: AsRef<TcpStream>>(stream: S, msg: RpcMessageKind) -> RpcMessage {
    let stream_ref = stream.as_ref();
    RpcMessage {
        msg,
        src_addr: stream_ref.local_addr().unwrap(),
        target_addr: stream_ref.peer_addr().unwrap(),
    }
}
