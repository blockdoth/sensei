use std::net::SocketAddr;
use crate::rpc_envelope::AdapterMode;

#[derive(Debug, Clone)]
pub struct Subscriber {
    pub socket_addr: SocketAddr,
    pub device_id: u64,
    pub adapter_mode: AdapterMode,
}

impl Subscriber {
    pub fn new(socket_addr: SocketAddr, device_id: u64, adapter_mode: AdapterMode) -> Self {
        Subscriber {
            socket_addr,
            device_id,
            adapter_mode,
        }
    }
}