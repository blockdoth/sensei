use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;

use log::LevelFilter;
use serialport::SerialPort;

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
    pub targets: Vec<SocketAddr>,
}

pub struct VisualiserConfig {
    pub target: SocketAddr,
    pub ui_type: String,
}

pub struct EspToolConfig {
    pub serial_port: String,
}

pub struct GlobalConfig {
    pub log_level: LevelFilter,
}

pub enum ServiceConfig {
    One(OrchestratorConfig),
    Two(RegistryConfig),
    Three(SystemNodeConfig),
    Four(VisualiserConfig),
    Five(EspToolConfig),
}

pub trait Run<ServiceConfig> {
    fn new() -> Self;
    async fn run(
        &mut self,
        global_config: GlobalConfig,
        config: ServiceConfig,
    ) -> Result<(), Box<dyn std::error::Error>>;
}
