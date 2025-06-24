//! Network RPC message types and related data structures for Sensei.
//!
//! This module defines the types used for remote procedure call (RPC) messages exchanged between nodes and orchestrators in the Sensei system. It includes message kinds, control commands, device and host status, and serialization helpers for network communication.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;

use crate::csi_types::CsiData;
use crate::experiments::{Experiment, ExperimentInfo};
use crate::handler::device_handler::DeviceHandlerConfig;

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
    /// Start a device handler on a node
    Start { device_id: DeviceId },
    /// Start all device handlers on a node
    StartAll,
    /// Stop a device handler on a node
    Stop { device_id: DeviceId },
    /// Stop all device handlers on a node
    StopAll,
    /// Subscribe to a device's data stream.
    Subscribe { device_id: DeviceId },
    /// Subscribe to all devices on a node
    SubscribeAll,
    /// Unsubscribe from a device's data stream.
    Unsubscribe { device_id: DeviceId },
    /// Unsubscribe from all devices on a node
    UnsubscribeAll,
    /// Subscribes a node to another node's device stream.
    SubscribeTo { target_addr: SocketAddr, device_id: DeviceId },
    /// Subscribes a node to all device streams of another node
    SubscribeToAll { target_addr: SocketAddr },
    /// Unsubscribes a node from another node's device stream.
    UnsubscribeFrom { target_addr: SocketAddr, device_id: DeviceId },
    /// Unsubscribe from all data streams of another host.
    UnsubscribeFromAll { target_addr: SocketAddr },
    /// Sends an experiment configuration
    StartExperiment { experiment: Experiment },
    /// Stops an running experiment
    StopExperiment,
    /// Updates the status of an experiment
    UpdateExperimentInfo { info: ExperimentInfo },
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
    pub addr: SocketAddr,
    /// The unique ID of the host.
    pub host_id: HostId,
    /// The status of each device managed by the host.
    pub device_statuses: Vec<DeviceInfo>,
    /// How well the host is responding.
    pub responsiveness: Responsiveness,
}

impl From<HostStatus> for RegCtrl {
    fn from(value: HostStatus) -> Self {
        RegCtrl::HostStatus(value)
    }
}

/// Status information for a device managed by a host.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub struct DeviceInfo {
    /// The unique ID of the device.
    pub id: DeviceId,
    /// The type of the device/source.
    pub dev_type: SourceType,
}

impl From<&DeviceHandlerConfig> for DeviceInfo {
    fn from(value: &DeviceHandlerConfig) -> Self {
        DeviceInfo {
            id: value.device_id,
            dev_type: value.stype,
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
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
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
