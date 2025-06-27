//! # Command-Line Interface (CLI) Argument Parsing
//!
//! This module defines the command-line argument structures and parsing logic
//! for the Sensei application. It uses the `argh` crate for parsing arguments
//! and provides structures for global arguments, subcommands, and their respective
//! configurations.
//!
//! The module includes:
//! - `Args`: The main struct for top-level arguments, including log level and subcommands.
//! - `SubCommandsArgs`: An enum representing the available subcommands (SystemNode, Orchestrator, Visualiser, EspTool).
//! - Structs for each subcommand's arguments (e.g., `SystemNodeSubcommandArgs`).
//! - Traits `ConfigFromCli` and `OverlaySubcommandArgs` for handling configuration loading and merging
//!   with command-line overrides.
//!
//! Default configuration paths are also defined here.
//!
//! # Example
//!
//! To run the application with a specific subcommand and options:
//!
//! ```sh
//! cargo run node --addr 192.168.1.10 --port 7070
//! ```
//!
//! This command sets the system node address and port, overriding the defaults.
#[cfg(feature = "registry")]
#[cfg(feature = "sys_node")]
use std::path::PathBuf;
use std::vec;

use argh::FromArgs;
#[cfg(feature = "sys_node")]
use lib::handler::device_handler::DeviceHandlerConfig;
use lib::network::rpc_message::{DEFAULT_ADDRESS, DeviceId};
#[cfg(feature = "sys_node")]
use log::debug;
use serde::{Deserialize, Serialize};
use simplelog::LevelFilter;

#[cfg(feature = "esp_tool")]
use crate::services::EspToolConfig;
use crate::services::GlobalConfig;
#[cfg(feature = "orchestrator")]
use crate::services::OrchestratorConfig;
#[cfg(feature = "registry")]
use crate::services::RegistryConfig;
#[cfg(feature = "sys_node")]
use crate::services::SystemNodeConfig;
#[cfg(feature = "visualiser")]
use crate::services::VisualiserConfig;
use crate::visualiser::state::{AmplitudeConfig, Graph, GraphConfig, PDPConfig};

/// Default path to the host configuration YAML file.
pub static DEFAULT_HOST_CONFIG: &str = "examples/default/node.yaml";
/// Default path to the orchestrator configuration YAML file.
pub static DEFAULT_ORCHESTRATOR_CONFIG: &str = "examples/default/orchestrator.yaml";
pub static DEFAULT_REGISTRY_CONFIG: &str = "examples/default/registry.yaml";
pub static DEFAULT_VISUALIZER_CONFIG: &str = "examples/default/visualiser.yaml";
pub static DEFAULT_EXPERIMENT_CONFIGS: &str = "examples/experiments/";
pub static DEFAULT_ORCH_POLL_INTERVAL: u64 = 5;

/// A trait for overlaying subcommand arguments onto an existing configuration.
///
/// This trait allows reads the config from the YAML file specified in the subcommand arguments,
/// and then overlays the subcommand arguments onto that configuration.
///
/// # Type Parameters
/// - `T`: The type of the configuration object to overlay.
///
/// # Example
/// Implement this trait for your subcommand argument struct to merge its values
/// into a configuration loaded from disk.
///
/// # Returns
/// A new configuration object with subcommand arguments applied.
pub trait MergeWithConfig<T> {
    /// Overlays fields from subcommand arguments onto a configuration loaded from a YAML file.
    ///
    /// If a field is specified in the subcommand arguments, it will override the corresponding
    /// value from the YAML configuration. Otherwise, the YAML configuration value is retained.
    fn merge_with_config(&self, config_file: T) -> T;
}

/// A simple app to perform collection from configured sources
#[derive(FromArgs)]
pub struct Args {
    /// log level to use for terminal logging
    /// Possible values: OFF, ERROR, WARN, INFO, DEBUG, TRACE
    #[argh(option, default = "LevelFilter::Info")]
    pub level: LevelFilter,

    /// the number of workers sensei will be started with.
    #[argh(option, default = "4")]
    pub num_workers: usize,

    #[argh(subcommand)]
    pub subcommand: Option<SubCommandsArgs>,
}

impl Args {
    /// Parses the global configuration from the command-line arguments.
    ///
    /// Currently, this primarily involves extracting the log level.
    pub fn parse_global_config(&self) -> Result<GlobalConfig, Box<dyn std::error::Error>> {
        Ok(GlobalConfig {
            log_level: self.level,
            num_workers: self.num_workers,
        })
    }
}

#[derive(FromArgs)]
#[argh(subcommand)]
pub enum SubCommandsArgs {
    #[cfg(feature = "sys_node")]
    SystemNode(SystemNodeSubcommandArgs),
    #[cfg(feature = "orchestrator")]
    Orchestrator(OrchestratorSubcommandArgs),
    #[cfg(feature = "visualiser")]
    Visualiser(VisualiserSubcommandArgs),
    #[cfg(feature = "esp_tool")]
    EspTool(EspToolSubcommandArgs),
    #[cfg(feature = "registry")]
    Registry(RegistrySubcommandArgs),
}

const DEFAULT_PORT_CLI: u16 = 6969;
const DEFAULT_IP_CLI: &str = "127.0.0.1";

/// System node commands
#[cfg(feature = "sys_node")]
#[derive(FromArgs)]
#[argh(subcommand, name = "node")]
pub struct SystemNodeSubcommandArgs {
    /// server address (default: 127.0.0.1)
    #[argh(option, default = "DEFAULT_IP_CLI.to_owned()")]
    pub address: String,

    /// server port (default: 6969)
    #[argh(option, default = "DEFAULT_PORT_CLI")]
    pub port: u16,

    /// location of config file
    #[argh(option, default = "PathBuf::from(DEFAULT_HOST_CONFIG)")]
    pub config_path: PathBuf,
}

#[cfg(feature = "sys_node")]
impl From<&SystemNodeSubcommandArgs> for SystemNodeConfig {
    /// Parses system node subcommand arguments into a `SystemNodeConfig`.
    ///
    /// This involves constructing an address from `addr` and `port` fields,
    /// and loading device configurations from the specified `config_path`.
    /// Default values are used for fields not directly provided by arguments.
    fn from(args: &SystemNodeSubcommandArgs) -> Self {
        SystemNodeConfig {
            address: format!("{}:{}", args.address, args.port).parse().unwrap_or(DEFAULT_ADDRESS),
            device_configs: DeviceHandlerConfig::from_yaml(args.config_path.clone()).unwrap_or_default(),
            host_id: 0,                      // Default host_id, might be overwritten by YAML or other logic
            registry: None,                  // Default, might be overwritten by YAML
            registry_polling_interval: None, // Default, might be overwritten by YAML
            sinks: Vec::new(),               // Initialize with an empty Vec, to be populated from YAML
        }
    }
}

#[cfg(feature = "sys_node")]
/// Overlays subcommand arguments onto a SystemNodeConfig, overriding fields if provided.
impl MergeWithConfig<SystemNodeConfig> for SystemNodeSubcommandArgs {
    fn merge_with_config(&self, mut device_config: SystemNodeConfig) -> SystemNodeConfig {
        // Because of the default value we expact that there's always a file to read
        debug!("Loading system node configuration from YAML file: {}", self.config_path.display());
        // overwrite fields when set by the CLI while keeping CLI defaults
        if self.address != DEFAULT_IP_CLI {
            device_config.address.set_ip(self.address.parse().unwrap_or(device_config.address.ip()));
        }
        if self.port != DEFAULT_PORT_CLI {
            device_config.address.set_port(self.port);
        }
        device_config
    }
}

#[cfg(feature = "orchestrator")]
/// Orchestrator node commands
#[derive(FromArgs)]
#[argh(subcommand, name = "orchestrator")]
pub struct OrchestratorSubcommandArgs {
    /// whether to enable tui input
    #[argh(option, default = "true")]
    pub tui: bool,

    /// file path of the experiment config
    #[argh(option, default = "DEFAULT_EXPERIMENT_CONFIGS.parse().unwrap()")]
    pub experiments_dir: PathBuf,

    /// polling interval of the registry
    #[argh(option, default = "DEFAULT_ORCH_POLL_INTERVAL")]
    pub polling_interval: u64,

    /// file path of the experiment config
    #[argh(option, default = "DEFAULT_ORCHESTRATOR_CONFIG.parse().unwrap()")]
    pub config_path: PathBuf,

}

#[cfg(feature = "orchestrator")]
impl From<&OrchestratorSubcommandArgs> for OrchestratorConfig {
    fn from(args: &OrchestratorSubcommandArgs) -> Self {
        OrchestratorConfig {
            experiments_dir: args.experiments_dir.clone(),
            tui: Some(args.tui),
            polling_interval: args.polling_interval,
            default_hosts: Vec::new(),
            registry: DEFAULT_ADDRESS
        }
    }
}

#[cfg(feature = "orchestrator")]
/// Overlays subcommand arguments onto a SystemNodeConfig, overriding fields if provided.
impl MergeWithConfig<OrchestratorConfig> for OrchestratorSubcommandArgs {
    fn merge_with_config(&self, mut device_config: OrchestratorConfig) -> OrchestratorConfig {
        // Because of the default value we expact that there's always a file to read
        debug!("Loading orchestrator node configuration from YAML file: {:?}", self.config_path);
        // overwrite fields when set by the CLI while keeping CLI defaults
        if self.polling_interval != DEFAULT_ORCH_POLL_INTERVAL {
            device_config.polling_interval = self.polling_interval;
        }
        if self.experiments_dir != DEFAULT_EXPERIMENT_CONFIGS.parse::<PathBuf>().unwrap() {
            device_config.experiments_dir = self.experiments_dir.clone();
        }

        device_config
    }
}

/// Visualiser commands
#[cfg(feature = "visualiser")]
#[derive(FromArgs)]
#[argh(subcommand, name = "visualiser")]
pub struct VisualiserSubcommandArgs {
    /// server port (default: 6969)
    #[argh(option, default = "String::from(\"127.0.0.1:6969\")")]
    pub target: String,

    /// graph update interval (default: 6969)
    #[argh(option, default = "100")]
    pub update_interval: usize,

    /// file path of the visualiser config
    #[argh(option, default = "DEFAULT_VISUALIZER_CONFIG.parse().unwrap()")]
    pub config_path: PathBuf,
}

#[cfg(feature = "visualiser")]
impl From<&VisualiserSubcommandArgs> for VisualiserConfig {
    fn from(args: &VisualiserSubcommandArgs) -> Self {
        VisualiserConfig {
            target: args.target.parse().unwrap_or(DEFAULT_ADDRESS),
            graphs: vec![],
            update_interval: args.update_interval,
        }
    }
}

#[cfg(feature = "visualiser")]
/// Overlays subcommand arguments onto a VisualiserConfig, overriding fields if provided.
impl MergeWithConfig<VisualiserConfig> for VisualiserSubcommandArgs {
    fn merge_with_config(&self, mut full_config: VisualiserConfig) -> VisualiserConfig {
        // Because of the default value we expact that there's always a file to read
        debug!("Loading visualizer configuration from YAML file: {}", self.config_path.display());

        if self.target != DEFAULT_ADDRESS.to_string()
            && let Ok(target) = self.target.parse()
        {
            full_config.target = target
        }

        full_config
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub enum GraphConfigWithId {
    Amplitude(AmplitudeConfigWithId),
    #[allow(clippy::upper_case_acronyms)]
    PDP(PDPConfigWithId),
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub struct AmplitudeConfigWithId {
    pub device_id: DeviceId,
    pub core: usize,
    pub stream: usize,
    pub subcarrier: usize,
    pub time_range: usize,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub struct PDPConfigWithId {
    pub device_id: DeviceId,
    pub core: usize,
    pub stream: usize,
    pub y_axis_bounds: [f64; 2],
}

impl From<GraphConfigWithId> for Graph {
    fn from(graph_with_id: GraphConfigWithId) -> Self {
        match graph_with_id {
            GraphConfigWithId::Amplitude(AmplitudeConfigWithId {
                device_id,
                core,
                stream,
                subcarrier,
                time_range,
            }) => Graph {
                gtype: GraphConfig::Amplitude(AmplitudeConfig {
                    core,
                    stream,
                    subcarrier,
                    time_range,
                }),
                device_id,
                data: vec![],
            },

            GraphConfigWithId::PDP(PDPConfigWithId {
                device_id,
                core,
                stream,
                y_axis_bounds,
            }) => Graph {
                gtype: GraphConfig::PDP(PDPConfig { core, stream, y_axis_bounds }),
                device_id,
                data: vec![],
            },
        }
    }
}

/// Arguments for the ESP Test Tool subcommand
#[cfg(feature = "esp_tool")]
#[derive(FromArgs, Debug, Clone)]
#[argh(subcommand, name = "esp-tool")]
pub struct EspToolSubcommandArgs {
    /// serial port
    #[argh(option, default = "String::from(\"/dev/ttyUSB0\")")]
    pub serial_port: String,
}

#[cfg(feature = "registry")]
impl From<&EspToolSubcommandArgs> for EspToolConfig {
    fn from(args: &EspToolSubcommandArgs) -> Self {
        EspToolConfig {
            serial_port: args.serial_port.clone(),
        }
    }
}

/// Registry node commands
#[cfg(feature = "registry")]
#[derive(FromArgs)]
#[argh(subcommand, name = "registry")]
pub struct RegistrySubcommandArgs {
    /// registry address (default: 127.0.0.1:6969)
    #[argh(option, default = "format!(\"{DEFAULT_IP_CLI}:{DEFAULT_PORT_CLI}\")")]
    pub address: String,

    /// server port (default: 6969)
    #[argh(option, default = "6969")]
    pub port: u16,

    /// polling_interval (default: 5 sec)
    #[argh(option, default = "5")]
    pub polling_interval: u64,

    /// path to registry config file
    #[argh(option, default = "PathBuf::from(DEFAULT_REGISTRY_CONFIG)")]
    pub config_path: PathBuf,
}

#[cfg(feature = "registry")]
impl From<&RegistrySubcommandArgs> for RegistryConfig {
    fn from(args: &RegistrySubcommandArgs) -> Self {
        RegistryConfig {
            address: args.address.parse().unwrap_or(DEFAULT_ADDRESS),
            polling_interval: args.polling_interval,
        }
    }
}

#[cfg(feature = "sys_node")]
/// Overlays subcommand arguments onto a SystemNodeConfig, overriding fields if provided.
impl MergeWithConfig<RegistryConfig> for RegistrySubcommandArgs {
    fn merge_with_config(&self, mut full_config: RegistryConfig) -> RegistryConfig {
        // Because of the default value we expact that there's always a file to read
        debug!("Loading system node configuration from YAML file: {}", self.config_path.display());
        if self.polling_interval != DEFAULT_ORCH_POLL_INTERVAL {
            full_config.polling_interval = self.polling_interval;
        }

        if self.address != DEFAULT_IP_CLI {
            full_config.address.set_ip(self.address.parse().unwrap_or(full_config.address.ip()));
        }
        if self.port != DEFAULT_PORT_CLI {
            full_config.address.set_port(self.port);
        }

        full_config
    }
}

// Tests

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, SocketAddr};

    use super::*;

    fn create_testing_config() -> SystemNodeConfig {
        SystemNodeConfig {
            address: SocketAddr::new(IpAddr::V4(DEFAULT_IP_CLI.parse().unwrap()), DEFAULT_PORT_CLI),
            host_id: 1,
            registry: Option::None,
            device_configs: vec![],
            registry_polling_interval: Option::None,
            sinks: Vec::new(), // Add sinks field
        }
    }

    #[test]
    fn test_overlay_subcommand_args_overwrites_addr_and_port() {
        let args = SystemNodeSubcommandArgs {
            address: "10.0.0.1".to_string(),
            port: 4321,
            config_path: PathBuf::from("does_not_matter.yaml"), // This will not be used
        };

        let base_cfg = create_testing_config();
        let config = args.merge_with_config(base_cfg);
        assert_eq!(config.address, "10.0.0.1:4321".parse().unwrap());
    }

    #[test]
    fn test_overlay_subcommand_args_uses_yaml_when_no_override() {
        let args = SystemNodeSubcommandArgs {
            address: DEFAULT_IP_CLI.to_string(),
            port: DEFAULT_PORT_CLI,
            config_path: PathBuf::from("does_not_matter.yaml"), // This will not be used
        };

        // If addr is empty and port is 0, overlay will parse ":0" which is invalid,
        // so we expect it to fallback to the YAML value.
        let base_cfg = create_testing_config();
        let config = args.merge_with_config(base_cfg.clone());
        assert_eq!(config.address, base_cfg.address);
    }

    #[test]
    fn test_overlay_subcommand_args_invalid_addr_falls_back_to_yaml() {
        let args = SystemNodeSubcommandArgs {
            address: "invalid_addr".to_string(),
            port: 1234,
            config_path: PathBuf::from("does_not_matter.yaml"), // This will not be used
        };

        // "invalid_addr:1234" is not a valid SocketAddr, so should fallback to YAML
        let mut base_cfg = create_testing_config();
        let config = args.merge_with_config(base_cfg.clone());
        base_cfg.address.set_port(1234); // Port is always valid
        assert_eq!(config.address, base_cfg.address);
    }
}
