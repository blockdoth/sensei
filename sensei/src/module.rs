use std::{net::SocketAddr, sync::Arc};

use argh::SubCommand;
use tokio::{net::UdpSocket, task::JoinHandle};

use crate::cli::{GlobalConfig, SubCommandsArgs};

pub trait Run<ServiceConfig> {
    fn new(config: ServiceConfig) -> Self;
    async fn run(&self, config: ServiceConfig) -> Result<(), Box<dyn std::error::Error>>;
}
