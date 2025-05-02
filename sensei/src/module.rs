use std::net::SocketAddr;

use argh::SubCommand;

use crate::cli::{GlobalConfig, SubCommandsArgsEnum};

pub trait RunsServer {
    async fn start_server(&self) -> Result<(), Box<dyn std::error::Error>>;
}

pub trait CliInit<SubCommandsArgsEnum> {
    fn init(config: &SubCommandsArgsEnum, global: &GlobalConfig) -> Self;
}
