use std::collections::HashMap;
use std::env;
use std::net::SocketAddr;
use std::ops::Deref;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use argh::{CommandInfo, FromArgs};
use async_trait::async_trait;
use lib::FromConfig;
use lib::adapters::CsiDataAdapter;
use lib::csi_types::{Complex, CsiData};
use lib::errors::NetworkError;
use lib::handler::device_handler::{DeviceHandler, DeviceHandlerConfig};
use lib::network::rpc_message::CtrlMsg::*;
use lib::network::rpc_message::DataMsg::*;
use lib::network::rpc_message::RpcMessageKind::{Ctrl as RpcMessageKindCtrl, Data as RpcMessageKindData};
use lib::network::rpc_message::SourceType::*;
use lib::network::rpc_message::{AdapterMode, CtrlMsg, DataMsg, RpcMessage, RpcMessageKind, SourceType, make_msg};
use lib::network::tcp::client::TcpClient;
use lib::network::tcp::server::TcpServer;
use lib::network::tcp::{ChannelMsg, ConnectionHandler, SubscribeDataChannel, send_message};
use lib::network::*;
use lib::sinks::file::{FileConfig, FileSink};
use lib::sources::DataSourceT;
use lib::sources::controllers::Controller;
use lib::sources::controllers::esp32_controller::{
    Bandwidth as EspBandwidth, CsiType as EspCsiType, Esp32Controller, Esp32DeviceConfig, OperationMode as EspOperationMode,
    SecondaryChannel as EspSecondaryChannel,
};
use lib::sources::esp32::{Esp32Source, Esp32SourceConfig};
#[cfg(target_os = "linux")]
use lib::sources::netlink::NetlinkConfig;
use log::*;
use tokio::io::AsyncWriteExt;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::{Mutex, broadcast, watch};
use tokio::task::JoinHandle;

use crate::cli::*;
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
    send_data_channel: broadcast::Sender<DataMsg>,
}

impl SubscribeDataChannel for SystemNode {
    /// Creates a mew receiver for the System Nodes send data channel
    fn subscribe_data_channel(&self) -> broadcast::Receiver<DataMsg> {
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
            RpcMessageKindCtrl(command) => match command {
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
                Subscribe { device_id, mode } => {
                    // device_id and mode are unused for now
                    if send_channel.send(ChannelMsg::Subscribe).is_err() {
                        warn!("Failed to send Subscribe to own handle_send task; already closed?");
                        return Err(NetworkError::UnableToConnect);
                    }
                    info!("Client {} subscribed to data stream for device_id: {}", request.src_addr, device_id);
                }
                Unsubscribe { device_id } => {
                    // device_id is unused for now
                    if send_channel.send(ChannelMsg::Unsubscribe).is_err() {
                        warn!("Failed to send Unsubscribe to own handle_send task; already closed?");
                        return Err(NetworkError::UnableToConnect);
                    }
                    info!("Client {} unsubscribed from data stream for device_id: {}", request.src_addr, device_id);
                }
                m => {
                    warn!("Received unhandled CtrlMsg: {m:?}");
                    // todo!("{:?}", m); // Avoid panic on unhandled
                }
            },
            RpcMessageKindData {
                // SystemNode typically doesn't receive Data messages, it sends them.
                data_msg,
                device_id,
            } => {
                warn!("Received unexpected DataMsg: {data_msg:?} for device_id: {device_id}");
                // todo!();
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
        mut recv_data_channel: broadcast::Receiver<DataMsg>, // This is from the SystemNode's own broadcast
        mut send_stream: OwnedWriteHalf,
    ) -> Result<(), NetworkError> {
        let mut sending_active = false; // Renamed for clarity
        loop {
            tokio::select! {
                biased; // Prioritize command changes
                Ok(_) = recv_command_channel.changed() => {
                    let msg_opt = recv_command_channel.borrow_and_update().clone();
                    debug!("Received command {msg_opt:?} in handle_send");
                    match msg_opt {
                        ChannelMsg::Disconnect => {
                            // We don't send Disconnect message here usually,
                            // handle_recv signals this task to break by returning Err or closing channel.
                            // Or, if a Disconnect message must be sent to client:
                            // if send_message(&mut send_stream, Ctrl(CtrlMsg::Disconnect)).await.is_err() {
                            //     warn!("Failed to send Disconnect confirmation to client");
                            // }
                            debug!("Disconnect command received in handle_send, terminating send loop.");
                            return Ok(()); // Gracefully exit
                        }
                        ChannelMsg::Subscribe => {
                            info!("Subscription activated for client, will start sending data.");
                            sending_active = true;
                        }
                        ChannelMsg::Unsubscribe => {
                            info!("Subscription deactivated for client, will stop sending data.");
                            sending_active = false;
                        }
                        _ => (), // Other ChannelMsg types not relevant here
                    }
                }
                // Only try to receive from data channel if we are actively sending
                Ok(data_msg) = recv_data_channel.recv(), if sending_active => {
                    // TODO: device_id should ideally come from the DataMsg if it's heterogeneous,
                    // or be based on the subscription. For now, using a default.
                    let device_id = 0; // Assuming ESP32 is device 0
                    if tcp::send_message(
                        &mut send_stream,
                        RpcMessageKindData { data_msg, device_id },
                    ).await.is_err() {
                        warn!("Failed to send DataMsg to client, connection likely closed.");
                        return Err(NetworkError::UnableToConnect); // Propagate error to close connection
                    }
                    debug!("Sent DataMsg to client"); // Changed to debug to reduce log spam
                }
                // Break loop if recv_data_channel is closed and no longer sending.
                // recv() returns Err when channel is closed and empty.
                else => {
                    // This branch is taken if recv_data_channel.recv() errors (e.g. closed)
                    // or if !sending_active and the recv was skipped.
                    if sending_active { // If we were sending, an error on recv means the channel closed.
                        warn!("Data broadcast channel closed while subscribed. Terminating send loop.");
                        return Err(NetworkError::UnableToConnect);
                    }
                    // If not sending_active, we might just be waiting for commands.
                    // However, if recv_command_channel also closes, this select might livelock.
                    // A small yield or timeout can prevent tight loops if both conditions are inactive.
                    tokio::task::yield_now().await;
                }
            }
        }
        // Ok(()) // Loop is infinite unless broken by Disconnect or error
    }
}

impl Run<SystemNodeConfig> for SystemNode {
    fn new() -> Self {
        let (send_data_channel, _) = broadcast::channel::<DataMsg>(16); // Buffer size 16
        SystemNode { send_data_channel }
    }

    async fn run(&mut self, global_config: GlobalConfig, config: SystemNodeConfig) -> Result<(), Box<dyn std::error::Error>> {
        let connection_handler = Arc::new(self.clone());

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

        info!("ESP32 data reading task started.");

        // Start TCP server to handle client connections
        info!("Starting TCP server on {}...", config.addr);
        TcpServer::serve(config.addr, connection_handler).await;
        Ok(())
    }
}
