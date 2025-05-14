use crate::cli::{self, GlobalConfig, OrchestratorSubcommandArgs, SubCommandsArgsEnum};
use crate::module::*;
use lib::network::rpc_message::CtrlMsg::*;
use lib::network::rpc_message::RpcMessage;
use lib::network::rpc_message::RpcMessageKind::Ctrl;
use lib::network::rpc_message::{AdapterMode, CtrlMsg, DataMsg, SourceType};
use lib::network::tcp::client::TcpClient;
use lib::network::tcp::ChannelMsg;
use log::*;
use ratatui::backend::ClearType;
use std::arch::global_asm;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader, Split};
use tokio::net::{TcpStream, UdpSocket};
use tokio::signal;
use tokio::sync::{watch, Mutex};
use tokio::task::JoinHandle;

pub struct Orchestrator {}

impl Run<OrchestratorSubcommandArgs> for Orchestrator {
    fn new() -> Self {
        Orchestrator {}
    }

    async fn run(
        &self,
        config: &OrchestratorSubcommandArgs,
        global: &GlobalConfig,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let client = Arc::new(Mutex::new(TcpClient::new().await));
        let recv_client = client.clone();

        let target_addr = global.socket_addr;

        let (send_commands_channel, mut recv_commands_channel) = watch::channel::<ChannelMsg>(ChannelMsg::Empty);


        let client_task = tokio::spawn(async move {
            // Create the input reader
            let stdin: BufReader<io::Stdin> = BufReader::new(io::stdin());
            let mut lines = stdin.lines();


            info!("Connected to socket");
            info!("Manual mode, type 'help' for commands");

            while let Ok(Some(line)) = lines.next_line().await {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if line.eq("help") {
                    todo!("help message");
                    continue;
                }

                match line.parse::<CtrlMsg>() {
                    Ok(Connect) => {
                        client.lock().await.connect(target_addr).await;
                        
                    }
                    Ok(Disconnect) => {
                        client.lock().await.disconnect(target_addr).await;
                    }
                    Ok(Subscribe { device_id, mode }) => {
                        let msg = RpcMessage {
                          src_addr: client.lock().await.get_src_addr(target_addr).await,
                            target_addr,
                            msg: Ctrl(CtrlMsg::Subscribe {
                                device_id: 0,
                                mode: AdapterMode::RAW,
                            }),
                        };

                        client.lock().await.send_message(target_addr, msg).await;
                        

                        send_commands_channel.send(ChannelMsg::Subscribe);
                        
                        info!("Subscribed to node {target_addr}")
                      }
                      Ok(Unsubscribe { device_id }) => {
                        
                        let msg = RpcMessage {
                          src_addr: client.lock().await.get_src_addr(target_addr).await,
                          target_addr,
                          msg: Ctrl(CtrlMsg::Unsubscribe { device_id: 0 }),
                        };
                        client.lock().await.send_message(target_addr, msg).await;
                        send_commands_channel.send(ChannelMsg::Unsubscribe);
                        info!("Subscribed from node {target_addr}")
                    }
                    Ok(Configure { device_id, cfg }) => {
                        todo!("Configure not yet implemented")
                    }
                    Ok(PollDevices) => {
                        todo!("PollDevices not yet implemented")
                    }
                    Ok(Heartbeat) => {
                        todo!("Heartbeat not yet implemented")
                    }
                    Err(e) => {
                        println!("Failed to parse CtrlMsg: {e}");
                        continue;
                    }
                }
                io::stdout().flush(); // Ensure prompt shows up again
            }
            println!("Send loop ended (stdin closed).");
        });

        tokio::spawn(async move {
          let mut receiving = false;
          loop {
            if recv_commands_channel.has_changed().unwrap_or(false) {
              let msg_opt = recv_commands_channel.borrow_and_update().clone();
              match msg_opt {
                ChannelMsg::Subscribe => {
                  receiving = true;
                }
                ChannelMsg::Unsubscribe => {
                  receiving = false;
                }
                _ => ()
              }
            }
            if receiving {
              let msg = recv_client.lock().await.read_message(target_addr).await.unwrap();
              let data= msg.msg;
              info!("{:?}", data);
            } 
          }
        });



        signal::ctrl_c().await?;
        info!("Shutdown signal received. Exiting.");
        client_task.abort();

        Ok(())
    }
}
