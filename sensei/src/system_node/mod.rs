//! # System Node Module
//!
//! The System Node is a core component in the Sensei network. It acts as both a sender and receiver of CSI data, hosting devices that generate or consume this data.
//! System Nodes are responsible for forwarding CSI data to other receivers, adapting data to a universal standard, and managing device handlers.
//!
//! Devices can subscribe to CSI data streams by sending a `Subscribe` message with a device ID. The System Node manages these subscriptions and ensures data is routed appropriately.
//!
//! The System Node also interacts with registries to announce its presence and can periodically poll other nodes when functioning as a registry.

use core::panic;
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use lib::FromConfig;
use lib::adapters::CsiDataAdapter;
use lib::errors::{AppError, ExperimentError, NetworkError};
use lib::handler::device_handler::{DeviceHandler, DeviceHandlerConfig};
use lib::network::experiment_config::IsRecurring::{NotRecurring, Recurring};
use lib::network::experiment_config::{Block, Command, Experiment, Stage};
use lib::network::rpc_message::CfgType::{Create, Delete, Edit};
use lib::network::rpc_message::SourceType::*;
use lib::network::rpc_message::{
    CfgType, DataMsg, DeviceId, DeviceStatus, HostCtrl, HostId, HostStatus, RegCtrl, Responsiveness, RpcMessage, RpcMessageKind,
};
use lib::network::tcp::client::TcpClient;
use lib::network::tcp::server::TcpServer;
use lib::network::tcp::{ChannelMsg, ConnectionHandler, HostChannel, RegChannel, SubscribeDataChannel, send_message};
use lib::sinks::{Sink, SinkConfig};
use lib::sources::tcp::TCPConfig;
use lib::sources::{DataSourceConfig, DataSourceT};
use log::*;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::sync::{Mutex, broadcast, mpsc, watch};
use tokio::task::{self, JoinHandle};

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
    sinks: Arc<Mutex<HashMap<String, Box<dyn Sink>>>>,
    addr: SocketAddr,
    host_id: u64,
    registry_addrs: Option<Vec<SocketAddr>>,
    device_configs: Vec<DeviceHandlerConfig>,
    sink_configs: Vec<SinkConfigWithName>,
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

    async fn connect(src_addr: SocketAddr) -> Result<(), NetworkError> {
        info!("Started connection with {src_addr}");
        Ok(())
    }

    async fn disconnect(src_addr: SocketAddr, send_channel_msg_channel: watch::Sender<ChannelMsg>) -> Result<(), NetworkError> {
        send_channel_msg_channel.send(ChannelMsg::from(HostChannel::Disconnect))?;
        info!("Disconnecting from the connection with {src_addr}");
        Ok(())
    }

    async fn subscribe(src_addr: SocketAddr, device_id: DeviceId, send_channel_msg_channel: watch::Sender<ChannelMsg>) -> Result<(), NetworkError> {
        send_channel_msg_channel.send(ChannelMsg::from(HostChannel::Subscribe { device_id }))?;
        info!("Client {src_addr} subscribed to data stream for device_id: {device_id}");

        Ok(())
    }

    async fn unsubscribe(src_addr: SocketAddr, device_id: DeviceId, send_channel_msg_channel: watch::Sender<ChannelMsg>) -> Result<(), NetworkError> {
        send_channel_msg_channel.send(ChannelMsg::from(HostChannel::Unsubscribe { device_id }))?;
        info!("Client {src_addr} unsubscribed from data stream for device_id: {device_id}");

        Ok(())
    }

    async fn subscribe_to(
        target_addr: SocketAddr,
        device_id: DeviceId,
        handlers: Arc<Mutex<HashMap<u64, Box<DeviceHandler>>>>,
    ) -> Result<(), NetworkError> {
        // Create a device handler with a source that will connect to the node server of the target
        // The sink will connect to this nodes server
        // Node servers broadcast all incoming data to all connections, but only relevant sources will process this data
        let source: DataSourceConfig = lib::sources::DataSourceConfig::Tcp(TCPConfig { target_addr, device_id });
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

        // This can be allowed to unwrap, as it will literally always succeed
        let new_handler = DeviceHandler::from_config(new_handler_config).await.unwrap();

        info!("Handler created to subscribe to {target_addr}");

        handlers.lock().await.insert(device_id, new_handler);

        Ok(())
    }

    async fn unsubscribe_from(device_id: DeviceId, handlers: Arc<Mutex<HashMap<u64, Box<DeviceHandler>>>>) -> Result<(), NetworkError> {
        // TODO: Make it target specific, but for now removing based on device id should be fine.
        // Would require extracting the source itself from the device handler
        info!("Removing handler subscribing to {device_id}");
        match handlers.lock().await.remove(&device_id) {
            Some(mut handler) => {
                handler.stop().await;
            }
            _ => info!("This handler does not exist."),
        }

        Ok(())
    }

    async fn configure(device_id: DeviceId, cfg_type: CfgType, handlers: Arc<Mutex<HashMap<u64, Box<DeviceHandler>>>>) -> Result<(), NetworkError> {
        match cfg_type {
            Create { cfg } => {
                info!("Creating a new device handler for device id {device_id}");
                let handler = DeviceHandler::from_config(cfg).await;
                match handler {
                    Ok(handler) => {
                        handlers.lock().await.insert(device_id, handler);
                    }
                    Err(e) => {
                        info!("Creating device handler went wrong: {e}")
                    }
                }
            }
            Edit { cfg } => {
                info!("Editing existing device handler for device id {device_id}");
                match handlers.lock().await.get_mut(&device_id) {
                    Some(handler) => handler.reconfigure(cfg).await.expect("Whoopsy"),
                    _ => info!("This handler does not exist."),
                }
            }
            Delete => {
                info!("Deleting device handler for device id {device_id}");
                match handlers.lock().await.remove(&device_id) {
                    Some(mut handler) => handler.stop().await.expect("Whoopsy"),
                    _ => info!("This handler does not exist."),
                }
            }
        }

        Ok(())
    }

    async fn load_experiment(experiment: Experiment, handlers: Arc<Mutex<HashMap<u64, Box<DeviceHandler>>>>) -> Result<(), NetworkError> {
        info!("Running {}", experiment.metadata.name.clone());

        for (i, stage) in experiment.stages.into_iter().enumerate() {
            let name = stage.name.clone();
            info!("Executing stage {name}");
            Self::execute_stage(stage, handlers.clone()).await?;
            info!("Finished stage {name}");
        }

        Ok(())
    }

    async fn execute_stage(stage: Stage, handlers: Arc<Mutex<HashMap<u64, Box<DeviceHandler>>>>) -> Result<(), AppError> {
        let mut tasks = vec![];

        for block in stage.command_blocks {
            // Have to define the clone outside the tokio task and then move the clone, rather than create the clone inside the tokio task
            let handlers_clone = handlers.clone();
            let task = tokio::spawn(async move {
                Self::execute_command_block(block, handlers_clone).await;
            });
            tasks.push(task);
        }

        let results = futures::future::join_all(tasks).await;

        for result in results {
            if result.is_err() {
                return Err(ExperimentError::ExecutionError.into());
            }
        }
        Ok(())
    }

    async fn execute_command_block(block: Block, handlers: Arc<Mutex<HashMap<u64, Box<DeviceHandler>>>>) -> Result<(), NetworkError> {
        tokio::time::sleep(std::time::Duration::from_millis(block.delays.init_delay.unwrap_or(0u64))).await;
        let command_delay = block.delays.command_delay.unwrap_or(0u64);
        let command_types = block.commands;

        match block.delays.is_recurring.clone() {
            Recurring {
                recurrence_delay,
                iterations,
            } => {
                let r_delay = recurrence_delay.unwrap_or(0u64);
                let n = iterations.unwrap_or(0u64);
                if n == 0 {
                    loop {
                        Self::match_commands(command_types.clone(), handlers.clone(), command_delay).await?;
                        tokio::time::sleep(std::time::Duration::from_millis(r_delay)).await;
                    }
                } else {
                    for _ in 0..n {
                        Self::match_commands(command_types.clone(), handlers.clone(), command_delay).await?;
                        tokio::time::sleep(std::time::Duration::from_millis(r_delay)).await;
                    }
                }
                Ok(())
            }
            NotRecurring => {
                Self::match_commands(command_types, handlers, command_delay).await?;
                Ok(())
            }
        }
    }

    pub async fn match_commands(
        commands: Vec<Command>,
        handlers: Arc<Mutex<HashMap<u64, Box<DeviceHandler>>>>,
        command_delay: u64,
    ) -> Result<(), NetworkError> {
        for command in commands {
            Self::match_command(command.clone(), handlers.clone()).await?;
            tokio::time::sleep(std::time::Duration::from_millis(command_delay)).await;
        }
        Ok(())
    }

    async fn match_command(command: Command, handlers: Arc<Mutex<HashMap<u64, Box<DeviceHandler>>>>) -> Result<(), NetworkError> {
        match command {
            Command::Subscribe { target_addr, device_id } => Ok(Self::subscribe_to(target_addr, device_id, handlers).await?),
            Command::Unsubscribe { target_addr, device_id } => Ok(Self::unsubscribe_from(device_id, handlers).await?),
            Command::Configure {
                target_addr,
                device_id,
                cfg_type,
            } => Ok(Self::configure(device_id, cfg_type, handlers).await?),
            Command::Delay { delay } => {
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                Ok(())
            }
            c => {
                info!("The system node does not support this command {c:?}");
                Ok(())
            }
        }
    }

    /// Handles channel messages for host actions (subscribe, unsubscribe, disconnect).
    async fn handle_host_channel(
        &self,
        host_msg: HostChannel,
        send_stream: &mut OwnedWriteHalf,
        subscribed_ids: &mut HashSet<HostId>,
    ) -> Result<(), NetworkError> {
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
            trace!("SystemNode received local data for device_id: {device_id}");
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
            RpcMessageKind::HostCtrl(command) => match command {
                // regular Host commands
                HostCtrl::Connect => Self::connect(request.src_addr).await?,
                HostCtrl::Disconnect => Self::disconnect(request.src_addr, send_channel_msg_channel).await?,
                HostCtrl::Subscribe { device_id } => Self::subscribe(request.src_addr, *device_id, send_channel_msg_channel).await?,
                HostCtrl::Unsubscribe { device_id } => Self::unsubscribe(request.src_addr, *device_id, send_channel_msg_channel).await?,
                HostCtrl::SubscribeTo { target_addr, device_id } => Self::subscribe_to(*target_addr, *device_id, self.handlers.clone()).await?,
                HostCtrl::UnsubscribeFrom { target_addr: _, device_id } => {
                    Self::unsubscribe(request.src_addr, *device_id, send_channel_msg_channel).await?
                }
                HostCtrl::Configure { device_id, cfg_type } => Self::configure(*device_id, cfg_type.clone(), self.handlers.clone()).await?,
                HostCtrl::Experiment { experiment } => Self::load_experiment(experiment.clone(), self.handlers.clone()).await?,
                HostCtrl::Ping => {
                    debug!("Received ping from {:#?}.", request.src_addr);
                    send_channel_msg_channel.send(ChannelMsg::from(HostChannel::Pong))?;
                }
                _ => {
                    warn!("The client received an unsupported request. Responding with an empty message.");
                    send_channel_msg_channel.send(ChannelMsg::from(HostChannel::Empty))?;
                }
            },
            RpcMessageKind::RegCtrl(command) => match command {
                RegCtrl::PollHostStatus { host_id } => {
                    send_channel_msg_channel.send(ChannelMsg::RegChannel(RegChannel::SendHostStatus { host_id: self.host_id }))?
                }
                _ => {
                    warn!("The client received an unsupported request. Responding with an empty message.");
                    send_channel_msg_channel.send(ChannelMsg::from(HostChannel::Empty))?;
                }
            },
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
        };
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
            // This loop continues execution untill a client disconnects.
            // Since tokio only yiels execution back to the scheduler once a task finishes
            // or another async task is encountered, a thread can be block by this task
            // if no yield points are inserted.
            task::consume_budget().await;
            if recv_command_channel.has_changed()? {
                let msg_opt = recv_command_channel.borrow_and_update().clone();
                debug!("Received message {msg_opt:?} over channel");
                match msg_opt {
                    ChannelMsg::HostChannel(host_msg) => match host_msg {
                        HostChannel::Disconnect => {
                            send_message(&mut send_stream, RpcMessageKind::HostCtrl(HostCtrl::Disconnect)).await?;
                            debug!("Send close confirmation");
                            return Err(NetworkError::Closed); // Throwing an error here feels weird, but it's also done in the recv_handler
                        }
                        HostChannel::Subscribe { device_id } => {
                            subscribed_ids.insert(device_id);
                        }
                        HostChannel::Unsubscribe { device_id } => {
                            subscribed_ids.remove(&device_id);
                        }
                        HostChannel::Pong => {
                            debug!("Sending pong...");
                            send_message(&mut send_stream, RpcMessageKind::HostCtrl(HostCtrl::Pong)).await?;
                        }
                        HostChannel::Empty => send_message(&mut send_stream, RpcMessageKind::HostCtrl(HostCtrl::Empty)).await?,
                        _ => error!("Received an unsupported channel message."),
                    },
                    ChannelMsg::RegChannel(RegChannel::SendHostStatus { host_id }) if host_id == self.host_id => {
                        let status = self.get_host_status();
                        send_message(&mut send_stream, RpcMessageKind::RegCtrl(RegCtrl::HostStatus(status))).await?;
                    }
                    _ => error!("Received an unsupported channel message."),
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
            sink.open().await?;
            sinks_map.insert(sink_conf_with_name.id.clone(), sink);
        }
        drop(sinks_map); // Release lock

        // Initialize Device Handlers
        let mut handlers_map = self.handlers.lock().await;
        for cfg in &self.device_configs {
            let mut handler = DeviceHandler::from_config(cfg.clone()).await?;
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
                .await?;
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
                client.disconnect(registry_addr).await;
                info!("Presence announced to registry at {registry_addr}");
            }
        }
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

        // Task for processing local data
        let self_clone_for_local_data = self.clone();
        let local_data_processing_task = task::spawn(async move {
            self_clone_for_local_data.process_local_data().await;
        });

        // Run all tasks concurrently
        tokio::try_join!(tcp_server_task, local_data_processing_task)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use async_trait::async_trait;
    use tokio::net::tcp::OwnedWriteHalf;
    use tokio::sync::{broadcast, watch};

    use super::*;

    #[derive(Clone)]
    struct MockConnectionHandler {
        host_status_sender: broadcast::Sender<(DataMsg, DeviceId)>,
    }
    impl MockConnectionHandler {
        fn new() -> Self {
            let (host_status_sender, _) = broadcast::channel(16);
            Self { host_status_sender }
        }
    }
    #[async_trait]
    impl ConnectionHandler for MockConnectionHandler {
        async fn handle_recv(&self, _msg: RpcMessage, _send_commands_channel: watch::Sender<ChannelMsg>) -> Result<(), NetworkError> {
            Ok(())
        }
        async fn handle_send(
            &self,
            _recv_commands_channel: watch::Receiver<ChannelMsg>,
            _recv_data_channel: broadcast::Receiver<(DataMsg, DeviceId)>,
            _write_stream: OwnedWriteHalf,
        ) -> Result<(), NetworkError> {
            Ok(())
        }
    }
    impl SubscribeDataChannel for MockConnectionHandler {
        fn subscribe_data_channel(&self) -> broadcast::Receiver<(DataMsg, DeviceId)> {
            self.host_status_sender.subscribe()
        }
    }

    fn create_system_node_config(addr: SocketAddr, host_id: u64) -> SystemNodeConfig {
        SystemNodeConfig {
            addr,
            host_id,
            registries: None,
            registry_polling_rate_s: None,
            device_configs: vec![],
            sinks: vec![],
        }
    }

    #[tokio::test]
    async fn test_system_node_new() {
        let config = create_system_node_config("127.0.0.1:12345".parse().unwrap(), 1);
        let global_config = GlobalConfig {
            log_level: log::LevelFilter::Debug,
            num_workers: 4,
        };
        let _system_node = SystemNode::new(global_config, config);
        // Can't check private fields, but construction should succeed
    }
}
