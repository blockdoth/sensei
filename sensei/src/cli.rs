use std::net::{AddrParseError, SocketAddr};
use std::path::PathBuf;

use argh::FromArgs;
use log::debug;
use simplelog::{ColorChoice, CombinedLogger, LevelFilter, TermLogger, TerminalMode, WriteLogger};
use warp::filters::path::full;

use crate::config::{FromYaml, OrchestratorConfig, RegistryConfig, SystemNodeConfig, SystemNodeRegistryConfig, VisualiserConfig};
use crate::esp_tool;

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
    pub subcommand: SubCommandsArgs,
}

pub struct GlobalConfig {
    pub target_addr: SocketAddr,
    pub tui: bool,
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

    /// location of config file (default sensei/src/system_node/example_config.yaml)
    #[argh(option, default = "default_device_configs()")]
    pub config: PathBuf,
}

/// Overlays subcommand arguments onto a SystemNodeConfig, overriding fields if provided.
impl OverlaySubcommandArgs<SystemNodeConfig> for SystemNodeSubcommandArgs {
    fn overlay_subcommand_args(&self, full_config: SystemNodeConfig) -> Result<SystemNodeConfig, Box<dyn std::error::Error>> {
        // Because of the default value we expact that there's always a file to read
        debug!("Loading system node configuration from YAML file: {}", self.config.display());
        let mut config = full_config.clone();
        // overwrite fields when provided by the subcommand
        config.addr = format!("{}:{}", self.addr, self.port).parse().unwrap_or(config.addr);

        Ok(config)
    }
}

fn default_device_configs() -> PathBuf {
    "sensei/src/system_node/example_config.yaml".parse().unwrap()
}

/// Registry node commands
#[derive(FromArgs)]
#[argh(subcommand, name = "registry")]
pub struct RegistrySubcommandArgs {}

impl RegistrySubcommandArgs {
    pub fn parse(&self) -> Result<RegistryConfig, AddrParseError> {
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

impl OrchestratorSubcommandArgs {
    pub fn parse(&self) -> Result<OrchestratorConfig, AddrParseError> {
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
    #[argh(option, default = "default_height()")]
    pub height: usize,

    /// width of the eventual window
    #[argh(option, default = "default_width()")]
    pub width: usize,

    /// using tui (ratatui, default) or gui (plotters, minifb)
    #[argh(option, default = "String::from(\"tui\")")]
    pub ui_type: String,
}

fn default_height() -> usize {
    600
}

fn default_width() -> usize {
    800
}

impl VisualiserSubcommandArgs {
    pub fn parse(&self) -> Result<VisualiserConfig, AddrParseError> {
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
    /// serial port at which the ESP32 is connected (e.g., /dev/ttyUSB0 or COM3)
    #[argh(option)]
    pub port: String,
}

// Tests

#[cfg(test)]
mod tests {
    use super::*;

    fn create_testing_config() -> SystemNodeConfig {
        SystemNodeConfig {
            addr: "127.0.0.1:8080".parse().unwrap(),
            host_id: 1,
            registry: SystemNodeRegistryConfig {
                use_registry: true,
                addr: "127.0.0.2:8888".parse().unwrap(),
            },
            device_configs: vec![],
        }
    }

    #[test]
    fn test_overlay_subcommand_args_overwrites_addr_and_port() {
        let args = SystemNodeSubcommandArgs {
            addr: "10.0.0.1".to_string(),
            port: 4321,
            config: PathBuf::from("does_not_matter.yaml"), // This will not be used
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
            config: PathBuf::from("does_not_matter.yaml"), // This will not be used
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
            config: PathBuf::from("does_not_matter.yaml"), // This will not be used
        };

        // "invalid_addr:1234" is not a valid SocketAddr, so should fallback to YAML
        let base_cfg = create_testing_config();
        let config = args.overlay_subcommand_args(base_cfg.clone()).unwrap();
        assert_eq!(config.addr, base_cfg.addr);
    }
}
