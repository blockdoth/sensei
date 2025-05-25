use crate::cli::*;
use crate::services::{GlobalConfig, Run, SystemNodeConfig};
use std::collections::HashMap;

use argh::{CommandInfo, FromArgs};
use async_trait::async_trait;
use lib::adapters::CsiDataAdapter;
use lib::csi_types::{Complex, CsiData};
use lib::errors::NetworkError;
use lib::network::rpc_message::{AdapterMode, CtrlMsg};
use lib::network::rpc_message::{CtrlMsg::*, DataMsg};
use lib::network::rpc_message::{DataMsg::*, make_msg};
use lib::network::rpc_message::{RpcMessage, SourceType};
use lib::network::rpc_message::{RpcMessageKind, SourceType::*};
use lib::network::tcp::client::TcpClient;
use lib::network::tcp::server::TcpServer;
use lib::network::tcp::{ChannelMsg, ConnectionHandler, SubscribeDataChannel, send_message};
use lib::sources::esp32::{Esp32Source, Esp32SourceConfig};

use lib::sources::controllers::Controller;
use lib::sources::controllers::esp32_controller::{
    Bandwidth as EspBandwidth, CsiType as EspCsiType, Esp32Controller, Esp32DeviceConfig,
    OperationMode as EspOperationMode, SecondaryChannel as EspSecondaryChannel,
};

use lib::network::*;
use lib::sources::DataSourceT;

#[cfg(target_os = "linux")]
use lib::sources::netlink::NetlinkConfig;
use log::*;
use std::env;
use std::net::SocketAddr;
use std::sync::Arc;

use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::AsyncWriteExt;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::{Mutex, broadcast, watch};
use tokio::task::JoinHandle;

use lib::network::rpc_message::RpcMessageKind::{
    Ctrl as RpcMessageKindCtrl, Data as RpcMessageKindData,
};

#[derive(Clone)]
pub struct SystemNode {
    send_data_channel: broadcast::Sender<DataMsg>,
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
                    info!(
                        "Client {} subscribed to data stream for device_id: {}",
                        request.src_addr, device_id
                    );
                }
                Unsubscribe { device_id } => {
                    // device_id is unused for now
                    if send_channel.send(ChannelMsg::Unsubscribe).is_err() {
                        warn!(
                            "Failed to send Unsubscribe to own handle_send task; already closed?"
                        );
                        return Err(NetworkError::UnableToConnect);
                    }
                    info!(
                        "Client {} unsubscribed from data stream for device_id: {}",
                        request.src_addr, device_id
                    );
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

    async fn run(
        &mut self,
        global_config: GlobalConfig,
        config: SystemNodeConfig,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let connection_handler = Arc::new(self.clone());
        let sender_data_channel_clone = self.send_data_channel.clone(); // Clone for the source reading task

        // --- ESP32 Setup ---
        info!("Setting up ESP32 source...");
        let esp_source_config = Esp32SourceConfig {
            port_name: "/dev/cu.usbmodem2101".to_string(), // Using the port from your example run
            baud_rate: 3_000_000,
            csi_buffer_size: 4096,
            ack_timeout_ms: 1000,
        };

        let port_name_for_log = esp_source_config.port_name.clone();

        // Create the ESP32 source instance
        let mut esp_source_instance = Esp32Source::new(esp_source_config)
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

        // Start the ESP32 source (connects and starts its reader thread)
        esp_source_instance
            .start()
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
        info!("ESP32 Source started on port {port_name_for_log}");

        // Wrap in Box<dyn DataSourceT> for storage
        let esp_source_boxed: Box<dyn DataSourceT> = Box::new(esp_source_instance);

        // --- Adapter Setup ---
        // Direct instantiation for minimal change, assuming ESP32Adapter is in lib::adapters::esp32
        let mut esp_adapter =
            Box::new(lib::adapters::esp32::ESP32Adapter::new(false)) as Box<dyn CsiDataAdapter>;
        info!("ESP32 Adapter created.");

        // --- Device Management ---
        let devices: Arc<Mutex<HashMap<u64, Box<dyn DataSourceT>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        devices.lock().await.insert(0, esp_source_boxed); // ESP32 is device 0
        info!("ESP32 source added to managed devices with ID 0.");

        // --- Initial ESP32 Configuration by SystemNode ---
        info!("Sending initial configuration to ESP32 (ID 0) to unpause acquisition...");
        {
            // Scope for devices_guard
            let mut devices_guard = devices.lock().await;
            if let Some(device_source_dyn) = devices_guard.get_mut(&0) {
                // We have &mut Box<dyn DataSourceT>. We need &mut dyn DataSourceT for apply.
                // Dereferencing the Box gives us &mut dyn DataSourceT.
                let controller = Esp32Controller {
                    device_config: Esp32DeviceConfig::default(),
                    mac_filters_to_add: todo!(),
                    clear_all_mac_filters: todo!(),
                    control_acquisition: todo!(),
                    control_wifi_transmit: todo!(),
                    synchronize_time: todo!(),
                    transmit_custom_frame: todo!(),
                };

                match controller.apply(&mut **device_source_dyn).await {
                    Ok(_) => info!(
                        "Initial configuration (Unpause CSI Acquisition) sent to ESP32 (ID 0) successfully."
                    ),
                    Err(e) => error!("Failed to send initial configuration to ESP32 (ID 0): {e}"),
                }
            } else {
                error!(
                    "Could not get ESP32 source (ID 0) from devices map for initial configuration."
                );
            }
        } // devices_guard is dropped here

        // --- Data Reading and Processing Task ---
        // This task reads from the ESP32, adapts the data, and broadcasts it.
        // --- Data Reading and Processing Task ---
        // This task reads from the ESP32, adapts the data, and broadcasts it.
        tokio::spawn(async move {
            let mut read_buffer = vec![0u8; 4096]; // Buffer for reading raw data
            info!("Data reading task: Started."); // LOG: Task started

            loop {
                let mut devices_guard = devices.lock().await;
                if let Some(source_device) = devices_guard.get_mut(&0) {
                    // ESP32 is device 0
                    match source_device.read(&mut read_buffer).await {
                        Ok(bytes_read) => {
                            if bytes_read > 0 {
                                // LOG: Data received from ESP32 source
                                info!(
                                    "Data reading task: Read {bytes_read} bytes from ESP32 source."
                                );
                                // For more detail (can be very verbose):
                                // trace!("Data reading task: Raw bytes: {:?}", &read_buffer[..bytes_read]);

                                let raw_data_slice = &read_buffer[..bytes_read];

                                let current_timestamp = SystemTime::now()
                                    .duration_since(UNIX_EPOCH)
                                    .unwrap_or_else(|e| {
                                        warn!("SystemTime before UNIX EPOCH: {e}");
                                        Duration::ZERO
                                    })
                                    .as_secs_f64();

                                let raw_frame_msg = DataMsg::RawFrame {
                                    ts: current_timestamp,
                                    bytes: raw_data_slice.to_vec(),
                                    source_type: SourceType::ESP32,
                                };

                                /*
                                pub enum DataMsg {
                                    RawFrame {
                                            ts: f64,
                                            bytes: Vec<u8>,
                                            source_type: SourceType,
                                        }, // raw bytestream, requires decoding adapter
                                        CsiFrame {
                                            csi: CsiData,
                                        }, // This would contain a proper deserialized CSI
                                    }
                                 */
                                // LOG: RawFrame created
                                debug!(
                                    "Data reading task: Created Frame: {}",
                                    match &raw_frame_msg {
                                        DataMsg::RawFrame {
                                            ts,
                                            bytes,
                                            source_type,
                                        } => {
                                            format!(
                                                "Timestamp: {:.3}, Bytes: {}, SourceType: {:?}",
                                                ts,
                                                bytes.len(),
                                                source_type
                                            )
                                        }
                                        _ => "Invalid Frame".to_string(),
                                    }
                                );

                                match esp_adapter.produce(raw_frame_msg).await {
                                    Ok(Some(ref csi_data_msg @ DataMsg::CsiFrame { ref csi })) => {
                                        // LOG: CSI Frame produced by adapter
                                        info!(
                                            "Data reading task: ESP32Adapter produced CsiFrame. Timestamp: {:.3}, Seq: {}, RSSI: {:?}, Subcarriers: {}",
                                            csi.timestamp,
                                            csi.sequence_number,
                                            csi.rssi,
                                            csi.csi
                                                .first()
                                                .and_then(|tx| tx.first().map(|rx| rx.len()))
                                                .unwrap_or(0) // Get subcarrier count safely
                                        );

                                        if sender_data_channel_clone
                                            .send(csi_data_msg.clone())
                                            .is_err()
                                        {
                                            debug!(
                                                "Data reading task: No active subscribers for ESP32 CSI data."
                                            );
                                        } else {
                                            info!(
                                                "Data reading task: Broadcasted CsiFrame to client handlers."
                                            );
                                        }
                                    }
                                    Ok(Some(unexpected_msg)) => {
                                        warn!(
                                            "Data reading task: ESP32Adapter produced an unexpected DataMsg variant: {unexpected_msg:?}"
                                        );
                                    }
                                    Ok(None) => {
                                        info!(
                                            "Data reading task: ESP32Adapter produced None (e.g., incomplete data, non-CSI packet, or waiting for more bytes)."
                                        );
                                    }
                                    Err(e) => {
                                        error!(
                                            "Data reading task: ESP32Adapter failed to produce CSI data: {e:?}"
                                        );
                                    }
                                }
                            } else {
                                // No data read in this cycle, this is normal if ESP32 isn't sending
                                trace!(
                                    "Data reading task: No data read from ESP32 source in this cycle (bytes_read == 0)."
                                );
                            }
                        }
                        Err(e) => {
                            error!(
                                "Data reading task: Error reading from ESP32 source (ID 0): {e:?}"
                            );
                            tokio::time::sleep(Duration::from_secs(1)).await; // Backoff on error
                        }
                    }
                } else {
                    warn!("Data reading task: ESP32 source (ID 0) not found in devices map.");
                    tokio::time::sleep(Duration::from_secs(1)).await; // Wait if device not found
                }
                drop(devices_guard); // Release the lock before sleeping
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });

        info!("ESP32 data reading task started.");

        // Start TCP server to handle client connections
        info!("Starting TCP server on {}...", config.addr);
        TcpServer::serve(config.addr, connection_handler).await;
        Ok(())
    }
}
