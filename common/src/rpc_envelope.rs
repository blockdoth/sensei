use serde::{Deserialize, Serialize};

pub mod adapter_mode;
pub mod radio_config;

#[derive(Serialize, Deserialize, Debug)]
pub enum RpcEnvelope {
    Ctrl(CtrlMsg),
    Data(DataMsg),
}

#[derive(Serialize, Deserialize, Debug)]
pub enum CtrlMsg {
    Configure {
        device_id: u64,
        cfg: radio_config::RadioConfig,
    },
    Subscribe {
        sink_addr: String,
        device_id: u64,
        mode: adapter_mode::AdapterMode,
    },
    Unsubscribe {
        sink_addr: String,
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

pub fn serialize_envelope(env: RpcEnvelope) -> Vec<u8> {
    bincode::serialize(&env).expect("Failed to serialize rpc envelope")
}

pub fn deserialize_envelope(buf: &[u8]) -> RpcEnvelope {
    bincode::deserialize(buf).expect("Failed to deserialize rpc envelope")
}
