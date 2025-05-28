use async_trait::async_trait;
use lib::errors::NetworkError;
use lib::network::rpc_message::{CtrlMsg, DataMsg, RpcMessageKind, SourceType};
use lib::network::tcp::server::TcpServer;
use lib::network::tcp::{ChannelMsg, ConnectionHandler, SubscribeDataChannel, send_message};
use std::{collections::HashMap, sync::Arc};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::broadcast;
use tokio::sync::watch::{self, Receiver, Sender};

use lib::network::rpc_message::{
    RpcMessage,
    RpcMessageKind::{Ctrl, Data},
};
use log::*;
use std::net::SocketAddr;

use crate::cli::{GlobalConfig, RegistrySubcommandArgs, SubCommandsArgs};
use crate::config::RegistryConfig;
use crate::module::Run;

#[derive(Clone)]
pub struct Registry {
    host_table: HashMap<HostId, HostInfo>,
    device_table: HashMap<DeviceId, DeviceInfo>,
    send_data_channel: broadcast::Sender<DataMsg>,
}

#[derive(Clone, Eq, Hash, PartialEq)]
struct HostId {
    id: u32,
}
#[derive(Eq, Hash, PartialEq, Clone)]
struct DeviceId {
    id: u32,
}

#[derive(Clone)]
struct HostInfo {
    name: String,
    rpc_addr: String,
    state: String,
    last_heartbeat: u32,
}
#[derive(Clone)]
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
            send_data_channel: broadcast::channel(100).0, // magic buffer for now
        }
    }

    async fn run(&self, config: RegistryConfig) -> Result<(), Box<dyn std::error::Error>> {
        let connection_handler = Arc::new(self.clone());
        let sender_data_channel = connection_handler.send_data_channel.clone();
        info!("Starting TCP server on {}...", config.addr);
        TcpServer::serve(config.addr, connection_handler).await;
        Ok(())
    }
}

impl SubscribeDataChannel for Registry {
    fn subscribe_data_channel(&self) -> broadcast::Receiver<DataMsg> {
        self.send_data_channel.subscribe()
    }
}

#[async_trait]
impl ConnectionHandler for Registry {
    async fn handle_recv(
        &self,
        request: RpcMessage,
        send_commands_channel: watch::Sender<ChannelMsg>,
    ) -> Result<(), NetworkError> {
        Ok(())
    }

    async fn handle_send(
        &self,
        mut recv_commands_channel: watch::Receiver<ChannelMsg>,
        mut recv_data_channel: broadcast::Receiver<DataMsg>,
        mut send_stream: OwnedWriteHalf,
    ) -> Result<(), NetworkError> {
        Ok(())
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
