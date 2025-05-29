use crate::csi_types::CsiData;
use crate::handler::device_handler::DeviceHandlerConfig;
use crate::network::rpc_message::RpcMessageKind::Ctrl;
use bincode::Error;
use netlink_sys::Socket;
use serde::{Deserialize, Serialize};
use std::{
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    str::FromStr,
    sync::Arc,
};
use tokio::net::{TcpStream, UdpSocket};
use tokio_stream::Stream;

const DEFAULT_ADDRESS: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 6969));

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

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum CtrlMsg {
    Connect,
    Disconnect,
    Configure {
        device_id: u64,
        cfg: DeviceHandlerConfig,
    },
    Subscribe {
        device_id: u64,
        mode: AdapterMode,
    },
    Unsubscribe {
        device_id: u64,
    },
    PollHostStatus,
    Heartbeat {
        host_id: u64,
        host_address: SocketAddr,
    },
    /// This data message contains all the satus information a host shares with the registry
    HostStatus {
        host_id: u64,
        device_status: Vec<DeviceStatus>, // (device_id, status)
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DeviceStatus {
    pub d_id: u64,
    pub d_type: SourceType,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum DataMsg {
    RawFrame {
        ts: f64,
        bytes: Vec<u8>,
        source_type: SourceType,
    }, // raw bytestream, requires decoding adapter
    CsiFrame {
        csi: CsiData,
    }, // This would contain a proper deserialized CSI
}
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum SourceType {
    ESP32,
    IWL5300,
    AX200,
    AX210,
    AtherosQCA,
    CSV,
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
                let device_id = parts
                    .next()
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(0); // TODO better id assignment

                let mode = parts
                    .next()
                    .and_then(|s| s.parse::<AdapterMode>().ok())
                    .unwrap_or(AdapterMode::RAW);

                Ok(CtrlMsg::Subscribe { device_id, mode })
            }
            "unsubscribe" => {
                let device_id = parts
                    .next()
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(0);

                Ok(CtrlMsg::Unsubscribe { device_id })
            }
            "polldevices" => Ok(CtrlMsg::PollHostStatus),
            "heartbeat" => Ok(CtrlMsg::Heartbeat {
                host_id: 0,
                host_address: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 0)),
            }), // TODO better id assignment
            s => Err(s.to_owned()),
            _ => {
                todo!("An unsuppored case was reached!")
            }
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
