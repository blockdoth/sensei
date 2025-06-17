use std::net::SocketAddr;
use std::path::PathBuf;
use serde::{Deserialize, Serialize};
use crate::network::rpc_message::{CfgType, DeviceId, HostId};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Experiment {
    pub metadata: Metadata,
    pub stages: Vec<Stage>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum ExperimentHost {
    Orchestrator,
    SystemNode { target_addr: SocketAddr },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Metadata {
    pub name: String,
    pub experiment_host: ExperimentHost,
    pub output_path: Option<PathBuf>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Stage {
    pub name: String,
    pub command_blocks: Vec<Block>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Block {
    pub commands: Vec<Command>,
    pub delays: Delays,
}

impl Experiment {
    pub fn from_yaml(file: PathBuf) -> Result<Vec<Self>, Box<dyn std::error::Error>> {
        let yaml = std::fs::read_to_string(file.clone()).map_err(|e| format!("Failed to read YAML file: {}\n{}", file.display(), e))?;
        Ok(serde_yaml::from_str(&yaml)?)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Delays {
    pub init_delay: Option<u64>,
    pub command_delay: Option<u64>,
    pub is_recurring: IsRecurring,
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