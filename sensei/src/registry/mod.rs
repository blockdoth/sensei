//! A high level overview of the registry module.
//! The registry keeps tabs on the status of the hosts in the network.
//! When a new hosts joins, it registers itself with the registry.
//! The registry will then peroidically check the status of the hosts.
//! We are polling from the registry to the hosts.
//! Letting the hosts periodically send heartbeats to the registry would mean that
//! the hosts have to keep track another task, which could overwhelm the lowest compute
//! devices in the network.
use std::collections::HashMap;
use std::convert::From;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Error;
use async_trait::async_trait;
use lib::errors::{AppError, NetworkError};
use lib::network::rpc_message::RpcMessageKind::{Ctrl, Data};
use lib::network::rpc_message::{self, CtrlMsg, DataMsg, DeviceStatus, HostId, RpcMessage, RpcMessageKind};
use lib::network::tcp::client::TcpClient;
use lib::network::tcp::server::TcpServer;
use lib::network::tcp::{ChannelMsg, ConnectionHandler, SubscribeDataChannel, send_message};
use log::*;
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
    hosts: Arc<Mutex<HashMap<HostId, HostInfo>>>,
    send_data_channel: broadcast::Sender<DataMsg>,
}

#[derive(Eq, Hash, PartialEq, Clone, Debug)]
struct DeviceId {
    id: u64,
}

#[derive(Clone)]
struct HostInfo {
    addr: SocketAddr,
    status: RegHostStatus,
    responded_to_last_heardbeat: bool, // A host is allowed to miss one
}

/// This is nearly the same type as in rpc_message.
/// I don't want to match on rpc_message every time, so I transfer the type.
#[derive(Clone)]
struct RegHostStatus {
    host_id: HostId,
    device_status: Vec<DeviceStatus>, // (device_id, status)
}

impl From<CtrlMsg> for RegHostStatus {
    fn from(item: CtrlMsg) -> Self {
        match item {
            CtrlMsg::HostStatus { host_id, device_status } => RegHostStatus { host_id, device_status },
            _ => {
                todo!("Throw propper error here");
            }
        }
    }
}

/// The registry spawns two threads. A TCP server that allows new hosts to register themselves
/// and a background thread that periodically requests the status of the hosts.
impl Run<RegistryConfig> for Registry {
    fn new(config: RegistryConfig) -> Self {
        Registry {
            hosts: Arc::from(Mutex::from(HashMap::new())),
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
                self.register_host(host_id, host_address).await.unwrap();
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
    async fn poll_hosts(&self, mut client: TcpClient, poll_interval: Duration) -> Result<(), Error> {
        let mut interval = interval(poll_interval);
        loop {
            interval.tick().await;
            for (host, addr) in self.list_hosts().await {
                info!("Polling host: {host:#?} at address: {addr}");
                client.connect(addr).await?;
                client.send_message(addr, RpcMessageKind::Ctrl(CtrlMsg::PollHostStatus)).await?;
                let msg = client.read_message(addr).await?;
                info!("msg: {msg:?}");
                self.store_host_update(host, addr, msg.msg);
                client.disconnect(addr).await;
            }
        }
    }

    async fn remove_host(&self, host_id: HostId) -> Result<(), Box<dyn std::error::Error>> {
        info!("Could not reach host: {host_id:?}");
        let mut host_info_table = self.hosts.lock().await;
        let info = host_info_table.get(&host_id).unwrap();
        if !info.responded_to_last_heardbeat {
            let hosts = self.hosts.lock().await.remove(&host_id);
        } else {
            let new_info = HostInfo {
                addr: info.addr,
                status: info.status.clone(),
                responded_to_last_heardbeat: false,
            };
            host_info_table.insert(host_id, new_info);
        }
        Ok(())
    }
    /// List all registered hosts in the registry.
    async fn list_hosts(&self) -> Vec<(HostId, SocketAddr)> {
        self.hosts.lock().await.iter().map(|(id, info)| (*id, info.addr)).collect()
    }
    /// Register a new host with the registry.
    async fn register_host(&self, host_id: HostId, host_address: SocketAddr) -> Result<(), Box<dyn std::error::Error>> {
        // because the host has been registered with priority 0 it will be next in line
        self.hosts.lock().await.insert(
            host_id,
            HostInfo {
                addr: host_address,
                status: RegHostStatus {
                    host_id,
                    device_status: Vec::new(),
                },
                responded_to_last_heardbeat: true,
            },
        );
        info!("Registered host: {host_id:#?}");
        Ok(())
    }
    async fn store_host_update(&self, host_id: HostId, host_address: SocketAddr, status: RpcMessageKind) -> Result<(), AppError> {
        match status {
            Ctrl(CtrlMsg::HostStatus { host_id, device_status }) => {
                info!("{device_status:?}");
                let status = HostInfo {
                    addr: host_address,
                    status: RegHostStatus { host_id, device_status },
                    responded_to_last_heardbeat: true,
                };
                self.hosts.lock().await.insert(host_id, status);
                Ok(())
            }
            _ => Err(AppError::ConfigError("No such host".to_string())), // TODO: proper error handling
        }
    }
}
