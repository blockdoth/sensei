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
use lib::adapters::CsiDataAdapter; // Added CsiDataAdapter
use lib::errors::NetworkError;
use lib::handler::device_handler::{DeviceHandler, DeviceHandlerConfig};
use lib::network::rpc_message::CfgType::{Create, Delete, Edit};
use lib::network::rpc_message::SourceType::*;
use lib::network::rpc_message::{DataMsg, DeviceId, DeviceStatus, HostCtrl, HostId, HostStatus, RegCtrl, Responsiveness, RpcMessage, RpcMessageKind};
use lib::network::tcp::client::TcpClient;
use lib::network::tcp::server::TcpServer;
use lib::network::tcp::{ChannelMsg, ConnectionHandler, HostChannel, RegChannel, SubscribeDataChannel, send_message};
use lib::sinks::{Sink, SinkConfig};
use lib::sources::tcp::TCPConfig;
use lib::sources::{DataSourceConfig, DataSourceT}; // Added DataSourceT
use log::*;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::sync::{Mutex, broadcast, mpsc, watch}; // Added mpsc
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
/// - `sinks`: Map of sink IDs to their respective sink implementations.
/// - `addr`: The network address of this node.
/// - `host_id`: Unique identifier for this node.
/// - `registry_addrs`: Optional list of registry addresses to register with.
/// - `device_configs`: Configuration for each device managed by this node.
/// - `sink_configs`: Configuration for each sink managed by this node.
/// - `registry`: Local registry instance for host/device status.
/// - `registry_polling_rate_s`: Optional polling interval for registry updates.
///
#[derive(Clone)]
pub struct SystemNode {
    send_data_channel: broadcast::Sender<(DataMsg, DeviceId)>,      // For external TCP clients
    local_data_tx: mpsc::Sender<(DataMsg, DeviceId)>,               // For local DeviceHandler data
    local_data_rx: Arc<Mutex<mpsc::Receiver<(DataMsg, DeviceId)>>>, // Receiver for local data
    handlers: Arc<Mutex<HashMap<u64, Box<DeviceHandler>>>>,
    sinks: Arc<Mutex<HashMap<String, Box<dyn Sink>>>>, // Added shared sinks
    addr: SocketAddr,
    host_id: u64,
    registry_addrs: Option<Vec<SocketAddr>>,
    device_configs: Vec<DeviceHandlerConfig>,
    sink_configs: Vec<SinkConfigWithName>, // Added sink configurations
    registry: Registry,
}

// Helper struct to associate a name with a SinkConfig, mirroring the YAML structure
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct SinkConfigWithName {
    pub id: String,
    pub config: SinkConfig,
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
                // Sinks are now managed by SystemNode, this specific logic for TCP sink might need adjustment
                // based on how `output_to` is handled for such dynamically created handlers.
                // For now, we assume it might output to a default or pre-configured sink if necessary.
                let new_handler_config = DeviceHandlerConfig {
                    device_id,
                    stype: TCP,
                    source,
                    controller,
                    adapter,
                    output_to: vec![],
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
            HostCtrl::Experiment { experiment } => {},
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

    /// Helper function to route data (from local or external sources) to configured sinks.
    async fn route_data_to_sinks(&self, data_msg: DataMsg, device_id: DeviceId) {
        let handlers_guard = self.handlers.lock().await;
        if let Some(handler_config) = handlers_guard.get(&device_id).map(|h| h.config().clone()) {
            drop(handlers_guard); // Release lock on handlers before locking sinks
            let mut sinks_guard = self.sinks.lock().await;
            for sink_id in &handler_config.output_to {
                if let Some(sink) = sinks_guard.get_mut(sink_id) {
                    info!("Routing data from device {device_id} to sink {sink_id}");
                    if let Err(e) = sink.provide(data_msg.clone()).await {
                        error!("Error providing data to sink {sink_id}: {e:?}");
                    }
                } else {
                    warn!("Sink ID {sink_id} configured for device {device_id} not found");
                }
            }
        } else {
            warn!("Device ID {device_id} not found in handlers for data routing");
        }
    }

    /// Task to process data from local device handlers.
    async fn process_local_data(&self) {
        let mut rx = self.local_data_rx.lock().await;
        info!("Starting local data processing task.");
        while let Some((data_msg, device_id)) = rx.recv().await {
            info!("SystemNode received local data for device_id: {device_id}");
            // 1. Broadcast to connected TCP clients
            if self.send_data_channel.receiver_count() > 0 {
                if let Err(e) = self.send_data_channel.send((data_msg.clone(), device_id)) {
                    error!("Failed to broadcast local data: {e:?}");
                }
            }
            // 2. Route to configured sinks
            self.route_data_to_sinks(data_msg, device_id).await;
        }
        info!("Local data processing task finished.");
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
        debug!("Received message {:?} from {:?}", request.msg, request.src_addr);
        match &request.msg {
            RpcMessageKind::HostCtrl(command) => self.handle_host_ctrl(request.clone(), command.clone(), send_channel_msg_channel).await?,
            RpcMessageKind::RegCtrl(command) => {
                self.registry
                    .handle_reg_ctrl(request.clone(), command.clone(), send_channel_msg_channel)
                    .await?
            }
            RpcMessageKind::Data { data_msg, device_id } => {
                // Data from EXTERNAL source (network)
                info!("SystemNode received external data for device_id: {device_id}");
                // 1. Broadcast to connected TCP clients
                if self.send_data_channel.receiver_count() > 0 {
                    if let Err(e) = self.send_data_channel.send((data_msg.clone(), *device_id)) {
                        error!("Failed to broadcast external data: {e:?}");
                    }
                }
                // 2. Route to configured sinks for this device_id
                self.route_data_to_sinks(data_msg.clone(), *device_id).await;
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
        let (local_data_tx, local_data_rx) = mpsc::channel::<(DataMsg, DeviceId)>(100); // Channel for local data

        SystemNode {
            send_data_channel,
            local_data_tx,                                      // Store the sender
            local_data_rx: Arc::new(Mutex::new(local_data_rx)), // Store the receiver
            handlers: Arc::new(Mutex::new(HashMap::new())),
            sinks: Arc::new(Mutex::new(HashMap::new())), // Initialize sinks map
            addr: config.addr,
            host_id: config.host_id,
            registry_addrs: config.registries,
            device_configs: config.device_configs,
            sink_configs: config.sinks, // Store sink configurations from SystemNodeConfig
            registry: Registry::new(config.registry_polling_rate_s),
        }
    }

    /// Starts the system node.
    ///
    /// Initializes a hashmap of device handlers and sinks based on the configuration file on startup
    ///
    /// # Arguments
    ///
    /// SystemNodeConfig: Specifies the target address
    async fn run(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let connection_handler = Arc::new(self.clone());

        // Initialize Sinks
        let mut sinks_map = self.sinks.lock().await;
        for sink_conf_with_name in &self.sink_configs {
            info!("Initializing sink: {}", sink_conf_with_name.id);
            let mut sink = <dyn Sink>::from_config(sink_conf_with_name.config.clone()).await?;
            sink.open()
                .await
                .map_err(|e| format!("Failed to open sink {}: {:?}", sink_conf_with_name.id, e))?;
            sinks_map.insert(sink_conf_with_name.id.clone(), sink);
        }
        drop(sinks_map); // Release lock

        // Initialize Device Handlers
        let mut handlers_map = self.handlers.lock().await;
        for cfg in &self.device_configs {
            let mut handler = DeviceHandler::from_config(cfg.clone()).await.unwrap();
            // Pass the sender for local data to the handler's start method
            handler
                .start(
                    <dyn DataSourceT>::from_config(cfg.source.clone()).await?,
                    if let Some(adapter_cfg) = cfg.adapter {
                        Some(<dyn CsiDataAdapter>::from_config(adapter_cfg).await?)
                    } else {
                        None
                    },
                    self.local_data_tx.clone(),
                )
                .await
                .expect("Failed to start device handler");
            handlers_map.insert(cfg.device_id, handler);
        }
        drop(handlers_map); // Release lock

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
        let tcp_server_task: tokio::task::JoinHandle<()> = task::spawn(async move {
            TcpServer::serve(connection_handler.addr, connection_handler).await.unwrap();
        });

        // Task for processing local data
        let self_clone_for_local_data = self.clone();
        let local_data_processing_task = task::spawn(async move {
            self_clone_for_local_data.process_local_data().await;
        });

        // create registry polling task, if configured
        let polling_task = self.registry.create_polling_task();
        // Run all tasks concurrently
        tokio::try_join!(tcp_server_task, polling_task, local_data_processing_task)?;
        Ok(())
    }
}
