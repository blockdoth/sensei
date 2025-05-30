//! A high level overview of the registry module.
//! The registry keeps tabs on the status of the hosts in the network.
//! When a new hosts joins, it registers itself with the registry.
//! The registry will then peroidically check the status of the hosts.
//! We are polling from the registry to the hosts.
//! Letting the hosts periodically send heartbeats to the registry would mean that
//! the hosts have to keep track another task, which could overwhelm the lowest compute
//! devices in the network.
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Error;
use async_trait::async_trait;
use lib::errors::{AppError, NetworkError};
use lib::network::rpc_message::RpcMessageKind::{Ctrl, Data};
use lib::network::rpc_message::{CtrlMsg, DataMsg, RpcMessage, RpcMessageKind};
use lib::network::tcp::client::TcpClient;
use lib::network::tcp::server::TcpServer;
use lib::network::tcp::{ChannelMsg, ConnectionHandler, SubscribeDataChannel, send_message};
use log::*;
use priority_queue::PriorityQueue;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::watch::{self, Receiver, Sender};
use tokio::sync::{Mutex, broadcast};
use tokio::task;
use tokio::time::{Duration, interval};

use crate::cli::{GlobalConfig, RegistrySubcommandArgs, SubCommandsArgs};
use crate::config::RegistryConfig;
use crate::module::Run;

#[derive(Clone)]
pub struct Registry {
    hosts: Arc<Mutex<PriorityQueue<(HostId, SocketAddr), u32>>>,
    host_table: Arc<Mutex<HashMap<HostId, CtrlMsg>>>,
    send_data_channel: broadcast::Sender<DataMsg>,
}

#[derive(Clone, Eq, Hash, PartialEq, Debug)]
struct HostId {
    id: u64,
}
#[derive(Eq, Hash, PartialEq, Clone, Debug)]
struct DeviceId {
    id: u64,
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

/// The registry spawns two threads. A TCP server that allows new hosts to register themselves
/// and a background thread that periodically requests the status of the hosts.
impl Run<RegistryConfig> for Registry {
    fn new(config: RegistryConfig) -> Self {
        Registry {
            hosts: Arc::from(Mutex::from(PriorityQueue::new())),
            host_table: Arc::from(Mutex::from(HashMap::new())),
            send_data_channel: broadcast::channel(100).0, // magic buffer for now
        }
    }

    async fn run(&self, config: RegistryConfig) -> Result<(), Box<dyn std::error::Error>> {
        let sender_data_channel = &self.send_data_channel;

        let server_task = {
            let connection_handler = Arc::new(self.clone());
            task::spawn(async move {
                info!("Starting TCP server on {}...", config.addr);
                TcpServer::serve(config.addr, connection_handler).await;
            })
        };

        let client_task = {
            let connection_handler = Arc::new(self.clone());
            task::spawn(async move {
                info!("Starting TCP client to poll hosts...");
                let mut client = TcpClient::new();
                connection_handler.poll_hosts(client, Duration::from_secs(config.poll_interval)).await;
            })
        };

        let _ = tokio::try_join!(server_task, client_task);
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
    async fn handle_recv(&self, request: RpcMessage, send_commands_channel: watch::Sender<ChannelMsg>) -> Result<(), NetworkError> {
        debug!("Received request: {:?}", request);

        match request.msg {
            Ctrl(CtrlMsg::Heartbeat { host_id, host_address }) => {
                self.register_host(HostId { id: host_id }, host_address).await.unwrap();
                Ok(())
            }
            _ => Ok(()),
        }
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
    /// Go though the list of hosts and poll their status
    async fn poll_hosts(&self, mut client: TcpClient, poll_interval: Duration) {
        let mut interval = interval(poll_interval);
        loop {
            interval.tick().await;
            for (host, addr) in self.list_hosts().await {
                info!("Polling host: {host:#?} at address: {addr}");
                client.connect(addr).await;
                client.send_message(addr, RpcMessageKind::Ctrl(CtrlMsg::PollHostStatus)).await;
                let msg = client.read_message(addr).await;
                info!("msg: {msg:?}");
                client.disconnect(addr).await;
            }
        }
    }

    fn remove_host(&self, host_id: HostId) -> Result<(), Box<dyn std::error::Error>> {
        info!("Could not reach host: {host_id:?}");
        Ok(())
    }
    /// List all registered hosts in the registry.
    async fn list_hosts(&self) -> Vec<(HostId, SocketAddr)> {
        self.hosts.lock().await.iter().map(|(host, _)| host.clone()).collect()
    }
    /// Register a new host with the registry.
    async fn register_host(&self, host_id: HostId, host_address: SocketAddr) -> Result<(), Box<dyn std::error::Error>> {
        // because the host has been registered with priority 0 it will be next in line
        self.hosts.lock().await.push((host_id.clone(), host_address), 0);
        info!("Registered host: {host_id:#?}");
        Ok(())
    }
    async fn store_host_update(&self, host_id: HostId, host_address: SocketAddr, status: CtrlMsg) -> Result<(), AppError> {
        match status {
            CtrlMsg::HostStatus { host_id, device_status } => {
                // actually store host at some point
                info!("{device_status:?}");
                Ok(())
            }
            _ => Err(AppError::ConfigError("No such host".to_string())), // TODO: proper error handling
        }
    }
}
