use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;

use crate::csi_types::CsiData;
use crate::handler::device_handler::CfgType;

pub const DEFAULT_ADDRESS: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 6969));

#[derive(Serialize, Deserialize, Debug)]
pub struct RpcMessage {
    pub msg: RpcMessageKind,
    pub src_addr: SocketAddr,
    pub target_addr: SocketAddr,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum RpcMessageKind {
    Ctrl(CtrlMsg),
    Data { data_msg: DataMsg, device_id: u64 },
}

/// There was some discussion about what we should use as a host id.
/// This makes it more flexible
pub type HostId = u64;
pub type DeviceId = u64;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum CtrlMsg {
    Connect,
    Disconnect,
    Configure { device_id: DeviceId, cfg_type: CfgType },
    Subscribe { device_id: DeviceId },
    Unsubscribe { device_id: DeviceId },
    SubscribeTo { target: SocketAddr, device_id: DeviceId }, // Orchestrator to node, node subscribes to another node
    UnsubscribeFrom { target: SocketAddr, device_id: DeviceId }, // Orchestrator to node
    PollHostStatus { host_id: HostId },
    PollHostStatuses,
    AnnouncePresence { host_id: HostId, host_address: SocketAddr },
    HostStatus { host_id: HostId, device_status: Vec<DeviceInfo> },
    HostStatuses { host_statuses: Vec<CtrlMsg> },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct DeviceInfo {
    pub id: DeviceId,
    pub dev_type: SourceType,
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
impl FromStr for CtrlMsg {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let lowercase = s.to_lowercase();
        let mut parts = lowercase.split_whitespace();
        let kind = parts.next().unwrap_or("not a valid command");

        match kind {
            "connect" => Ok(CtrlMsg::Connect),
            "disconnect" => Ok(CtrlMsg::Disconnect),
            "configure" => {
                todo!("support this")
            }
            "subscribe" => {
                let device_id = parts.next().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0); // TODO better id assignment

                Ok(CtrlMsg::Subscribe { device_id })
            }
            "unsubscribe" => {
                let device_id = parts.next().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);

                Ok(CtrlMsg::Unsubscribe { device_id })
            }
            "pollhoststatus" => {
                let host_id = parts.next().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
                Ok(CtrlMsg::PollHostStatus { host_id })
            }
            "pollhoststatuses" => Ok(CtrlMsg::PollHostStatuses),
            "hoststatus" => Ok(CtrlMsg::HostStatus {
                host_id: 0,
                device_status: vec![],
            }),
            "heartbeat" => Ok(CtrlMsg::AnnouncePresence {
                host_id: 0,
                host_address: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 8080)),
            }), // TODO better id assignment
            s => Err(s.to_owned()),
            _ => Err(format!("An unsuppored case was reached! {kind}")),
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
