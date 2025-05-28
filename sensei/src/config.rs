use lib::handler::device_handler::DeviceHandlerConfig;
use serde::{Deserialize, Serialize};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::vec;

pub const DEFAULT_ADDRESS: SocketAddr =
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 6969));

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
        let yaml = std::fs::read_to_string(file.clone())
            .map_err(|e| format!("Failed to read YAML file: {}\n{}", file.display(), e))?;
        Ok(serde_yaml::from_str(&yaml)?)
    }
}

pub struct OrchestratorConfig {
    pub targets: Vec<SocketAddr>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct SystemNodeRegistryConfig {
    pub use_registry: bool,
    pub addr: SocketAddr,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct SystemNodeConfig {
    pub addr: SocketAddr,
    pub host_id: u64,
    pub registry: SystemNodeRegistryConfig,
    pub device_configs: Vec<DeviceHandlerConfig>,
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

impl FromYaml for SystemNodeRegistryConfig {}
impl FromYaml for SystemNodeConfig {}
