use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::time::Duration;

use lib::errors::RegistryError;
use lib::handler::device_handler::DeviceHandlerConfig;
use lib::network::rpc_message::{HostId, RpcMessageKind};
use lib::network::tcp::client::TcpClient;
use log::LevelFilter;
use serde::Deserialize;

use crate::registry::RegHostStatus;

pub const DEFAULT_ADDRESS: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 6969));

/// A trait for parsing a YAML file into a struct using Serde.
///
/// This trait provides a generic method to deserialize a YAML file into any type that implements
/// the `Deserialize` trait from Serde. The method reads the contents of the specified file path,
/// and attempts to parse it into the desired type.
///
/// # Methods
/// - `fn from_yaml(file: PathBuf) -> Result<Self, Box<dyn std::error::Error>>`: Reads the YAML file at the given
///   path and deserializes it into the implementing type.
///
/// # Errors
/// Returns a `Box<dyn std::error::Error>` if the file cannot be read or if deserialization fails.
///
/// # Example
/// ```rust,ignore
/// let config: MyConfig = MyTrait::from_yaml(PathBuf::from("config.yaml"))?;
/// ```
pub trait FromYaml: Sized + for<'de> Deserialize<'de> {
    /// Loads an instance of the implementing type from a YAML file.
    ///
    /// # Arguments
    ///
    /// * `file` - The path to the YAML file to be read.
    ///
    /// # Returns
    ///
    /// * `Ok(Self)` if deserialization is successful.
    /// * `Err(Box<dyn std::error::Error>)` if reading or deserialization fails.
    ///
    /// # Panics
    ///
    /// This function will panic if the file cannot be read.
    fn from_yaml(file: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        let yaml = std::fs::read_to_string(file.clone()).map_err(|e| format!("Failed to read YAML file: {}\n{}", file.display(), e))?;
        Ok(serde_yaml::from_str(&yaml)?)
    }
}

/// The `Registry` trait defines the interface for a host registry service in a distributed system.
/// It provides asynchronous methods for polling hosts, retrieving and updating host information,
/// handling unresponsive hosts, and listing registered hosts and their statuses.
///
/// # Methods
/// - `poll_hosts`: Polls all registered hosts using the provided TCP client and interval. It is intended to be called periodically.
/// - `get_host_by_id`: Retrieves the status of a host by its unique identifier.
/// - `handle_unresponsive_host`: Handles a host that has become unresponsive.
/// - `list_hosts`: Returns a list of all registered hosts and their socket addresses.
/// - `list_host_statuses`: Returns a list of all registered hosts and their current statuses.
/// - `register_host`: Registers a new host with its identifier and address.
/// - `store_host_update`: Stores updates to a host's address or status.
///
/// # Errors
/// Methods may return errors if network communication fails, if a host cannot be found,
/// or if updates cannot be stored.
pub trait Registry {
    async fn poll_hosts(&self, client: TcpClient, poll_interval: Duration) -> Result<(), RegistryError>;
    async fn get_host_by_id(&self, host_id: HostId) -> Result<RegHostStatus, RegistryError>;
    async fn handle_unresponsive_host(&self, host_id: HostId) -> Result<(), RegistryError>;
    async fn list_hosts(&self) -> Vec<(HostId, SocketAddr)>;
    async fn list_host_statuses(&self) -> Vec<(HostId, RegHostStatus)>;
    async fn register_host(&self, host_id: HostId, host_address: SocketAddr) -> Result<(), RegistryError>;
    async fn store_host_update(&self, _host_id: HostId, host_address: SocketAddr, status: RpcMessageKind) -> Result<(), RegistryError>;
}

pub struct OrchestratorConfig {
    pub targets: Vec<SocketAddr>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct SystemNodeConfig {
    pub addr: SocketAddr,
    pub host_id: u64,
    pub registry: Option<SocketAddr>,
    pub device_configs: Vec<DeviceHandlerConfig>,
}

pub struct RegistryConfig {
    pub addr: SocketAddr,
    pub poll_interval: u64,
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
    // Initialize standalone state which does not depend on any config
    fn new(global_config: GlobalConfig, config: ServiceConfig) -> Self;

    // Actually applies given config and runs the service
    async fn run(&mut self) -> Result<(), Box<dyn std::error::Error>>;
}

impl FromYaml for SystemNodeConfig {}
