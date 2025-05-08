use crate::csi_types::CsiData;
use std::{net::SocketAddr, sync::Arc};
use tokio::net::UdpSocket;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub enum RpcEnvelope {
    Ctrl(CtrlMsg),
    Data(DataMsg),
}

#[derive(Serialize, Deserialize, Debug)]
pub enum CtrlMsg {
    Configure {
        device_id: u64,
        cfg: RadioConfig,
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
        ts: u128,
        bytes: Vec<u8>,
        source_type: SourceType,
    }, // raw bytestream, requires decoding adapter
    CsiFrame {
        ts: u128,
        csi: CsiData,
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

#[derive(Serialize, Deserialize, Debug)]
pub struct RadioConfig {}

impl std::str::FromStr for AdapterMode {
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

pub fn serialize_envelope(env: RpcEnvelope) -> Vec<u8> {
    bincode::serialize(&env).expect("Failed to serialize rpc envelope")
}

pub fn deserialize_envelope(buf: &[u8]) -> RpcEnvelope {
    bincode::deserialize(buf).expect("Failed to deserialize rpc envelope")
}

pub async fn send_envelope(
    send_socket: Arc<UdpSocket>,
    envelope: Vec<u8>,
    socket_addr: SocketAddr,
) {
    match send_socket.send_to(&envelope, socket_addr).await {
        Ok(n) => println!("Sent {n} bytes to {socket_addr}"),
        Err(e) => eprintln!("Failed to send: {e}"),
    }
}
