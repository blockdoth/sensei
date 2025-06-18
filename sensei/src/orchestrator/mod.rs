mod experiment;
mod state;
mod tui;

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
use tokio::signal;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::{Mutex, mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::sleep;

use crate::orchestrator::experiment::{ActiveExperiment, ExperimentChannelMsg, ExperimentSession, ExperimentStatus};
use crate::orchestrator::state::{OrgTuiState, OrgUpdate};
use crate::services::{GlobalConfig, OrchestratorConfig, Run};

pub struct Orchestrator {
    log_level: LevelFilter,
    experiments_folder: PathBuf,
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
    GetHostStatuses(SocketAddr),
    Delay(u64),
    Shutdown,
    Ping(SocketAddr),
    SelectExperiment(usize),
    StartExperiment,
    StopExperiment,
}

impl Run<OrchestratorConfig> for Orchestrator {
    fn new(global_config: GlobalConfig, config: OrchestratorConfig) -> Self {
        Orchestrator {
            log_level: global_config.log_level,
            experiments_folder: config.experiments_folder,
            tui: config.tui,
        }
    }
    async fn run(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let (command_send, mut command_recv) = mpsc::channel::<OrgChannelMsg>(1000);
        let (update_send, mut update_recv) = mpsc::channel::<OrgUpdate>(1000);
        let (experiment_send, mut experiment_recv) = mpsc::channel::<ExperimentChannelMsg>(1000);

        let client = Arc::new(Mutex::new(TcpClient::new()));

        let (cancel_signal_send, cancel_signal_recv) = watch::channel(false);
        let session = ExperimentSession::new(client.clone(), update_send.clone(), cancel_signal_recv);
        // Tasks needs to be boxed and pinned in order to make the type checker happy
        let tasks: Vec<Pin<Box<dyn Future<Output = ()> + Send>>> = vec![
            Box::pin(Self::command_handler(command_recv, update_send.clone(), experiment_send, client.clone())),
            Box::pin(Self::experiment_handler(
                session,
                self.experiments_folder.clone(),
                client.clone(),
                experiment_recv,
                update_send.clone(),
                cancel_signal_send,
            )),
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
    async fn experiment_handler(
        mut session: ExperimentSession,
        experiment_config_path: PathBuf,
        client: Arc<Mutex<TcpClient>>,
        mut experiment_recv: Receiver<ExperimentChannelMsg>,
        update_send: Sender<OrgUpdate>,
        cancel_signal_send: watch::Sender<bool>,
    ) {
        info!("Started experiment handler task");

        session.load_experiments(experiment_config_path.clone());
        info!("Loaded {} experiments from {experiment_config_path:?}", session.experiments.len());
        update_send
            .send(OrgUpdate::UpdateExperimentList(
                session.experiments.iter().map(|f| f.metadata.clone()).collect(),
            ))
            .await;

        info!("Waiting for experiment updates");
        while let Some(msg) = experiment_recv.recv().await {
            match msg {
                ExperimentChannelMsg::Start => {
                    if let Some(active) = &session.active_experiment {
                        match active.status {
                            ExperimentStatus::Running => {
                                debug!("Can't start experiment while another is running");
                            }
                            _ => {
                                cancel_signal_send.send(false);
                                let mut session = session.clone();
                                let client = client.clone();
                                let update_send = update_send.clone();
                                tokio::spawn(async move {
                                    session.run(client, update_send).await;
                                });
                            }
                        }
                    }
                }

                ExperimentChannelMsg::Stop => {
                    if let Some(active) = &mut session.active_experiment {
                        active.status = ExperimentStatus::Stopped;
                        cancel_signal_send.send(true);
                    }
                }

                ExperimentChannelMsg::Select(i) => {
                    if let Some(exp) = session.experiments.get(i) {
                        session.active_experiment = Some(ActiveExperiment {
                            experiment: exp.clone(),
                            status: ExperimentStatus::Ready,
                            current_stage: 0,
                        })
                    }
                }
            }

            update_send
                .send(OrgUpdate::ActiveExperiment(session.active_experiment.clone().unwrap()))
                .await;
        }
    }

    async fn command_handler(
        mut recv_commands_channel: Receiver<OrgChannelMsg>,
        update_send: Sender<OrgUpdate>,
        experiment_send: Sender<ExperimentChannelMsg>,
        client: Arc<Mutex<TcpClient>>,
    ) {
        info!("Started stream handler task");
        let mut receiving = false;
        let mut targets: Vec<SocketAddr> = vec![];

        loop {
            tokio::select! {
                // Commands from channel
                msg_opt = recv_commands_channel.recv() => {
                    debug!("Received channel message {msg_opt:?}");
                    if let Some(msg) = msg_opt {
                        Self::handle_msg(client.clone(), msg, update_send.clone(), Some(experiment_send.clone())).await;
                    }
                    debug!("Handled");
                }

                // TCP Client messages if in receiving mode
                _ = async {
                    if receiving {
                        for target_addr in &targets {
                            if let Ok(msg) = client.lock().await.wait_for_read_message(*target_addr).await {
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
                    } else {
                      sleep(Duration::from_millis(10)).await; // Prevents this tasks from starving the other one
                    }
                } => {}
            }
        }
    }

    pub async fn handle_msg(
        client: Arc<Mutex<TcpClient>>,
        msg: OrgChannelMsg,
        update_send: Sender<OrgUpdate>,
        experiment_send: Option<Sender<ExperimentChannelMsg>>, // Option allows reuse of this function in experiment.rs
    ) {
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
                    info!("Unsubscribing from {target_addr} for device id {device_id}");
                    let msg = HostCtrl::UnsubscribeFrom { target_addr, device_id };
                    client.lock().await.send_message(msg_origin_addr, RpcMessageKind::HostCtrl(msg)).await;
                } else {
                    info!("Unsubscribing from {target_addr} for device id {device_id}");
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
            OrgChannelMsg::SelectExperiment(idx) => {
                if let Some(experiment_send) = experiment_send {
                    experiment_send.send(ExperimentChannelMsg::Select(idx)).await;
                }
            }
            OrgChannelMsg::StartExperiment => {
                if let Some(experiment_send) = experiment_send {
                    experiment_send.send(ExperimentChannelMsg::Start).await;
                }
            }
            OrgChannelMsg::StopExperiment => {
                if let Some(experiment_send) = experiment_send {
                    experiment_send.send(ExperimentChannelMsg::Stop).await;
                }
            }
            OrgChannelMsg::GetHostStatuses(target_addr) => {
                let msg = RpcMessageKind::RegCtrl(RegCtrl::PollHostStatuses);
                let mut client = client.lock().await;
                client.send_message(target_addr, msg).await;
                if let Ok(response) = client.wait_for_read_message(target_addr).await {
                    if let RpcMessageKind::RegCtrl(RegCtrl::HostStatuses { host_statuses }) = response.msg {
                        info!("{host_statuses:?}");
                    }
                }
            }

            OrgChannelMsg::Ping(target_addr) => {
                let msg = RpcMessageKind::HostCtrl(HostCtrl::Ping);
                let mut client = client.lock().await;
                client.send_message(target_addr, msg).await;
                if let Ok(response) = client.read_message(target_addr).await {
                    if let RpcMessageKind::HostCtrl(HostCtrl::Pong) = response.msg {
                        debug!("idk");
                    } else {
                        error!("Expected HostStatuses response")
                    }
                } else {
                    error!("Channel error")
                }
            }
        }
    }
}
