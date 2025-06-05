use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use lib::FromConfig;
use lib::errors::NetworkError;
use lib::handler::device_handler::CfgType::{Create, Delete, Edit};
use lib::handler::device_handler::{DeviceHandler, DeviceHandlerConfig};
use lib::network::rpc_message::CtrlMsg::*;
use lib::network::rpc_message::RpcMessageKind::{Ctrl, Data};
use lib::network::rpc_message::SourceType::*;
use lib::network::rpc_message::{CtrlMsg, DataMsg, DeviceId, RpcMessage};
use lib::network::tcp::client::TcpClient;
use lib::network::tcp::server::TcpServer;
use lib::network::tcp::{ChannelMsg, ConnectionHandler, SubscribeDataChannel, send_message};
use lib::sources::DataSourceConfig;
#[cfg(target_os = "linux")]
use lib::sources::tcp::TCPConfig;
use log::*;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::sync::{Mutex, broadcast, watch};

use crate::services::{GlobalConfig, Run, SystemNodeConfig};

/// The System Node is a sender and a receiver in the network of Sensei.
/// It hosts the devices that send and receive CSI data, and is responsible for sending this data further to other receivers in the system.
///
/// Devices can "ask" for the CSI data generated at a System Node by sending it a Subscribe message, specifying the device id.
///
/// The System Node can also adapt this CSI data to our universal standard, which lets it be used by other parts of Sensei, like the Visualiser.
///
/// # Arguments
///
/// send_data_channel: the System Node communicates which data should be sent to other receivers across its threads using this tokio channel
#[derive(Clone)]
pub struct SystemNode {
    send_data_channel: broadcast::Sender<(DataMsg, DeviceId)>, // Call .subscribe() on the sender in order to get a receiver
    handlers: Arc<Mutex<HashMap<u64, Box<DeviceHandler>>>>,
    addr: SocketAddr,
    host_id: u64,
    registry_addr: Option<SocketAddr>,
    device_configs: Vec<DeviceHandlerConfig>,
}

impl SubscribeDataChannel for SystemNode {
    /// Creates a mew receiver for the System Nodes send data channel
    fn subscribe_data_channel(&self) -> broadcast::Receiver<(DataMsg, u64)> {
        self.send_data_channel.subscribe()
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
        info!("Received message {:?} from {}", request.msg, request.src_addr);
        match request.msg {
            Ctrl(command) => match command {
                Connect => {
                    let src = request.src_addr;
                    info!("Started connection with {src}");
                }
                Disconnect => {
                    // Correct way to signal disconnect to the sending task for this connection
                    send_channel_msg_channel.send(ChannelMsg::Disconnect)?;
                    return Err(NetworkError::Closed); // Indicate connection should close
                }
                Subscribe { device_id } => {
                    send_channel_msg_channel.send(ChannelMsg::Subscribe { device_id })?;
                    info!("Client {} subscribed to data stream for device_id: {}", request.src_addr, device_id);
                }
                Unsubscribe { device_id } => {
                    send_channel_msg_channel.send(ChannelMsg::Unsubscribe { device_id })?;
                    info!("Client {} unsubscribed from data stream for device_id: {}", request.src_addr, device_id);
                }
                SubscribeTo { target, device_id } => {
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
                UnsubscribeFrom { target: _, device_id } => {
                    // TODO: Make it target specific, but for now removing based on device id should be fine.
                    // Would require extracting the source itself from the device handler
                    info!("Removing handler subscribing to {device_id}");
                    match self.handlers.lock().await.remove(&device_id) {
                        Some(mut handler) => handler.stop().await.expect("Whoopsy"),
                        _ => info!("This handler does not exist."),
                    }
                }
                PollHostStatus { host_id } => {
                    let reg_addr = request.src_addr;
                    info!("Received PollDevices from {reg_addr}");
                    send_channel_msg_channel.send(ChannelMsg::SendHostStatus { reg_addr, host_id })?
                }
                Configure { device_id, cfg_type } => match cfg_type {
                    Create { cfg } => {
                        info!("Creating a new device handler for device id {device_id}");
                        let handler = DeviceHandler::from_config(cfg).await.unwrap();
                        self.handlers.lock().await.insert(device_id, handler);
                    }
                    Edit { cfg } => {
                        info!("Editing existing device handler for device id {device_id}");
                        match self.handlers.lock().await.get_mut(&device_id) {
                            Some(handler) => handler.reconfigure(cfg).await.expect("Whoopsy"),
                            _ => info!("This handler does not exist."),
                        }
                    }
                    Delete => {
                        info!("Deleting device handler for device id {device_id}");
                        match self.handlers.lock().await.remove(&device_id) {
                            Some(mut handler) => handler.stop().await.expect("Whoopsy"),
                            _ => info!("This handler does not exist."),
                        }
                    }
                },
                m => {
                    warn!("Received unhandled CtrlMsg: {m:?}");
                }
            },
            Data { data_msg, device_id } => {
                // TODO: Pass it through relevant TCP sources
                self.send_data_channel.send((data_msg, device_id))?;
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
        let mut subscribed_ids: HashSet<u64> = HashSet::new();
        loop {
            if recv_command_channel.has_changed().unwrap_or(false) {
                let msg_opt = recv_command_channel.borrow_and_update().clone();
                debug!("Received message {msg_opt:?} over channel");
                match msg_opt {
                    ChannelMsg::Disconnect => {
                        send_message(&mut send_stream, Ctrl(CtrlMsg::Disconnect)).await?;
                        debug!("Send close confirmation");
                        break;
                    }
                    ChannelMsg::Subscribe { device_id } => {
                        info!("Subscribed");
                        subscribed_ids.insert(device_id);
                    }
                    ChannelMsg::Unsubscribe { device_id } => {
                        info!("Unsubscribed");
                        subscribed_ids.remove(&device_id);
                    }
                    ChannelMsg::SendHostStatus { reg_addr: _, host_id: _ } => {
                        let host_status = HostStatus {
                            host_id: self.host_id,
                            device_status: vec![],
                        };
                        let msg = Ctrl(host_status);
                        send_message(&mut send_stream, msg).await?;
                    }
                    _ => (),
                }
            }

            if !recv_data_channel.is_empty() {
                let (data_msg, device_id) = recv_data_channel.recv().await.unwrap();

                info!("Sending data {data_msg:?} for {device_id} to {send_stream:?}");
                let msg = Data { data_msg, device_id };

                send_message(&mut send_stream, msg).await?;
            }
        }
        // Loop is infinite unless broken by Disconnect or error
        Ok(())
    }
}

impl Run<SystemNodeConfig> for SystemNode {
    fn new(global_config: GlobalConfig, config: SystemNodeConfig) -> Self {
        let (send_data_channel, _) = broadcast::channel::<(DataMsg, DeviceId)>(16);

        SystemNode {
            send_data_channel,
            handlers: Arc::new(Mutex::new(HashMap::new())),
            addr: config.addr,
            host_id: 0,
            registry_addr: config.registry,
            device_configs: config.device_configs,
        }
    }

    /// Starts the system node
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

        if let Some(registry) = &self.registry_addr {
            info!("Connecting to registry at {registry}");
            let registry_addr: SocketAddr = *registry;
            let heartbeat_msg = Ctrl(CtrlMsg::AnnouncePresence {
                host_id: self.host_id,
                host_address: self.addr,
            });
            let mut client = TcpClient::new();
            client.connect(registry_addr).await?;
            client.send_message(registry_addr, heartbeat_msg).await?;
            client.disconnect(registry_addr);
            info!("Heartbeat sent to registry at {registry_addr}");
        }

        // Start TCP server to handle client connections
        info!("Starting TCP server on {}...", self.addr);
        TcpServer::serve(self.addr, connection_handler).await?;
        Ok(())
    }
}
