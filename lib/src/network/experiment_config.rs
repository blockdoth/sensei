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

use crate::experiments::Command;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "orchestrator")]
    #[test]
    fn test_command_fromstr_connect() {
        let cmd = Command::from_str("connect 127.0.0.1:8080").unwrap();
        match cmd {
            Command::Connect { target_addr } => {
                assert_eq!(target_addr, "127.0.0.1:8080".parse().unwrap());
            }
            _ => panic!("Expected Connect command"),
        }
    }

    #[cfg(feature = "orchestrator")]
    #[test]
    fn test_command_fromstr_disconnect() {
        let cmd = Command::from_str("disconnect 192.168.1.1:1234").unwrap();
        match cmd {
            Command::Disconnect { target_addr } => {
                assert_eq!(target_addr, "192.168.1.1:1234".parse().unwrap());
            }
            _ => panic!("Expected Disconnect command"),
        }
    }

    #[cfg(feature = "orchestrator")]
    #[test]
    fn test_command_fromstr_sendstatus() {
        let cmd = Command::from_str("sendstatus 10.0.0.1:5555 42").unwrap();
        match cmd {
            Command::SendStatus { target_addr, host_id } => {
                assert_eq!(target_addr, "10.0.0.1:5555".parse().unwrap());
                assert_eq!(host_id, 42);
            }
            _ => panic!("Expected SendStatus command"),
        }
    }

    #[cfg(feature = "orchestrator")]
    #[test]
    fn test_command_fromstr_subscribe() {
        let cmd = Command::from_str("sub 127.0.0.1:8080 7").unwrap();
        match cmd {
            Command::Subscribe { target_addr, device_id } => {
                assert_eq!(target_addr, "127.0.0.1:8080".parse().unwrap());
                assert_eq!(device_id, 7);
            }
            _ => panic!("Expected Subscribe command"),
        }
    }

    #[cfg(feature = "orchestrator")]
    #[test]
    fn test_command_fromstr_unsubscribe() {
        let cmd = Command::from_str("unsub 127.0.0.1:8080 8").unwrap();
        match cmd {
            Command::Unsubscribe { target_addr, device_id } => {
                assert_eq!(target_addr, "127.0.0.1:8080".parse().unwrap());
                assert_eq!(device_id, 8);
            }
            _ => panic!("Expected Unsubscribe command"),
        }
    }

    #[cfg(feature = "orchestrator")]
    #[test]
    fn test_command_fromstr_invalid_command() {
        let err = Command::from_str("foobar 127.0.0.1:8080").unwrap_err();
        assert_eq!(format!("{err:?}"), format!("{:?}", crate::errors::CommandError::NoSuchCommand));
    }

    #[cfg(feature = "orchestrator")]
    #[test]
    fn test_command_fromstr_missing_argument() {
        let err = Command::from_str("connect").unwrap_err();
        assert_eq!(format!("{err:?}"), format!("{:?}", crate::errors::CommandError::MissingArgument));
    }

    #[cfg(feature = "orchestrator")]
    #[test]
    fn test_command_fromstr_invalid_argument() {
        let err = Command::from_str("sub 127.0.0.1:8080 notanumber").unwrap_err();
        assert_eq!(format!("{err:?}"), format!("{:?}", crate::errors::CommandError::InvalidArgument));
    }
}
