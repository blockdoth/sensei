use crate::cli::*;
use crate::cli::{SubCommandsArgsEnum, SystemNodeSubcommandArgs};
use crate::module::*;
use anyhow::Ok;
use argh::FromArgs;
use async_trait::async_trait;
use lib::errors::NetworkError;
use lib::network::rpc_message::CtrlMsg::*;
use lib::network::rpc_message::RpcMessage::*;
use lib::network::rpc_message::{AdapterMode, CtrlMsg, DataMsg, RpcMessage, SourceType};
use lib::network::tcp::RequestHandler;
use lib::network::tcp::client::TcpClient;
use lib::network::tcp::server::TcpServer;
use lib::network::tcp::*;
use lib::network::tcp::*;
use log::*;
use std::env;
use std::net::SocketAddr;
use std::sync::Arc;
use std::thread::sleep;
use std::time::Duration;
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
pub struct SystemNode {
    socket_addr: SocketAddr,
    remote_addrs: Arc<Mutex<Vec<(SocketAddr, u64, AdapterMode)>>>, // Address, device id, adapter mode
    devices: Vec<u64>,
}

#[async_trait]
impl RequestHandler for SystemNode {
    async fn handle_request(
        &self,
        request: RpcMessage,
        stream: &mut TcpStream,
    ) -> Result<(), anyhow::Error> {
        info!("received request");
        match request {
            Ctrl(command) => match command {
                Subscribe {
                    sink_addr,
                    device_id,
                    mode,
                } => {
                    // You must lock the reference in order to edit
                    // TODO: find a better way of doing this such that this does not potentially lock while reading (maybe not an issue)
                    self.remote_addrs
                        .lock()
                        .await
                        .push((sink_addr, device_id, mode));
                    info!("Subscribed by {sink_addr}");
                    //  send_message(stream, RpcMessage::Ctrl(Heartbeat));
                    Ok(())
                }
                Unsubscribe {
                    sink_addr,
                    device_id,
                } => {
                    self.remote_addrs
                        .lock()
                        .await
                        .retain(|(tup1, tup2, _)| !(*tup1 == sink_addr && *tup2 == device_id));
                    println!("Unsubscribed by {sink_addr}");
                    todo!("update channel")
                }
                Configure { device_id, cfg } => {
                    todo!("Configure")
                }
                PollDevices => {
                    todo!("PollDevices")
                }
                Heartbeat => {
                    todo!("Heartbeat")
                    // More of a test message for, can add other test functionality for messages here
                    // Nodes are supposed to send heartbeats, not receive them
                    // TODO: Maybe nodes can be pinged using heartbeats?
                }
            },
            Data(data_msg) => todo!(),
        }
    }
}

impl RunsServer for SystemNode {
    async fn start_server(self: Arc<Self>) -> Result<(), Box<dyn std::error::Error>> {
        info!("Starting node on address {}", self.socket_addr);

        let server = TcpServer::serve(self.socket_addr, self).await;
        panic!()
    }
}

impl CliInit<SystemNodeSubcommandArgs> for SystemNode {
    fn init(config: &SystemNodeSubcommandArgs, global: &GlobalConfig) -> Self {
        SystemNode {
            socket_addr: global.socket_addr,
            remote_addrs: Arc::new(Mutex::new(Vec::new())),
            devices: vec![],
        }
    }
}
