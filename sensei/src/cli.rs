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
#[cfg(feature = "sys_node")]
use std::path::PathBuf;

use anyhow::Error;
use argh::FromArgs;
#[cfg(feature = "sys_node")]
use lib::handler::device_handler::DeviceHandlerConfig;
#[cfg(feature = "sys_node")]
use log::debug;
use simplelog::LevelFilter;

#[cfg(feature = "esp_tool")]
use crate::services::EspToolConfig;
use crate::services::GlobalConfig;
#[cfg(feature = "orchestrator")]
use crate::services::OrchestratorConfig;
#[cfg(feature = "sys_node")]
use crate::services::SystemNodeConfig;
#[cfg(feature = "visualiser")]
use crate::services::VisualiserConfig;

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
pub trait OverlaySubcommandArgs<T> {
    /// Overlays fields from subcommand arguments onto a configuration loaded from a YAML file.
    ///
    /// If a field is specified in the subcommand arguments, it will override the corresponding
    /// value from the YAML configuration. Otherwise, the YAML configuration value is retained.
    fn overlay_subcommand_args(&self, full_config: T) -> Result<T, Box<dyn std::error::Error>>;
}

/// Default path to the host configuration YAML file.
pub static DEFAULT_HOST_CONFIG: &str = "resources/testing_configs/minimal.yaml";
/// Default path to the orchestrator configuration YAML file.
pub static DEFAULT_ORCHESTRATOR_CONFIG: &str = "resources/example_configs/orchestrator";

/// A simple app to perform collection from configured sources
#[derive(FromArgs)]
pub struct Args {
    /// log level to use for terminal logging
    /// Possible values: OFF, ERROR, WARN, INFO, DEBUG, TRACE
    #[argh(option, default = "LevelFilter::Info")]
    pub level: LevelFilter,

    #[argh(subcommand)]
    pub subcommand: Option<SubCommandsArgs>,
}

impl Args {
    /// Parses the global configuration from the command-line arguments.
    ///
    /// Currently, this primarily involves extracting the log level.
    pub fn parse_global_config(&self) -> Result<GlobalConfig, Error> {
        Ok(GlobalConfig { log_level: self.level })
    }
}

/// A trait for parsing command-line arguments into a specific configuration type.
///
/// Implementors of this trait define how to convert raw command-line arguments
/// (typically a dedicated struct derived with `argh::FromArgs`) into a structured
/// configuration object (e.g., `SystemNodeConfig`, `OrchestratorConfig`).
pub trait ConfigFromCli<Config> {
    /// Parses the command-line arguments into a configuration object.
    fn parse(&self) -> Result<Config, Error>;
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
    pub addr: String,

    /// server port (default: 6969)
    #[argh(option, default = "DEFAULT_PORT_CLI")]
    pub port: u16,

    /// location of config file
    #[argh(option, default = "PathBuf::from(DEFAULT_HOST_CONFIG)")]
    pub config_path: PathBuf,
}

#[cfg(feature = "sys_node")]
impl ConfigFromCli<SystemNodeConfig> for SystemNodeSubcommandArgs {
    /// Parses system node subcommand arguments into a `SystemNodeConfig`.
    ///
    /// This involves constructing an address from `addr` and `port` fields,
    /// and loading device configurations from the specified `config_path`.
    /// Default values are used for fields not directly provided by arguments.
    fn parse(&self) -> Result<SystemNodeConfig, Error> {
        Ok(SystemNodeConfig {
            addr: format!("{}:{}", self.addr, self.port).parse()?,
            device_configs: DeviceHandlerConfig::from_yaml(self.config_path.clone())?,
            host_id: 0,                    // Default host_id, might be overwritten by YAML or other logic
            registries: None,              // Default, might be overwritten by YAML
            registry_polling_rate_s: None, // Default, might be overwritten by YAML
            sinks: Vec::new(),             // Initialize with an empty Vec, to be populated from YAML
        })
    }
}

#[cfg(feature = "sys_node")]
/// Overlays subcommand arguments onto a SystemNodeConfig, overriding fields if provided.
impl OverlaySubcommandArgs<SystemNodeConfig> for SystemNodeSubcommandArgs {
    fn overlay_subcommand_args(&self, mut full_config: SystemNodeConfig) -> Result<SystemNodeConfig, Box<dyn std::error::Error>> {
        // Because of the default value we expact that there's always a file to read
        debug!("Loading system node configuration from YAML file: {}", self.config_path.display());
        // overwrite fields when provided by the subcommand
        if self.addr != DEFAULT_IP_CLI {
            full_config.addr.set_ip(self.addr.parse().unwrap_or(full_config.addr.ip()));
        }
        if self.port != DEFAULT_PORT_CLI {
            full_config.addr.set_port(self.port);
        }
        Ok(full_config)
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
    #[argh(option, default = "DEFAULT_ORCHESTRATOR_CONFIG.parse().unwrap()")]
    pub experiments_folder: PathBuf,

    /// polling interval of the registry
    #[argh(option, default = "5")]
    pub polling_interval: u64,
}

#[cfg(feature = "orchestrator")]
impl ConfigFromCli<OrchestratorConfig> for OrchestratorSubcommandArgs {
    /// Parses orchestrator subcommand arguments into an `OrchestratorConfig`.
    ///
    /// This primarily involves setting the path to the experiment configuration file.
    fn parse(&self) -> Result<OrchestratorConfig, Error> {
        // TODO input validation
        Ok(OrchestratorConfig {
            experiments_folder: self.experiments_folder.clone(),
            tui: self.tui,
            polling_interval: self.polling_interval,
        })
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

    /// height of the eventual window
    #[argh(option, default = "600")]
    pub height: usize,

    /// width of the eventual window
    #[argh(option, default = "800")]
    pub width: usize,

    /// using tui (ratatui, default) or gui (plotters, minifb)
    #[argh(option, default = "String::from(\"tui\")")]
    pub ui_type: String,
}

#[cfg(feature = "visualiser")]
impl ConfigFromCli<VisualiserConfig> for VisualiserSubcommandArgs {
    /// Parses visualiser subcommand arguments into a `VisualiserConfig`.
    ///
    /// This involves setting the target address and UI type for the visualiser.
    fn parse(&self) -> Result<VisualiserConfig, Error> {
        // TODO input validation
        Ok(VisualiserConfig {
            target: self.target.parse()?,
            ui_type: self.ui_type.clone(),
        })
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

#[cfg(feature = "esp_tool")]
impl ConfigFromCli<EspToolConfig> for EspToolSubcommandArgs {
    /// Parses ESP tool subcommand arguments into an `EspToolConfig`.
    ///
    /// This involves setting the serial port for communication with the ESP device.
    fn parse(&self) -> Result<EspToolConfig, Error> {
        Ok(EspToolConfig {
            serial_port: self.serial_port.clone(), // TODO remove clone
        })
    }
}

// Tests

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, SocketAddr};

    use super::*;

    fn create_testing_config() -> SystemNodeConfig {
        SystemNodeConfig {
            addr: SocketAddr::new(IpAddr::V4(DEFAULT_IP_CLI.parse().unwrap()), DEFAULT_PORT_CLI),
            host_id: 1,
            registries: Option::None,
            device_configs: vec![],
            registry_polling_rate_s: Option::None,
            sinks: Vec::new(), // Add sinks field
        }
    }

    #[test]
    fn test_overlay_subcommand_args_overwrites_addr_and_port() {
        let args = SystemNodeSubcommandArgs {
            addr: "10.0.0.1".to_string(),
            port: 4321,
            config_path: PathBuf::from("does_not_matter.yaml"), // This will not be used
        };

        let base_cfg = create_testing_config();
        let config = args.overlay_subcommand_args(base_cfg).unwrap();
        assert_eq!(config.addr, "10.0.0.1:4321".parse().unwrap());
    }

    #[test]
    fn test_overlay_subcommand_args_uses_yaml_when_no_override() {
        let args = SystemNodeSubcommandArgs {
            addr: DEFAULT_IP_CLI.to_string(),
            port: DEFAULT_PORT_CLI,
            config_path: PathBuf::from("does_not_matter.yaml"), // This will not be used
        };

        // If addr is empty and port is 0, overlay will parse ":0" which is invalid,
        // so we expect it to fallback to the YAML value.
        let base_cfg = create_testing_config();
        let config = args.overlay_subcommand_args(base_cfg.clone()).unwrap();
        assert_eq!(config.addr, base_cfg.addr);
    }

    #[test]
    fn test_overlay_subcommand_args_invalid_addr_falls_back_to_yaml() {
        let args = SystemNodeSubcommandArgs {
            addr: "invalid_addr".to_string(),
            port: 1234,
            config_path: PathBuf::from("does_not_matter.yaml"), // This will not be used
        };

        // "invalid_addr:1234" is not a valid SocketAddr, so should fallback to YAML
        let mut base_cfg = create_testing_config();
        let config = args.overlay_subcommand_args(base_cfg.clone()).unwrap();
        base_cfg.addr.set_port(1234); // Port is always valid
        assert_eq!(config.addr, base_cfg.addr);
    }
}
