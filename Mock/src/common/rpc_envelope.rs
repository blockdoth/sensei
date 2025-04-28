use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub enum RpcEnvelope {
    Ctrl(CtrlMsg),
    Data(DataMsg),
}

pub struct RadioConfig {
    pub channel: u16,
    pub bandwidth: Bandwidth, // Ht20 | Ht40 | Ht80
    pub chains: u8, // bit-mask
    pub vendor: Option<VendorOpt>, // ESP-only quirks, etc.
}

enum Bandwidth {
    Ht20, Ht40, Ht80
}

#[non_exhaustive]
pub enum VendorOpt {
    Esp32( Opts),
    Nexmon( Opts),
}
#[derive(Serialize, Deserialize)]
pub enum CtrlMsg {
    Configure { device: DeviceId, cfg: RadioConfig },
    Subscribe { device: DeviceId, mode: AdapterMode },
    Unsubscribe { stream: StreamId },
}

#[derive(Serialize, Deserialize)]
pub enum DataMsg {
    RawFrame { ts: u64, bytes: Vec<u8> }, // raw bytestream, requires decoding adapter
    CsiFrame { ts: u64, csi: Vec<Complex<f32>> }, // This would contain a proper deserialized CSI
}

