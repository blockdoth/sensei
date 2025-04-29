use std::collections::HashMap;

use common::RpcEnvelope;

#[derive(Clone, Eq, Hash, PartialEq)]
struct HostId {id: u32}
#[derive(Eq, Hash, PartialEq)]
struct DeviceId {id: u32}

struct HostInfo {name: String, rpc_addr: String, state: String, last_heartbeat: u32}
struct DeviceInfo { host_id: HostId, kind: String, state: String, current_cfg: String}

struct Registry {
    host_table: HashMap<HostId, HostInfo>,
    device_table: HashMap<DeviceId, DeviceInfo>,
}
impl Registry {
    fn new() -> Self {
        Registry { host_table: HashMap::new(), device_table: HashMap::new() }
    }

    fn list_hosts(&self) -> anyhow::Result<Vec<HostId>> {
        let hosts: Vec<HostId> = self.host_table.keys().cloned().collect();
        Ok(hosts)
    }

    fn register_device(&mut self, device_id: DeviceId, device_info: DeviceInfo) -> anyhow::Result<()> {
        self.device_table.insert(device_id, device_info);
        Ok(())
    }

    fn register_host(&mut self, host_id: HostId, host_info: HostInfo) -> anyhow::Result<()> {
        self.host_table.insert(host_id, host_info);
        Ok(())
    }

    fn handle_message(&self, msg: RpcEnvelope) ->  anyhow::Result<()> {
        Ok(())
    }

    async fn check_for_heartbeat(&self, msg: RpcEnvelope, host: HostId) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {
    println!("Hello, world!");
}
