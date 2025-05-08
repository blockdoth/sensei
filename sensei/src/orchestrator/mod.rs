use crate::cli::{self, GlobalConfig, OrchestratorSubcommandArgs};
use crate::module::*;
use lib::network::rpc_message::CtrlMsg::*;
use lib::network::rpc_message::RpcMessage;
use lib::network::rpc_message::RpcMessage::*;
use lib::network::rpc_message::{AdapterMode, CtrlMsg, DataMsg, SourceType};
use lib::network::tcp::RequestHandler;
use lib::network::tcp::client::TcpClient;
use log::*;
use ratatui::backend::ClearType;
use std::arch::global_asm;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader, Split};
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

pub struct Orchestrator {
    socket_addr: SocketAddr,
    active_subscriptions: Vec<u64>,
}

impl CliInit<OrchestratorSubcommandArgs> for Orchestrator {
    fn init(config: &OrchestratorSubcommandArgs, global: &GlobalConfig) -> Self {
        Orchestrator {
            socket_addr: global.socket_addr,
            active_subscriptions: vec![],
        }
    }
}

impl RunsServer for Orchestrator {
    async fn start_server(self: Arc<Self>) -> Result<(), Box<dyn std::error::Error>> {
        let client = Arc::new(Mutex::new(TcpClient::new().await));
        info!("Started orchestrator");

        tokio::spawn(async move {
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
                    Ok(Subscribe {
                        sink_addr,
                        device_id,
                        mode,
                    }) => {
                        client.lock().await.connect(sink_addr).await;
                        info!("subscribed")
                    }
                    Ok(Unsubscribe {
                        sink_addr,
                        device_id,
                    }) => {
                        todo!("Unsubscribe not yet implemented")
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

        Ok(())
    }
}
