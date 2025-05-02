use crate::cli::*;
use crate::cli::{SubCommandsArgsEnum, SystemNodeSubcommandArgs};
use crate::module::*;
use argh::FromArgs;
use async_trait::async_trait;
use common::adapter_mode::AdapterMode;
use common::deserialize_envelope;
use common::radio_config::RadioConfig;
use std::env;
use std::net::SocketAddr;
use tokio::net::UdpSocket;

pub struct SystemNode {
    socket_addr: SocketAddr,
    pub devices: Vec<u64>,
}

impl RunsServer for SystemNode {
    async fn start_server(&self) -> Result<(), Box<dyn std::error::Error>> {
        println!("Starting system node");
        let socket = UdpSocket::bind(self.socket_addr).await?;
        //Respond to message
        loop {
            let mut buf = [0u8; 1024];
            match socket.recv(&mut buf).await {
                Ok(received) => {
                    let msg = deserialize_envelope(&buf[..received]);
                    println!("{msg:?} received");
                }
                Err(e) => {
                    println!("Received error: {e}");
                }
            }
        }
    }
}

impl CliInit<SystemNodeSubcommandArgs> for SystemNode {
    fn init(config: &SystemNodeSubcommandArgs, global: &GlobalConfig) -> Self {
        SystemNode {
            socket_addr: global.socket_addr,
            devices: vec![],
        }
    }
}

#[async_trait]
trait WifiController {
    async fn apply(&self, cfg: RadioConfig) -> anyhow::Result<()>;
}

#[async_trait]
trait DataPipeline {
    async fn subscribe(&self, mode: AdapterMode) -> anyhow::Result<()>;
}
