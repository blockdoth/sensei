use std::net::{AddrParseError, SocketAddr};

use argh::FromArgs;

use simplelog::{ColorChoice, CombinedLogger, LevelFilter, TermLogger, TerminalMode, WriteLogger};

use crate::config::{OrchestratorConfig, RegistryConfig, SystemNodeConfig, VisualiserConfig};

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
}

impl SystemNodeSubcommandArgs {
    pub fn parse(&self) -> Result<SystemNodeConfig, AddrParseError> {
        Ok(SystemNodeConfig {
            addr: format!("{}:{}", self.addr, self.port).parse()?,
        })
    }
}

/// Registry node commands
#[derive(FromArgs)]
#[argh(subcommand, name = "registry")]
pub struct RegistrySubcommandArgs {}

impl RegistrySubcommandArgs {
    pub fn parse(&self) -> Result<RegistryConfig, AddrParseError> {
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

impl OrchestratorSubcommandArgs {
    pub fn parse(&self) -> Result<OrchestratorConfig, AddrParseError> {
        // TODO input validation
        Ok(OrchestratorConfig {
            targets: self
                .target
                .iter()
                .map(|addr| addr.parse().unwrap())
                .collect(),
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
            target: self.target.parse()?
        })
    }
}
