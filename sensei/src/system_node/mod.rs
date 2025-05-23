use crate::cli::*;
use crate::cli::{SubCommandsArgs, SystemNodeSubcommandArgs};
use crate::config::{OrchestratorConfig, SystemNodeConfig};
use crate::module::*;
use std::collections::HashMap;

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
    send_data_channel: broadcast::Sender<DataMsg>, // Call .subscribe() on the sender in order to get a receiver
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
                    send_channel.send(ChannelMsg::Subscribe);

                    info!("Subscribed to data stream");
                }
                Unsubscribe { device_id } => {
                    send_channel.send(ChannelMsg::Unsubscribe);
                    println!("Unsubscribed from data stream");
                }
                m => {
                    todo!("{:?}", m);
                }
            },
            Data {
                data_msg,
                device_id,
            } => todo!(),
        }
        Ok(())
    }

    /// Handles sending messages for the nodes to other receivers in the network. 
    /// 
    /// The node will only send messages to subscribers of relevant messages.
    async fn handle_send(
        &self,
        mut recv_command_channel: watch::Receiver<ChannelMsg>,
        mut recv_data_channel: broadcast::Receiver<DataMsg>,
        mut send_stream: OwnedWriteHalf,
    ) -> Result<(), NetworkError> {
        let mut sending = false;
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
                    ChannelMsg::Subscribe => {
                        info!("Subscribed");
                        sending = true;
                    }
                    ChannelMsg::Unsubscribe => {
                        info!("Unsubscribed");
                        sending = false;
                    }
                    _ => (),
                }
            }

            if sending {
                let Ok(data_msg) = recv_data_channel.recv().await else {
                    todo!()
                };
                let device_id = 0;
                tcp::send_message(
                    &mut send_stream,
                    Data {
                        data_msg,
                        device_id,
                    },
                )
                .await;
                info!("Sending")
            }
        }
        Ok(())
    }
}

impl Run<SystemNodeConfig> for SystemNode {
    fn new(config: SystemNodeConfig) -> Self {
        let (send_data_channel, _) = broadcast::channel::<DataMsg>(16);
        SystemNode { send_data_channel }
    }

    /// Starts the system node
    /// 
    /// # Arguments
    /// 
    /// SystemNodeConfig: Specifies the target address
    async fn run(&self, config: SystemNodeConfig) -> Result<(), Box<dyn std::error::Error>> {
        let connection_handler = Arc::new(self.clone());

        let sender_data_channel = connection_handler.send_data_channel.clone();

        let default_config_path: PathBuf = config.device_configs;

        let device_handler_configs: Vec<DeviceHandlerConfig> =
            DeviceHandlerConfig::from_yaml(default_config_path).await;

        let handlers: Vec<Box<DeviceHandler>> = futures::future::join_all(device_handler_configs.iter().map(|x| {
            async {
                DeviceHandler::from_config(x.clone()).await.unwrap()
            }
        })).await;

        TcpServer::serve(config.addr, connection_handler).await;
        Ok(())
    }
}
