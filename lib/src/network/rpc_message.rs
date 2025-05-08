use crate::devices::DeviceCfg;
use bincode::Error;
use serde::{Deserialize, Serialize};
use std::{
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    str::FromStr,
    sync::Arc,
};
use tokio::net::UdpSocket;

#[derive(Serialize, Deserialize, Debug)]
pub enum RpcMessage {
    Ctrl(CtrlMsg),
    Data(DataMsg),
}

#[derive(Serialize, Deserialize, Debug)]
pub enum CtrlMsg {
    Configure {
        device_id: u64,
        cfg: DeviceCfg,
    },
    Subscribe {
        sink_addr: SocketAddr,
        device_id: u64,
        mode: AdapterMode,
    },
    Unsubscribe {
        sink_addr: SocketAddr,
        device_id: u64,
    },
    PollDevices,
    Heartbeat,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum DataMsg {
    RawFrame {
        ts: u64,
        bytes: Vec<u8>,
        source_type: SourceType,
    }, // raw bytestream, requires decoding adapter
    CsiFrame {
        ts: u64,
        csi: Vec<f32>,
    }, // This would contain a proper deserialized CSI
}
#[derive(Serialize, Deserialize, Debug)]
pub enum SourceType {
    ESP32,
    IWL5300,
    AX200,
    AX210,
    AtherosQCA,
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

const DEFAULT_ADDRESS: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 6969));

// FromStr implementations for easy cli usage
impl FromStr for CtrlMsg {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let lowercase = s.to_lowercase();
        let mut parts = lowercase.split_whitespace();
        let kind = parts.next().unwrap_or("not a valid command");

        match kind {
            "configure" => {
                todo!("support this")
            }
            "subscribe" => {
                let addr = parts
                    .next()
                    .and_then(|s| s.parse::<SocketAddr>().ok())
                    .unwrap_or(DEFAULT_ADDRESS);

                let device_id = parts
                    .next()
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(0); // TODO better id assignment

                let mode = parts
                    .next()
                    .and_then(|s| s.parse::<AdapterMode>().ok())
                    .unwrap_or(AdapterMode::RAW);

                Ok(CtrlMsg::Subscribe {
                    sink_addr: addr,
                    device_id,
                    mode,
                })
            }
            "unsubscribe" => {
                let addr = parts
                    .next()
                    .and_then(|s| s.parse::<SocketAddr>().ok())
                    .unwrap_or(DEFAULT_ADDRESS);

                let device_id = parts
                    .next()
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(0);

                Ok(CtrlMsg::Unsubscribe {
                    sink_addr: addr,
                    device_id,
                })
            }
            "polldevices" => Ok(CtrlMsg::PollDevices),
            "heartbeat" => Ok(CtrlMsg::Heartbeat),
            s => Err(s.to_owned()),
        }
    }
}
