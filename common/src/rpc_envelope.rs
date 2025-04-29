use serde::{Deserialize, Serialize};

mod radio_config;
mod adapter_mode;

#[derive(Serialize, Deserialize)]
pub enum RpcEnvelope {
    Ctrl(CtrlMsg),
    Data(DataMsg),
}

#[derive(Serialize, Deserialize)]
pub enum CtrlMsg {
    Configure { device: u64, cfg: radio_config::RadioConfig },
    Subscribe { device: u64, mode: adapter_mode::AdapterMode },
    Unsubscribe { stream: u64 },
}

#[derive(Serialize, Deserialize)]
pub enum DataMsg {
    RawFrame { ts: u64, bytes: Vec<u8> }, // raw bytestream, requires decoding adapter
    CsiFrame { ts: u64, csi: Vec<f32> }, // This would contain a proper deserialized CSI
}