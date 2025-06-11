//! # System Node Module
//!
//! The System Node is a core component in the Sensei network. It acts as both a sender and receiver of CSI data, hosting devices that generate or consume this data.
//! System Nodes are responsible for forwarding CSI data to other receivers, adapting data to a universal standard, and managing device handlers.
//!
//! Devices can subscribe to CSI data streams by sending a `Subscribe` message with a device ID. The System Node manages these subscriptions and ensures data is routed appropriately.
//!
//! The System Node also interacts with registries to announce its presence and can periodically poll other nodes when functioning as a registry.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use lib::FromConfig;
use lib::errors::NetworkError;
use lib::handler::device_handler::{DeviceHandler, DeviceHandlerConfig};
use lib::network::rpc_message::CfgType::{Create, Delete, Edit};
use lib::network::rpc_message::SourceType::*;
use lib::network::rpc_message::{DataMsg, DeviceId, DeviceStatus, HostCtrl, HostId, HostStatus, RegCtrl, Responsiveness, RpcMessage, RpcMessageKind};
use lib::network::tcp::client::TcpClient;
use lib::network::tcp::server::TcpServer;
use lib::network::tcp::{ChannelMsg, ConnectionHandler, HostChannel, RegChannel, SubscribeDataChannel, send_message};
use lib::sources::DataSourceConfig;
use lib::sources::tcp::TCPConfig;
use log::*;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::sync::{Mutex, broadcast, watch};
use tokio::task;

use crate::registry::Registry;
use crate::services::{GlobalConfig, Run, SystemNodeConfig};

/// The System Node is a sender and a receiver in the network of Sensei.
/// It hosts the devices that send and receive CSI data, and is responsible for sending this data further to other receivers in the system.
///
/// The System Node can also adapt this CSI data to our universal standard, which lets it be used by other parts of Sensei, like the Visualiser.
///
/// # Fields
/// - `send_data_channel`: Tokio broadcast channel for distributing data to subscribers.
/// - `handlers`: Map of device IDs to their respective device handlers.
/// - `addr`: The network address of this node.
/// - `host_id`: Unique identifier for this node.
/// - `registry_addrs`: Optional list of registry addresses to register with.
/// - `device_configs`: Configuration for each device managed by this node.
/// - `registry`: Local registry instance for host/device status.
/// - `registry_polling_rate_s`: Optional polling interval for registry updates.
///
#[derive(Clone)]
pub struct SystemNode {
    send_data_channel: broadcast::Sender<(DataMsg, DeviceId)>, // Call .subscribe() on the sender in order to get a receiver
    handlers: Arc<Mutex<HashMap<u64, Box<DeviceHandler>>>>,
    addr: SocketAddr,
    host_id: u64,
    registry_addrs: Option<Vec<SocketAddr>>,
    device_configs: Vec<DeviceHandlerConfig>,
    registry: Registry,
}

impl SubscribeDataChannel for SystemNode {
    /// Creates a new receiver for the System Node's send data channel.
    fn subscribe_data_channel(&self) -> broadcast::Receiver<(DataMsg, DeviceId)> {
        self.send_data_channel.subscribe()
    }
}

impl SystemNode {
    /// Returns the current host status as a `RegCtrl` message.
    fn get_host_status(&self) -> HostStatus {
        HostStatus {
            host_id: self.host_id,
            device_statuses: self.device_configs.iter().map(DeviceStatus::from).collect(),
            responsiveness: Responsiveness::Connected,
        }
    }

    /// Handles incoming host control messages and performs the corresponding actions.
    ///
    /// # Arguments
    /// * `request` - The incoming RPC message containing the request details.
    /// * `message` - The host control command to handle.
    /// * `send_channel_msg_channel` - A channel to send messages to the connection's sending task.
    ///
    /// # Returns
    /// * `Result<(), NetworkError>` - Returns `Ok(())` if the command was handled successfully, or a `NetworkError` if an error occurred.
    ///
    /// # Behavior
    /// - Handles various `HostCtrl` commands such as `Connect`, `Disconnect`, `Subscribe`, `Unsubscribe`, `SubscribeTo`, `UnsubscribeFrom`, and `Configure`.
    /// - For `Disconnect`, signals the sending task to disconnect and returns an error to indicate the connection should close.
    /// - For subscription commands, updates the relevant handlers and logs the actions.
    /// - For configuration commands, creates, edits, or deletes device handlers as specified.
    async fn handle_host_ctrl(
        &self,
        request: RpcMessage,
        message: HostCtrl,
        send_channel_msg_channel: watch::Sender<ChannelMsg>,
    ) -> Result<(), NetworkError> {
        match message {
            // regular Host commands
            HostCtrl::Connect => {
                let src = request.src_addr;
                info!("Started connection with {src}");
            }
            HostCtrl::Disconnect => {
                // Correct way to signal disconnect to the sending task for this connection
                send_channel_msg_channel.send(ChannelMsg::from(HostChannel::Disconnect))?;
                return Err(NetworkError::Closed); // Indicate connection should close
            }
            HostCtrl::Subscribe { device_id } => {
                send_channel_msg_channel.send(ChannelMsg::from(HostChannel::Subscribe { device_id }))?;
                info!("Client {} subscribed to data stream for device_id: {device_id}", request.src_addr);
            }
            HostCtrl::Unsubscribe { device_id } => {
                send_channel_msg_channel.send(ChannelMsg::from(HostChannel::Unsubscribe { device_id }))?;
                info!("Client {} unsubscribed from data stream for device_id: {device_id}", request.src_addr);
            }
            HostCtrl::SubscribeTo { target, device_id } => {
                // Create a device handler with a source that will connect to the node server of the target
                // The sink will connect to this nodes server
                // Node servers broadcast all incoming data to all connections, but only relevant sources will process this data
                let source: DataSourceConfig = lib::sources::DataSourceConfig::Tcp(TCPConfig {
                    target_addr: target,
                    device_id,
                });
                let controller = None;
                let adapter = None;
                let tcp_sink_config = lib::sinks::tcp::TCPConfig {
                    target_addr: self.addr,
                    device_id,
                };
                let sinks = vec![lib::sinks::SinkConfig::Tcp(tcp_sink_config)];
                let new_handler_config = DeviceHandlerConfig {
                    device_id,
                    stype: TCP,
                    source,
                    controller,
                    adapter,
                    sinks,
                };

                let new_handler = DeviceHandler::from_config(new_handler_config).await.unwrap();

                info!("Handler created to subscribe to {target}");

                self.handlers.lock().await.insert(device_id, new_handler);
            }
            HostCtrl::UnsubscribeFrom { target: _, device_id } => {
                // TODO: Make it target specific, but for now removing based on device id should be fine.
                // Would require extracting the source itself from the device handler
                info!("Removing handler subscribing to {device_id}");
                match self.handlers.lock().await.remove(&device_id) {
                    Some(mut handler) => handler.stop().await.expect("Whoopsy"),
                    _ => warn!("This handler does not exist."),
                }
            }
            HostCtrl::Configure { device_id, cfg_type } => match cfg_type {
                Create { cfg } => {
                    info!("Creating a new device handler for device id {device_id}");
                    let handler = DeviceHandler::from_config(cfg).await.unwrap();
                    self.handlers.lock().await.insert(device_id, handler);
                }
                Edit { cfg } => {
                    info!("Editing existing device handler for device id {device_id}");
                    match self.handlers.lock().await.get_mut(&device_id) {
                        Some(handler) => handler.reconfigure(cfg).await.expect("Whoopsy"),
                        _ => warn!("This handler does not exist."),
                    }
                }
                Delete => {
                    info!("Deleting device handler for device id {device_id}");
                    match self.handlers.lock().await.remove(&device_id) {
                        Some(mut handler) => handler.stop().await.expect("Whoopsy"),
                        _ => warn!("This handler does not exist."),
                    }
                }
            },
            m => {
                warn!("Received unhandled HostCtrl: {m:?}");
            }
        };
        Ok(())
    }

    /// Handles channel messages for host actions (subscribe, unsubscribe, disconnect).
    async fn handle_host_channel(
        &self,
        host_msg: HostChannel,
        send_stream: &mut OwnedWriteHalf,
        subscribed_ids: &mut HashSet<HostId>,
    ) -> Result<(), NetworkError> {
        match host_msg {
            HostChannel::Disconnect => {
                send_message(send_stream, RpcMessageKind::HostCtrl(HostCtrl::Disconnect)).await?;
                debug!("Send close confirmation");
                return Err(NetworkError::Closed); // Throwing an error here feels weird, but it's also done in the recv_handler
            }
            HostChannel::Subscribe { device_id } => {
                info!("Subscribed");
                subscribed_ids.insert(device_id);
            }
            HostChannel::Unsubscribe { device_id } => {
                info!("Unsubscribed");
                subscribed_ids.remove(&device_id);
            }
            _ => {}
        };
        Ok(())
    }

    /// Handles channel messages used for registry actions.
    async fn handle_reg_channel(&self, reg_msg: RegChannel, send_stream: &mut OwnedWriteHalf) -> Result<(), NetworkError> {
        match reg_msg {
            RegChannel::SendHostStatus { host_id } => {
                let host_status = if host_id == self.host_id {
                    RegCtrl::HostStatus(self.get_host_status())
                } else {
                    RegCtrl::from(self.registry.get_host_by_id(host_id).await?)
                };
                let msg = RpcMessageKind::RegCtrl(host_status);
                send_message(send_stream, msg).await?;
            }
            RegChannel::SendHostStatuses => {
                let own_status = self.get_host_status();
                let mut host_statuses: Vec<HostStatus> = self
                    .registry
                    .list_host_statuses()
                    .await
                    .iter()
                    .map(|(_, info)| HostStatus::from(RegCtrl::from(info.clone())))
                    .collect();
                host_statuses.push(own_status);
                let msg = RegCtrl::HostStatuses { host_statuses };
                send_message(send_stream, RpcMessageKind::RegCtrl(msg)).await?;
            }
        }
        Ok(())
    }
}

#[async_trait]
impl ConnectionHandler for SystemNode {
    /// Handles receiving messages from other senders in the network.
    /// This communicates with the sender function using channel messages.
    ///
    /// # Types
    ///
    /// - Connect/Disconnect
    /// - Subscribe/Unsubscribe
    /// - Configure
    async fn handle_recv(&self, request: RpcMessage, send_channel_msg_channel: watch::Sender<ChannelMsg>) -> Result<(), NetworkError> {
        debug!("Received message {:?} from {}", request.msg, request.src_addr);
        match &request.msg {
            RpcMessageKind::HostCtrl(command) => self.handle_host_ctrl(request.clone(), command.clone(), send_channel_msg_channel).await?,
            RpcMessageKind::RegCtrl(command) => {
                self.registry
                    .handle_reg_ctrl(request.clone(), command.clone(), send_channel_msg_channel)
                    .await?
            }
            RpcMessageKind::Data { data_msg, device_id } => {
                // TODO: Pass it through relevant TCP sources
                self.send_data_channel.send((data_msg.clone(), *device_id))?;
            }
        }
        Ok(())
    }

    /// Handles sending messages for the nodes to other receivers in the network.
    ///
    /// The node will only send messages to subscribers of relevant messages.
    async fn handle_send(
        &self,
        mut recv_command_channel: watch::Receiver<ChannelMsg>,
        mut recv_data_channel: broadcast::Receiver<(DataMsg, DeviceId)>,
        mut send_stream: OwnedWriteHalf,
    ) -> Result<(), NetworkError> {
        let mut subscribed_ids: HashSet<HostId> = HashSet::new();
        loop {
            if recv_command_channel.has_changed().unwrap_or(false) {
                let msg_opt = recv_command_channel.borrow_and_update().clone();
                debug!("Received message {msg_opt:?} over channel");
                match msg_opt {
                    ChannelMsg::HostChannel(host_msg) => self.handle_host_channel(host_msg, &mut send_stream, &mut subscribed_ids).await?,
                    ChannelMsg::RegChannel(reg_msg) => self.handle_reg_channel(reg_msg, &mut send_stream).await?,
                    _ => (),
                }
            }

            if !recv_data_channel.is_empty() {
                let (data_msg, device_id) = recv_data_channel.recv().await.unwrap();

                debug!("Sending data {data_msg:?} for {device_id} to {send_stream:?}");
                let msg = RpcMessageKind::Data { data_msg, device_id };

                send_message(&mut send_stream, msg).await?;
            }
        }
    }
}

impl Run<SystemNodeConfig> for SystemNode {
    /// Constructs a new `SystemNode` from the given global and node-specific configuration.
    fn new(global_config: GlobalConfig, config: SystemNodeConfig) -> Self {
        let (send_data_channel, _) = broadcast::channel::<(DataMsg, DeviceId)>(16);

        SystemNode {
            send_data_channel,
            handlers: Arc::new(Mutex::new(HashMap::new())),
            addr: config.addr,
            host_id: config.host_id,
            registry_addrs: config.registries,
            device_configs: config.device_configs,
            registry: Registry::new(config.registry_polling_rate_s),
        }
    }

    /// Starts the system node.
    ///
    /// Initializes a hashmap of device handlers based on the configuration file on startup
    ///
    /// # Arguments
    ///
    /// SystemNodeConfig: Specifies the target address
    async fn run(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let connection_handler = Arc::new(self.clone());
        for cfg in &self.device_configs {
            self.handlers
                .lock()
                .await
                .insert(cfg.device_id, DeviceHandler::from_config(cfg.clone()).await.unwrap());
        }
        // Register at provided registries. When a single registry refuses, the client exits.
        if let Some(registries) = &self.registry_addrs {
            let mut client = TcpClient::new();
            for registry in registries {
                info!("Connecting to registry at {registry}");
                let registry_addr: SocketAddr = *registry;
                let heartbeat_msg = RpcMessageKind::RegCtrl(RegCtrl::AnnouncePresence {
                    host_id: self.host_id,
                    host_address: self.addr,
                });
                client.connect(registry_addr).await?;
                client.send_message(registry_addr, heartbeat_msg).await?;
                client.disconnect(registry_addr);
                info!("Presence announced to registry at {registry_addr}");
            }
        }
        // Create a TCP host server task
        info!("Starting TCP server on {}...", self.addr);
        let connection_handler = Arc::new(self.clone());
        let tcp_server: tokio::task::JoinHandle<()> = task::spawn(async {
            TcpServer::serve(connection_handler.addr, connection_handler).await.unwrap();
        });
        // create registry polling task, if configured
        let polling_task = self.registry.create_polling_task();
        // Run both tasks concurrently, utill either errors, or both exit
        tokio::try_join!(tcp_server, polling_task)?;
        Ok(())
    }
}
