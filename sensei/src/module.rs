use std::{net::SocketAddr, sync::Arc};

use argh::SubCommand;
use tokio::{net::UdpSocket, task::JoinHandle};

use crate::cli::{GlobalConfig, SubCommandsArgsEnum};

pub trait RunsServer {
    async fn start_server(self: Arc<Self>) -> Result<(), Box<dyn std::error::Error>>;
}

pub trait CliInit<SubCommandsArgsEnum> {
    fn init(config: &SubCommandsArgsEnum, global: &GlobalConfig) -> Self;
}
