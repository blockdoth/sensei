pub mod state;
pub mod tui;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use lib::csi_types::Complex as LibComplex; // Alias to avoid confusion
use lib::csi_types::CsiData;
use lib::errors::NetworkError;
use lib::network::rpc_message::DataMsg::*;
use lib::network::rpc_message::RpcMessageKind::Data;
use lib::network::rpc_message::{DeviceId, HostCtrl, RpcMessage, RpcMessageKind};
use lib::network::tcp::client::TcpClient;
use lib::tui::TuiRunner;
use log::{LevelFilter, debug, error, info};
use rustfft::FftPlanner;
use rustfft::num_complex::Complex as FftComplex;
use tokio::sync::Mutex;
use tokio::sync::mpsc::{
    Receiver, Sender, {self},
};
use tokio::time::sleep;

use crate::cli::GraphConfigWithId;
use crate::services::{GlobalConfig, Run, VisualiserConfig};
use crate::visualiser::state::{AmplitudeConfig, Graph, GraphConfig, PDPConfig, VisCommand, VisState, VisUpdate};

const ACTOR_CHANNEL_CAPACITY: usize = 100;
const _DECAY_RATE: f64 = 0.9;
const _STALE_THRESHOLD: Duration = Duration::from_millis(200);
const _MIN_POWER_THRESHOLD: f64 = 0.015;
pub struct Visualiser {
    target: SocketAddr,
    log_level: LevelFilter,
    graph_update_interval: usize,
    graph_configs: Vec<GraphConfigWithId>,
}

impl Run<VisualiserConfig> for Visualiser {
    fn new(global_config: GlobalConfig, config: VisualiserConfig) -> Self {
        Visualiser {
            target: config.target,
            log_level: global_config.log_level,
            graph_configs: config.graphs,
            graph_update_interval: config.update_interval,
        }
    }

    async fn run(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        // Technically, the visualiser has cli tools for connecting to multiple nodes
        // At the moment, it is sufficient to connect to one target node on startup
        // Manually start the subscription by typing subscribe

        let (command_send, command_recv) = mpsc::channel::<VisCommand>(ACTOR_CHANNEL_CAPACITY);
        let (update_send, update_recv) = mpsc::channel::<VisUpdate>(ACTOR_CHANNEL_CAPACITY);

        let mut vis_state = VisState::new(self.graph_update_interval);
        vis_state.graphs.extend(self.graph_configs.iter().cloned().map(Graph::from));
        vis_state.addr_input = self.target.to_string();
        let tasks: Vec<Pin<Box<dyn Future<Output = ()> + Send>>> = vec![
            Box::pin(Self::listen_task(vis_state.csi_data.clone(), command_recv, update_send.clone())),
            Box::pin(Self::init(command_send.clone(), self.target, self.graph_configs.clone())),
        ];

        let tui_runner = TuiRunner::new(vis_state, command_send, update_recv, update_send, self.log_level);
        tui_runner.run(tasks).await?;

        Ok(())
    }
}

impl Visualiser {
    async fn init(command_send: Sender<VisCommand>, target_addr: SocketAddr, graph_configs: Vec<GraphConfigWithId>) {
        sleep(Duration::from_millis(200)).await;
        if command_send.send(VisCommand::Connect(target_addr)).await.is_ok() {
            for graph in graph_configs {
                let device_id = match graph {
                    GraphConfigWithId::Amplitude(amplitude_config_with_id) => amplitude_config_with_id.device_id,
                    GraphConfigWithId::PDP(pdpconfig_with_id) => pdpconfig_with_id.device_id,
                };
                let _ = command_send.send(VisCommand::Subscribe(device_id)).await;
            }
        }
    }
    #[allow(clippy::type_complexity)]
    async fn listen_task(data: Arc<Mutex<HashMap<u64, Vec<CsiData>>>>, mut command_recv: Receiver<VisCommand>, update_send: Sender<VisUpdate>) {
        let mut client = TcpClient::new();
        let mut connected_addr: Option<SocketAddr> = None;

        let mut subs: HashMap<DeviceId, usize> = HashMap::new();

        loop {
            tokio::select! {
            // Handle commands
            maybe_command = command_recv.recv() => {
                match maybe_command {
                    Some(command) => match command {
                        VisCommand::Connect(target_addr) => {
                            match connected_addr {
                                Some(current) if current == target_addr => {
                                    info!("Already connected to {target_addr:?}");
                                }
                                Some(current) => {
                                    let _ = client.disconnect(current).await;
                                    info!("Disconnected from {current:?}");

                                    if client.connect(target_addr).await.is_ok() {
                                        connected_addr = Some(target_addr);
                                        let _ = update_send.send(
                                            VisUpdate::UpdateConnectionStatus(state::ConnectionStatus::Connected)
                                        ).await;
                                    } else {
                                        error!("Failed to connect to {target_addr:?}");
                                        let _ = update_send.send(
                                            VisUpdate::UpdateConnectionStatus(state::ConnectionStatus::NotConnected)
                                        ).await;
                                    }
                                }
                                None => {
                                    if client.connect(target_addr).await.is_ok() {
                                        connected_addr = Some(target_addr);
                                        let _ = update_send.send(
                                            VisUpdate::UpdateConnectionStatus(state::ConnectionStatus::Connected)
                                        ).await;
                                    } else {
                                        error!("Failed to connect to {target_addr:?}");
                                        let _ = update_send.send(
                                            VisUpdate::UpdateConnectionStatus(state::ConnectionStatus::NotConnected)
                                        ).await;
                                    }
                                }
                            }
                        }

                        VisCommand::Subscribe(device_id) => {
                            if let Some(addr) = connected_addr {
                                let sub_count = subs.entry(device_id).or_insert(0);
                                if *sub_count > 0 {
                                    info!("Already subscribed to {device_id} on {addr}");
                                } else {
                                    let msg = RpcMessageKind::HostCtrl(HostCtrl::Subscribe { device_id });
                                    let _ = client.send_message(addr, msg).await;
                                    info!("Subscribing to {device_id} on {addr}");
                                }
                                // Increment the subscription count
                                *sub_count += 1;
                            } else {
                                error!("Cant subscribe, not connected");
                            }
                        }

                        VisCommand::Unsubscribe(device_id) => {
                            if let Some(addr) = connected_addr {
                                if let Some(sub_count) = subs.get_mut(&device_id) {
                                    if *sub_count > 1 {
                                        *sub_count -= 1;
                                        info!("Decremented subscription count for device id {} -> {}", device_id, *sub_count);
                                    } else {
                                        // Last subscription, send unsubscribe message and remove entry
                                        let msg = RpcMessageKind::HostCtrl(HostCtrl::Unsubscribe { device_id });
                                        let _ = client.send_message(addr, msg).await;
                                        info!("Unsubscribing from {device_id} on {addr}");
                                        subs.remove(&device_id);
                                    }
                                } else {
                                    info!("No active subscription for {device_id}");
                                }
                            } else {
                                error!("Not connected");
                            }
                        }

                        VisCommand::UnsubscribeAll => {
                            if let Some(addr) = connected_addr {
                                let msg = RpcMessageKind::HostCtrl(HostCtrl::UnsubscribeAll);
                                let _ = client.send_message(addr, msg).await;
                                info!("Unsubscribing from all on {addr}");
                            } else {
                                error!("Not connected");
                            }
                        }
                    },
                    None => {
                        error!("Command channel closed");
                        break;
                    }
                }
            }

            // Handle incoming messages if connected
            msg = async {
                if let Some(addr) = connected_addr {
                    Some(client.wait_for_read_message(addr).await)
                } else {
                    sleep(Duration::from_millis(500)).await; // Prevents starvation
                    None
                }
            } =>  match msg {
                      Some(Ok(RpcMessage {
                          msg: Data {
                              data_msg: CsiFrame { csi },
                              device_id,
                          },
                          ..
                      })) => {
                          debug!("Adding {:?} for device Id {}",csi.sequence_number, device_id);
                          data.lock().await.entry(device_id).or_default().push(csi);
                      }
                      Some(Err(NetworkError::Closed)) => {
                          error!("Connection closed");
                          connected_addr = None;
                      }
                      Some(m) => {
                          error!("Received unexpected message: {m:?}");
                        }
                      None => {}
                  }
            }
        }
    }
}

impl Visualiser {
    /// Processing data turns each data point into a vec of tuples (taimestamp, datapoint), such that it can be charted easily.
    /// For PDP, it returns (delay_bin, power) for the latest CSI packet.
    /// Returns the processed data and an Option containing the timestamp of the CSI data used.
    fn process_data(csi_data: &[CsiData], graph_type: GraphConfig) -> (Vec<(f64, f64)>, Option<f64>) {
        if csi_data.is_empty() {
            return (vec![], None);
        }
        match graph_type {
            GraphConfig::Amplitude(AmplitudeConfig {
                core, stream, subcarrier, ..
            }) => Self::process_amplitude_data(csi_data, core, stream, subcarrier),
            GraphConfig::PDP(PDPConfig { core, stream, .. }) => Self::process_pdp_data(csi_data, core, stream),
        }
    }

    fn process_amplitude_data(device_data: &[CsiData], core: usize, stream: usize, subcarrier: usize) -> (Vec<(f64, f64)>, Option<f64>) {
        let latest_timestamp = device_data.last().map(|x| x.timestamp);
        let data = device_data.iter().map(|x| (x.timestamp, x.csi[core][stream][subcarrier].re)).collect();
        (data, latest_timestamp)
    }

    fn process_pdp_data(device_data: &[CsiData], core: usize, stream: usize) -> (Vec<(f64, f64)>, Option<f64>) {
        if let Some(latest_csi_data) = device_data.last() {
            let csi_timestamp = latest_csi_data.timestamp;
            if core < latest_csi_data.csi.len() && stream < latest_csi_data.csi[core].len() {
                let csi_for_ifft: Vec<LibComplex> = latest_csi_data.csi[core][stream].clone();
                if csi_for_ifft.is_empty() {
                    return (vec![], Some(csi_timestamp));
                }
                let ifft_result = Self::perform_ifft(&csi_for_ifft);
                let mut power_profile: Vec<f64> = ifft_result.iter().map(|c| c.norm_sqr()).collect();

                let n = power_profile.len();
                if n > 0 {
                    power_profile.rotate_left(n / 2);
                }

                (
                    power_profile.into_iter().enumerate().map(|(idx, p)| (idx as f64, p)).collect(),
                    Some(csi_timestamp),
                )
            } else {
                (vec![], Some(csi_timestamp))
            }
        } else {
            (vec![], None)
        }
    }

    fn perform_ifft(csi_subcarriers: &[LibComplex]) -> Vec<FftComplex<f64>> {
        if csi_subcarriers.is_empty() {
            return vec![];
        }
        let mut planner = FftPlanner::<f64>::new();
        let fft = planner.plan_fft_inverse(csi_subcarriers.len());

        let mut buffer: Vec<FftComplex<f64>> = csi_subcarriers.iter().map(|c| FftComplex::new(c.re, c.im)).collect();

        fft.process(&mut buffer);

        // Normalize IFFT output
        let norm_factor = 1.0 / (buffer.len() as f64);
        for val in buffer.iter_mut() {
            *val *= norm_factor;
        }
        buffer
    }

    pub fn _decay(data_points: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
        data_points
            .iter()
            .map(|(x, y)| {
                let new_y = *y * _DECAY_RATE;
                if new_y.abs() < _MIN_POWER_THRESHOLD { (*x, 0.0) } else { (*x, new_y) }
            })
            .collect()
    }
}

impl Visualiser {
    // fn update_pdp_display_state(
    //     current_display_state_slot: &mut Option<GraphDisplayState>,
    //     opt_timestamp_from_processor: Option<f64>,
    //     data: Vec<(f64, f64)>,
    //     now: Instant,
    // ) -> Vec<(f64, f64)> {
    //     match (opt_timestamp_from_processor, current_display_state_slot.as_mut()) {
    //         (Some(timestamp_proc), Some(state)) => {
    //             if timestamp_proc > state.csi_timestamp || data.len() != state.data_points.len() {
    //                 state.data_points = data.clone();
    //                 state.csi_timestamp = timestamp_proc;
    //                 state.last_loop_update_time = now;
    //                 data
    //             } else {
    //                 if now.duration_since(state.last_loop_update_time) > STALE_THRESHOLD {
    //                     state.data_points = decay(state.data_points);
    //                     state.last_loop_update_time = now;
    //                 }
    //                 state.data_points.clone()
    //             }
    //         }
    //         (Some(timestamp_proc), None) => {
    //             *current_display_state_slot = Some(GraphDisplayState {
    //                 data_points: data.clone(),
    //                 csi_timestamp: timestamp_proc,
    //                 last_loop_update_time: now,
    //             });
    //             data
    //         }
    //         (None, Some(state)) => {
    //             if now.duration_since(state.last_loop_update_time) > STALE_THRESHOLD {
    //                 state.data_points = decay(state.data_points);
    //                 state.last_loop_update_time = now;
    //             }
    //             state.data_points.clone()
    //         }
    //         (None, None) => {
    //             vec![]
    //         }
    //     }
    // }
}
