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
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::{Mutex, mpsc};
use tokio::time::sleep;

use crate::orchestrator::IsRecurring::{NotRecurring, Recurring};
use crate::orchestrator::state::{OrgTuiState, OrgUpdate};
use crate::services::{GlobalConfig, OrchestratorConfig, Run};

pub struct Orchestrator {
    client: Arc<Mutex<TcpClient>>,
    log_level: LevelFilter,
    experiment_config_path: Option<PathBuf>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Experiment {
    metadata: ExperimentMetadata,
    stages: Vec<Stage>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ExperimentMetadata {
    name: String,
    output_path: Option<PathBuf>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Stage {
    name: String,
    command_blocks: Vec<Block>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Block {
    commands: Vec<ExperimentCommand>,
    delays: Delays,
}

impl Experiment {
    pub fn from_yaml(file: PathBuf) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let yaml = std::fs::read_to_string(file.clone()).map_err(|e| format!("Failed to read YAML file: {}\n{}", file.display(), e))?;
        Ok(serde_yaml::from_str(&yaml)?)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Delays {
    init_delay: Option<u64>,
    command_delay: Option<u64>,
    is_recurring: IsRecurring,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum IsRecurring {
    Recurring {
        recurrence_delay: Option<u64>,
        iterations: Option<u64>, /* 0 is infinite */
    },
    NotRecurring,
}

#[derive(Debug)]
pub enum ExperimentStatus {
    Running,
    Ready,
    Done,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum ExperimentCommand {
    Connect {
        target_addr: SocketAddr,
    },
    Disconnect {
        target_addr: SocketAddr,
    },
    Subscribe {
        target_addr: SocketAddr,
        device_id: DeviceId,
    },
    Unsubscribe {
        target_addr: SocketAddr,
        device_id: DeviceId,
    },
    SubscribeTo {
        target_addr: SocketAddr,
        source_addr: SocketAddr,
        device_id: DeviceId,
    },
    UnsubscribeFrom {
        target_addr: SocketAddr,
        source_addr: SocketAddr,
        device_id: DeviceId,
    },
    SendStatus {
        target_addr: SocketAddr,
        host_id: HostId,
    },
    Configure {
        target_addr: SocketAddr,
        device_id: DeviceId,
        cfg_type: CfgType,
    },
    Delay {
        delay: u64,
    },
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
    RunExperiment(Experiment),
}

impl From<ExperimentCommand> for OrgChannelMsg {
    fn from(cmd: ExperimentCommand) -> Self {
        match cmd {
            ExperimentCommand::Connect { target_addr } => OrgChannelMsg::Connect(target_addr),
            ExperimentCommand::Disconnect { target_addr } => OrgChannelMsg::Disconnect(target_addr),
            ExperimentCommand::Subscribe { target_addr, device_id } => OrgChannelMsg::Subscribe(target_addr, None, device_id),
            ExperimentCommand::Unsubscribe { target_addr, device_id } => OrgChannelMsg::Unsubscribe(target_addr, None, device_id),
            ExperimentCommand::SubscribeTo {
                target_addr,
                source_addr,
                device_id,
            } => OrgChannelMsg::Subscribe(target_addr, Some(source_addr), device_id),
            ExperimentCommand::UnsubscribeFrom {
                target_addr,
                source_addr,
                device_id,
            } => OrgChannelMsg::Unsubscribe(target_addr, Some(source_addr), device_id),
            ExperimentCommand::SendStatus { target_addr, host_id } => OrgChannelMsg::SendStatus(target_addr, host_id),
            ExperimentCommand::Configure {
                target_addr,
                device_id,
                cfg_type,
            } => OrgChannelMsg::Configure(target_addr, device_id, cfg_type),
            ExperimentCommand::Delay { delay } => OrgChannelMsg::Delay(delay),
        }
    }
}

impl Run<OrchestratorConfig> for Orchestrator {
    fn new(global_config: GlobalConfig, config: OrchestratorConfig) -> Self {
        Orchestrator {
            client: Arc::new(Mutex::new(TcpClient::new())),
            log_level: global_config.log_level,
            experiment_config_path: Some(config.experiment_config),
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

        let tui = OrgTuiState::new();
        let tui_runner = TuiRunner::new(tui, command_send, update_recv, update_send, self.log_level);

        tui_runner.run(tasks).await;
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
                    error!("Unable to load experiment config {:?} {e}", experiment_config_path);
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
              info!("A");
              let mut l = client.lock().await;
              l.connect(target_addr).await;
              info!("B");
            }
            OrgChannelMsg::Disconnect(target_addr) => {
              info!("C");
              client.lock().await.disconnect(target_addr).await;
              info!("D");
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
        }
    }

    pub async fn execute_stage(client: Arc<Mutex<TcpClient>>, stage: Stage, update_send: Sender<OrgUpdate>) {
        let mut tasks = vec![];

        for block in stage.command_blocks {
            let client_clone = client.clone();
            let update_send_clone = update_send.clone();
            let task = tokio::spawn(async move { Self::execute_command_block(client_clone, block, update_send_clone).await });
            tasks.push(task);
        }

        futures::future::join_all(tasks).await;
    }

    pub async fn execute_command_block(client: Arc<Mutex<TcpClient>>, block: Block, update_send: Sender<OrgUpdate>) {
        sleep(Duration::from_millis(block.delays.init_delay.unwrap_or(0u64))).await;
        let command_delay = block.delays.command_delay.unwrap_or(0u64);
        let command_types = block.commands;

        match block.delays.is_recurring.clone() {
            Recurring {
                recurrence_delay,
                iterations,
            } => {
                let r_delay = recurrence_delay.unwrap_or(0u64);
                let n = iterations.unwrap_or(0u64);
                let mut i = 0;
                while n == 0 || i < n {
                    Self::match_commands(client.clone(), command_types.clone(), command_delay, update_send.clone()).await;
                    sleep(Duration::from_millis(r_delay)).await;
                    i += 1;
                }
            }
            NotRecurring => {
                Self::match_commands(client.clone(), command_types, command_delay, update_send).await;
            }
        }
    }

    pub async fn match_commands(client: Arc<Mutex<TcpClient>>, commands: Vec<ExperimentCommand>, command_delay: u64, update_send: Sender<OrgUpdate>) {
        for command in commands {
            Self::handle_msg(client.clone(), update_send.clone(), command.into());
            sleep(Duration::from_millis(command_delay)).await;
        }
    }
}
