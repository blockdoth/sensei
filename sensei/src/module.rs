use std::net::SocketAddr;
use std::sync::Arc;

use argh::SubCommand;
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;

use crate::cli::{GlobalConfig, SubCommandsArgs};

pub trait Run<ServiceConfig> {
    fn new(config: ServiceConfig) -> Self;
    async fn run(&self, config: ServiceConfig) -> Result<(), Box<dyn std::error::Error>>;
}
