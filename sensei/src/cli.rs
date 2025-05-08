use std::net::SocketAddr;

use argh::FromArgs;

use simplelog::{ColorChoice, CombinedLogger, LevelFilter, TermLogger, TerminalMode, WriteLogger};

/// A simple app to perform collection from configured sources
#[derive(FromArgs)]
pub struct Args {
    /// server address (default: 127.0.0.1)
    #[argh(option, default = "String::from(\"127.0.0.1\")")]
    addr: String,

    /// server port (default: 6969)
    #[argh(option, default = "1278")]
    port: u16,

    /// log level to use for terminal logging
    #[argh(option, default = "LevelFilter::Info")]
    pub level: LevelFilter,

    #[argh(subcommand)]
    pub subcommand: SubCommandsArgsEnum,
}

pub struct GlobalConfig {
    pub socket_addr: SocketAddr,
}

impl Args {
    pub fn global_config(&self) -> GlobalConfig {
        GlobalConfig {
            socket_addr: format!("{}:{}", self.addr, self.port).parse().unwrap(),
        }
    }
}

#[derive(FromArgs)]
#[argh(subcommand)]
pub enum SubCommandsArgsEnum {
    One(SystemNodeSubcommandArgs),
    Two(RegistrySubcommandArgs),
    Three(OrchestratorSubcommandArgs),
}

/// System node commands
#[derive(FromArgs)]
#[argh(subcommand, name = "node")]
pub struct SystemNodeSubcommandArgs {}

/// Registry node commands
#[derive(FromArgs)]
#[argh(subcommand, name = "registry")]
pub struct RegistrySubcommandArgs {}

/// Orchestrator node commands
#[derive(FromArgs)]
#[argh(subcommand, name = "orchestrator")]
pub struct OrchestratorSubcommandArgs {}
