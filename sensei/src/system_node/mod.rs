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
use lib::errors::{AppError, ExperimentError, NetworkError};
use lib::experiments::{ActiveExperiment, Command, Experiment, ExperimentInfo, ExperimentSession, ExperimentStatus};
use lib::handler::device_handler::{DeviceHandler, DeviceHandlerConfig};
use lib::network::rpc_message::CfgType::{Create, Delete, Edit};
use lib::network::rpc_message::SourceType::*;
use lib::network::rpc_message::{
    CfgType, DataMsg, DeviceId, DeviceInfo, HostCtrl, HostId, HostStatus, RegCtrl, Responsiveness, RpcMessage, RpcMessageKind,
};
use lib::network::tcp::client::TcpClient;
use lib::network::tcp::server::TcpServer;
use lib::network::tcp::{ChannelMsg, ConnectionHandler, HostChannel, RegChannel, SubscribeDataChannel, send_message};
use lib::sinks::{Sink, SinkConfig};
use lib::sources::DataSourceConfig;
use lib::sources::tcp::TCPConfig;
use log::*;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::sync::mpsc::{Receiver, Sender};
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
    experiment_send: Sender<ExperimentChannelMsg>,
    experiment_recv: Arc<Mutex<Receiver<ExperimentChannelMsg>>>,
    handlers: Arc<Mutex<HashMap<DeviceId, Box<DeviceHandler>>>>,
    sinks: Arc<Mutex<HashMap<String, Box<dyn Sink>>>>,
    addr: SocketAddr,
    host_id: HostId,
    registry_addr: Option<SocketAddr>,
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

impl Run<SystemNodeConfig> for SystemNode {
    /// Constructs a new `SystemNode` from the given global and node-specific configuration.
    fn new(_global_config: GlobalConfig, config: SystemNodeConfig) -> Self {
        let (send_data_channel, _) = broadcast::channel::<(DataMsg, DeviceId)>(16);
        let (local_data_tx, local_data_rx) = mpsc::channel::<(DataMsg, DeviceId)>(100); // Channel for local data
        let (experiment_send, experiment_recv) = mpsc::channel::<ExperimentChannelMsg>(5);

        SystemNode {
            send_data_channel,
            local_data_tx,                                      // Store the sender
            local_data_rx: Arc::new(Mutex::new(local_data_rx)), // Store the receiver
            experiment_send,
            experiment_recv: Arc::new(Mutex::new(experiment_recv)),
            handlers: Arc::new(Mutex::new(HashMap::new())),
            sinks: Arc::new(Mutex::new(HashMap::new())), // Initialize sinks map
            addr: config.address,
            host_id: config.host_id,
            registry_addr: config.registry,
            device_configs: config.device_configs,
            sink_configs: config.sinks, // Store sink configurations from SystemNodeConfig
        }
    }

    /// Starts the system node.
    ///
    /// Initializes a hashmap of device handlers and sinks based on the configuration file on startup
    ///
    async fn run(&mut self) -> Result<(), Box<dyn std::error::Error>> {
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
            handler.start(self.local_data_tx.clone()).await?;
            handlers_map.insert(cfg.device_id, handler);
        }
        drop(handlers_map); // Release lock

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

        let experiment_task = Self::experiment_handler(
            self.handlers.clone(),
            self.experiment_send.clone(),
            self.experiment_recv.clone(),
            self.local_data_tx.clone(),
        );

        if let Some(registry_addr) = self.registry_addr {
            let self_addr = self.addr;
            let self_id = self.host_id;
            task::spawn(Self::announce_presence_to_registry(registry_addr, self_addr, self_id));
        }

        // Run all tasks concurrently
        tokio::try_join!(tcp_server_task, local_data_processing_task, experiment_task)?;
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub enum ExperimentChannelMsg {
    Start(Experiment, watch::Sender<ChannelMsg>),
    Stop,
    UpdateExperimentStatus(ExperimentInfo),
}

impl SystemNode {
    // Continuously running task responsible for managing experiment related functionality

    async fn announce_presence_to_registry(registry_addr: SocketAddr, self_addr: SocketAddr, self_id: HostId) -> Result<(), NetworkError> {
        let mut client = TcpClient::new();
        let msg = RpcMessageKind::RegCtrl(RegCtrl::AnnouncePresence {
            host_id: self_id,
            host_address: self_addr,
        });
        debug!("Connecting to registry at {registry_addr}");
        client.connect(registry_addr).await?;
        client.send_message(registry_addr, msg).await?;
        info!("Presence announced to registry at {registry_addr}");
        client.disconnect(registry_addr).await?;
        Ok(())
    }

    fn experiment_handler(
        handlers: Arc<Mutex<HashMap<u64, Box<DeviceHandler>>>>,
        experiment_send: Sender<ExperimentChannelMsg>,
        experiment_recv: Arc<Mutex<Receiver<ExperimentChannelMsg>>>,
        local_data_tx: mpsc::Sender<(DataMsg, DeviceId)>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            info!("Started experiment handler task");
            let (cancel_signal_send, cancel_signal_recv) = watch::channel(false);
            let session = ExperimentSession::new(experiment_send.clone(), cancel_signal_recv);

            let mut experiment_recv = experiment_recv.lock().await;
            let mut is_running_experiment = false;
            // let mut send_channel_opt = None;

            while let Some(msg) = experiment_recv.recv().await {
                match msg {
                    ExperimentChannelMsg::Start(experiment, _send_channel) if !is_running_experiment => {
                        info!("starting experiment");
                        // send_channel_opt = Some(send_channel); // Very hacky solution
                        is_running_experiment = true;
                        if let Err(err) = cancel_signal_send.send(false) {
                            panic!("{err}");
                        };
                        let mut session = session.clone();
                        session.active_experiment = Some(ActiveExperiment {
                            experiment: experiment.clone(),
                            info: ExperimentInfo {
                                status: ExperimentStatus::Ready,
                                current_stage: 0,
                            },
                        });
                        let converter = |exp: ActiveExperiment| ExperimentChannelMsg::UpdateExperimentStatus(exp.info);

                        let handlers = handlers.clone();
                        let experiment_send = experiment_send.clone();
                        let local_data_tx = local_data_tx.clone();

                        let handler = Arc::new(move |command: Command, _update_send: Sender<ExperimentChannelMsg>| {
                            let handlers = handlers.clone(); // clone *inside* closure body
                            let local_data_tx = local_data_tx.clone(); // clone *inside* closure body
                            info!("started experiment task");
                            async move {
                                if let Err(err) = Self::match_command(command, handlers, local_data_tx).await {
                                    panic!("{err}");
                                };
                            }
                        });

                        tokio::spawn(async move {
                            if let Err(err) = session.run(experiment_send, converter, handler).await {
                                panic!("{err}");
                            };
                        });
                    }
                    ExperimentChannelMsg::Start(_, _) => {
                        error!("Cant start, experiment already running")
                    }
                    ExperimentChannelMsg::Stop => {
                        is_running_experiment = false;
                        if let Err(err) = cancel_signal_send.send(true) {
                            panic!("{err}");
                        };
                        info!("Stopped exp");
                    }
                    ExperimentChannelMsg::UpdateExperimentStatus(experiment_info) => {
                        info!("{experiment_info:?}");
                        // broken
                        // if let Some(ref send_channel) = send_channel_opt {
                        //     send_channel.send(ChannelMsg::HostChannel(HostChannel::UpdateExperimentStatus { experiment_info }));
                        // }
                    }
                }
            }
        })
    }

    async fn match_command(
        command: Command,
        handlers: Arc<Mutex<HashMap<u64, Box<DeviceHandler>>>>,
        local_data_tx: mpsc::Sender<(DataMsg, DeviceId)>,
    ) -> Result<(), AppError> {
        match command {
            Command::Subscribe { target_addr, device_id } => Ok(Self::subscribe_to(target_addr, device_id, handlers, local_data_tx).await?),
            Command::Unsubscribe { target_addr: _, device_id } => Ok(Self::unsubscribe_from(device_id, handlers).await?),
            Command::Configure {
                target_addr: _,
                device_id,
                cfg_type,
            } => Ok(Self::configure(device_id, cfg_type, handlers).await?),
            Command::Delay { delay } => {
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                Ok(())
            }
            Command::Start { target_addr: _, device_id } => Ok(Self::start(device_id, handlers, local_data_tx).await?),
            Command::StartAll { target_addr: _ } => Ok(Self::start_all(handlers, local_data_tx).await?),
            Command::Stop { target_addr: _, device_id } => Ok(Self::stop(device_id, handlers).await?),
            Command::StopAll { target_addr: _ } => Ok(Self::stop_all(handlers).await?),
            c => {
                info!("The system node does not support this command {c:?}");
                Ok(())
            }
        }
    }

    /// Returns the current host status as a `RegCtrl` message.
    async fn get_host_status(&self) -> HostStatus {
        HostStatus {
            addr: self.addr,
            host_id: self.host_id,
            device_statuses: self.handlers.lock().await.iter().map(|x| DeviceInfo::from(&x.1.config())).collect(),
            responsiveness: Responsiveness::Connected,
        }
    }

    async fn connect(src_addr: SocketAddr) -> Result<(), Box<dyn std::error::Error>> {
        info!("Connected with {src_addr}");

        Ok(())
    }

    async fn disconnect(src_addr: SocketAddr, send_channel_msg_channel: watch::Sender<ChannelMsg>) -> Result<(), AppError> {
        send_channel_msg_channel.send(ChannelMsg::from(HostChannel::Disconnect))?;

        info!("Disconnected from {src_addr}");

        Ok(())
    }

    async fn subscribe(src_addr: SocketAddr, device_id: DeviceId, send_channel_msg_channel: watch::Sender<ChannelMsg>) -> Result<(), AppError> {
        send_channel_msg_channel.send(ChannelMsg::from(HostChannel::Subscribe { device_id }))?;
        info!("Client {src_addr} subscribed to data stream for device_id: {device_id}");

        Ok(())
    }

    async fn unsubscribe(src_addr: SocketAddr, device_id: DeviceId, send_channel_msg_channel: watch::Sender<ChannelMsg>) -> Result<(), AppError> {
        send_channel_msg_channel.send(ChannelMsg::from(HostChannel::Unsubscribe { device_id }))?;
        info!("Client {src_addr} unsubscribed from data stream for device_id: {device_id}");

        Ok(())
    }

    async fn subscribe_to(
        target_addr: SocketAddr,
        device_id: DeviceId,
        handlers: Arc<Mutex<HashMap<u64, Box<DeviceHandler>>>>,
        local_data_tx: mpsc::Sender<(DataMsg, DeviceId)>,
    ) -> Result<(), AppError> {
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
            source_type: TCP,
            source,
            controller,
            adapter,
            output_to: vec![],
        };

        // This can be allowed to unwrap, as it will literally always succeed
        let mut new_handler = DeviceHandler::from_config(new_handler_config).await.unwrap();

        new_handler.start(local_data_tx).await?;

        info!("Handler created to subscribe to {target_addr}");

        handlers.lock().await.insert(device_id, new_handler);

        Ok(())
    }

    async fn unsubscribe_from(device_id: DeviceId, handlers: Arc<Mutex<HashMap<u64, Box<DeviceHandler>>>>) -> Result<(), AppError> {
        // TODO: Make it target specific, but for now removing based on device id should be fine.
        // Would require extracting the source itself from the device handler
        info!("Removing handler subscribing to {device_id}");
        match handlers.lock().await.remove(&device_id) {
            Some(mut handler) => {
                handler.stop().await?;
            }
            _ => info!("This handler does not exist."),
        }

        Ok(())
    }

    async fn configure(device_id: DeviceId, cfg_type: CfgType, handlers: Arc<Mutex<HashMap<u64, Box<DeviceHandler>>>>) -> Result<(), AppError> {
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

    async fn start(
        device_id: DeviceId,
        handlers: Arc<Mutex<HashMap<u64, Box<DeviceHandler>>>>,
        local_data_tx: mpsc::Sender<(DataMsg, DeviceId)>,
    ) -> Result<(), ExperimentError> {
        match handlers.lock().await.get_mut(&device_id) {
            Some(handler) => {
                info!("Starting device handler {device_id}");
                handler.start(local_data_tx.clone()).await?;
            }
            _ => {
                info!("There does not exist a device handler {device_id}")
            }
        }

        Ok(())
    }

    async fn start_all(
        handlers: Arc<Mutex<HashMap<u64, Box<DeviceHandler>>>>,
        local_data_tx: mpsc::Sender<(DataMsg, DeviceId)>,
    ) -> Result<(), ExperimentError> {
        info!("Starting all device handlers");
        for (device_id, handler) in handlers.lock().await.iter_mut() {
            info!("Starting device handler {device_id}");
            handler.start(local_data_tx.clone()).await?;
        }

        Ok(())
    }

    async fn stop(device_id: DeviceId, handlers: Arc<Mutex<HashMap<u64, Box<DeviceHandler>>>>) -> Result<(), ExperimentError> {
        match handlers.lock().await.get_mut(&device_id) {
            Some(handler) => {
                info!("Stopping device handler {device_id}");
                handler.stop().await?;
            }
            _ => {
                info!("There does not exist a device handler {device_id}")
            }
        }

        Ok(())
    }

    async fn stop_all(handlers: Arc<Mutex<HashMap<u64, Box<DeviceHandler>>>>) -> Result<(), ExperimentError> {
        info!("Stopping all device handlers");
        for (device_id, handler) in handlers.lock().await.iter_mut() {
            info!("Stopping device handler {device_id}");
            handler.stop().await?;
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
                    debug!("Routing data from device {device_id} to sink {sink_id}");
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
        info!("Started local stream handler task");
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
            RpcMessageKind::HostCtrl(command) => async move {
                match command {
                    // regular Host commands
                    HostCtrl::Connect => {
                        Self::connect(request.src_addr).await?;
                    }
                    HostCtrl::Disconnect => {
                        Self::disconnect(request.src_addr, send_channel_msg_channel).await?;
                    }
                    HostCtrl::Subscribe { device_id } => {
                        Self::subscribe(request.src_addr, *device_id, send_channel_msg_channel).await?;
                    }
                    HostCtrl::SubscribeAll => {
                        for device_id in self.handlers.lock().await.keys() {
                            Self::subscribe(request.src_addr, *device_id, send_channel_msg_channel.clone()).await?;
                        }
                    }
                    HostCtrl::Unsubscribe { device_id } => {
                        Self::unsubscribe(request.src_addr, *device_id, send_channel_msg_channel).await?;
                    }
                    HostCtrl::UnsubscribeAll => {
                        for device_id in self.handlers.lock().await.keys() {
                            Self::unsubscribe(request.src_addr, *device_id, send_channel_msg_channel.clone()).await?;
                        }
                    }
                    HostCtrl::SubscribeTo { target_addr, device_id } => {
                        Self::subscribe_to(*target_addr, *device_id, self.handlers.clone(), self.local_data_tx.clone()).await?;
                    }
                    HostCtrl::UnsubscribeFrom { target_addr: _, device_id } => {
                        Self::unsubscribe(request.src_addr, *device_id, send_channel_msg_channel).await?;
                    }
                    HostCtrl::StartExperiment { experiment } => {
                        self.experiment_send
                            .send(ExperimentChannelMsg::Start(experiment.clone(), send_channel_msg_channel.clone()))
                            .await?;
                    }
                    HostCtrl::StopExperiment => {
                        self.experiment_send.send(ExperimentChannelMsg::Stop).await?;
                    }
                    HostCtrl::Configure { device_id, cfg_type } => {
                        Self::configure(*device_id, cfg_type.clone(), self.handlers.clone()).await?;
                    }
                    HostCtrl::Start { device_id } => {
                        Self::start(*device_id, self.handlers.clone(), self.local_data_tx.clone()).await?;
                    }
                    HostCtrl::StartAll => {
                        Self::start_all(self.handlers.clone(), self.local_data_tx.clone()).await?;
                    }
                    HostCtrl::Stop { device_id } => {
                        Self::stop(*device_id, self.handlers.clone()).await?;
                    }
                    HostCtrl::StopAll => {
                        Self::stop_all(self.handlers.clone()).await?;
                    }
                    HostCtrl::Ping => {
                        debug!("Received ping from {:#?}.", request.src_addr);
                        send_channel_msg_channel.send(ChannelMsg::from(HostChannel::Pong))?;
                    }
                    m => {
                        warn!("Received unhandled HostCtrl: {m:?}");
                    }
                };
                Ok::<(), Box<dyn std::error::Error>>(())
            }
            .await
            .map_err(|err| NetworkError::ProcessingError(err.to_string()))?,
            RpcMessageKind::RegCtrl(command) => match command {
                RegCtrl::PollHostStatus { host_id: _ } => {
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
                        let status = self.get_host_status().await;
                        send_message(&mut send_stream, RpcMessageKind::RegCtrl(RegCtrl::HostStatus(status))).await?;
                    }
                    _ => error!("Received an unsupported channel message."),
                }
            }

            if !recv_data_channel.is_empty() {
                let (data_msg, device_id) = recv_data_channel.recv().await.unwrap();
                if subscribed_ids.contains(&device_id) {
                    debug!("Sending data {data_msg:?} for {device_id} to {send_stream:?}");
                    let msg = RpcMessageKind::Data { data_msg, device_id };
                    send_message(&mut send_stream, msg).await?;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use super::*;

    fn create_system_node_config(addr: SocketAddr, host_id: u64) -> SystemNodeConfig {
        SystemNodeConfig {
            address: addr,
            host_id,
            registry: None,
            registry_polling_interval: None,
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
