use crate::cli::{self, GlobalConfig, OrchestratorSubcommandArgs};
use crate::module::*;
use lib::network::rpc_message::CtrlMsg::*;
use lib::network::rpc_message::RpcMessage;
use lib::network::rpc_message::RpcMessageKind::Ctrl;
use lib::network::rpc_message::{AdapterMode, CtrlMsg, DataMsg, SourceType};
use lib::network::tcp::client::TcpClient;
use log::*;
use ratatui::backend::ClearType;
use std::arch::global_asm;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader, Split};
use tokio::net::{TcpStream, UdpSocket};
use tokio::signal;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

pub struct Orchestrator {
    target_addr: SocketAddr,
}

impl CliInit<OrchestratorSubcommandArgs> for Orchestrator {
    fn init(config: &OrchestratorSubcommandArgs, global: &GlobalConfig) -> Self {
        Orchestrator {
            target_addr: global.socket_addr,
        }
    }
}

impl RunsServer for Orchestrator {
    async fn start_server(self: Arc<Self>) -> Result<(), Box<dyn std::error::Error>> {
        let client = Arc::new(Mutex::new(TcpClient::new().await));

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
                        let mut client = client.lock().await;

                        client.connect(self.target_addr).await;
                        let msg = RpcMessage {
                            src_addr: client.self_addr.unwrap(),
                            target_addr: self.target_addr,
                            msg: Ctrl(CtrlMsg::Connect),
                        };

                        client.send_message(self.target_addr, msg).await;
                    }
                    Ok(Disconnect) => {
                        client.lock().await.disconnect(self.target_addr).await;
                    }
                    Ok(Subscribe { device_id, mode }) => {
                        let src_addr = client.lock().await.self_addr.unwrap();
                        let msg = RpcMessage {
                            src_addr,
                            target_addr: self.target_addr,
                            msg: Ctrl(CtrlMsg::Subscribe {
                                device_id: 0,
                                mode: AdapterMode::RAW,
                            }),
                        };
                        client.lock().await.send_message(self.target_addr, msg);
                        info!("Subscribed to node {}", self.target_addr)
                    }
                    Ok(Unsubscribe { device_id }) => {
                        let src_addr = client.lock().await.self_addr.unwrap();
                        let msg = RpcMessage {
                            src_addr,
                            target_addr: self.target_addr,
                            msg: Ctrl(CtrlMsg::Unsubscribe { device_id: 0 }),
                        };
                        client.lock().await.send_message(self.target_addr, msg);
                        info!("Subscribed from node {}", self.target_addr)
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
        signal::ctrl_c().await?;
        info!("Shutdown signal received. Exiting.");
        client_task.abort();

        Ok(())
    }
}
