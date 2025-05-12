use crate::cli::*;
use crate::cli::{SubCommandsArgsEnum, SystemNodeSubcommandArgs};
use crate::module::*;

use crate::system_node::rpc_message::RpcMessageKind::*;
use anyhow::Ok;
use argh::FromArgs;
use async_trait::async_trait;
use lib::errors::NetworkError;
use lib::network::rpc_message::{AdapterMode, CtrlMsg};
use lib::network::rpc_message::CtrlMsg::*;
use lib::network::rpc_message::DataMsg::*;
use lib::network::rpc_message::RpcMessage;
use lib::network::rpc_message::SourceType::*;
use lib::network::tcp::client::TcpClient;
use lib::network::tcp::server::TcpServer;
use lib::network::tcp::{send_message, ChannelMsg, ConnectionHandler};
use lib::network::*;
use log::*;
use std::env;
use std::net::SocketAddr;
use std::sync::Arc;
use std::thread::sleep;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::AsyncWriteExt;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::watch::{Receiver, Sender};
use tokio::sync::{Mutex, watch};
use tokio::task::JoinHandle;
pub struct SystemNode {
    socket_addr: SocketAddr,
}

#[async_trait]
impl ConnectionHandler for SystemNode {
    async fn handle_recv(
        &self,
        request: RpcMessage,
        send_channel: Sender<ChannelMsg>,
    ) -> Result<(), anyhow::Error> {
        info!("Received message {:?}", request.msg);
        match request.msg {
            Ctrl(command) => match command {
                Connect => {
                    let src = request.src_addr;
                    info!("Started connection with {src}");
                    Ok(());
                }
                Disconnect => {
                    send_channel.send(ChannelMsg::Disconnect);

                    // todo!("disconnect logic")
                    Ok(());
                }
                Subscribe { device_id, mode } => {
                    send_channel.send(ChannelMsg::Subscribe);

                    info!("Subscribed to data stream");
                    Ok(());
                }
                Unsubscribe { device_id } => {
                    send_channel.send(ChannelMsg::Unsubscribe);
                    println!("Unsubscribed from data stream");
                    Ok(());
                }
                Configure { device_id, cfg } => {
                    todo!("Configure")
                }
                PollDevices => {
                    send_channel.send(ChannelMsg::Poll);
                    Ok(());
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
        Ok(())
    }

    async fn handle_send(
        &self,
        mut recv_channel: Receiver<ChannelMsg>,
        mut send_stream: OwnedWriteHalf,
    ) -> Result<(), anyhow::Error> {
      
      let mut sending = false;
      loop {
        if recv_channel.has_changed().unwrap_or(false) {
                let msg_opt = recv_channel.borrow_and_update().clone();
                debug!("Received message {msg_opt:?} over channel");
                match msg_opt {
                    ChannelMsg::Empty => (), // For init
                    ChannelMsg::Poll => todo!(),
                    ChannelMsg::Disconnect => {
                      let msg = RpcMessage {
                        msg: Ctrl(CtrlMsg::Disconnect),
                        src_addr: send_stream.local_addr().unwrap(), 
                        target_addr: send_stream.peer_addr().unwrap(),
                      };
                      
                      send_message(&mut send_stream, msg).await;
                      debug!("Send close confirmation");
                      break;
                    },
                    ChannelMsg::Subscribe => {
                        sending = true;
                    }
                    ChannelMsg::Unsubscribe => {
                        sending = false;
                    }
                    ChannelMsg::Data(date_msg) => {
                        if sending {
                            let msg = RpcMessage {
                                src_addr: self.socket_addr,
                                target_addr: send_stream.peer_addr().unwrap(),
                                msg: Data(date_msg),
                            };
                            tcp::send_message(&mut send_stream, msg).await;
                            info!("Sending")
                        }
                    }
                }
            }
            
        }
        Ok(())
    }
}

impl RunsServer for SystemNode {
    async fn start_server(self: Arc<Self>) -> Result<(), Box<dyn std::error::Error>> {
        info!("Starting node on address {}", self.socket_addr);

        let server = TcpServer::serve(self.socket_addr, self.clone()).await;
        panic!()
    }
}

impl CliInit<SystemNodeSubcommandArgs> for SystemNode {
    fn init(config: &SystemNodeSubcommandArgs, global: &GlobalConfig) -> Self {
        SystemNode {
            socket_addr: global.socket_addr,
        }
    }
}
