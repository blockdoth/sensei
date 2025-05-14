use std::{net::SocketAddr, sync::Arc};

use argh::SubCommand;
use tokio::{net::UdpSocket, task::JoinHandle};

use crate::cli::{GlobalConfig, SubCommandsArgsEnum};

pub trait Run<SubCommandsArgsEnum> {
    fn new() -> Self;
    async fn run(
        &self,
        config: &SubCommandsArgsEnum,
        global: &GlobalConfig,
    ) -> Result<(), Box<dyn std::error::Error>>;
}
