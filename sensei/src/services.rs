//! # Sensei Service Configurations and Traits
//!
//! This module defines the core configurations and traits for various services
//! within the Sensei application. It includes:
//!
//! - **Configuration Structs**: Typed configurations for different services like
//!   `OrchestratorConfig`, `SystemNodeConfig`, `VisualiserConfig`, and `EspToolConfig`.
//!   These structs are used to pass settings and parameters to their respective services.
//! - **`GlobalConfig`**: A struct for global application settings, such as the logging level.
//! - **`ServiceConfig` Enum**: An enumeration to represent the different types of service
//!   configurations, allowing for type-safe handling of various service setups.
//! - **`FromYaml` Trait**: A utility trait for deserializing configurations from YAML files.
//!   This promotes a consistent way of loading settings across different components.
//! - **`Run` Trait**: A fundamental trait that defines the lifecycle of a service.
//!   Implementors of this trait can be initialized with global and service-specific
//!   configurations and then started to perform their designated tasks.
//!
//! The module aims to provide a clear and structured way to manage service-specific
//! settings and their execution flow.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;

use lib::handler::device_handler::DeviceHandlerConfig;
use log::LevelFilter;
use serde::Deserialize;

use crate::system_node::SinkConfigWithName;

pub const DEFAULT_ADDRESS: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 6969));

/// A trait for parsing a YAML file into a struct using Serde.
///
/// This trait provides a generic method to deserialize a YAML file into any type that implements
/// the `Deserialize` trait from Serde. The method reads the contents of the specified file path,
/// and attempts to parse it into the desired type.
///
/// # Type Parameters
///
/// - `Self`: The type that will be deserialized from the YAML file. Must implement `Sized`
///   and `serde::Deserialize`.
///
/// # Methods
/// - `fn from_yaml(file: PathBuf) -> Result<Self, Box<dyn std::error::Error>>`: Reads the YAML file at the given
///   path and deserializes it into the implementing type.
///
/// # Errors
/// Returns a `Box<dyn std::error::Error>` if the file cannot be read or if deserialization fails.
///
/// # Example
///
/// ```rust,ignore
/// use std::path::PathBuf;
/// use serde::Deserialize;
/// use sensei::services::FromYaml; // Assuming FromYaml is in a crate named sensei
///
/// #[derive(Deserialize)]
/// struct MyConfig {
///     setting: String,
/// }
///
/// impl FromYaml for MyConfig {}
///
/// fn load_config() -> Result<MyConfig, Box<dyn std::error::Error>> {
///     let config: MyConfig = MyConfig::from_yaml(PathBuf::from("config.yaml"))?;
///     Ok(config)
/// }
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

/// Configuration for the Orchestrator service.
pub struct OrchestratorConfig {
    /// Path to the experiment configuration file.
    pub experiments_folder: PathBuf,
    pub tui: bool,
}

/// Configuration for a System Node service.
///
/// This struct holds all necessary settings for a system node,
/// including its network address, unique ID, registry information,
/// device configurations, and sink configurations.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct SystemNodeConfig {
    pub addr: SocketAddr,
    pub host_id: u64,
    pub registries: Option<Vec<SocketAddr>>,
    pub registry_polling_rate_s: Option<u64>,
    pub device_configs: Vec<DeviceHandlerConfig>,
    #[serde(default)]
    pub sinks: Vec<SinkConfigWithName>,
}

/// Configuration for the Visualiser service.
pub struct VisualiserConfig {
    /// The network address of the target service (e.g., a System Node or Orchestrator)
    /// from which the visualiser will fetch data.
    pub target: SocketAddr,
    /// The type of user interface to use for visualization (e.g., "tui", "gui").
    pub ui_type: String,
}

/// Configuration for the ESP Tool service.
///
/// Contains settings related to interacting with ESP-based devices,
/// primarily the serial port for communication.
pub struct EspToolConfig {
    /// The serial port path (e.g., "/dev/ttyUSB0" or "COM3") to use for communicating
    /// with the ESP device.
    pub serial_port: String,
}

/// Global configuration applicable to all services.
pub struct GlobalConfig {
    /// The logging level filter to be applied across the application.
    pub log_level: LevelFilter,
}

/// An enum representing the configuration for any of the available services.
pub enum ServiceConfig {
    /// Configuration for the Orchestrator service.
    Orchestrator(OrchestratorConfig),
    /// Configuration for a System Node service.
    SystemNode(SystemNodeConfig),
    /// Configuration for the Visualiser service.
    Visualiser(VisualiserConfig),
    /// Configuration for the ESP Tool service.
    EspTool(EspToolConfig),
}

/// A trait defining the runnable lifecycle of a service.
///
/// Services implementing this trait can be initialized with global and service-specific
/// configurations, and then started to perform their designated tasks.
pub trait Run<Config> {
    /// Creates a new instance of the service.
    ///
    /// This method should initialize any standalone state of the service depending on the specific `Config`.
    ///
    /// # Arguments
    ///
    /// * `global_config`: Global application settings.
    /// * `config`: Service-specific configuration.
    fn new(global_config: GlobalConfig, config: Config) -> Self;

    /// Runs the service.
    ///
    /// This method applies the given configuration and starts the main execution
    /// loop or primary task of the service. It is an asynchronous operation.
    ///
    /// # Returns
    ///
    /// * `Ok(())` if the service runs and completes successfully (or is designed to run indefinitely
    ///   and is gracefully shut down).
    /// * `Err(Box<dyn std::error::Error>)` if an error occurs during the service's execution.
    async fn run(&mut self) -> Result<(), Box<dyn std::error::Error>>;
}

impl FromYaml for SystemNodeConfig {}
