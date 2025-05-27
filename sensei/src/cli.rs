use std::net::{AddrParseError, SocketAddr};
use std::path::PathBuf;

use anyhow::Error;
use argh::FromArgs;
use simplelog::{ColorChoice, CombinedLogger, LevelFilter, TermLogger, TerminalMode, WriteLogger};

use crate::esp_tool;
use crate::services::{EspToolConfig, GlobalConfig, OrchestratorConfig, RegistryConfig, SystemNodeConfig, VisualiserConfig};

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

/// System node commands
#[derive(FromArgs)]
#[argh(subcommand, name = "node")]
pub struct SystemNodeSubcommandArgs {
    /// server address (default: 127.0.0.1)
    #[argh(option, default = "String::from(\"127.0.0.1\")")]
    pub addr: String,

    /// server port (default: 6969)
    #[argh(option, default = "6969")]
    pub port: u16,

    /// location of config file (default sensei/src/system_node/example_config.yaml)
    #[argh(option, default = "PathBuf::from(\"sensei/src/system_node/example_config.yaml\")")]
    pub device_configs: PathBuf,
}

impl ConfigFromCli<SystemNodeConfig> for SystemNodeSubcommandArgs {
    fn parse(&self) -> Result<SystemNodeConfig, Error> {
        Ok(SystemNodeConfig {
            addr: format!("{}:{}", self.addr, self.port).parse()?,
            device_configs: self.device_configs.clone(),
        })
    }
}

/// Registry node commands
#[derive(FromArgs)]
#[argh(subcommand, name = "registry")]
pub struct RegistrySubcommandArgs {}

impl ConfigFromCli<RegistryConfig> for RegistrySubcommandArgs {
    fn parse(&self) -> Result<RegistryConfig, Error> {
        Ok(RegistryConfig { targets: vec![] })
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
