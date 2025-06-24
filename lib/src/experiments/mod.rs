use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use log::{debug, info};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::Sender;
use tokio::sync::watch;
use tokio::time::sleep;

use crate::network::rpc_message::{CfgType, DeviceId, HostId};

impl Experiment {
    pub fn from_yaml(file: PathBuf) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let yaml = std::fs::read_to_string(file.clone()).map_err(|e| format!("Failed to read YAML file: {}\n{}", file.display(), e))?;
        Ok(serde_yaml::from_str(&yaml)?)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Delays {
    pub init_delay: Option<u64>,
    pub command_delay: Option<u64>,
    pub delay_type: DelayType,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum DelayType {
    Recurring {
        recurrence_delay: Option<u64>,
        iterations: Option<u64>, /* 0 is infinite */
    },
    NotRecurring,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ExperimentStatus {
    Ready,
    Running,
    Done,
    Stopped,
}

/// Metadata about the experiment such as `name`, `experiment_host`, and `output_path`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Metadata {
    pub name: String,
    pub output_path: PathBuf,
    pub remote_host: Option<SocketAddr>,
}

/// Represents a full experiment composed of multiple sequential stages.
/// Includes `Metadata`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Experiment {
    pub metadata: Metadata,
    pub stages: Vec<Stage>,
}

/// `Commands` are the actions that the orchestrator or system node can undertake in an experiment
///
/// These commands are `Connect`, `Disconnect`, `Subscribe(All)`, `Unsubscribe(All)`, `SubscribeTo`, `UnsubscribeFrom`, `SendStatus`, `Configure`, `Start(All)`, `Stop(All)` and `Delay`
///
/// The orchestrator executes these commands by sending messages to system nodes and telling them to run the commands
///
/// The system node executes these commands by running them locally.
/// System nodes can only run the `Subscribe`, `Unsubscribe`, `Configure`, `Start(All)`, `Stop(All)` and `Delay` commands,
/// as connecting and disconnecting are not relevant concepts to a system node,
/// and it is not necessary for a system node to tell another system node to subscribe to a third system node.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Command {
    /// Tells the orchestrator to connect to a system node
    Connect {
        target_addr: SocketAddr,
    },
    /// Tells the orchestrator to disconnect from a system node
    Disconnect {
        target_addr: SocketAddr,
    },
    /// Tells the orchestrator to subscribe to a system node
    ///
    /// Tells a system node to subscribe to a system node
    ///
    /// This is done by creating a device handler with a tcp source pointing to the target node.
    /// This device handler is immediately started
    Subscribe {
        target_addr: SocketAddr,
        device_id: DeviceId,
    },
    /// Tells the orchestrator to subscribe to all devices of a node
    ///
    /// Tells a system node to subscribe to all devices of a node
    SubscribeAll {
        target_addr: SocketAddr,
    },
    /// Tells the orchestrator to unsubscribe to a system node
    ///
    /// Tells a system node to unsubscribe from a system node
    Unsubscribe {
        target_addr: SocketAddr,
        device_id: DeviceId,
    },
    /// Tells the orchestrator to unsubscribe from all devices of a node
    ///
    /// Tells a system node to subscribe to all devices of a node
    UnsubscribeAll {
        target_addr: SocketAddr,
    },
    /// Tells the orchestrator to tell a system node to subscribe to a device on another system node
    SubscribeTo {
        target_addr: SocketAddr,
        device_id: DeviceId,
        source_addr: SocketAddr,
    },
    /// Tells the orchestrator to tell a system node to subscribe to all devices on another system node
    SubscribeToAll {
        target_addr: SocketAddr,
        source_addr: SocketAddr,
    },
    /// Tells the orchestrator to tell a system node to unsubscribe from a device on another system node
    UnsubscribeFrom {
        target_addr: SocketAddr,
        device_id: DeviceId,
        source_addr: SocketAddr,
    },
    /// Tells the orchestrator to tell a system node to unsubscribe from all devices on another system node
    UnsubscribeFromAll {
        target_addr: SocketAddr,
        source_addr: SocketAddr,
    },
    SendStatus {
        target_addr: SocketAddr,
        host_id: HostId,
    },
    GetHostStatuses {
        target_addr: SocketAddr,
    },
    /// Tells a node how to configure a device handler
    ///
    /// Creating or editing a device handler does not start it
    Configure {
        target_addr: SocketAddr,
        device_id: DeviceId,
        cfg_type: CfgType,
    },
    /// Tells the orchestrator to send a ping message to a node (expected response is Pong)
    Ping {
        target_addr: SocketAddr,
    },
    /// A manual delay in an experiment command block
    Delay {
        delay: u64,
    },
    /// Tells a device handler to start
    Start {
        target_addr: SocketAddr,
        device_id: DeviceId,
    },
    /// Starts all devices on a system node
    StartAll {
        target_addr: SocketAddr,
    },
    /// Tells a device handler to stop
    Stop {
        target_addr: SocketAddr,
        device_id: DeviceId,
    },
    StopAll {
        target_addr: SocketAddr,
    },
    DummyData {},
}

#[derive(Clone, Debug)]
pub struct ActiveExperiment {
    pub experiment: Experiment,
    pub info: ExperimentInfo,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ExperimentInfo {
    pub status: ExperimentStatus,
    pub current_stage: usize,
}

#[derive(Clone)]
pub struct ExperimentSession<UpdateMsg>
where
    UpdateMsg: Clone + Send + std::fmt::Debug + 'static,
{
    update_send_channel: Sender<UpdateMsg>,
    pub cancel_signal: watch::Receiver<bool>,
    pub experiments: Vec<Experiment>,
    pub active_experiment: Option<ActiveExperiment>,
}

/// Represents a stage in the experiment, which contains multiple command blocks.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Stage {
    pub name: String,
    pub blocks: Vec<Block>,
}

/// A block of commands to be executed sequentially, with associated delays.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Block {
    pub commands: Vec<Command>,
    pub delays: Delays,
}

impl<UpdateMsg> ExperimentSession<UpdateMsg>
where
    UpdateMsg: Clone + Send + std::fmt::Debug + 'static,
{
    /// Creates a new session instance for managing experiments.
    pub fn new(update_send: Sender<UpdateMsg>, cancel_signal: watch::Receiver<bool>) -> Self {
        ExperimentSession {
            update_send_channel: update_send,
            experiments: vec![],
            active_experiment: None,
            cancel_signal,
        }
    }
    /// Loads all YAML experiment files from the specified folder.
    pub fn load_experiments(&mut self, folder: PathBuf) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.experiments.clear();
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

    /// Starts running the currently selected experiment.
    /// Executes all stages and handles cancellation asynchronously.
    pub async fn run<H, F, Fut>(&mut self, update_send: Sender<UpdateMsg>, update_msg_fn: F, handler: Arc<H>)
    where
        F: Fn(ActiveExperiment) -> UpdateMsg + Send + Sync + 'static,
        H: Fn(Command, Sender<UpdateMsg>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        if let Some(mut active_exp) = self.active_experiment.clone() {
            active_exp.info.status = ExperimentStatus::Running;
            active_exp.info.current_stage = 0;
            update_send.send(update_msg_fn(active_exp.clone())).await;
            info!("Running experiment {}", active_exp.experiment.metadata.name);

            // active_exp.status = ExperimentStatus::Stopped;
            // update_send.send(OrgUpdate::ActiveExperiment(active_exp.clone())).await;
            //       self.active_experiment = Some(active_exp);
            //       debug!("Finished experiment");
            //       return;
            //   }
            self.cancel_signal.changed().await; // Clear reset value
            let cancel_signal_task = self.cancel_signal.clone();
            let mut cancel_signal_cancel = self.cancel_signal.clone();
            let update_send = update_send.clone();

            let task = async {
                for stage in &active_exp.experiment.stages {
                    stage.execute(update_send.clone(), cancel_signal_task.clone(), handler.clone()).await;
                    active_exp.info.current_stage += 1;
                    let _ = update_send.send(update_msg_fn(active_exp.clone())).await;
                    debug!("Completed stage {}", active_exp.info.current_stage);
                }
            };
            tokio::select! {
              _ = task => {
                active_exp.info.status = ExperimentStatus::Done;
                let _ = update_send.send(update_msg_fn(active_exp.clone())).await;
                self.active_experiment = Some(active_exp);
                debug!("Finished experiment");
              }
              _ = cancel_signal_cancel.changed() => {
                  if *cancel_signal_cancel.borrow() {
                      active_exp.info.status = ExperimentStatus::Stopped;
                      let _ = update_send.send(update_msg_fn(active_exp.clone())).await;
                      debug!("Experiment cancelled during stage {}", active_exp.info.current_stage);
                      self.active_experiment = Some(active_exp);
                  }
              }
            }
        }
    }

    /// Matches and executes a list of commands with a delay between each.
    ///
    /// Used by blocks and stages to orchestrate command flow.
    pub async fn match_commands<H, Fut>(commands: Vec<Command>, command_delay: u64, update_send: Sender<UpdateMsg>, handler: Arc<H>)
    where
        H: Fn(Command, Sender<UpdateMsg>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        for command in commands {
            handler(command, update_send.clone());
            sleep(Duration::from_millis(command_delay)).await;
        }
    }
}

impl Stage {
    /// Executes all blocks in the stage concurrently.
    ///
    /// Each block runs in a separate task. Blocks share the same cancel signal.  
    pub async fn execute<UpdateMsg, H, Fut>(&self, update_send: Sender<UpdateMsg>, cancel_signal: watch::Receiver<bool>, handler: Arc<H>)
    where
        UpdateMsg: Clone + Send + std::fmt::Debug + 'static,
        H: Fn(Command, Sender<UpdateMsg>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let mut tasks = vec![];

        for block in self.blocks.clone() {
            let update_send = update_send.clone();
            let cancel_signal = cancel_signal.clone();
            let handler = handler.clone();
            tasks.push(tokio::spawn(async move {
                block.execute(update_send, cancel_signal, handler).await;
            }));
        }
        futures::future::join_all(tasks).await;
    }
}

impl Block {
    /// Executes all commands in the block based on delay settings.
    ///
    /// Commands can run once or multiple times depending on `DelayType`.  
    pub async fn execute<UpdateMsg, H, Fut>(&self, update_send: Sender<UpdateMsg>, mut cancel_signal: watch::Receiver<bool>, handler: Arc<H>)
    where
        UpdateMsg: Clone + Send + std::fmt::Debug + 'static,
        H: Fn(Command, Sender<UpdateMsg>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
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
                        _ = ExperimentSession::match_commands(self.commands.clone(), command_delay, update_send.clone(), handler.clone()) => {},
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
                    _ = ExperimentSession::match_commands(self.commands.clone(), command_delay, update_send.clone(), handler.clone()) => {},
                    _ = cancel_signal.changed() => {}
                }
            }
        }
    }
}

// #[cfg(test)]
// mod tests {
//     use super::*;

//     #[cfg(feature = "orchestrator")]
//     #[test]
//     fn test_command_fromstr_connect() {
//         use std::net::SocketAddrV4;

//         let cmd = Command::Connect{target_addr: SocketAddr::from(SocketAddrV4::from("127.0.0.1:8080"))};
//         match cmd {
//             Command::Connect { target_addr } => {
//                 assert_eq!(target_addr, "127.0.0.1:8080".parse().unwrap());
//             }
//             _ => panic!("Expected Connect command"),
//         }
//     }

//     #[cfg(feature = "orchestrator")]
//     #[test]
//     fn test_command_fromstr_disconnect() {
//         let cmd = Command::from_str("disconnect 192.168.1.1:1234").unwrap();
//         match cmd {
//             Command::Disconnect { target_addr } => {
//                 assert_eq!(target_addr, "192.168.1.1:1234".parse().unwrap());
//             }
//             _ => panic!("Expected Disconnect command"),
//         }
//     }

//     #[cfg(feature = "orchestrator")]
//     #[test]
//     fn test_command_fromstr_sendstatus() {
//         let cmd = Command::from_str("sendstatus 10.0.0.1:5555 42").unwrap();
//         match cmd {
//             Command::SendStatus { target_addr, host_id } => {
//                 assert_eq!(target_addr, "10.0.0.1:5555".parse().unwrap());
//                 assert_eq!(host_id, 42);
//             }
//             _ => panic!("Expected SendStatus command"),
//         }
//     }

//     #[cfg(feature = "orchestrator")]
//     #[test]
//     fn test_command_fromstr_subscribe() {
//         let cmd = Command::from_str("sub 127.0.0.1:8080 7").unwrap();
//         match cmd {
//             Command::Subscribe { target_addr, device_id } => {
//                 assert_eq!(target_addr, "127.0.0.1:8080".parse().unwrap());
//                 assert_eq!(device_id, 7);
//             }
//             _ => panic!("Expected Subscribe command"),
//         }
//     }

//     #[cfg(feature = "orchestrator")]
//     #[test]
//     fn test_command_fromstr_unsubscribe() {
//         let cmd = Command::from_str("unsub 127.0.0.1:8080 8").unwrap();
//         match cmd {
//             Command::Unsubscribe { target_addr, device_id } => {
//                 assert_eq!(target_addr, "127.0.0.1:8080".parse().unwrap());
//                 assert_eq!(device_id, 8);
//             }
//             _ => panic!("Expected Unsubscribe command"),
//         }
//     }

//     #[cfg(feature = "orchestrator")]
//     #[test]
//     fn test_command_fromstr_invalid_command() {
//         let err = Command::from_str("foobar 127.0.0.1:8080").unwrap_err();
//         assert_eq!(format!("{err:?}"), format!("{:?}", crate::errors::CommandError::NoSuchCommand));
//     }

//     #[cfg(feature = "orchestrator")]
//     #[test]
//     fn test_command_fromstr_missing_argument() {
//         let err = Command::from_str("connect").unwrap_err();
//         assert_eq!(format!("{err:?}"), format!("{:?}", crate::errors::CommandError::MissingArgument));
//     }

//     #[cfg(feature = "orchestrator")]
//     #[test]
//     fn test_command_fromstr_invalid_argument() {
//         let err = Command::from_str("sub 127.0.0.1:8080 notanumber").unwrap_err();
//         assert_eq!(format!("{err:?}"), format!("{:?}", crate::errors::CommandError::InvalidArgument));
//     }
// }
