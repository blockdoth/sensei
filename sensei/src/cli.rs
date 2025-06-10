use std::path::PathBuf;

use anyhow::Error;
use argh::FromArgs;
use lib::handler::device_handler::DeviceHandlerConfig;
use log::debug;
use simplelog::LevelFilter;

use crate::services::{EspToolConfig, GlobalConfig, OrchestratorConfig, SystemNodeConfig, VisualiserConfig};

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
    /// This trait is used to overlay fields set in the subcommands over the config read from the YAML file.
    fn overlay_subcommand_args(&self, full_config: T) -> Result<T, Box<dyn std::error::Error>>;
}

pub static DEFAULT_HOST_CONFIG: &str = "resources/test_data/test_configs/minimal.yaml";
pub static DEFAULT_ORCHESTRATOR_CONFIG: &str = "resources/example_configs/orchstrator/experiment_config.yaml";

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
    pub fn parse_global_config(&self) -> Result<GlobalConfig, Error> {
        Ok(GlobalConfig { log_level: self.level })
    }
}

pub trait ConfigFromCli<Config> {
    fn parse(&self) -> Result<Config, Error>;
}

#[derive(FromArgs)]
#[argh(subcommand)]
pub enum SubCommandsArgs {
    SystemNode(SystemNodeSubcommandArgs),
    Orchestrator(OrchestratorSubcommandArgs),
    Visualiser(VisualiserSubcommandArgs),
    EspTool(EspToolSubcommandArgs),
}

/// System node commands
#[derive(FromArgs)]
#[argh(subcommand, name = "node")]
pub struct SystemNodeSubcommandArgs {
    /// server address (default: 127.0.0.1)
    #[argh(option, default = "String::from(\"\")")]
    pub addr: String,

    /// server port (default: 6969)
    #[argh(option, default = "0")]
    pub port: u16,

    /// location of config file
    #[argh(option, default = "\"sensei/src/system_node/example_full.yaml\".parse().unwrap()")]
    pub config_path: PathBuf,
}

impl ConfigFromCli<SystemNodeConfig> for SystemNodeSubcommandArgs {
    fn parse(&self) -> Result<SystemNodeConfig, Error> {
        Ok(SystemNodeConfig {
            addr: format!("{}:{}", self.addr, self.port).parse()?,
            device_configs: DeviceHandlerConfig::from_yaml(self.config_path.clone())?,
            host_id: 0, // Default host_id, might be overwritten by YAML or other logic
            registries: None, // Default, might be overwritten by YAML
            registry_polling_rate_s: None, // Default, might be overwritten by YAML
            sinks: Vec::new(), // Initialize with an empty Vec, to be populated from YAML
        })
    }
}

/// Overlays subcommand arguments onto a SystemNodeConfig, overriding fields if provided.
impl OverlaySubcommandArgs<SystemNodeConfig> for SystemNodeSubcommandArgs {
    fn overlay_subcommand_args(&self, full_config: SystemNodeConfig) -> Result<SystemNodeConfig, Box<dyn std::error::Error>> {
        // Because of the default value we expact that there's always a file to read
        debug!("Loading system node configuration from YAML file: {}", self.config_path.display());
        let mut config = full_config.clone();
        // overwrite fields when provided by the subcommand
        config.addr = format!("{}:{}", self.addr, self.port).parse().unwrap_or(config.addr);

        Ok(config)
    }
}

/// Orchestrator node commands
#[derive(FromArgs)]
#[argh(subcommand, name = "orchestrator")]
pub struct OrchestratorSubcommandArgs {
    /// whether to enable tui input
    #[argh(option, default = "true")]
    pub tui: bool,

    /// file path of the experiment config
    #[argh(option, default = "DEFAULT_ORCHESTRATOR_CONFIG.parse().unwrap()")]
    pub experiment_config: PathBuf,
}

impl ConfigFromCli<OrchestratorConfig> for OrchestratorSubcommandArgs {
    fn parse(&self) -> Result<OrchestratorConfig, Error> {
        // TODO input validation
        Ok(OrchestratorConfig {
            experiment_config: self.experiment_config.clone(),
        })
    }
}

/// Visualiser commands
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

impl ConfigFromCli<VisualiserConfig> for VisualiserSubcommandArgs {
    fn parse(&self) -> Result<VisualiserConfig, Error> {
        // TODO input validation
        Ok(VisualiserConfig {
            target: self.target.parse()?,
            ui_type: self.ui_type.clone(),
        })
    }
}

/// Arguments for the ESP Test Tool subcommand
#[derive(FromArgs, Debug, Clone)]
#[argh(subcommand, name = "esp-tool")]
pub struct EspToolSubcommandArgs {
    /// serial port
    #[argh(option, default = "String::from(\"/dev/ttyUSB0\")")]
    pub serial_port: String,
}

impl ConfigFromCli<EspToolConfig> for EspToolSubcommandArgs {
    fn parse(&self) -> Result<EspToolConfig, Error> {
        Ok(EspToolConfig {
            serial_port: self.serial_port.clone(), // TODO remove clone
        })
    }
}

// Tests

#[cfg(test)]
mod tests {
    use super::*;

    fn create_testing_config() -> SystemNodeConfig {
        SystemNodeConfig {
            addr: "127.0.0.1:8080".parse().unwrap(),
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
            addr: "".to_string(),
            port: 0,
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
        let base_cfg = create_testing_config();
        let config = args.overlay_subcommand_args(base_cfg.clone()).unwrap();
        assert_eq!(config.addr, base_cfg.addr);
    }
}
