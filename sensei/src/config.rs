use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;

pub const DEFAULT_ADDRESS: SocketAddr =
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 6969));

pub struct OrchestratorConfig {
    pub targets: Vec<SocketAddr>,
}

pub struct SystemNodeConfig {
    pub addr: SocketAddr,
    pub device_configs: PathBuf,
}

pub struct RegistryConfig {
    pub addr: SocketAddr,
}

pub struct VisualiserConfig {
    pub target: SocketAddr,
    pub ui_type: String,
}

pub enum ServiceConfig {
    One(OrchestratorConfig),
    Two(RegistryConfig),
    Three(SystemNodeConfig),
    Four(VisualiserConfig),
}
