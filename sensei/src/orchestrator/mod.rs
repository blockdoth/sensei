mod state;
mod tui;

use std::error::Error;
use std::fs::File;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::vec;

use lib::network::rpc_message::RpcMessageKind::Data;
use lib::network::rpc_message::{CfgType, DataMsg, DeviceId, HostCtrl, HostId, RegCtrl, RpcMessageKind};
use lib::network::tcp::client::TcpClient;
use lib::tui::TuiRunner;
use log::*;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::Receiver;
use tokio::sync::{Mutex, mpsc};
use tokio::time::sleep;

use crate::orchestrator::IsRecurring::{NotRecurring, Recurring};
use crate::orchestrator::state::{OrgTuiState, OrgUpdate};
use crate::services::{GlobalConfig, OrchestratorConfig, Run};

pub struct Orchestrator {
    client: Arc<Mutex<TcpClient>>,
    log_level: LevelFilter,
    experiment_config: PathBuf,
    output_path: Option<PathBuf>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Experiment {
    metadata: Metadata,
    stages: Vec<Stage>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Metadata {
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
    pub fn from_yaml(file: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
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
    Shutdown,
}

impl Run<OrchestratorConfig> for Orchestrator {
    fn new(global_config: GlobalConfig, config: OrchestratorConfig) -> Self {
        Orchestrator {
            client: Arc::new(Mutex::new(TcpClient::new())),
            log_level: global_config.log_level,
            experiment_config: config.experiment_config,
            output_path: None,
        }
    }
    async fn run(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let (command_send, mut command_recv) = mpsc::channel::<OrgChannelMsg>(1000);
        let (update_send, mut update_recv) = mpsc::channel::<OrgUpdate>(1000);

        let tasks = vec![Self::listen(command_recv, self.client.clone())];

        let tui = OrgTuiState::new();

        // let experiment = Experiment::from_yaml(self.experiment_config.clone())?;

        // self.load_experiment(self.client.clone(), experiment).await;

        let tui_runner = TuiRunner::new(tui, command_send, update_recv, update_send, self.log_level);
        tui_runner.run(tasks).await;
        Ok(())
    }
}

impl Orchestrator {
    async fn listen(mut recv_commands_channel: Receiver<OrgChannelMsg>, client: Arc<Mutex<TcpClient>>) {
        info!("Started stream processor task");
        let mut receiving = false;
        let mut targets: Vec<SocketAddr> = vec![];
        loop {
            if !recv_commands_channel.is_empty() {
                let msg_opt = recv_commands_channel.recv().await;
                debug!("Received channel message {msg_opt:?}");
                if let Some(msg) = msg_opt {
                    match msg {
                        OrgChannelMsg::Connect(to_addr) => {
                          Self::connect(&client, to_addr).await;
                        }
                        OrgChannelMsg::Disconnect(to_addr) => {
                            Self::disconnect(&client, to_addr).await;
                        }
                        OrgChannelMsg::Subscribe(to_addr, msg_origin_addr, device_id) => {
                            if let Some(msg_origin_addr) = msg_origin_addr {
                                Self::subscribe_to(&client, to_addr, msg_origin_addr, device_id).await;
                            } else {
                                Self::subscribe(&client, to_addr, device_id).await;
                            }
                        }
                        OrgChannelMsg::Unsubscribe(to_addr, msg_origin_addr, device_id) => {
                            if let Some(msg_origin_addr) = msg_origin_addr {
                                Self::unsubscribe_from(&client, to_addr, msg_origin_addr, device_id).await;
                            } else {
                                Self::unsubscribe(&client, to_addr, device_id).await;
                            }
                        }
                        OrgChannelMsg::SubscribeAll(to_addr, msg_origin_addr) => {
                            todo!()
                        }
                        OrgChannelMsg::UnsubscribeAll(to_addr, msg_origin_addr) => {
                            todo!()
                        }
                        OrgChannelMsg::Shutdown => todo!(),
                    }
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

    pub async fn load_experiment(&mut self, client: Arc<Mutex<TcpClient>>, experiment: Experiment) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.output_path = experiment.metadata.output_path;

        if let Some(path) = &self.output_path {
            File::create(path)?;
        }

        for (i, stage) in experiment.stages.into_iter().enumerate() {
            let name = stage.name.clone();
            info!("Executing stage {name}");
            Self::execute_stage(&client, stage).await?;
            info!("Finished stage {name}");
        }

        Ok(())
    }
    pub async fn execute_stage(client: &Arc<Mutex<TcpClient>>, stage: Stage) -> Result<(), Box<dyn Error + Send + Sync>> {
        let mut tasks = vec![];

        for block in stage.command_blocks {
            let clone_client = client.clone();
            let task = tokio::spawn(async move { Self::execute_command_block(clone_client, block).await.expect("Failed to execute command") });
            tasks.push(task);
        }

        let results = futures::future::join_all(tasks).await;

        for result in results {
            result?;
        }

        Ok(())
    }
    pub async fn execute_command_block(client: Arc<Mutex<TcpClient>>, block: Block) -> Result<(), Box<dyn Error + Send + Sync>> {
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
                if n == 0 {
                    loop {
                        Self::match_commands(client.clone(), command_types.clone(), command_delay).await;
                        sleep(Duration::from_millis(r_delay)).await;
                    }
                } else {
                    for _ in 0..n {
                        Self::match_commands(client.clone(), command_types.clone(), command_delay).await;
                        sleep(Duration::from_millis(r_delay)).await;
                    }
                }
                Ok(())
            }
            NotRecurring => {
                Self::match_commands(client, command_types, command_delay).await;
                Ok(())
            }
        }
    }

    pub async fn match_commands(
        client: Arc<Mutex<TcpClient>>,
        commands: Vec<ExperimentCommand>,
        command_delay: u64,
    ) -> Result<(), Box<dyn std::error::Error>> {
        for command in commands {
            match command {
                ExperimentCommand::Connect { target_addr } => Self::connect(&client, target_addr).await?,
                ExperimentCommand::Disconnect { target_addr } => Self::disconnect(&client, target_addr).await?,
                ExperimentCommand::Subscribe { target_addr, device_id } => Self::subscribe(&client, target_addr, device_id).await?,
                ExperimentCommand::Unsubscribe { target_addr, device_id } => Self::unsubscribe(&client, target_addr, device_id).await?,
                ExperimentCommand::SubscribeTo {
                    target_addr,
                    source_addr,
                    device_id,
                } => Self::subscribe_to(&client, target_addr, source_addr, device_id).await?,
                ExperimentCommand::UnsubscribeFrom {
                    target_addr,
                    source_addr,
                    device_id,
                } => Self::unsubscribe_from(&client, target_addr, source_addr, device_id).await?,
                ExperimentCommand::SendStatus { target_addr, host_id } => Self::send_status(&client, target_addr, host_id).await?,
                ExperimentCommand::Configure {
                    target_addr,
                    device_id,
                    cfg_type,
                } => Self::configure(&client, target_addr, device_id, cfg_type).await?,
                ExperimentCommand::Delay { delay } => {
                    sleep(Duration::from_millis(delay)).await;
                }
            }
            sleep(Duration::from_millis(command_delay)).await;
        }
        Ok(())
    }

    async fn connect(client: &Arc<Mutex<TcpClient>>, target_addr: SocketAddr) -> Result<(), Box<dyn std::error::Error>> {
        Ok(client.lock().await.connect(target_addr).await?)
    }

    async fn disconnect(client: &Arc<Mutex<TcpClient>>, target_addr: SocketAddr) -> Result<(), Box<dyn std::error::Error>> {
        Ok(client.lock().await.disconnect(target_addr).await?)
    }

    async fn subscribe(client: &Arc<Mutex<TcpClient>>, target_addr: SocketAddr, device_id: u64) -> Result<(), Box<dyn std::error::Error>> {
        let msg = HostCtrl::Subscribe { device_id };
        info!("Subscribing to {target_addr} for device id {device_id}");
        Ok(client.lock().await.send_message(target_addr, RpcMessageKind::HostCtrl(msg)).await?)
    }

    async fn unsubscribe(client: &Arc<Mutex<TcpClient>>, target_addr: SocketAddr, device_id: u64) -> Result<(), Box<dyn std::error::Error>> {
        let msg = HostCtrl::Unsubscribe { device_id };
        info!("Unsubscribing from {target_addr} for device id {device_id}");
        Ok(client.lock().await.send_message(target_addr, RpcMessageKind::HostCtrl(msg)).await?)
    }

    async fn subscribe_to(
        client: &Arc<Mutex<TcpClient>>,
        target_addr: SocketAddr,
        source_addr: SocketAddr,
        device_id: u64,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let msg = RpcMessageKind::HostCtrl(HostCtrl::SubscribeTo {
            target: source_addr,
            device_id,
        });

        info!("Telling {target_addr} to subscribe to {source_addr} on device id {device_id}");

        Ok(client.lock().await.send_message(target_addr, msg).await?)
    }

    async fn unsubscribe_from(
        client: &Arc<Mutex<TcpClient>>,
        target_addr: SocketAddr,
        source_addr: SocketAddr,
        device_id: u64,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let msg = RpcMessageKind::HostCtrl(HostCtrl::UnsubscribeFrom {
            target: source_addr,
            device_id,
        });

        info!("Telling {target_addr} to unsubscribe from device id {device_id} from {source_addr}");

        Ok(client.lock().await.send_message(target_addr, msg).await?)
    }

    async fn send_status(client: &Arc<Mutex<TcpClient>>, target_addr: SocketAddr, host_id: HostId) -> Result<(), Box<dyn std::error::Error>> {
        let msg = RpcMessageKind::RegCtrl(RegCtrl::PollHostStatus { host_id });

        Ok(client.lock().await.send_message(target_addr, msg).await?)
    }

    async fn configure(
        client: &Arc<Mutex<TcpClient>>,
        target_addr: SocketAddr,
        device_id: DeviceId,
        cfg_type: CfgType,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let msg = RpcMessageKind::HostCtrl(HostCtrl::Configure { device_id, cfg_type });

        info!("Telling {target_addr} to configure the device handler");

        Ok(client.lock().await.send_message(target_addr, msg).await?)
    }
}
