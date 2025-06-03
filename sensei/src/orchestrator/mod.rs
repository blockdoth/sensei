mod state;
mod tui;

use std::arch::global_asm;
use std::net::SocketAddr;
use std::ops::Index;
use std::sync::Arc;
use std::vec;

use lib::network::rpc_message::CtrlMsg::*;
use lib::network::rpc_message::RpcMessageKind::{Ctrl, Data};
use lib::network::rpc_message::{AdapterMode, CtrlMsg, DataMsg, RpcMessage, SourceType};
use lib::network::tcp::client::TcpClient;
use lib::network::tcp::{ChannelMsg, client};
use lib::tui::TuiRunner;
use log::*;
use ratatui::backend::ClearType;
use state::{OrgCommand, OrgTuiState, OrgUpdate};
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader, Split};
use tokio::net::{TcpStream, UdpSocket};
use tokio::signal;
use tokio::sync::mpsc::Receiver;
use tokio::sync::{Mutex, mpsc, watch};
use tokio::task::JoinHandle;

use crate::cli::{self, OrchestratorSubcommandArgs, SubCommandsArgs};
use crate::services::{DEFAULT_ADDRESS, GlobalConfig, OrchestratorConfig, Run};

pub struct Orchestrator {
    client: Arc<Mutex<TcpClient>>,
}

impl Run<OrchestratorConfig> for Orchestrator {
    fn new() -> Self {
        Orchestrator {
            client: Arc::new(Mutex::new(TcpClient::new())),
        }
    }
    async fn run(&mut self, global_config: GlobalConfig, config: OrchestratorConfig) -> Result<(), Box<dyn std::error::Error>> {
        let (command_send, mut command_recv) = mpsc::channel::<ChannelMsg>(1000);
        let (update_send, mut update_recv) = mpsc::channel::<OrgUpdate>(1000);

        for target_addr in &config.targets {
          update_send.send(OrgUpdate::Connect(*target_addr));
        }

        let tasks = vec![Self::listen(command_recv,self.client.clone())];

        let tui = OrgTuiState::new(self.client.clone());

        let tui_runner = TuiRunner::new(tui, command_send, update_recv, update_send, global_config.log_level);
        tui_runner.run(tasks).await;
        Ok(())
    }
}

impl Orchestrator {

    async fn listen(mut recv_commands_channel: Receiver<ChannelMsg>, client: Arc<Mutex<TcpClient>>) {
      async move {
        let mut receiving = false;
        let mut targets: Vec<SocketAddr> = vec![];
        loop {
            if !recv_commands_channel.is_empty() {
              let msg_opt = recv_commands_channel.recv().await;
              match msg_opt {
                  Some(ChannelMsg::ListenSubscribe { addr }) => {
                      if !targets.contains(&addr) {
                          targets.push(addr);
                      }
                      receiving = true;
                  }
                  Some(ChannelMsg::ListenSubscribe { addr }) => {
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
                    let msg = client.lock().await.read_message(*target_addr).await.unwrap();
                    match msg.msg {
                        Data {
                            data_msg: DataMsg::CsiFrame { csi },
                            device_id,
                        } => {
                            info!("{}: {}", msg.src_addr, csi.timestamp)
                        }
                        Data {
                            data_msg: DataMsg::RawFrame { ts, bytes, source_type },
                            device_id,
                        } => info!("{}: {ts}", msg.src_addr),
                        _ => (),
                    }
                }
            }
        }
      };
    }
}
