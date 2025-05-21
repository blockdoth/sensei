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
use lib::network::rpc_message::SourceType::*;
use lib::network::rpc_message::{AdapterMode, CtrlMsg};
use lib::network::rpc_message::{CtrlMsg::*, DataMsg};
use lib::network::rpc_message::{DataMsg::*, make_msg};
use lib::network::rpc_message::{RpcMessage, SourceType};
use lib::network::tcp::client::TcpClient;
use lib::network::tcp::server::TcpServer;
use lib::network::tcp::{ChannelMsg, ConnectionHandler, SubscribeDataChannel, send_message};
use lib::network::*;
use lib::sources::DataSourceT;
use lib::sources::csv::{CsvConfig, CsvSource};
use lib::sources::netlink::NetlinkConfig;
use log::*;
use std::env;
use std::net::SocketAddr;
use std::sync::Arc;
use std::thread::sleep;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::AsyncWriteExt;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::{Mutex, broadcast, watch};
use tokio::task::JoinHandle;

#[derive(Clone)]
pub struct SystemNode {
    send_data_channel: broadcast::Sender<DataMsg>, // Call .subscribe() on the sender in order to get a receiver
}

impl SubscribeDataChannel for SystemNode {
    fn subscribe_data_channel(&self) -> broadcast::Receiver<DataMsg> {
        self.send_data_channel.subscribe()
    }
}

#[async_trait]
impl ConnectionHandler for SystemNode {
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

    async fn run(&self, config: SystemNodeConfig) -> Result<(), Box<dyn std::error::Error>> {
        let connection_handler = Arc::new(self.clone());

        let sender_data_channel = connection_handler.send_data_channel.clone();

        // TODO: For every possible device, acquire a configuration
        // TODO: Also acquire an adapter configuration

        let csv_config = CsvConfig {
            path: "resources/test_data/csv/csi_data.csv".parse().unwrap(),
            cell_delimiter: b',',
            row_delimiter: b'\n',
            header: true,
            delay: 50,
        };
        let csv_adapter_config = DataAdapterConfig::CSV {};
        let mut csv_adapter = <dyn CsiDataAdapter>::from_config(csv_adapter_config).await?;

        let iwl_config = NetlinkConfig {
            group: 0,
        };
        let iwl_adapter_config = DataAdapterConfig::Iwl {
            scale_csi: true,
        };

        let devices: Arc<Mutex<HashMap<u64, Box<dyn DataSourceT>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        // TODO: Store devices onto node from a configuration

        let csv_source = Box::new(CsvSource::new(csv_config).unwrap());

        devices.lock().await.insert(0, csv_source);

        // TODO: Start the read processes (tokio task per device?) and send read data to main thread

        // Example sender which just spams packets
        // The most important thing is the ability to clone send ends of channels around
        tokio::spawn(async move {
            let mut i = 0;
            loop {
                let mut output_buf: &mut [u8] = &mut [0u8; 4096];

                for mut device in devices.lock().await.iter_mut() {
                    let test_out = device.1.read(output_buf).await.unwrap();
                    let test_data_msg = RawFrame {
                        ts: 0f64,
                        bytes: Vec::from(&mut *output_buf),
                        source_type: SourceType::CSV,
                    };
                    let test_out_csi = csv_adapter.produce(test_data_msg).await.unwrap();
                    sender_data_channel.send(test_out_csi.unwrap());
                }
                tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
            }
        });

        TcpServer::serve(config.addr, connection_handler).await;
        Ok(())
    }
}
