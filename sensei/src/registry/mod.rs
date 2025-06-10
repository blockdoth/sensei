//! # Registry Module
//!
//! The registry keeps track of the status of hosts in the network. When a new host joins, it registers itself with the registry.
//! The registry periodically checks the status of the hosts by polling them. This design avoids requiring hosts to run extra tasks for heartbeats,
//! which is important for low-compute devices. The registry spawns two threads: a TCP server for host registration and a background thread for polling hosts.
use std::collections::HashMap;
use std::convert::From;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Error;
use async_trait::async_trait;
use lib::errors::{AppError, NetworkError};
use lib::network::rpc_message::RpcMessageKind::Ctrl;
use lib::network::rpc_message::{CtrlMsg, DataMsg, DeviceId, DeviceInfo, HostId, RpcMessage, RpcMessageKind};
use lib::network::tcp::client::TcpClient;
use lib::network::tcp::server::TcpServer;
use lib::network::tcp::{ChannelMsg, ConnectionHandler, SubscribeDataChannel, send_message};
use log::*;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::sync::watch::{self};
use tokio::sync::{Mutex, broadcast};
use tokio::task;
use tokio::time::{Duration, interval};

use crate::services::{GlobalConfig, RegistryConfig, Run};

#[derive(Clone)]
pub struct Registry {
    hosts: Arc<Mutex<HashMap<HostId, HostInfo>>>,
    send_data_channel: broadcast::Sender<(DataMsg, DeviceId)>,
    addr: SocketAddr,
    poll_interval: u64,
}

/// Information about a registered host.
#[derive(Clone)]
struct HostInfo {
    addr: SocketAddr,
    status: RegHostStatus,
    responded_to_last_heardbeat: bool, // A host is allowed to miss one
}

/// Registry's internal representation of a host's status.
/// This is similar to the type in rpc_message, but avoids matching on rpc_message every time.
#[derive(Clone)]
struct RegHostStatus {
    host_id: HostId,
    device_status: Vec<DeviceInfo>, // (device_id, status)
}

/// Conversion from a control message to a registry host status.
impl From<CtrlMsg> for RegHostStatus {
    fn from(item: CtrlMsg) -> Self {
        match item {
            CtrlMsg::HostStatus { host_id, device_status } => RegHostStatus { host_id, device_status },
            _ => {
                panic!("Could not convert from this type of CtrlMsg: {item:?}");
            }
        }
    }
}
/// Conversion from an internal RegistryHostStatus type to a CtrlMsg
impl From<RegHostStatus> for CtrlMsg {
    fn from(value: RegHostStatus) -> Self {
        CtrlMsg::HostStatus {
            host_id: value.host_id,
            device_status: value.device_status,
        }
    }
}

/// The registry spawns two threads: a TCP server for host registration and a separate task for polling hosts.
impl Run<RegistryConfig> for Registry {
    /// Create a new registry with the given configuration.
    fn new(global_config: GlobalConfig, config: RegistryConfig) -> Self {
        Registry {
            hosts: Arc::from(Mutex::from(HashMap::new())),
            send_data_channel: broadcast::channel(100).0, // magic buffer for now
            addr: config.addr,
            poll_interval: config.poll_interval,
        }
    }

    /// Run the registry, spawning the TCP server and polling background task.
    async fn run(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let sender_data_channel = &self.send_data_channel;

        let addr: SocketAddr = self.addr;
        let server_task = {
            let connection_handler = Arc::new(self.clone());
            task::spawn(async move {
                info!("Starting TCP server on {addr}...");
                TcpServer::serve(addr, connection_handler).await;
            })
        };

        let client_task = {
            let connection_handler = Arc::new(self.clone());
            task::spawn(async move {
                info!("Starting TCP client to poll hosts...");
                let client = TcpClient::new();
                connection_handler
                    .poll_hosts(client, Duration::from_secs(connection_handler.poll_interval))
                    .await
                    .unwrap();
            })
        };

        let _ = tokio::try_join!(server_task, client_task);
        Ok(())
    }
}

/// Allows clients to subscribe to the registry's data channel.
impl SubscribeDataChannel for Registry {
    fn subscribe_data_channel(&self) -> broadcast::Receiver<(DataMsg, DeviceId)> {
        self.send_data_channel.subscribe()
    }
}

/// The `Registry` struct manages a collection of hosts, providing asynchronous methods to poll their status,
/// register new hosts, remove unresponsive hosts, list all registered hosts, and store updates to host status.
///
/// # Methods
/// - `poll_hosts`: Periodically polls all registered hosts for their status using a TCP client.
/// - `handle_unresponsive_host`: Removes a host from the registry if it did not respond to the last two heartbeats.
/// - `list_hosts`: Returns a list of all registered hosts and their socket addresses.
/// - `register_host`: Registers a new host with the registry.
/// - `store_host_update`: Stores an update to a host's status in the registry.
impl Registry {
    /// Go though the list of hosts and poll their status
    async fn poll_hosts(&self, mut client: TcpClient, poll_interval: Duration) -> Result<(), Error> {
        let mut interval = interval(poll_interval);
        loop {
            interval.tick().await;
            for (host_id, target_addr) in self.list_hosts().await {
                let res: Result<(), Error> = async {
                    info!("Polling host: {host_id:#?} at address: {target_addr}");
                    client.connect(target_addr).await?;
                    client
                        .send_message(target_addr, RpcMessageKind::Ctrl(CtrlMsg::PollHostStatus { host_id }))
                        .await?;
                    let msg = client.read_message(target_addr).await?;
                    self.store_host_update(host_id, target_addr, msg.msg).await?;
                    client.disconnect(target_addr).await?;
                    Ok(())
                }
                .await;
                if res.is_err() {
                    // if a host throws errors, handle them here
                    // Might have to be split out into error types later
                    self.handle_unresponsive_host(host_id).await?;
                }
            }
        }
    }

    /// Retrieve a host from the table by its HostId, or throw an AppError::NoSuchHost
    async fn get_host_by_id(&self, host_id: HostId) -> Result<RegHostStatus, AppError> {
        let host_info_table = self.hosts.lock().await;
        let host_info = host_info_table.get(&host_id).ok_or(AppError::NoSuchHost)?;
        Ok(host_info.status.clone())
    }

    /// Remove a host from the registry if it did not respond to the last two heartbeats.
    async fn handle_unresponsive_host(&self, host_id: HostId) -> Result<(), Error> {
        info!("Could not reach host: {host_id:?}");
        let mut host_info_table = self.hosts.lock().await;
        if let Some(info) = host_info_table.get_mut(&host_id) {
            if !info.responded_to_last_heardbeat {
                host_info_table.remove(&host_id);
            } else {
                info.responded_to_last_heardbeat = false;
            }
        } else {
            info!("Could not remove host: {host_id} does not exit.")
        }
        Ok(())
    }
    /// List all registered hosts in the registry.
    async fn list_hosts(&self) -> Vec<(HostId, SocketAddr)> {
        self.hosts.lock().await.iter().map(|(id, info)| (*id, info.addr)).collect()
    }
    /// List the status of every host in teh registry
    async fn list_host_statuses(&self) -> Vec<(HostId, RegHostStatus)> {
        self.hosts.lock().await.iter().map(|(id, info)| (*id, info.status.clone())).collect()
    }
    /// Register a new host with the registry.
    async fn register_host(&self, host_id: HostId, host_address: SocketAddr) -> Result<(), Error> {
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
    /// Store an update to a host's status in the registry.
    async fn store_host_update(&self, _host_id: HostId, host_address: SocketAddr, status: RpcMessageKind) -> Result<(), AppError> {
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
            _ => Err(AppError::NoSuchHost),
        }
    }
}

#[async_trait]
/// Handles incoming and outgoing network connections for the registry.
impl ConnectionHandler for Registry {
    /// Handle an incoming message from a host or client.
    async fn handle_recv(&self, request: RpcMessage, send_commands_channel: watch::Sender<ChannelMsg>) -> Result<(), NetworkError> {
        debug!("Received request: {:?}", request);

        match request.msg {
            Ctrl(CtrlMsg::AnnouncePresence { host_id, host_address }) => {
                self.register_host(host_id, host_address).await.unwrap();
                Ok(())
            }
            Ctrl(CtrlMsg::PollHostStatus { host_id }) => {
                let reg_addr = request.src_addr;
                info!("Received PollDevices from {reg_addr}");
                send_commands_channel.send(ChannelMsg::SendHostStatus { reg_addr, host_id })?;
                Ok(())
            }
            Ctrl(CtrlMsg::PollHostStatuses) => {
                send_commands_channel.send(ChannelMsg::SendHostStatuses)?;
                Ok(())
            }
            _ => Err(NetworkError::MessageError),
        }
    }

    /// Handle outgoing messages to a host or client.
    async fn handle_send(
        &self,
        mut recv_command_channel: watch::Receiver<ChannelMsg>,
        mut recv_data_channel: broadcast::Receiver<(DataMsg, DeviceId)>,
        mut send_stream: OwnedWriteHalf,
    ) -> Result<(), NetworkError> {
        loop {
            if recv_command_channel.has_changed().unwrap_or(false) {
                let msg_opt = recv_command_channel.borrow_and_update().clone();
                debug!("Received message {msg_opt:?} over channel");
                match msg_opt {
                    ChannelMsg::SendHostStatus { reg_addr: _, host_id } => {
                        let host_status = self.get_host_by_id(host_id).await?;
                        let msg = Ctrl(CtrlMsg::from(host_status));
                        send_message(&mut send_stream, msg).await?;
                    }
                    ChannelMsg::SendHostStatuses => {
                        let msg = Ctrl(CtrlMsg::HostStatuses {
                            host_statuses: self
                                .list_host_statuses()
                                .await
                                .iter()
                                .map(|(id, info)| CtrlMsg::from(info.clone()))
                                .collect(),
                        });
                        send_message(&mut send_stream, msg).await?;
                    }
                    _ => (), // Ignore the message
                }
            }
        }
        // Loop is infinite unless broken by Disconnect or error
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::Arc;

    use lib::errors::AppError;
    use lib::network::rpc_message::{CtrlMsg, DeviceInfo, HostId, RpcMessageKind, SourceType};
    use tokio::sync::{Mutex, broadcast};

    use super::Registry;

    fn test_host_id(n: u64) -> HostId {
        // placeholder in case the IDs get more complex
        n
    }

    fn test_socket_addr(port: u16) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
    }

    fn make_registry() -> Registry {
        Registry {
            hosts: Arc::new(Mutex::new(HashMap::new())),
            send_data_channel: broadcast::channel(10).0,
            addr: test_socket_addr(1234),
            poll_interval: 0,
        }
    }

    #[tokio::test]
    async fn test_register_and_list_hosts() {
        let registry = make_registry();
        let host_id = test_host_id(1);
        let addr = test_socket_addr(1234);

        registry.register_host(host_id, addr).await.unwrap();
        let hosts = registry.list_hosts().await;
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0], (host_id, addr));
    }

    #[tokio::test]
    async fn test_remove_host_unresponsive() {
        let registry = make_registry();
        let host_id = test_host_id(2);
        let addr = test_socket_addr(2345);

        registry.register_host(host_id, addr).await.unwrap();
        // Mark as not responded
        registry.handle_unresponsive_host(host_id).await.unwrap();
        assert!(!registry.list_hosts().await.is_empty());
        // Now remove it if it happens again
        registry.handle_unresponsive_host(host_id).await.unwrap();
        assert!(registry.list_hosts().await.is_empty());
    }

    #[tokio::test]
    async fn test_remove_host_responsive() {
        let registry = make_registry();
        let host_id = test_host_id(3);
        let addr = test_socket_addr(3456);

        registry.register_host(host_id, addr).await.unwrap();
        // Mark as responded
        {
            let mut hosts = registry.hosts.lock().await;
            if let Some(info) = hosts.get_mut(&host_id) {
                info.responded_to_last_heardbeat = true;
            }
        }
        registry.handle_unresponsive_host(host_id).await.unwrap();
        let hosts = registry.list_hosts().await;
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0], (host_id, addr));
        // Should now be marked as not responded
        let hosts_map = registry.hosts.lock().await;
        assert!(!hosts_map.get(&host_id).unwrap().responded_to_last_heardbeat);
    }

    #[tokio::test]
    async fn test_store_host_update_success() {
        let registry = make_registry();
        let host_id = test_host_id(4);
        let addr = test_socket_addr(4567);
        let device_status = vec![DeviceInfo {
            id: 1,
            dev_type: SourceType::ESP32,
        }];
        let msg_kind = RpcMessageKind::Ctrl(CtrlMsg::HostStatus {
            host_id,
            device_status: device_status.clone(),
        });

        let result = registry.store_host_update(host_id, addr, msg_kind).await;
        assert!(result.is_ok());
        let hosts = registry.hosts.lock().await;
        let info = hosts.get(&host_id).unwrap();
        assert_eq!(info.addr, addr);
        assert_eq!(info.status.host_id, host_id);
        assert_eq!(info.status.device_status, device_status);
        assert!(info.responded_to_last_heardbeat);
    }

    #[tokio::test]
    async fn test_store_host_update_invalid() {
        let registry = make_registry();
        let host_id = test_host_id(5);
        let addr = test_socket_addr(5678);

        let result = registry
            .store_host_update(host_id, addr, RpcMessageKind::Ctrl(CtrlMsg::PollHostStatus { host_id }))
            .await;
        assert!(matches!(result, Err(AppError::NoSuchHost)));
    }

    #[tokio::test]
    async fn end_to_end_test() {
        // Setup registry
        let registry = make_registry();
        let host_id = test_host_id(10);
        let addr = test_socket_addr(10101);

        // Simulate host announcing presence
        registry.register_host(host_id, addr).await.unwrap();

        // Simulate host sending status update
        let device_status = vec![DeviceInfo {
            id: 42,
            dev_type: SourceType::ESP32,
        }];
        let msg_kind = RpcMessageKind::Ctrl(CtrlMsg::HostStatus {
            host_id,
            device_status: device_status.clone(),
        });
        registry.store_host_update(host_id, addr, msg_kind).await.unwrap();

        // Simulate registry receiving PollHostStatus and responding
        let status = registry.get_host_by_id(host_id).await.unwrap();
        assert_eq!(status.host_id, host_id);
        assert_eq!(status.device_status, device_status);

        // Simulate listing all hosts
        let hosts = registry.list_hosts().await;
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0], (host_id, addr));
    }
}
