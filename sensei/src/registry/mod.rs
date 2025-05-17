use std::{collections::HashMap, sync::Arc};

use crate::{
    cli::{GlobalConfig, RegistrySubcommandArgs, SubCommandsArgs},
    config::RegistryConfig,
    module::*,
};
use anyhow::Ok;
use lib::network::rpc_message::RpcMessage;
use log::*;
use std::net::SocketAddr;

pub struct Registry {
    host_table: HashMap<HostId, HostInfo>,
    device_table: HashMap<DeviceId, DeviceInfo>,
}

#[derive(Clone, Eq, Hash, PartialEq)]
struct HostId {
    id: u32,
}
#[derive(Eq, Hash, PartialEq)]
struct DeviceId {
    id: u32,
}

struct HostInfo {
    name: String,
    rpc_addr: String,
    state: String,
    last_heartbeat: u32,
}
struct DeviceInfo {
    host_id: HostId,
    kind: String,
    state: String,
    current_cfg: String,
}

impl Run<RegistryConfig> for Registry {
    fn new(config: RegistryConfig) -> Self {
        Registry {
            host_table: HashMap::new(),
            device_table: HashMap::new(),
        }
    }

    async fn run(&self, config: RegistryConfig) -> Result<(), Box<dyn std::error::Error>> {
        // info!("Starting registry on address {}", config.targets);
        loop {
            println!("Balls");
        }
    }
}

impl Registry {
    fn list_hosts(&self) -> anyhow::Result<Vec<HostId>> {
        let hosts: Vec<HostId> = self.host_table.keys().cloned().collect();
        Ok(hosts)
    }

    fn register_device(
        &mut self,
        device_id: DeviceId,
        device_info: DeviceInfo,
    ) -> anyhow::Result<()> {
        self.device_table.insert(device_id, device_info);
        Ok(())
    }

    fn register_host(&mut self, host_id: HostId, host_info: HostInfo) -> anyhow::Result<()> {
        self.host_table.insert(host_id, host_info);
        Ok(())
    }

    fn handle_message(&self, msg: RpcMessage) -> anyhow::Result<()> {
        Ok(())
    }

    async fn check_for_heartbeat(&self, msg: RpcMessage, host: HostId) -> anyhow::Result<()> {
        Ok(())
    }
}
