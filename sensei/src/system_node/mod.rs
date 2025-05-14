use crate::cli::*;
use crate::cli::{SubCommandsArgsEnum, SystemNodeSubcommandArgs};
use crate::module::*;

use crate::system_node::rpc_message::RpcMessageKind::*;
use argh::FromArgs;
use async_trait::async_trait;
use lib::errors::NetworkError;
use lib::network::rpc_message::CtrlMsg::*;
use lib::network::rpc_message::DataMsg::*;
use lib::network::rpc_message::RpcMessage;
use lib::network::rpc_message::SourceType::*;
use lib::network::rpc_message::{AdapterMode, CtrlMsg};
use lib::network::tcp::client::TcpClient;
use lib::network::tcp::server::TcpServer;
use lib::network::tcp::{ChannelMsg, ConnectionHandler, send_message};
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

#[derive(Clone)]
pub struct SystemNode {}

#[async_trait]
impl ConnectionHandler for SystemNode {
    async fn handle_recv(
        &self,
        request: RpcMessage,
        send_channel: Sender<ChannelMsg>,
    ) -> Result<(), NetworkError> {
        info!(
            "Received message {:?} from {}",
            request.msg, request.src_addr
        );
        match request.msg {
            Ctrl(command) => match command {
                Connect => {
                    let src = request.src_addr;
                    info!("Started connection with {src}");
                }
                Disconnect => {
                    send_channel.send(ChannelMsg::Disconnect);

                    // todo!("disconnect logic")
                    return Err(NetworkError::Closed);
                }
                Subscribe { device_id, mode } => {
                    send_channel.send(ChannelMsg::Subscribe);

                    info!("Subscribed to data stream");
                }
                Unsubscribe { device_id } => {
                    send_channel.send(ChannelMsg::Unsubscribe);
                    println!("Unsubscribed from data stream");
                }
                Configure { device_id, cfg } => {
                    todo!("Configure")
                }
                PollDevices => {
                    send_channel.send(ChannelMsg::Poll);
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
    ) -> Result<(), NetworkError> {
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
                    }
                    ChannelMsg::Subscribe => {
                        sending = true;
                    }
                    ChannelMsg::Unsubscribe => {
                        sending = false;
                    }
                    ChannelMsg::Data(date_msg) => {
                        if sending {
                            let msg = RpcMessage {
                                src_addr: send_stream.local_addr().unwrap(),
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

impl Run<SystemNodeSubcommandArgs> for SystemNode {
    fn new() -> Self {
        SystemNode {}
    }

    async fn run(
        &self,
        config: &SystemNodeSubcommandArgs,
        global: &GlobalConfig,
    ) -> Result<(), Box<dyn std::error::Error>> {
        TcpServer::serve(global.socket_addr, Arc::new(self.clone())).await;
        Ok(())
    }
}
