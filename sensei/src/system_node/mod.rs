use std::collections::{HashMap, HashSet};
use std::env;
use std::net::SocketAddr;
use std::ops::Deref;
use std::path::PathBuf;
use std::sync::Arc;
// use std::thread::sleep; // Not used
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use argh::{CommandInfo, FromArgs};
use async_trait::async_trait;
use lib::FromConfig;
// use lib::FromConfig; // Not using FromConfig for adapter to keep changes minimal here
use lib::adapters::CsiDataAdapter; // Removed esp32 module import here, will use full path
use lib::csi_types::{Complex, CsiData};
use lib::errors::NetworkError;
use lib::handler::device_handler::{DeviceHandler, DeviceHandlerConfig};
use lib::network::rpc_message::CtrlMsg::*;
use lib::network::rpc_message::DataMsg::*;
use lib::network::rpc_message::RpcMessageKind::{Ctrl as RpcMessageKindCtrl, Ctrl, Data as RpcMessageKindData, Data};
use lib::network::rpc_message::SourceType::*;
use lib::network::rpc_message::{AdapterMode, CtrlMsg, DataMsg, RpcMessage, SourceType, make_msg};
use lib::network::tcp::client::TcpClient;
use lib::network::tcp::server::TcpServer;
use lib::network::tcp::{ChannelMsg, ConnectionHandler, SubscribeDataChannel, send_message};
use lib::network::*;
use lib::sinks::file::{FileConfig, FileSink};
use lib::sources::DataSourceT;
use lib::sources::controllers::Controller; // For the .apply() method
use lib::sources::controllers::esp32_controller::{
    Bandwidth as EspBandwidth,               // Enum for bandwidth
    CsiType as EspCsiType,                   // Enum for CSI type
    Esp32ControllerParams,                   // The parameters struct
    Esp32DeviceConfigPayload,                // Payload for device config
    OperationMode as EspOperationMode,       // Enum for operation mode
    SecondaryChannel as EspSecondaryChannel, // Enum for secondary channel
};
use lib::sources::esp32::{Esp32Source, Esp32SourceConfig};
// use lib::sources::csv::{CsvConfig, CsvSource}; // Removed CSV source
#[cfg(target_os = "linux")]
use lib::sources::netlink::NetlinkConfig; // Keep for conditional compilation
use log::*;
use tokio::io::AsyncWriteExt;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::{Mutex, broadcast, watch};
use tokio::task::JoinHandle;

use crate::cli::*;
// use crate::cli::{SubCommandsArgs, SystemNodeSubcommandArgs}; // SystemNodeSubcommandArgs not used here
use crate::config::{OrchestratorConfig, SystemNodeConfig};
use crate::module::*;

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
    send_data_channel: broadcast::Sender<(DataMsg, u64)>, // Call .subscribe() on the sender in order to get a receiver
    client: Arc<Mutex<TcpClient>>,
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
    async fn handle_recv(&self, request: RpcMessage, send_channel: watch::Sender<ChannelMsg>) -> Result<(), NetworkError> {
        info!("Received message {:?} from {}", request.msg, request.src_addr);
        match request.msg {
            Ctrl(command) => match command {
                Connect => {
                    let src = request.src_addr;
                    info!("Started connection with {src}");
                }
                Disconnect => {
                    // Correct way to signal disconnect to the sending task for this connection
                    if send_channel.send(ChannelMsg::Disconnect).is_err() {
                        warn!("Failed to send Disconnect to own handle_send task; already closed?");
                    }
                    return Err(NetworkError::Closed); // Indicate connection should close
                }
                Subscribe { device_id} => {
                    // device_id and mode are unused for now
                    if send_channel.send(ChannelMsg::Subscribe { device_id }).is_err() {
                        warn!("Failed to send Subscribe to own handle_send task; already closed?");
                        return Err(NetworkError::UnableToConnect);
                    }

                    info!("Client {} subscribed to data stream for device_id: {}", request.src_addr, device_id);
                }
                Unsubscribe { device_id } => {
                    // device_id is unused for now
                    if send_channel.send(ChannelMsg::Unsubscribe { device_id }).is_err() {
                        warn!("Failed to send Unsubscribe to own handle_send task; already closed?");
                        return Err(NetworkError::UnableToConnect);
                    }
                    info!("Client {} unsubscribed from data stream for device_id: {}", request.src_addr, device_id);
                }
                SubscribeTo { target, device_id} => {
                    let msg = Ctrl(Subscribe { device_id });

                    self.client.lock().await.connect(target);

                    self.client.lock().await.send_message(target, msg);
                }
                UnsubscribeFrom { target, device_id } => {
                    let msg = Ctrl(Unsubscribe { device_id });

                    self.client.lock().await.send_message(target, msg);

                    self.client.lock().await.disconnect(target);
                }
                Configure { device_id, cfg } => {}
                PollDevices => {}
                Heartbeat => {}
            },
            Data {
                // SystemNode typically doesn't receive Data messages, it sends them.
                data_msg,
                device_id,
            } => {
                self.send_data_channel.send((data_msg, device_id));
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
        mut recv_data_channel: broadcast::Receiver<(DataMsg, u64)>,
        mut send_stream: OwnedWriteHalf,
    ) -> Result<(), NetworkError> {
        let mut subscribed_ids: HashSet<u64> = HashSet::new();
        loop {
            if recv_command_channel.has_changed().unwrap_or(false) {
                let msg_opt = recv_command_channel.borrow_and_update().clone();
                debug!("Received message {msg_opt:?} over channel");
                match msg_opt {
                    ChannelMsg::Disconnect => {
                        send_message(&mut send_stream, Ctrl(CtrlMsg::Disconnect)).await;
                        debug!("Send close confirmation");
                        break;
                    }
                    ChannelMsg::Subscribe { device_id  } => {
                        info!("Subscribed");
                        subscribed_ids.insert(device_id);
                    }
                    ChannelMsg::Unsubscribe { device_id } => {
                        info!("Unsubscribed");
                        subscribed_ids.remove(&device_id);
                    }
                    _ => (),
                }
            }

            if !recv_data_channel.is_empty() {
                let (data_msg, device_id) = recv_data_channel.recv().await.unwrap();

                for subscribed_id in subscribed_ids.clone() {
                    if (subscribed_id == device_id) {
                        // TODO: Use the tcp sink and stuff to implement this with adapter mode
                        let msg = Data {
                            data_msg: data_msg.clone(),
                            device_id,
                        };
                        send_message(&mut send_stream, msg);
                    }
                }
            }
        }
        Ok(())
    }
}

impl Run<SystemNodeConfig> for SystemNode {
    fn new(config: SystemNodeConfig) -> Self {
        let (send_data_channel, _) = broadcast::channel::<(DataMsg, u64)>(16);
        SystemNode {
            send_data_channel,
            client: Arc::new(Mutex::new(TcpClient::new())),
        }
    }

    /// Starts the system node
    ///
    /// # Arguments
    ///
    /// SystemNodeConfig: Specifies the target address
    async fn run(&self, config: SystemNodeConfig) -> Result<(), Box<dyn std::error::Error>> {
        let connection_handler = Arc::new(self.clone());

        let send_client = self.client.clone();
        let recv_client = self.client.clone();

        let sender_data_channel = connection_handler.send_data_channel.clone();

        let default_config_path: PathBuf = config.device_configs;

        let device_handler_configs: Vec<DeviceHandlerConfig> = DeviceHandlerConfig::from_yaml(default_config_path).await;

        let handlers: Arc<Mutex<HashMap<u64, Box<DeviceHandler>>>> = Arc::new(Mutex::new(HashMap::new()));

        for cfg in device_handler_configs {
            handlers
                .lock()
                .await
                .insert(cfg.device_id, DeviceHandler::from_config(cfg.clone()).await.unwrap());
        }

        TcpServer::serve(config.addr, connection_handler).await;
        Ok(())
    }
}
