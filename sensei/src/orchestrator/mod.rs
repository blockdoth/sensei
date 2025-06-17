mod experiment;
mod state;
mod tui;

use std::fs::File;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use std::vec;

use lib::network::rpc_message::RpcMessageKind::Data;
use lib::network::rpc_message::{CfgType, DataMsg, DeviceId, HostCtrl, HostId, RegCtrl, RpcMessageKind};
use lib::network::tcp::client::TcpClient;
use lib::tui::TuiRunner;
use log::*;
use serde::{Deserialize, Serialize};
use tokio::signal;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tokio::time::sleep;

use crate::orchestrator::IsRecurring::{NotRecurring, Recurring};
use crate::orchestrator::state::{OrgTuiState, OrgUpdate};
use crate::services::{GlobalConfig, OrchestratorConfig, Run};

pub struct Orchestrator {
    client: Arc<Mutex<TcpClient>>,
    log_level: LevelFilter,
    experiment_config_path: Option<PathBuf>,
    tui: bool,
}

#[derive(Debug)]
pub enum OrgChannelMsg {
    Connect(SocketAddr),
    Disconnect(SocketAddr),
    Subscribe(SocketAddr, Option<SocketAddr>, DeviceId),
    Unsubscribe(SocketAddr, Option<SocketAddr>, DeviceId),
    SubscribeAll(SocketAddr, Option<SocketAddr>),
    UnsubscribeAll(SocketAddr, Option<SocketAddr>),
    SendStatus(SocketAddr, HostId),
    Configure(SocketAddr, DeviceId, CfgType),
    Delay(u64),
    Shutdown,
    Ping(SocketAddr),
    RunExperiment(Experiment),
}

impl Run<OrchestratorConfig> for Orchestrator {
    fn new(global_config: GlobalConfig, config: OrchestratorConfig) -> Self {
        Orchestrator {
            client: Arc::new(Mutex::new(TcpClient::new())),
            log_level: global_config.log_level,
            experiment_config_path: Some(config.experiment_config),
            tui: config.tui,
        }
    }
    async fn run(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let (command_send, mut command_recv) = mpsc::channel::<OrgChannelMsg>(1000);
        let (update_send, mut update_recv) = mpsc::channel::<OrgUpdate>(1000);

        // Tasks needs to be boxed and pinned in order to make the type checker happy
        let tasks: Vec<Pin<Box<dyn Future<Output = ()> + Send>>> = vec![
            Box::pin(Self::command_handler(command_recv, update_send.clone(), self.client.clone())),
            // Box::pin(Self::initial_experiment(command_send.clone(), self.experiment_config_path.clone())),
        ];

        if self.tui {
            let tui = OrgTuiState::new();
            let tui_runner = TuiRunner::new(tui, command_send, update_recv, update_send, self.log_level);

            tui_runner.run(tasks).await;
        } else {
            let mut handles: Vec<JoinHandle<()>> = vec![];
            for task in tasks {
                handles.push(tokio::spawn(task));
            }

            // Create a future that resolves when Ctrl+C is received
            let ctrl_c = async {
                signal::ctrl_c().await.expect("Failed to listen for ctrl + c");
                println!(" Received Ctrl+c, shutting down...");
            };

            tokio::select! {
                _ = ctrl_c => {}
                _ = futures::future::join_all(handles) => {
                    println!("All tasks completed");
                }
            }
        }

        Ok(())
    }
}

impl Orchestrator {
    async fn initial_experiment(command_send: Sender<OrgChannelMsg>, experiment_config_path_opt: Option<PathBuf>) {
        info!("Loading initial experiment");

        if let Some(experiment_config_path) = experiment_config_path_opt {
            match Experiment::from_yaml(experiment_config_path.clone()) {
                Ok(experiment) => {
                    debug!("Successfully parsed config");
                    command_send.send(OrgChannelMsg::RunExperiment(experiment)).await;
                }
                Err(e) => {
                    error!("Unable to load experiment config {experiment_config_path:?} {e}");
                }
            };
        } else {
            debug!("No initial experiment found")
        }
    }

    async fn command_handler(mut recv_commands_channel: Receiver<OrgChannelMsg>, update_send: Sender<OrgUpdate>, client: Arc<Mutex<TcpClient>>) {
        info!("Started stream processor task");
        let mut receiving = false;
        let mut targets: Vec<SocketAddr> = vec![];

        loop {
            tokio::select! {
                // Commands from channel
                msg_opt = recv_commands_channel.recv() => {
                    info!("Received channel message {msg_opt:?}");
                    if let Some(msg) = msg_opt {
                        Self::handle_msg(client.clone(), update_send.clone(), msg).await;
                    }
                    info!("Handled");
                }

                // TCP Client messages if in receiving mode
                _ = async {
                    if receiving {
                        for target_addr in &targets {
                            if let Ok(msg) = client.lock().await.read_message(*target_addr).await {
                                match msg.msg {
                                    Data { data_msg: DataMsg::CsiFrame { csi }, .. } => {
                                        info!("{}: {}", msg.src_addr, csi.timestamp)
                                    }
                                    Data { data_msg: DataMsg::RawFrame { ts, .. }, .. } => {
                                        info!("{}: {ts}", msg.src_addr)
                                    }
                                    _ => (),
                                }
                            }
                        }
                    }
                } => {}
            }
        }
    }

    pub async fn handle_msg(client: Arc<Mutex<TcpClient>>, update_send: Sender<OrgUpdate>, msg: OrgChannelMsg) {
        match msg {
            OrgChannelMsg::Connect(target_addr) => {
                client.lock().await.connect(target_addr).await;
            }
            OrgChannelMsg::Disconnect(target_addr) => {
                client.lock().await.disconnect(target_addr).await;
            }
            OrgChannelMsg::Subscribe(target_addr, msg_origin_addr, device_id) => {
                if let Some(msg_origin_addr) = msg_origin_addr {
                    info!("Subscribing to {target_addr} for device id {device_id}");
                    let msg = HostCtrl::SubscribeTo { target_addr, device_id };
                    client.lock().await.send_message(msg_origin_addr, RpcMessageKind::HostCtrl(msg)).await;
                } else {
                    info!("Subscribing to {target_addr} for device id {device_id}");
                    let msg = HostCtrl::Subscribe { device_id };
                    client.lock().await.send_message(target_addr, RpcMessageKind::HostCtrl(msg)).await;
                }
            }
            OrgChannelMsg::Unsubscribe(target_addr, msg_origin_addr, device_id) => {
                if let Some(msg_origin_addr) = msg_origin_addr {
                    info!("Unubscribing from {target_addr} for device id {device_id}");
                    let msg = HostCtrl::UnsubscribeFrom { target_addr, device_id };
                    client.lock().await.send_message(msg_origin_addr, RpcMessageKind::HostCtrl(msg)).await;
                } else {
                    info!("Unubscribing from {target_addr} for device id {device_id}");
                    let msg = HostCtrl::Unsubscribe { device_id };
                    client.lock().await.send_message(target_addr, RpcMessageKind::HostCtrl(msg)).await;
                }
            }
            OrgChannelMsg::SubscribeAll(to_addr, msg_origin_addr) => {
                todo!()
            }
            OrgChannelMsg::UnsubscribeAll(to_addr, msg_origin_addr) => {
                todo!()
            }
            OrgChannelMsg::SendStatus(target_addr, host_id) => {
                let msg = RpcMessageKind::RegCtrl(RegCtrl::PollHostStatus { host_id });
                client.lock().await.send_message(target_addr, msg).await;
            }
            OrgChannelMsg::Configure(target_addr, device_id, cfg_type) => {
                let msg = RpcMessageKind::HostCtrl(HostCtrl::Configure { device_id, cfg_type });

                info!("Telling {target_addr} to configure the device handler");

                client.lock().await.send_message(target_addr, msg).await;
            }
            OrgChannelMsg::Delay(ms_delay) => sleep(Duration::from_millis(ms_delay)).await,
            OrgChannelMsg::Shutdown => todo!(),
            OrgChannelMsg::RunExperiment(experiment) => {
                info!("Starting experiment");
                if let Some(path) = &experiment.metadata.output_path {
                    File::create(path);
                }
                update_send.send(OrgUpdate::UpdateExperimentStatus(ExperimentStatus::Running)).await;
                update_send.send(OrgUpdate::UpdateExperimentMetadata(experiment.metadata.clone())).await;
                for (i, stage) in experiment.stages.clone().into_iter().enumerate() {
                    let name = stage.name.clone();
                    info!("Executing stage {name}");
                    update_send.send(OrgUpdate::UpdateExperimentStage(name.clone())).await;
                    Self::execute_stage(client.clone(), stage, update_send.clone()).await;
                    info!("Finished stage {name}");
                }
                update_send.send(OrgUpdate::UpdateExperimentStatus(ExperimentStatus::Done)).await;
            }
            OrgChannelMsg::Ping(socket_addr) => {
                let msg = RpcMessageKind::HostCtrl(HostCtrl::Ping);
                let mut client = client.lock().await;
                client.send_message(target_addr, msg).await;
                let response = client.read_message(target_addr).await?;
                if let RpcMessageKind::HostCtrl(HostCtrl::Pong) = response.msg {
                    Ok(())
                } else {
                    Err("Expected HostStatuses response".into())
                }
            }
        }
    }
}
