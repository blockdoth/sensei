use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;

use crate::csi_types::CsiData;
use crate::handler::device_handler::{CfgType, DeviceHandlerConfig};

pub const DEFAULT_ADDRESS: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 6969));

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RpcMessage {
    pub msg: RpcMessageKind,
    pub src_addr: SocketAddr,
    pub target_addr: SocketAddr,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum RpcMessageKind {
    HostCtrl(HostCtrl),
    RegCtrl(RegCtrl),
    Data { data_msg: DataMsg, device_id: u64 },
}

/// There was some discussion about what we should use as a host id.
/// This makes it more flexible
pub type HostId = u64;
pub type DeviceId = u64;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum HostCtrl {
    Connect,
    Disconnect,
    Configure { device_id: DeviceId, cfg_type: CfgType },
    Subscribe { device_id: DeviceId },
    Unsubscribe { device_id: DeviceId },
    SubscribeTo { target: SocketAddr, device_id: DeviceId }, // Orchestrator to node, node subscribes to another node
    UnsubscribeFrom { target: SocketAddr, device_id: DeviceId }, // Orchestrator to node
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum RegCtrl {
    PollHostStatus { host_id: HostId },
    PollHostStatuses,
    AnnouncePresence { host_id: HostId, host_address: SocketAddr },
    HostStatus { host_id: HostId, device_status: Vec<DeviceStatus> },
    HostStatuses { host_statuses: Vec<RegCtrl> },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct DeviceStatus {
    pub id: DeviceId,
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

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum DataMsg {
    RawFrame { ts: f64, bytes: Vec<u8>, source_type: SourceType }, // raw bytestream, requires decoding adapter
    CsiFrame { csi: CsiData },                                     // This would contain a proper deserialized CSI
}
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum SourceType {
    ESP32,
    IWL5300,
    AX200,
    AX210,
    AtherosQCA,
    CSV,
    TCP,
    Unknown,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub enum AdapterMode {
    RAW,
    SOURCE,
    TARGET,
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
            "hoststatus" => Ok(RegCtrl::HostStatus {
                host_id: 0,
                device_status: vec![],
            }),
            "heartbeat" => Ok(RegCtrl::AnnouncePresence {
                host_id: 0,
                host_address: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 8080)),
            }), // TODO better id assignment
            s => Err(s.to_owned()),
        }
    }
}

// Convenient wrapper to add src/target data to RpcMessage's
// Takes any type that implements AsRef, such as TcpStream/OwnedReadHalf/OwnedWriteHalf
// as refs (&), Arc<>, Box<> and Rc<>
pub fn make_msg<S: AsRef<TcpStream>>(stream: S, msg: RpcMessageKind) -> RpcMessage {
    let stream_ref = stream.as_ref();
    RpcMessage {
        msg,
        src_addr: stream_ref.local_addr().unwrap(),
        target_addr: stream_ref.peer_addr().unwrap(),
    }
}
