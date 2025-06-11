use std::path::PathBuf;

use anyhow::Error;
use argh::FromArgs;
use lib::handler::device_handler::DeviceHandlerConfig;
use log::debug;
use simplelog::LevelFilter;

use crate::services::{EspToolConfig, GlobalConfig, OrchestratorConfig, RegistryConfig, SystemNodeConfig, VisualiserConfig};

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

/// A simple app to perform collection from configured sources
#[derive(FromArgs)]
pub struct Args {
    /// log level to use for terminal logging
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
    One(SystemNodeSubcommandArgs),
    Two(RegistrySubcommandArgs),
    Three(OrchestratorSubcommandArgs),
    Four(VisualiserSubcommandArgs),
    Five(EspToolSubcommandArgs),
}

const DEFAULT_PORT_CLI: u16 = 6969;
const DEFAULT_IP_CLI: &str = "127.0.0.1";

/// System node commands
#[derive(FromArgs)]
#[argh(subcommand, name = "node")]
pub struct SystemNodeSubcommandArgs {
    /// server address (default: 127.0.0.1)
    #[argh(option, default = "DEFAULT_IP_CLI.to_owned()")]
    pub addr: String,

    /// server port (default: 6969)
    #[argh(option, default = "DEFAULT_PORT_CLI")]
    pub port: u16,

    /// location of config file (default sensei/src/system_node/example_config.yaml)
    #[argh(option, default = "PathBuf::from(\"sensei/src/system_node/example_config.yaml\")")]
    pub config_path: PathBuf,
}

impl ConfigFromCli<SystemNodeConfig> for SystemNodeSubcommandArgs {
    fn parse(&self) -> Result<SystemNodeConfig, Error> {
        Ok(SystemNodeConfig {
            addr: format!("{}:{}", self.addr, self.port).parse()?,
            device_configs: DeviceHandlerConfig::from_yaml(self.config_path.clone())?,
            host_id: 0,
            registry: todo!(),
        })
    }
}

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

fn default_device_configs() -> PathBuf {
    "sensei/src/system_node/example_config.yaml".parse().unwrap()
}

/// Registry node commands
#[derive(FromArgs)]
#[argh(subcommand, name = "registry")]
pub struct RegistrySubcommandArgs {}

impl ConfigFromCli<RegistryConfig> for RegistrySubcommandArgs {
    fn parse(&self) -> Result<RegistryConfig, Error> {
        Ok(RegistryConfig {
            addr: "127.0.0.1:8080".parse()?,
            poll_interval: 5,
        })
    }
}
/// Orchestrator node commands
#[derive(FromArgs)]
#[argh(subcommand, name = "orchestrator")]
pub struct OrchestratorSubcommandArgs {
    /// server port (default: 6969)
    #[argh(option, default = "vec![String::from(\"127.0.0.1:6969\")]")]
    pub target: Vec<String>,

    /// whether to enable tui input
    #[argh(option, default = "true")]
    pub tui: bool,
}

impl ConfigFromCli<OrchestratorConfig> for OrchestratorSubcommandArgs {
    fn parse(&self) -> Result<OrchestratorConfig, Error> {
        // TODO input validation
        Ok(OrchestratorConfig {
            targets: self.target.iter().map(|addr| addr.parse().unwrap()).collect(),
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
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    use super::*;

    fn create_testing_config() -> SystemNodeConfig {
        SystemNodeConfig {
            addr: "127.0.0.1:8080".parse().unwrap(),
            host_id: 1,
            registry: Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8080)),
            device_configs: vec![],
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
