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

use async_trait::async_trait;
use futures::try_join;
use lib::errors::{ConfigError, NetworkError, RegistryError};
use lib::network::rpc_message::{DataMsg, DeviceId, DeviceInfo, HostCtrl, HostId, HostStatus, RegCtrl, Responsiveness, RpcMessage, RpcMessageKind};
use lib::network::tcp::client::TcpClient;
use lib::network::tcp::server::TcpServer;
use lib::network::tcp::{ChannelMsg, ConnectionHandler, HostChannel, RegChannel, SubscribeDataChannel, send_message};
use log::{debug, error, info, trace, warn};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::sync::watch::{self};
use tokio::sync::{Mutex, broadcast};
use tokio::task::{self, JoinHandle};
use tokio::time::{Duration, interval};

use crate::services::{GlobalConfig, RegistryConfig, Run};

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
    /// Host ID
    host_id: HostId,
    /// Server address
    addr: SocketAddr,
    /// The polling rate a registry will use. As indicated in the method field, the integer represents the number of seconds between polls.
    polling_interval: u64,
    /// Map of host IDs to their information.
    hosts: Arc<Mutex<HashMap<HostId, HostStatus>>>,
    /// Broadcast channel for sending data messages to subscribers.
    send_data_channel: broadcast::Sender<(DataMsg, DeviceId)>,
}

/// Allows clients to subscribe to the registry's data channel.
impl SubscribeDataChannel for Registry {
    fn subscribe_data_channel(&self) -> broadcast::Receiver<(DataMsg, DeviceId)> {
        self.send_data_channel.subscribe()
    }
}

impl Run<RegistryConfig> for Registry {
    /// Constructs a new `Registry` from the given global and node-specific configuration.
    fn new(global_config: GlobalConfig, config: RegistryConfig) -> Self {
        trace!("{config:#?}");
        let (send_data_channel, _) = broadcast::channel::<(DataMsg, DeviceId)>(16); // magic buffer
        Registry {
            host_id: config.host_id,
            addr: config.address,
            polling_interval: config.polling_interval,
            hosts: Arc::from(Mutex::from(HashMap::new())),
            send_data_channel,
        }
    }

    /// Starts the system node.
    ///
    /// Initializes a hashmap of device handlers and sinks based on the configuration file on startup
    ///
    /// # Arguments
    ///
    /// RegistryConfig: Specifies the target address
    async fn run(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if self.polling_interval == 0 {
            return Err(Box::new(ConfigError::InvalidConfig("Polling interval can not be 0".to_owned())));
        }

        let polling_interval = self.polling_interval;

        let self_clone = Arc::new(self.clone());
        let polling_task = task::spawn(async move {
            info!("Starting TCP client to poll hosts...");
            self_clone.poll_hosts(polling_interval).await;
        });

        // Create a TCP host server task
        info!("Starting TCP server on {}...", self.addr);
        let connection_handler = Arc::new(self.clone());
        let tcp_server_task: JoinHandle<()> = task::spawn(async move {
            match TcpServer::serve(connection_handler.addr, connection_handler).await {
                Ok(_) => (),
                Err(e) => {
                    panic!("The TCP server encountered an error: {e}")
                }
            };
        });

        try_join!(tcp_server_task, polling_task);

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
    pub async fn poll_hosts(&self, poll_interval: u64) -> Result<(), RegistryError> {
        let mut interval = interval(Duration::from_secs(poll_interval));
        let mut client = TcpClient::new();
        loop {
            interval.tick().await;
            let hosts = self.list_hosts().await;
            let hosts_len = hosts.len();
            for (host_id, target_addr) in hosts {
                let res: Result<(), RegistryError> = async {
                    debug!("Polling host: {host_id:#?} at address: {target_addr}");
                    if !client.is_connected(target_addr).await {
                        client.connect(target_addr).await;
                    }
                    client
                        .send_message(target_addr, RpcMessageKind::RegCtrl(RegCtrl::PollHostStatus { host_id }))
                        .await?;
                    let msg = client.read_message(target_addr).await?;
                    if let RpcMessageKind::RegCtrl(RegCtrl::HostStatus(host_status)) = msg.msg {
                        self.store_host_update(host_id, target_addr, host_status.device_statuses).await?;
                    } else {
                        error!("Received an unexpected response from {host_id:#?} at address: {target_addr}\n{msg:?}");
                        return Err(RegistryError::NetworkError(NetworkError::MessageError));
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
            info!("Polled {hosts_len} hosts");
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
        let mut host_info_table = self.hosts.lock().await;
        let info = host_info_table.get_mut(&host_id).ok_or(RegistryError::NoSuchHost)?;
        match info.responsiveness {
            Responsiveness::Connected => {
                info.responsiveness = Responsiveness::Lossy;
                warn!("Could not reach host: {host_id:?}.");
            }
            Responsiveness::Lossy => {
                info.responsiveness = Responsiveness::Disconnected;
                warn!("Could not reach host: {host_id:?}. Won't log untill a connection can be established again.");
            }
            Responsiveness::Disconnected => {
                trace!("Could not reach host: {host_id:?}");
            }
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
        info!(
            "Added host {host_address:#?} to the registry, {} hosts registered",
            self.hosts.lock().await.len()
        );
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

#[async_trait]
impl ConnectionHandler for Registry {
    /// Handles receiving messages from other senders in the network.
    /// This communicates with the sender function using channel messages.
    ///
    /// # Types
    ///
    /// - Connect/Disconnect
    /// - Subscribe/Unsubscribe
    /// - Configure
    async fn handle_recv(&self, request: RpcMessage, send_channel_msg_channel: watch::Sender<ChannelMsg>) -> Result<(), NetworkError> {
        match request.msg {
            RpcMessageKind::HostCtrl(host_ctrl) => match host_ctrl {
                HostCtrl::Connect => info!("Started connection with {}", request.src_addr),
                HostCtrl::Disconnect => send_channel_msg_channel.send(ChannelMsg::from(HostChannel::Disconnect))?,
                HostCtrl::Ping => {
                    debug!("Received ping from {:#?}.", request.src_addr);
                    send_channel_msg_channel.send(ChannelMsg::from(HostChannel::Pong))?;
                }
                HostCtrl::Pong => info!("Received pong from {}", request.src_addr),
                _ => {
                    warn!("The client received an unsupported request. Responding with an empty message.");
                    send_channel_msg_channel.send(ChannelMsg::from(HostChannel::Empty))?;
                }
            },
            RpcMessageKind::RegCtrl(reg_ctrl) => match reg_ctrl {
                RegCtrl::AnnouncePresence { host_id, host_address } => {
                    self.register_host(host_id, host_address).await.unwrap();
                }
                RegCtrl::PollHostStatus { host_id } => {
                    send_channel_msg_channel.send(ChannelMsg::from(RegChannel::SendHostStatus { host_id }))?;
                }
                RegCtrl::PollHostStatuses => {
                    send_channel_msg_channel.send(ChannelMsg::from(RegChannel::SendHostStatuses))?;
                }
                RegCtrl::HostStatus(HostStatus {
                    host_id,
                    device_statuses: device_status,
                    responsiveness,
                    addr,
                }) => self
                    .store_host_update(host_id, request.src_addr, device_status)
                    .await
                    .map_err(|err| NetworkError::ProcessingError(err.to_string()))?,
                RegCtrl::HostStatuses { host_statuses } => {
                    for host_status in host_statuses {
                        self.store_host_update(host_status.host_id, request.src_addr, host_status.device_statuses)
                            .await
                            .map_err(|err| NetworkError::ProcessingError(err.to_string()))?
                    }
                }
                _ => {
                    warn!("The client received an unsupported request. Responding with an empty message.");
                    send_channel_msg_channel.send(ChannelMsg::from(HostChannel::Empty))?;
                }
            },
            RpcMessageKind::Data { data_msg, device_id } => panic!("A registry can't handle data messages."),
        };
        Ok(())
    }

    /// Handles sending messages for the nodes to other receivers in the network.
    ///
    /// The node will only send messages to subscribers of relevant messages.
    async fn handle_send(
        &self,
        mut recv_command_channel: watch::Receiver<ChannelMsg>,
        mut _recv_data_channel: broadcast::Receiver<(DataMsg, DeviceId)>,
        mut send_stream: OwnedWriteHalf,
    ) -> Result<(), NetworkError> {
        loop {
            recv_command_channel.changed().await?;
            let channel_msg = recv_command_channel.borrow_and_update().clone();
            match channel_msg {
                ChannelMsg::HostChannel(host_channel) => match host_channel {
                    HostChannel::Empty => send_message(&mut send_stream, RpcMessageKind::HostCtrl(HostCtrl::Empty)).await?,
                    HostChannel::Disconnect => {
                        send_message(&mut send_stream, RpcMessageKind::HostCtrl(HostCtrl::Disconnect)).await?;
                        return Err(NetworkError::Closed);
                    }
                    HostChannel::Pong => send_message(&mut send_stream, RpcMessageKind::HostCtrl(HostCtrl::Pong)).await?,
                    _ => panic!("Received an unsupported channel message."),
                },
                ChannelMsg::RegChannel(reg_channel) => match reg_channel {
                    RegChannel::SendHostStatus { host_id } => {
                        let host_status = RegCtrl::from(
                            self.get_host_by_id(host_id)
                                .await
                                .map_err(|err| NetworkError::ProcessingError(err.to_string()))?,
                        );
                        let msg = RpcMessageKind::RegCtrl(host_status);
                        send_message(&mut send_stream, msg).await?;
                    }
                    RegChannel::SendHostStatuses => {
                        let mut host_statuses: Vec<HostStatus> = self.list_host_statuses().await.iter().map(|(_, info)| info.clone()).collect();
                        let msg = RegCtrl::HostStatuses { host_statuses };
                        send_message(&mut send_stream, RpcMessageKind::RegCtrl(msg)).await?;
                    }
                },
                ChannelMsg::Data { data: _ } => panic!("Registry produced a data message?"),
            }
        }
        // Ok(()) is unreachable, but keep for completeness
        #[allow(unreachable_code)]
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::Arc;

    use lib::network::rpc_message::{DeviceInfo, HostId, HostStatus, RegCtrl, Responsiveness, RpcMessageKind, SourceType};
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
            polling_interval: 0,
            host_id: 1,
            addr: "127.0.0.1:6969".parse().unwrap(),
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
