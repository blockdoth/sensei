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
use lib::network::rpc_message::{CtrlMsg, DataMsg, DeviceId, DeviceStatus, HostId, RpcMessage, RpcMessageKind};
use lib::network::tcp::client::TcpClient;
use lib::network::tcp::server::TcpServer;
use lib::network::tcp::{ChannelMsg, ConnectionHandler, SubscribeDataChannel};
use log::*;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::sync::watch::{self};
use tokio::sync::{Mutex, broadcast};
use tokio::task;
use tokio::time::{Duration, interval};

use crate::config::RegistryConfig;
use crate::module::Run;

#[derive(Clone)]
pub struct Registry {
    hosts: Arc<Mutex<HashMap<HostId, HostInfo>>>,
    send_data_channel: broadcast::Sender<(DataMsg, DeviceId)>,
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
    device_status: Vec<DeviceStatus>, // (device_id, status)
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

/// The registry spawns two threads: a TCP server for host registration and a separate task for polling hosts.
impl Run<RegistryConfig> for Registry {
    /// Create a new registry with the given configuration.
    fn new(_config: RegistryConfig) -> Self {
        Registry {
            hosts: Arc::from(Mutex::from(HashMap::new())),
            send_data_channel: broadcast::channel(100).0, // magic buffer for now
        }
    }

    /// Run the registry, spawning the TCP server and polling background task.
    async fn run(&self, config: RegistryConfig) -> Result<(), Box<dyn std::error::Error>> {
        let _sender_data_channel = &self.send_data_channel;

        let server_task = {
            let connection_handler = Arc::new(self.clone());
            task::spawn(async move {
                info!("Starting TCP server on {}...", config.addr);
                TcpServer::serve(config.addr, connection_handler).await.unwrap();
            })
        };

        let client_task = {
            let connection_handler = Arc::new(self.clone());
            task::spawn(async move {
                info!("Starting TCP client to poll hosts...");
                let client = TcpClient::new();
                connection_handler
                    .poll_hosts(client, Duration::from_secs(config.poll_interval))
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

#[async_trait]
/// Handles incoming and outgoing network connections for the registry.
impl ConnectionHandler for Registry {
    /// Handle an incoming message from a host or client.
    async fn handle_recv(&self, request: RpcMessage, _send_commands_channel: watch::Sender<ChannelMsg>) -> Result<(), NetworkError> {
        debug!("Received request: {:?}", request);

        match request.msg {
            Ctrl(CtrlMsg::AnnouncePresence { host_id, host_address }) => {
                self.register_host(host_id, host_address).await.unwrap();
                Ok(())
            }
            _ => Err(NetworkError::MessageError),
        }
    }

    /// Handle outgoing messages to a host or client.
    async fn handle_send(
        &self,
        _recv_commands_channel: watch::Receiver<ChannelMsg>,
        _recv_data_channel: tokio::sync::broadcast::Receiver<(DataMsg, HostId)>,
        _send_stream: OwnedWriteHalf,
    ) -> Result<(), NetworkError> {
        Ok(())
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
            for (host, addr) in self.list_hosts().await {
                let res: Result<(), Error> = async {
                    info!("Polling host: {host:#?} at address: {addr}");
                    client.connect(addr).await?;
                    client.send_message(addr, RpcMessageKind::Ctrl(CtrlMsg::PollHostStatus)).await?;
                    let msg = client.read_message(addr).await?;
                    info!("msg: {msg:?}");
                    self.store_host_update(host, addr, msg.msg).await?;
                    client.disconnect(addr).await?;
                    Ok(())
                }
                .await;
                if res.is_err() {
                    // if a host throws errors, handle them here
                    // Might have to be split out into error types later
                    self.handle_unresponsive_host(host).await?;
                }
            }
        }
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
#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use lib::network::rpc_message::SourceType;

    use super::*;

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
        let device_status = vec![DeviceStatus {
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
            .store_host_update(host_id, addr, RpcMessageKind::Ctrl(CtrlMsg::PollHostStatus))
            .await;
        assert!(matches!(result, Err(AppError::NoSuchHost)));
    }
}
