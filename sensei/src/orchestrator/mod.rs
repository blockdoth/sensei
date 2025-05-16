use crate::cli::{self, GlobalConfig, OrchestratorSubcommandArgs, SubCommandsArgs};
use crate::config::{DEFAULT_ADDRESS, OrchestratorConfig};
use crate::module::*;
use lib::devices;
use lib::network::rpc_message::CtrlMsg::*;
use lib::network::rpc_message::RpcMessage;
use lib::network::rpc_message::RpcMessageKind::Ctrl;
use lib::network::rpc_message::RpcMessageKind::Data;
use lib::network::rpc_message::{AdapterMode, CtrlMsg, DataMsg, SourceType};
use lib::network::tcp::client::TcpClient;
use lib::network::tcp::{ChannelMsg, client};
use log::*;
use ratatui::backend::ClearType;
use std::arch::global_asm;
use std::net::SocketAddr;
use std::ops::Index;
use std::sync::Arc;
use std::vec;
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader, Split};
use tokio::net::{TcpStream, UdpSocket};
use tokio::signal;
use tokio::sync::{Mutex, watch};
use tokio::task::JoinHandle;

pub struct Orchestrator {
    client: Arc<Mutex<TcpClient>>,
}

impl Run<OrchestratorConfig> for Orchestrator {
  fn new() -> Self {
      Orchestrator {
          client: Arc::new(Mutex::new(TcpClient::new())),
      }
  }

  async fn run(&self, config: OrchestratorConfig) -> Result<(), Box<dyn std::error::Error>> {
      for target_addr in config.targets.into_iter() {
          Self::connect(&self.client, target_addr).await
      }
      self.cli_interface().await;
      Ok(())
  }
}

impl Orchestrator {
    
    async fn connect(client: &Arc<Mutex<TcpClient>>, target_addr: SocketAddr) {
        client.lock().await.connect(target_addr).await;
    }
    
    async fn disconnect(client: &Arc<Mutex<TcpClient>>, target_addr: SocketAddr) {
        client.lock().await.disconnect(target_addr).await;
    }
    
    async fn subscribe(
        client: &Arc<Mutex<TcpClient>>,
        target_addr: SocketAddr,
        device_id: u64,
        mode: AdapterMode,
    ) {
        let msg = Ctrl(CtrlMsg::Subscribe { device_id, mode });
        client.lock().await.send_message(target_addr, msg).await;
    }

    async fn unsubscribe(client: &Arc<Mutex<TcpClient>>, target_addr: SocketAddr, device_id: u64) {
        let msg = Ctrl(CtrlMsg::Unsubscribe { device_id });
        client.lock().await.send_message(target_addr, msg).await;
    }

    // Temporary, refactor once TUI gets added
    async fn cli_interface(&self) -> Result<(), Box<dyn std::error::Error>> {
        let (send_commands_channel, mut recv_commands_channel) =
            watch::channel::<ChannelMsg>(ChannelMsg::Empty);

        let send_client = self.client.clone();
        let recv_client = self.client.clone();

        let command_task = tokio::spawn(async move {
            // Create the input reader
            let stdin: BufReader<io::Stdin> = BufReader::new(io::stdin());
            let mut lines = stdin.lines();

            info!("Starting TUI interface");
            info!("Manual mode, type 'help' for commands");

            while let Ok(Some(line)) = lines.next_line().await {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }

                let mut line_iter = line.split_whitespace();
                match line_iter.next() {
                    Some("connect") => {
                        let target_addr: SocketAddr = line_iter
                            .next()
                            .unwrap() // #TODO remove unwrap
                            .parse()
                            .unwrap_or(DEFAULT_ADDRESS);
                        Self::connect(&send_client, target_addr).await;
                    }
                    Some("disconnect") => {
                        let target_addr: SocketAddr =
                            line_iter.next().unwrap().parse().unwrap_or(DEFAULT_ADDRESS);
                        Self::disconnect(&send_client, target_addr).await;
                    }
                    Some("sub") => {
                        let target_addr: SocketAddr =
                            line_iter.next().unwrap().parse().unwrap_or(DEFAULT_ADDRESS);
                        let device_id: u64 = line_iter.next().unwrap_or("0").parse().unwrap();
                        let mode: AdapterMode = match line_iter.next() {
                            Some("source") => AdapterMode::SOURCE,
                            Some("raw") => AdapterMode::RAW,
                            _ => AdapterMode::RAW,
                        };
                        Self::subscribe(&send_client, target_addr, device_id, mode).await;
                        send_commands_channel
                            .send(ChannelMsg::ListenSubscribe { addr: target_addr });
                    }
                    Some("unsub") => {
                        let target_addr: SocketAddr = line_iter
                            .next()
                            .unwrap() // #TODO remove unwrap
                            .parse()
                            .unwrap_or(DEFAULT_ADDRESS);
                        let device_id: u64 = line_iter.next().unwrap_or("0").parse().unwrap();
                        Self::unsubscribe(&send_client, target_addr, device_id).await;
                        send_commands_channel
                            .send(ChannelMsg::ListenUnsubscribe { addr: target_addr });
                    }
                    _ => {
                        info!("Failed to parse command")
                    }
                }
                io::stdout().flush(); // Ensure prompt shows up again
            }
            println!("Send loop ended (stdin closed).");
        });

        let recv_task = tokio::spawn(async move {
            let mut receiving = false;
            let mut targets: Vec<SocketAddr> = vec![];
            loop {
                if recv_commands_channel.has_changed().unwrap_or(false) {
                    let msg_opt = recv_commands_channel.borrow_and_update().clone();
                    match msg_opt {
                        ChannelMsg::ListenSubscribe { addr } => {
                            if !targets.contains(&addr) {
                                targets.push(addr);
                            }
                            receiving = true;
                        }
                        ChannelMsg::ListenSubscribe { addr } => {
                            if let Some(pos) = targets.iter().position(|x| *x == addr) {
                                targets.remove(pos);
                            }
                            if targets.is_empty() {
                                receiving = false;
                            }
                        }
                        _ => (),
                    }
                }
                if receiving {
                    for target_addr in targets.iter() {
                        let msg = recv_client
                            .lock()
                            .await
                            .read_message(*target_addr)
                            .await
                            .unwrap();
                        match msg.msg {
                            Data(DataMsg::CsiFrame { ts, csi }) => info!("{}: {ts}", msg.src_addr),
                            Data(DataMsg::RawFrame {
                                ts,
                                bytes,
                                source_type,
                            }) => info!("{}: {ts}", msg.src_addr),
                            _ => (),
                        }
                    }
                }
            }
        });
        signal::ctrl_c().await?;
        info!("Shutdown signal received. Exiting.");
        command_task.abort();
        recv_task.abort();

        Ok(())
    }
}
