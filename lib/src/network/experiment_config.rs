//! # Experiment data structures
//!
//! Experiments are how independently running processes are defined in Sensei!
//!
//! To allow for customizable experiments, they are highly modular.
//! An `Experiment` consists of `Stages`, which are run sequentially (in order).
//!
//! A `Stage` consists of command `Blocks`, which are run at the same time (using tokio tasks).
//! All `Blocks` in a `Stage` must finish before the next `Stage` starts.
//!
//! A command `Block` consists of `Commands`, which are run sequentially, and it has `Delays` associated with them.
//! A `Delay` is also a command in itself.
//!
//! This structure is useful for deciding which commands to run at which times.

use std::net::SocketAddr;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
#[cfg(feature = "orchestrator")]
use {crate::errors::CommandError, crate::network::rpc_message::DEFAULT_ADDRESS, log::info, std::str::FromStr};

use crate::network::rpc_message::{CfgType, DeviceId, HostId};

/// Represents a full experiment composed of multiple sequential stages.
/// Includes `Metadata`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Experiment {
    pub metadata: Metadata,
    pub stages: Vec<Stage>,
}

/// Represents the host on which the experiment is executed.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum ExperimentHost {
    Orchestrator,
    SystemNode { target_addr: SocketAddr },
}

/// Metadata about the experiment such as `name`, `experiment_host`, and `output_path`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Metadata {
    pub name: String,
    pub experiment_host: ExperimentHost,
    pub output_path: Option<PathBuf>,
}

/// Represents a stage in the experiment, which contains multiple command blocks.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Stage {
    pub name: String,
    pub command_blocks: Vec<Block>,
}

/// A block of commands to be executed sequentially, with associated delays.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Block {
    pub commands: Vec<Command>,
    pub delays: Delays,
}

impl Experiment {
    /// Loads one or more experiments from a YAML file.
    ///
    /// # Arguments
    ///
    /// * `file` - Path to the YAML file containing experiment definitions.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or parsed.
    pub fn from_yaml(file: PathBuf) -> Result<Vec<Self>, Box<dyn std::error::Error>> {
        let yaml = std::fs::read_to_string(file.clone()).map_err(|e| format!("Failed to read YAML file: {}\n{}", file.display(), e))?;
        Ok(serde_yaml::from_str(&yaml)?)
    }
}

/// Delay configuration for a command block.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Delays {
    pub init_delay: Option<u64>,
    pub command_delay: Option<u64>,
    pub is_recurring: IsRecurring,
}

/// Indicates whether a block is recurring and its recurrence configuration.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum IsRecurring {
    Recurring {
        recurrence_delay: Option<u64>,
        iterations: Option<u64>, /* 0 or None is infinite */
    },
    NotRecurring,
}

/// `Commands` are the actions that the orchestrator or system node can undertake in an experiment
///
/// These commands are `Connect`, `Disconnect`, `Subscribe`, `Unsubscribe`, `SubscribeTo`, `UnsubscribeFrom`, `SendStatus`, `Configure` and `Delay`
///
/// The orchestrator executes these commands by sending messages to system nodes and telling them to run the commands
///
/// The system node executes these commands by running them locally.
/// System nodes can only run the `Subscribe`, `Unsubscribe`, `Configure` and `Delay` commands,
/// as connecting and disconnecting are not relevant concepts to a system node,
/// and it is not necessary for a system node to tell another system node to subscribe to a third system node.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Command {
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
        device_id: DeviceId,
        source_addr: SocketAddr,
    },
    UnsubscribeFrom {
        target_addr: SocketAddr,
        device_id: DeviceId,
        source_addr: SocketAddr,
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
    DummyData {},
}

#[cfg(feature = "orchestrator")]
impl FromStr for Command {
    type Err = CommandError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut split = s.split_whitespace();
        let base = split.next().ok_or(CommandError::NoSuchCommand)?;

        let target_addr = split.next().ok_or(CommandError::MissingArgument)?.parse().unwrap_or(DEFAULT_ADDRESS);

        match base {
            "connect" => Ok(Command::Connect { target_addr }),
            "disconnect" => Ok(Command::Disconnect { target_addr }),
            "sendstatus" => Ok(Command::SendStatus {
                target_addr,
                // Technically host_id and device_id could become different types. Might as well handle them separately.
                host_id: split
                    .next()
                    .ok_or(CommandError::MissingArgument)?
                    .parse::<u64>()
                    .map_err(|_| CommandError::InvalidArgument)?,
            }),
            base => {
                use crate::handler::device_handler::DeviceHandlerConfig;

                let device_id = split
                    .next()
                    .ok_or(CommandError::MissingArgument)?
                    .parse::<u64>()
                    .map_err(|_| CommandError::InvalidArgument)?;
                match base {
                    "sub" => Ok(Command::Subscribe { target_addr, device_id }),
                    "unsub" => Ok(Command::Unsubscribe { target_addr, device_id }),
                    "configure" => {
                        let config_path: PathBuf = split
                            .next()
                            .ok_or(CommandError::MissingArgument)?
                            .parse()
                            .map_err(|_| CommandError::InvalidArgument)?;
                        let cfgs = DeviceHandlerConfig::from_yaml(config_path.clone())?;
                        let cfg = if cfgs.len() > 1 {
                            info!("Several configs were supplied. Grabbing the first one");
                            cfgs.first().ok_or(CommandError::InvalidArgument)?
                        } else if cfgs.is_empty() {
                            return Err(CommandError::InvalidArgument);
                        } else {
                            cfgs.first().ok_or(CommandError::InvalidArgument)?
                        };
                        Ok(Command::Configure {
                            target_addr,
                            device_id,
                            cfg_type: todo!(),
                        })
                    }
                    base => {
                        let source_addr = todo!();
                        match base {
                            "unsubfrom" => Ok(Command::UnsubscribeFrom {
                                target_addr,
                                source_addr,
                                device_id,
                            }),
                            "subto" => Ok(Command::SubscribeTo {
                                target_addr,
                                device_id,
                                source_addr,
                            }),
                            _ => Err(CommandError::NoSuchCommand),
                        }
                    }
                }
            }
            "dummydata" => Ok(Command::DummyData {}),
            _ => Err(CommandError::NoSuchCommand),
        }
    }
}
