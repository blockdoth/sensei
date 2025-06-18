use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use lib::network::rpc_message::{CfgType, DeviceId, HostId};
use lib::network::tcp::client::TcpClient;
use log::{debug, info};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::Sender;
use tokio::sync::{Mutex, watch};
use tokio::time::sleep;

use crate::orchestrator::state::OrgUpdate;
use crate::orchestrator::{Orchestrator, OrgChannelMsg};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ExperimentMetadata {
    pub name: String,
    pub output_path: PathBuf,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Stage {
    pub name: String,
    pub blocks: Vec<Block>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Block {
    pub commands: Vec<ExperimentCommand>,
    pub delays: Delays,
}

impl Experiment {
    pub fn from_yaml(file: PathBuf) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let yaml = std::fs::read_to_string(file.clone()).map_err(|e| format!("Failed to read YAML file: {}\n{}", file.display(), e))?;
        Ok(serde_yaml::from_str(&yaml)?)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Delays {
    pub init_delay: Option<u64>,
    pub command_delay: Option<u64>,
    pub delay_type: DelayType,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum DelayType {
    Recurring {
        recurrence_delay: Option<u64>,
        iterations: Option<u64>, /* 0 is infinite */
    },
    NotRecurring,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExperimentStatus {
    Ready,
    Running,
    Done,
    Stopped,
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
    GetHostStatuses {
        target_addr: SocketAddr,
    },
    Configure {
        target_addr: SocketAddr,
        device_id: DeviceId,
        cfg_type: CfgType,
    },
    Ping {
        target_addr: SocketAddr,
    },
    Delay {
        delay: u64,
    },
}

#[derive(Debug)]
pub enum ExperimentChannelMsg {
    Start,
    Stop,
    Select(usize),
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
            ExperimentCommand::GetHostStatuses { target_addr } => OrgChannelMsg::GetHostStatuses(target_addr),
            ExperimentCommand::Ping { target_addr } => OrgChannelMsg::Ping(target_addr),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Experiment {
    pub metadata: ExperimentMetadata,
    pub stages: Vec<Stage>,
}

// Ugly wrapper to allow from yaml to work
#[derive(Clone, Debug)]
pub struct ActiveExperiment {
    pub experiment: Experiment,
    pub status: ExperimentStatus,
    pub current_stage: usize,
}

#[derive(Clone)]
pub struct ExperimentSession {
    client: Arc<Mutex<TcpClient>>,
    update_send_channel: Sender<OrgUpdate>,
    pub cancel_signal: watch::Receiver<bool>,
    pub experiments: Vec<Experiment>,
    pub active_experiment: Option<ActiveExperiment>,
}

impl ExperimentSession {
    pub fn new(client: Arc<Mutex<TcpClient>>, update_send: Sender<OrgUpdate>, cancel_signal: watch::Receiver<bool>) -> Self {
        ExperimentSession {
            client,
            update_send_channel: update_send,
            experiments: vec![],
            active_experiment: None,
            cancel_signal,
        }
    }

    pub fn load_experiments(&mut self, folder: PathBuf) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        for entry in fs::read_dir(&folder)? {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };

            let path = entry.path();

            if path.is_file() && (path.extension().is_some_and(|ext| ext == "yaml" || ext == "yml")) {
                match Experiment::from_yaml(path.clone()) {
                    Ok(experiment) => {
                        debug!("Successfully parsed config from {path:?}");
                        self.experiments.push(experiment);
                    }
                    Err(e) => {
                        debug!("Unable to load experiment config {path:?}: {e}");
                    }
                }
            }
        }
        Ok(())
    }

    pub async fn run(&mut self, client: Arc<Mutex<TcpClient>>, update_send: Sender<OrgUpdate>) {
        if let Some(mut active_exp) = self.active_experiment.clone() {
            active_exp.status = ExperimentStatus::Running;
            active_exp.current_stage = 0;
            update_send.send(OrgUpdate::ActiveExperiment(active_exp.clone())).await;
            info!("Running experiment {}", active_exp.experiment.metadata.name);

            // active_exp.status = ExperimentStatus::Stopped;
            // update_send.send(OrgUpdate::ActiveExperiment(active_exp.clone())).await;
            //       self.active_experiment = Some(active_exp);
            //       debug!("Finished experiment");
            //       return;
            //   }
            self.cancel_signal.changed().await;
            let cancel_signal_task = self.cancel_signal.clone();
            let mut cancel_signal_cancel = self.cancel_signal.clone();
            let client = client.clone();
            let update_send = update_send.clone();

            let task = async {
                for stage in &active_exp.experiment.stages {
                    stage.execute(client.clone(), update_send.clone(), cancel_signal_task.clone()).await;
                    active_exp.current_stage += 1;
                    let _ = update_send.send(OrgUpdate::ActiveExperiment(active_exp.clone())).await;
                    debug!("Completed stage {}", active_exp.current_stage);
                }
            };
            tokio::select! {
              _ = task => {
                active_exp.status = ExperimentStatus::Done;
                let _ = update_send.send(OrgUpdate::ActiveExperiment(active_exp.clone())).await;
                self.active_experiment = Some(active_exp);
                debug!("Finished experiment");
              }
              _ = cancel_signal_cancel.changed() => {
                  if *cancel_signal_cancel.borrow() {
                      active_exp.status = ExperimentStatus::Stopped;
                      let _ = update_send.send(OrgUpdate::ActiveExperiment(active_exp.clone())).await;
                      debug!("Experiment cancelled during stage {}", active_exp.current_stage);
                      self.active_experiment = Some(active_exp);
                  }
              }
            }
        }
    }

    pub async fn match_commands(commands: Vec<ExperimentCommand>, command_delay: u64, client: Arc<Mutex<TcpClient>>, update_send: Sender<OrgUpdate>) {
        for command in commands {
            Orchestrator::handle_msg(client.clone(), command.into(), update_send.clone(), None);
            sleep(Duration::from_millis(command_delay)).await;
        }
    }
}

impl Stage {
    pub async fn execute(&self, client: Arc<Mutex<TcpClient>>, update_send: Sender<OrgUpdate>, cancel_signal: watch::Receiver<bool>) {
        let mut tasks = vec![];

        for block in self.blocks.clone() {
            let client = client.clone();
            let update_send = update_send.clone();
            let cancel_signal = cancel_signal.clone();
            tasks.push(tokio::spawn(async move {
                block.execute(client, update_send, cancel_signal).await;
            }));
        }
        futures::future::join_all(tasks).await;
    }
}
impl Block {
    pub async fn execute(&self, client: Arc<Mutex<TcpClient>>, update_send: Sender<OrgUpdate>, mut cancel_signal: watch::Receiver<bool>) {
        sleep(Duration::from_millis(self.delays.init_delay.unwrap_or(0u64))).await;
        let command_delay = self.delays.command_delay.unwrap_or(0u64);

        match self.delays.delay_type.clone() {
            DelayType::Recurring {
                recurrence_delay,
                iterations,
            } => {
                let r_delay = recurrence_delay.unwrap_or(0u64);
                let n = iterations.unwrap_or(0u64);
                let mut i = 0;

                while n == 0 || i < n {
                    if *cancel_signal.borrow() {
                        break;
                    }

                    tokio::select! {
                        _ = ExperimentSession::match_commands(self.commands.clone(), command_delay, client.clone(), update_send.clone()) => {},
                        _ = cancel_signal.changed() => {
                            if *cancel_signal.borrow() { break; }
                        }
                    }

                    sleep(Duration::from_millis(r_delay)).await;
                    i += 1;
                }
            }
            DelayType::NotRecurring => {
                tokio::select! {
                    _ = ExperimentSession::match_commands(self.commands.clone(), command_delay, client.clone(), update_send.clone()) => {},
                    _ = cancel_signal.changed() => {}
                }
            }
        }
    }
}
