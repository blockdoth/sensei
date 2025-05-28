use crate::cli::*;
use crate::cli::{SubCommandsArgs, SystemNodeSubcommandArgs};
use crate::config::{OrchestratorConfig, SystemNodeConfig};
use crate::module::*;
use std::collections::{HashMap, HashSet};

use crate::system_node::rpc_message::RpcMessageKind::*;
use argh::{CommandInfo, FromArgs};
use async_trait::async_trait;
use lib::FromConfig;
use lib::adapters::{CsiDataAdapter, DataAdapterConfig};
use lib::csi_types::{Complex, CsiData};
use lib::errors::NetworkError;
use lib::handler::device_handler::{DeviceHandler, DeviceHandlerConfig};
use lib::network::rpc_message::SourceType::*;
use lib::network::rpc_message::{AdapterMode, CtrlMsg};
use lib::network::rpc_message::{CtrlMsg::*, DataMsg};
use lib::network::rpc_message::{DataMsg::*, make_msg};
use lib::network::rpc_message::{RpcMessage, SourceType};
use lib::network::tcp::client::TcpClient;
use lib::network::tcp::server::TcpServer;
use lib::network::tcp::{ChannelMsg, ConnectionHandler, SubscribeDataChannel, send_message};
use lib::network::*;
use lib::sinks::file::{FileConfig, FileSink};
use lib::sources::DataSourceT;
use lib::sources::csv::{CsvConfig, CsvSource};
#[cfg(target_os = "linux")]
use lib::sources::netlink::NetlinkConfig;
use log::*;
use std::env;
use std::net::SocketAddr;
use std::ops::Deref;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::sleep;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::AsyncWriteExt;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::{Mutex, broadcast, watch};
use tokio::task::JoinHandle;

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
    async fn handle_recv(
        &self,
        request: RpcMessage,
        send_channel: watch::Sender<ChannelMsg>,
    ) -> Result<(), NetworkError> {
        info!(
            "Received message {:?} from {}",
            request.msg, request.src_addr
        );
        match request.msg {
            Ctrl(command) => match command {
                Connect => {
                    let src = request.src_addr;
                    info!("Started connection with {src}");
                }
                Disconnect => {
                    send_channel.send(ChannelMsg::Disconnect);
                    return Err(NetworkError::Closed);
                }
                Subscribe { device_id, mode } => {
                    send_channel.send(ChannelMsg::Subscribe { device_id, mode });

                    info!("Subscribed to data stream");
                }
                Unsubscribe { device_id } => {
                    send_channel.send(ChannelMsg::Unsubscribe { device_id });
                    println!("Unsubscribed from data stream");
                }
                SubscribeTo {
                    target,
                    device_id,
                    mode,
                } => {
                    let msg = Ctrl(Subscribe { device_id, mode });

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
        let mut subscribed_ids: HashMap<u64, AdapterMode> = HashMap::new();
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
                    ChannelMsg::Subscribe { device_id, mode } => {
                        info!("Subscribed");
                        subscribed_ids.entry(device_id).or_insert(mode);
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

                for (subscribed_id, mode) in subscribed_ids.clone() {
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

            // if sending {
            //     let Ok(data_msg) = recv_data_channel.recv().await else {
            //         todo!()
            //     };
            //     let device_id = 0;
            //     tcp::send_message(
            //         &mut send_stream,
            //         Data {
            //             data_msg,
            //             device_id,
            //         },
            //     )
            //     .await;
            //     info!("Sending")
            // }
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

        let device_handler_configs: Vec<DeviceHandlerConfig> =
            DeviceHandlerConfig::from_yaml(default_config_path).await;

        let handlers: Arc<Mutex<HashMap<u64, Box<DeviceHandler>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        for cfg in device_handler_configs {
            handlers.lock().await.insert(
                cfg.device_id,
                DeviceHandler::from_config(cfg.clone()).await.unwrap(),
            );
        }

        TcpServer::serve(config.addr, connection_handler).await;
        Ok(())
    }
}
