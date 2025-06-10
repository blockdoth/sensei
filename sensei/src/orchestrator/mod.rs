mod state;
mod tui;

use std::net::SocketAddr;
use std::sync::Arc;
use std::vec;

use lib::network::rpc_message::DataMsg;
use lib::network::rpc_message::RpcMessageKind::Data;
use lib::network::tcp::ChannelMsg;
use lib::network::tcp::client::TcpClient;
use lib::tui::TuiRunner;
use log::*;
use state::{OrgTuiState, OrgUpdate};
use tokio::sync::mpsc::Receiver;
use tokio::sync::{Mutex, mpsc};

use crate::services::{GlobalConfig, OrchestratorConfig, Run};

pub struct Orchestrator {
    client: Arc<Mutex<TcpClient>>,
    targets: Vec<SocketAddr>,
    log_level: LevelFilter,
}

impl Run<OrchestratorConfig> for Orchestrator {
    fn new(global_config: GlobalConfig, config: OrchestratorConfig) -> Self {
        Orchestrator {
            client: Arc::new(Mutex::new(TcpClient::new())),
            targets: config.targets,
            log_level: global_config.log_level,
        }
    }
    async fn run(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let (command_send, mut command_recv) = mpsc::channel::<ChannelMsg>(1000);
        let (update_send, mut update_recv) = mpsc::channel::<OrgUpdate>(1000);

        for target_addr in &self.targets {
            update_send.send(OrgUpdate::Connect(*target_addr));
        }

        let tasks = vec![Self::listen(command_recv, self.client.clone())];

        let tui = OrgTuiState::new(self.client.clone());

        let tui_runner = TuiRunner::new(tui, command_send, update_recv, update_send, self.log_level);
        tui_runner.run(tasks).await;
        Ok(())
    }
}

impl Orchestrator {
    async fn listen(mut recv_commands_channel: Receiver<ChannelMsg>, client: Arc<Mutex<TcpClient>>) {
        info!("Started stream processor task");
        let mut receiving = false;
        let mut targets: Vec<SocketAddr> = vec![];
        loop {
            if !recv_commands_channel.is_empty() {
                let msg_opt = recv_commands_channel.recv().await;
                debug!("Received channel message {msg_opt:?}");
                match msg_opt {
                    Some(ChannelMsg::Shutdown) => break,
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
    }

    async fn recv_task(mut recv_commands_channel: Receiver<ChannelMsg>, recv_client: Arc<Mutex<TcpClient>>) {
        let mut receiving = false;
        let mut targets: Vec<SocketAddr> = vec![];
        loop {
            if !recv_commands_channel.is_empty() {
                let msg_opt = recv_commands_channel.recv().await.unwrap(); // TODO change
                match msg_opt {
                    ChannelMsg::ListenSubscribe { addr } => {
                        if !targets.contains(&addr) {
                            targets.push(addr);
                        }
                        receiving = true;
                    }
                    ChannelMsg::ListenUnsubscribe { addr } => {
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
                    let msg = recv_client.lock().await.read_message(*target_addr).await.unwrap();
                    match msg.msg {
                        Data {
                            data_msg: DataMsg::CsiFrame { csi },
                            device_id: _,
                        } => {
                            info!("{}: {}", msg.src_addr, csi.timestamp)
                        }
                        Data {
                            data_msg:
                                DataMsg::RawFrame {
                                    ts,
                                    bytes: _,
                                    source_type: _,
                                },
                            device_id: _,
                        } => info!("{}: {ts}", msg.src_addr),
                        _ => (),
                    }
                }
            }
        }
    }
}
