//! # Registry Module
//!
//! The registry keeps track of the status of hosts in the network. When a new host joins, it registers itself with the registry.
//! The registry periodically checks the status of the hosts by polling them. This design avoids requiring hosts to run extra tasks for heartbeats on registrees,
//! which is important for low-compute devices.
//! A registry is always a part of a system node and cannot be instantiated on its own
use std::collections::HashMap;
use std::convert::From;
use std::net::SocketAddr;
use std::sync::Arc;

use lib::errors::{NetworkError, RegistryError};
use lib::network::rpc_message::{DataMsg, DeviceId, DeviceInfo, HostId, HostStatus, RegCtrl, Responsiveness, RpcMessage, RpcMessageKind};
use lib::network::tcp::client::TcpClient;
use lib::network::tcp::{ChannelMsg, RegChannel, SubscribeDataChannel};
use log::{debug, info, warn};
use tokio::sync::watch::{self};
use tokio::sync::{Mutex, broadcast};
use tokio::task;
use tokio::time::{Duration, interval};


static DEFAULT_POLLING_INTERVAL: u64 = 4;

/// The `Registry` struct manages a collection of hosts, providing asynchronous methods to poll their status,
/// register new hosts, remove unresponsive hosts, list all registered hosts, and store updates to host status.
///
/// # Methods
/// - `poll_hosts`: Periodically polls all registered hosts for their status using a TCP client.
/// - `handle_unresponsive_host`: Removes a host from the registry if it did not respond to the last two heartbeats.
/// - `list_hosts`: Returns a list of all registered hosts and their socket addresses.
/// - `register_host`: Registers a new host with the registry.
/// - `store_host_update`: Stores an update to a host's status in the registry.
#[derive(Clone)]
pub struct Registry {
    /// Map of host IDs to their information.
    hosts: Arc<Mutex<HashMap<HostId, HostStatus>>>,
    /// Broadcast channel for sending data messages to subscribers.
    send_data_channel: broadcast::Sender<(DataMsg, DeviceId)>,
    /// The polling rate a registry will use. As indicated in the method field, the integer represents the number of seconds between polls.
    polling_rate_s: Option<u64>,
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
/// - `create_polling_task`: Creates a task that polls the registered hosts.
impl Registry {
    /// Create a new, empty registry.
    pub fn new(polling_rate_s: Option<u64>) -> Self {
        Registry {
            hosts: Arc::from(Mutex::from(HashMap::new())),
            send_data_channel: broadcast::channel(100).0, // magic buffer for now
            polling_rate_s,
        }
    }

    /// Handle a registration control message from a host or orchestrator.
    pub async fn handle_reg_ctrl(
        &self,
        request: RpcMessage,
        message: RegCtrl,
        send_commands_channel: watch::Sender<ChannelMsg>,
    ) -> Result<(), NetworkError> {
        match message {
            RegCtrl::AnnouncePresence { host_id, host_address } => {
                self.register_host(host_id, host_address).await.unwrap();
            }
            RegCtrl::PollHostStatus { host_id } => {
                send_commands_channel.send(ChannelMsg::from(RegChannel::SendHostStatus { host_id }))?;
            }
            RegCtrl::PollHostStatuses => {
                send_commands_channel.send(ChannelMsg::from(RegChannel::SendHostStatuses))?;
            }
            RegCtrl::HostStatus(HostStatus {
                addr,
                host_id,
                device_statuses: device_status,
                responsiveness,
            }) => self.store_host_update(host_id, addr, device_status).await?,
            RegCtrl::HostStatuses { host_statuses } => {
                for host_status in host_statuses {
                    self.store_host_update(host_status.host_id, request.src_addr, host_status.device_statuses)
                        .await?
                }
            }
            _ => {}
        };
        Ok(())
    }

    /// Spawns a new asynchronous task that periodically polls all registered hosts for their status iff theres an interval > 0
    /// set in the registry struct.
    ///
    /// This function creates a background task using Tokio's task spawning mechanism. The task will
    /// instantiate a `TcpClient` and repeatedly invoke [`poll_hosts`] at the specified interval (in seconds),
    /// polling each registered host for its current status. Any errors encountered during polling are
    /// unwrapped and will cause the task to panic.
    ///
    /// # Arguments
    ///
    /// # Returns
    ///
    /// Returns a [`tokio::task::JoinHandle`] to the spawned task, which can be used to await or manage the task.
    ///
    /// # Example
    ///
    /// ```rust
    /// let registry = Registry::new();
    /// let handle = registry.create_polling_client_task(10);
    /// // The polling task is now running in the background.
    /// ```
    pub fn create_polling_task(&self) -> tokio::task::JoinHandle<()> {
        if let Some(interval) = self.polling_rate_s {
            if interval > 0 {
                let connection_handler = Arc::new(self.clone());
                task::spawn(async move {
                    info!("Starting TCP client to poll hosts...");
                    let client = TcpClient::new();
                    connection_handler.poll_hosts(client, Duration::from_secs(interval)).await.unwrap();
                })
            } else {
                info!("No registry polling inteval was defined. Pollin task was not started");
                task::spawn(async {}) // return an empty task if interval is not > 0
            }
        } else {
            info!("No registry polling inteval was defined. Pollin task was not started");
            task::spawn(async {}) // return an empty task if no interval is defined
        }
    }
    /// Go though the list of hosts and poll their status
    pub async fn poll_hosts(&self, mut client: TcpClient, poll_interval: Duration) -> Result<(), RegistryError> {
        let mut interval = interval(poll_interval);

        // let connections:Vec<TcpClient> = vec![];
        loop {
            interval.tick().await;
            for (host_id, target_addr) in self.list_hosts().await {
                let res: Result<(), RegistryError> = async {
                    debug!("Polling host: {host_id:#?} at address: {target_addr}");
                    if !client.is_connected(target_addr).await {
                        client.connect(target_addr).await;
                    }
                    client
                        .send_message(target_addr, RpcMessageKind::RegCtrl(RegCtrl::PollHostStatus { host_id }))
                        .await
                        .map_err(|e| RegistryError::from(Box::new(e)))?;
                    let msg = client.read_message(target_addr).await.map_err(|e| RegistryError::from(Box::new(e)))?;
                    debug!("msg: {msg:?}");
                    if let RpcMessageKind::RegCtrl(RegCtrl::HostStatus(host_status)) = msg.msg {
                        self.store_host_update(host_id, target_addr, host_status.device_statuses).await?;
                    } else {
                        return Err(RegistryError::NetworkError(Box::from(NetworkError::MessageError)));
                    }
                    // client.disconnect(target_addr).await.map_err(|e| RegistryError::from(Box::new(e)))?;
                    Ok(())
                }
                .await;
                if res.is_err() {
                    // if a host throws errors, handle them here
                    // Might have to be split out into error types later
                    self.handle_unresponsive_host(host_id).await?;
                }
            }
            debug!("Current registry state:\n{:#?}", self.hosts.lock().await);
        }
    }

    /// Retrieve a host from the table by its HostId, or throw an AppError::NoSuchHost
    pub async fn get_host_by_id(&self, host_id: HostId) -> Result<HostStatus, RegistryError> {
        let host_info_table = self.hosts.lock().await;
        let host_info = host_info_table.get(&host_id).ok_or(RegistryError::NoSuchHost)?;
        Ok(host_info.clone())
    }

    /// Updates hosts responsiveness in the registry.
    pub async fn handle_unresponsive_host(&self, host_id: HostId) -> Result<(), RegistryError> {
        warn!("Could not reach host: {host_id:?}");
        let mut host_info_table = self.hosts.lock().await;
        let info = host_info_table.get_mut(&host_id).ok_or(RegistryError::NoSuchHost)?;
        match info.responsiveness {
            Responsiveness::Connected => info.responsiveness = Responsiveness::Lossy,
            Responsiveness::Lossy => info.responsiveness = Responsiveness::Disconnected,
            Responsiveness::Disconnected => (),
        }
        Ok(())
    }
    /// List all registered hosts in the registry.
    pub async fn list_hosts(&self) -> Vec<(HostId, SocketAddr)> {
        self.hosts.lock().await.iter().map(|(id, info)| (*id, info.addr)).collect()
    }
    /// List the status of every host in the registry.
    pub async fn list_host_statuses(&self) -> Vec<(HostId, HostStatus)> {
        self.hosts.lock().await.iter().map(|(id, info)| (*id, info.clone())).collect()
    }
    /// List the host info of every host in the registry.
    pub async fn list_host_info(&self) -> Vec<HostStatus> {
        self.hosts.lock().await.iter().map(|h| h.1.clone()).collect()
    }

    /// Register a new host with the registry.
    pub async fn register_host(&self, host_id: HostId, host_address: SocketAddr) -> Result<(), RegistryError> {
        // because the host has been registered with priority 0 it will be next in line
        self.hosts.lock().await.insert(
            host_id,
            HostStatus {
                addr: host_address,
                host_id,
                device_statuses: Vec::new(),
                responsiveness: Responsiveness::Connected,
            },
        );
        info!("Registered host: {host_id:#?}");
        Ok(())
    }
    /// Store an update to a host's status in the registry.
    pub async fn store_host_update(&self, host_id: HostId, host_address: SocketAddr, host_status: Vec<DeviceInfo>) -> Result<(), RegistryError> {
        debug!("{host_status:?}");
        let status = HostStatus {
            addr: host_address,
            host_id,
            device_statuses: host_status,
            responsiveness: Responsiveness::Connected,
        };
        self.hosts.lock().await.insert(host_id, status);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::Arc;

    use lib::network::rpc_message::{DeviceInfo, HostId, HostStatus, RegCtrl, RpcMessageKind, SourceType};
    use tokio::sync::{Mutex, broadcast};

    use super::Registry;
    use crate::registry::Responsiveness;

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
            polling_rate_s: None,
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
    async fn test_handle_unresponsive_host() {
        let registry = make_registry();
        let host_id = test_host_id(2);
        let addr = test_socket_addr(2345);

        registry.register_host(host_id, addr).await.unwrap();
        // Mark as not responded
        registry.handle_unresponsive_host(host_id).await.unwrap();
        assert!(!registry.list_hosts().await.is_empty());
        // Now update it if it happens again
        registry.handle_unresponsive_host(host_id).await.unwrap();
        let hosts_map = registry.hosts.lock().await;
        assert_eq!(hosts_map.get(&host_id).unwrap().responsiveness, Responsiveness::Disconnected);
    }

    #[tokio::test]
    async fn test_handle_responsive_host() {
        let registry = make_registry();
        let host_id = test_host_id(3);
        let addr = test_socket_addr(3456);

        registry.register_host(host_id, addr).await.unwrap();
        // Mark as responded
        {
            let mut hosts = registry.hosts.lock().await;
            if let Some(info) = hosts.get_mut(&host_id) {
                info.responsiveness = Responsiveness::Connected;
            }
        }
        registry.handle_unresponsive_host(host_id).await.unwrap();
        let hosts = registry.list_hosts().await;
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0], (host_id, addr));
        // Should now be marked as not responded
        let hosts_map = registry.hosts.lock().await;
        assert_eq!(hosts_map.get(&host_id).unwrap().responsiveness, Responsiveness::Lossy);
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

        let result = registry.store_host_update(host_id, addr, device_status.clone()).await;
        assert!(result.is_ok());
        let hosts = registry.hosts.lock().await;
        let info = hosts.get(&host_id).unwrap();
        assert_eq!(info.addr, addr);
        assert_eq!(info.host_id, host_id);
        assert_eq!(info.device_statuses, device_status);
        assert_eq!(info.responsiveness, Responsiveness::Connected);
    }

    #[tokio::test]
    async fn test_store_host_update_invalid() {
        let registry = make_registry();
        let host_id = test_host_id(5);
        let addr = test_socket_addr(5678);

        let result = registry.store_host_update(host_id, addr, Vec::new()).await;
        assert!(result.is_ok());
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
        let msg_kind = RpcMessageKind::RegCtrl(RegCtrl::HostStatus(HostStatus {
            addr,
            host_id,
            device_statuses: device_status.clone(),
            responsiveness: Responsiveness::Connected,
        }));
        // Extract device_status from msg_kind and pass it to store_host_update
        registry.store_host_update(host_id, addr, device_status.clone()).await.unwrap();

        // Simulate registry receiving PollHostStatus and responding
        let status = registry.get_host_by_id(host_id).await.unwrap();
        assert_eq!(status.host_id, host_id);
        assert_eq!(status.device_statuses, device_status);

        // Simulate listing all hosts
        let hosts = registry.list_hosts().await;
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0], (host_id, addr));
    }
}
