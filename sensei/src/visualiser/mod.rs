mod state;
mod tui;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

// Alias for charming::Chart
use lib::csi_types::Complex as LibComplex; // Alias to avoid confusion
use lib::csi_types::CsiData;
use lib::network::rpc_message::DataMsg::*;
use lib::network::rpc_message::RpcMessageKind::Data;
use lib::network::rpc_message::{HostCtrl, RpcMessage, RpcMessageKind};
use lib::network::tcp::client::TcpClient;
use lib::tui::TuiRunner;
use log::{LevelFilter, error, info};
use rustfft::FftPlanner;
use rustfft::num_complex::Complex as FftComplex;
use tokio::sync::Mutex;
use tokio::sync::mpsc::{self};

use crate::services::{GlobalConfig, Run, VisualiserConfig};
use crate::visualiser::state::{AmplitudeConfig, DECAY_RATE, GraphConfig, MIN_POWER_THRESHOLD, PDPConfig, VisCommand, VisState, VisUpdate};

const ACTOR_CHANNEL_CAPACITY: usize = 100;

pub struct Visualiser {
    target: SocketAddr,
    log_level: LevelFilter,
}

impl Run<VisualiserConfig> for Visualiser {
    fn new(_global_config: GlobalConfig, config: VisualiserConfig) -> Self {
        Visualiser {
            target: config.target,
            log_level: LevelFilter::Info,
        }
    }

    async fn run(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        // Technically, the visualiser has cli tools for connecting to multiple nodes
        // At the moment, it is sufficient to connect to one target node on startup
        // Manually start the subscription by typing subscribe

        let (command_send, command_recv) = mpsc::channel::<VisCommand>(ACTOR_CHANNEL_CAPACITY);
        let (update_send, update_recv) = mpsc::channel::<VisUpdate>(ACTOR_CHANNEL_CAPACITY);

        let vis_state = VisState::new();

        let tasks: Vec<Pin<Box<dyn Future<Output = ()> + Send>>> = vec![Box::pin(Self::listen_task(vis_state.csi_data.clone(), self.target))];

        let tui_runner = TuiRunner::new(vis_state, command_send, update_recv, update_send, self.log_level);
        tui_runner.run(tasks).await;

        Ok(())
    }
}

impl Visualiser {
    #[allow(clippy::type_complexity)]
    async fn listen_task(data: Arc<Mutex<HashMap<SocketAddr, HashMap<u64, Vec<CsiData>>>>>, target_addr: SocketAddr) {
        let mut client = TcpClient::new();
        if client.connect(target_addr).await.is_err() {
            error!("Failed to connect to {target_addr:?}");
            return;
        };

        let msg = HostCtrl::Subscribe { device_id: 0 };
        if client.send_message(target_addr, RpcMessageKind::HostCtrl(msg)).await.is_err() {
            error!("Failed to send message to {target_addr:?}");
            return;
        };
        info!("Subscribed to node {target_addr}");

        loop {
            match client.read_message(target_addr).await {
                Ok(RpcMessage {
                    msg: Data {
                        data_msg: CsiFrame { csi },
                        device_id,
                    },
                    src_addr,
                    target_addr: _,
                }) => {
                    data.lock()
                        .await
                        .entry(src_addr)
                        .and_modify(|devices| {
                            devices
                                .entry(device_id)
                                .and_modify(|csi_data| csi_data.push(csi.clone()))
                                .or_insert(vec![csi.clone()]);
                        })
                        .or_insert(HashMap::new());
                }
                _ => {
                    error!("Some shit is fucked up");
                    break;
                }
            }
        }
    }
}

impl Visualiser {
    /// Processing data turns each data point into a vec of tuples (timestamp, datapoint), such that it can be charted easily.
    /// For PDP, it returns (delay_bin, power) for the latest CSI packet.
    /// Returns the processed data and an Option containing the timestamp of the CSI data used.
    fn process_data(csi_data: &[CsiData], graph_type: GraphConfig) -> (Vec<(f64, f64)>, Option<f64>) {
        if csi_data.is_empty() {
            return (vec![], None);
        }
        match graph_type {
            GraphConfig::Amplitude(AmplitudeConfig {
                core,
                stream,
                subcarrier,
                time_range,
            }) => Self::process_amplitude_data(csi_data, core, stream, subcarrier),
            GraphConfig::PDP(PDPConfig { core, stream, y_axis_bounds }) => Self::process_pdp_data(csi_data, core, stream),
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

    pub fn decay(data_points: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
        data_points
            .iter()
            .map(|(x, y)| {
                let new_y = *y * DECAY_RATE;
                if new_y.abs() < MIN_POWER_THRESHOLD { (*x, 0.0) } else { (*x, new_y) }
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

    // async fn tui_loop<B: Backend>(
    //     &self,
    //     terminal: &mut Terminal<B>,
    //     graph_display_states: Arc<Mutex<Vec<Option<GraphDisplayState>>>>,
    // ) -> io::Result<()> {
    //     // Source address, device id, core, stream, subcarrier
    //     let graphs: Arc<Mutex<Vec<Graph>>> = Arc::new(Mutex::new(Vec::new()));

    //     loop {
    //         if last_tick.elapsed() >= tick_rate {
    //             let graphs_snapshot: Vec<Graph> = graphs.lock().await.iter().cloned().collect();
    //             let mut display_states_locked = graph_display_states.lock().await;

    //             // // Reconcile display_states_locked length with graphs_snapshot length
    //             // if display_states_locked.len() > graphs_snapshot.len() {
    //             //     display_states_locked.truncate(graphs_snapshot.len());
    //             // } else if display_states_locked.len() < graphs_snapshot.len() {
    //             //     display_states_locked.resize_with(graphs_snapshot.len(), || None);
    //             // }

    //             let mut data_to_render_this_frame: Vec<Vec<(f64, f64)>> = Vec::new();
    //             // let now = Instant::now();

    //             for (i, graph_spec) in graphs_snapshot.iter().enumerate() {
    //                 let (points_from_processor, opt_timestamp_from_processor) = Self::process_data(*graph_spec).await;

    //                 if graph_spec.graph_type == GraphType::PDP {
    //                     let processed_points_for_render =
    //                         Self::update_pdp_display_state(&mut display_states_locked[i], points_from_processor, opt_timestamp_from_processor, now);
    //                     data_to_render_this_frame.push(processed_points_for_render);
    //                 } else {
    //                     data_to_render_this_frame.push(points_from_processor);
    //                     if display_states_locked[i].is_some() {
    //                         display_states_locked[i] = None;
    //                     }
    //                 }
    //             }
    //         }
    //     }
    // }
}

impl Visualiser {
    // fn entry_from_command(parts: Vec<&str>) -> Option<Graph> {
    //     let graph_type: GraphType = match parts[1].parse() {
    //         Ok(addr) => addr,
    //         Err(_) => return None, // Exit on invalid input
    //     };
    //     let addr: SocketAddr = match parts[2].parse() {
    //         Ok(addr) => addr,
    //         Err(_) => return None, // Exit on invalid input
    //     };

    //     let device_id: u64 = match parts[3].parse() {
    //         Ok(addr) => addr,
    //         Err(_) => return None, // Exit on invalid input
    //     };
    //     let core: usize = match parts[4].parse() {
    //         Ok(addr) => addr,
    //         Err(_) => return None, // Exit on invalid input
    //     };
    //     let stream: usize = match parts[5].parse() {
    //         Ok(addr) => addr,
    //         Err(_) => return None, // Exit on invalid input
    //     };
    //     // For PDP, subcarrier is not strictly needed as we use all of them.
    //     // We'll parse it but it will be ignored in process_data for PDP.
    //     // If not provided for PDP, we can default it or handle the shorter command.
    //     // For now, assume it's always provided for simplicity of command structure.
    //     let subcarrier: usize = if parts.len() > 6 {
    //         match parts[6].parse() {
    //             Ok(addr) => addr,
    //             Err(_) => return None, // Exit on invalid input
    //         }
    //     } else {
    //         0 // Default or indicate all subcarriers for PDP if not provided
    //     };

    //     Some(Graph {
    //         gtype: graph_type,
    //         target_addr: addr,
    //         device_id,
    //         core,
    //         stream,
    //         subcarrier,
    //         time_interval: 1000,
    //         y_axis_bounds: None, // Initialize y_axis_bounds
    //     })
    // }

    // async fn execute_command(text_input: String, graphs: Arc<Mutex<Vec<Graph>>>) {
    //     let parts: Vec<&str> = text_input.split_whitespace().collect();
    //     if parts.is_empty() {
    //         return;
    //     }

    //     match parts[0] {
    //         "add" if parts.len() == 7 => {
    //             let entry = match Self::entry_from_command(parts) {
    //                 None => return,
    //                 Some(entry) => entry,
    //             };
    //             graphs.lock().await.push(entry);
    //         }
    //         "remove" if parts.len() == 2 => {
    //             let entry: usize = match parts[1].parse::<usize>() {
    //                 Ok(number) => number,
    //                 Err(_) => return,
    //             };
    //             graphs.lock().await.remove(entry);
    //         }
    //         "interval" if parts.len() == 3 => {
    //             let graph_idx: usize = match parts[1].parse::<usize>() {
    //                 Ok(number) => number,
    //                 Err(_) => return,
    //             };
    //             let value: f64 = match parts[2].parse::<f64>() {
    //                 Ok(val) => val,
    //                 Err(_) => return,
    //             };

    //             let mut graphs_locked = graphs.lock().await;
    //             if let Some(graph_to_modify) = graphs_locked.get_mut(graph_idx) {
    //                 match graph_to_modify.gtype {
    //                     GraphConfig::PDP => {
    //                         graph_to_modify.y_axis_bounds = Some([0.0, value]);
    //                     }
    //                     GraphConfig::Amplitude => {
    //                         graph_to_modify.time_interval = value as usize;
    //                     }
    //                 }
    //             }
    //         }
    //         "clear" => {
    //             graphs.lock().await.clear();
    //         }
    //         _ => {} // Dont execute on invalid input
    //     }
    // }
}

// #[cfg(test)]
// mod tests {
//     use std::time::{Duration, Instant};

//     use lib::csi_types::Complex;

//     use super::*;

//     #[tokio::test]
//     async fn test_graph_type_from_str() {
//         assert_eq!(GraphConfig::from_str("amp").unwrap(), GraphConfig::Amplitude);
//         assert_eq!(GraphConfig::from_str("amplitude").unwrap(), GraphConfig::Amplitude);
//         assert_eq!(GraphConfig::from_str("pdp").unwrap(), GraphConfig::PDP);
//         assert!(GraphConfig::from_str("invalid").is_err());
//     }

//     #[tokio::test]
//     async fn test_graph_partial_eq() {
//         let addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();
//         let graph1 = Graph {
//             gtype: GraphConfig::Amplitude,
//             target_addr: addr,
//             device_id: 1,
//             core: 2,
//             stream: 3,
//             subcarrier: 4,
//             time_interval: 1000,
//             y_axis_bounds: None,
//         };

//         let graph2 = Graph {
//             time_interval: 2000,
//             y_axis_bounds: Some([0.0, 1.0]),
//             ..graph1
//         };

//         assert_eq!(graph1, graph2);
//     }

//     #[tokio::test]
//     async fn test_perform_ifft() {
//         let empty_input: Vec<LibComplex> = vec![];
//         let empty_output = Visualiser::perform_ifft(&empty_input);
//         assert!(empty_output.is_empty());

//         let input: Vec<LibComplex> = vec![
//             Complex::new(1.0, 0.0),
//             Complex::new(1.0, 0.0),
//             Complex::new(1.0, 0.0),
//             Complex::new(1.0, 0.0),
//         ];
//         let output = Visualiser::perform_ifft(&input);
//         assert!((output[0] - FftComplex::new(1.0, 0.0)).norm() < 1e-9);
//         for val in output.iter().skip(1) {
//             assert!(val.norm() < 1e-9);
//         }
//     }

//     #[tokio::test]
//     async fn test_process_amplitude_data() {
//         let visualiser = setup_visualiser();
//         let device_data = setup_csi_data();
//         let (data, latest_timestamp) = visualiser.process_amplitude_data(&device_data, 0, 0, 0).await;

//         assert_eq!(data.len(), 2);
//         assert_eq!(data[0], (100.0, 1.0));
//         assert_eq!(data[1], (200.0, 5.0));
//         assert_eq!(latest_timestamp, Some(200.0));
//     }

//     #[tokio::test]
//     async fn test_process_pdp_data() {
//         let visualiser = setup_visualiser();
//         let device_data = setup_csi_data();

//         let (data, latest_timestamp) = visualiser.process_pdp_data(&device_data, 0, 0).await;
//         assert!(!data.is_empty());
//         assert_eq!(data.len(), 2);
//         assert_eq!(latest_timestamp, Some(200.0));

//         let (empty_data, empty_timestamp) = visualiser.process_pdp_data(&[], 0, 0).await;
//         assert!(empty_data.is_empty());
//         assert!(empty_timestamp.is_none());

//         let (data_oob_core, ts_oob_core) = visualiser.process_pdp_data(&device_data, 99, 0).await;
//         assert!(data_oob_core.is_empty());
//         assert_eq!(ts_oob_core, Some(200.0));
//     }

//     #[tokio::test]
//     async fn test_process_data() {
//         let visualiser = setup_visualiser();
//         let addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();
//         let device_data = setup_csi_data();

//         let mut data_guard = visualiser.data.lock().await;
//         let mut device_map = HashMap::new();
//         device_map.insert(1, device_data);
//         data_guard.insert(addr, device_map);
//         drop(data_guard);

//         let pdp_graph = Graph {
//             gtype: GraphConfig::PDP,
//             target_addr: addr,
//             device_id: 1,
//             core: 0,
//             stream: 0,
//             subcarrier: 0,
//             time_interval: 1000,
//             y_axis_bounds: None,
//         };
//         let (pdp_data, _) = visualiser.process_data(pdp_graph).await;
//         assert_eq!(pdp_data.len(), 2);

//         let amp_graph = Graph {
//             gtype: GraphConfig::Amplitude,
//             target_addr: addr,
//             device_id: 1,
//             core: 0,
//             stream: 0,
//             subcarrier: 0,
//             time_interval: 1000,
//             y_axis_bounds: None,
//         };
//         let (amp_data, _) = visualiser.process_data(amp_graph).await;
//         assert_eq!(amp_data.len(), 2);
//         assert_eq!(amp_data[0].1, 1.0);
//     }

//     #[tokio::test]
//     async fn test_update_pdp_display_state() {
//         let now = Instant::now();
//         let points = vec![(0.0, 10.0), (1.0, 20.0)];
//         let timestamp = 100.0;

//         let mut state_slot = Some(GraphDisplayState {
//             data_points: vec![(0.0, 5.0)],
//             csi_timestamp: 90.0,
//             last_loop_update_time: now,
//         });
//         let updated_points = Visualiser::update_pdp_display_state(&mut state_slot, points.clone(), Some(timestamp), now);
//         assert_eq!(updated_points, points);
//         assert_eq!(state_slot.as_ref().unwrap().csi_timestamp, timestamp);

//         let mut empty_state_slot = None;
//         let initial_points = Visualiser::update_pdp_display_state(&mut empty_state_slot, points.clone(), Some(timestamp), now);
//         assert_eq!(initial_points, points);
//         assert!(empty_state_slot.is_some());

//         let stale_time = now - Duration::from_millis(300);
//         let mut stale_state_slot = Some(GraphDisplayState {
//             data_points: points.clone(),
//             csi_timestamp: timestamp,
//             last_loop_update_time: stale_time,
//         });
//         let decayed_points = Visualiser::update_pdp_display_state(&mut stale_state_slot, vec![], None, now);
//         assert_eq!(decayed_points[0].1, 10.0 * 0.9);
//         assert_eq!(decayed_points[1].1, 20.0 * 0.9);

//         let mut none_state_slot = None;
//         let no_points = Visualiser::update_pdp_display_state(&mut none_state_slot, vec![], None, now);
//         assert!(no_points.is_empty());
//     }

//     #[tokio::test]
//     async fn test_get_x_axis_config() {
//         let amp_data = vec![(990.0, 1.0), (1000.0, 2.0)];
//         let (title, bounds, labels) = Visualiser::get_x_axis_config(GraphConfig::Amplitude, &amp_data, 50);
//         assert_eq!(labels.len(), 2);
//         assert_eq!(bounds, [949.0, 1001.0]);
//         assert_eq!(title, "Time");

//         let pdp_data = vec![(0.0, 1.0), (1.0, 2.0), (2.0, 1.5)];
//         let (title_pdp, bounds_pdp, labels_pdp) = Visualiser::get_x_axis_config(GraphConfig::PDP, &pdp_data, 0);
//         assert_eq!(title_pdp, "Delay Bin");
//         assert_eq!(bounds_pdp, [0.0, 2.0]);
//         assert_eq!(labels_pdp.len(), 2);

//         let (title_empty, bounds_empty, labels_empty) = Visualiser::get_x_axis_config(GraphConfig::PDP, &[], 0);
//         assert_eq!(title_empty, "Delay Bin");
//         assert_eq!(bounds_empty, [0.0, 0.0]);
//         assert_eq!(labels_empty.len(), 1);
//     }

//     #[tokio::test]
//     async fn test_calculate_dynamic_bounds() {
//         let data = vec![(0.0, 10.0), (1.0, 90.0)];
//         let bounds = Visualiser::calculate_dynamic_bounds(&data);
//         let range = 90.0 - 10.0;
//         let padding = range * 0.05;
//         assert_eq!(bounds, [10.0 - padding, 90.0 + padding]);

//         let empty_data: Vec<(f64, f64)> = vec![];
//         let bounds_empty = Visualiser::calculate_dynamic_bounds(&empty_data);
//         assert_eq!(bounds_empty, [0.0, 1.0]);

//         let same_data = vec![(0.0, 5.0), (1.0, 5.0)];
//         let bounds_same = Visualiser::calculate_dynamic_bounds(&same_data);
//         assert_eq!(bounds_same, [4.5, 5.5]);
//     }

//     #[tokio::test]
//     async fn test_get_y_axis_config() {
//         let data = vec![(0.0, 10.0), (1.0, 90.0)];

//         let (title_amp, _, _) = Visualiser::get_y_axis_config(GraphConfig::Amplitude, &data, None);
//         assert_eq!(title_amp, "Amplitude");

//         let y_bounds_spec = Some([0.0, 100.0]);
//         let (title_pdp, bounds_pdp, _) = Visualiser::get_y_axis_config(GraphConfig::PDP, &data, y_bounds_spec);
//         assert_eq!(title_pdp, "Power");
//         assert_eq!(bounds_pdp, [0.0, 100.0]);

//         let (_, bounds_pdp_dynamic, _) = Visualiser::get_y_axis_config(GraphConfig::PDP, &data, None);
//         let expected_bounds = Visualiser::calculate_dynamic_bounds(&data);
//         assert_eq!(bounds_pdp_dynamic, expected_bounds);
//     }

//     #[tokio::test]
//     async fn test_entry_from_command() {
//         let parts_amp = vec!["add", "amp", "127.0.0.1:8080", "1", "2", "3", "4"];
//         let graph_amp = Visualiser::entry_from_command(parts_amp).unwrap();
//         assert_eq!(graph_amp.gtype, GraphConfig::Amplitude);
//         assert_eq!(graph_amp.device_id, 1);
//         assert_eq!(graph_amp.subcarrier, 4);

//         let parts_pdp = vec!["add", "pdp", "192.168.1.1:1234", "10", "0", "1", "0"];
//         let graph_pdp = Visualiser::entry_from_command(parts_pdp).unwrap();
//         assert_eq!(graph_pdp.gtype, GraphConfig::PDP);
//         assert_eq!(graph_pdp.target_addr, "192.168.1.1:1234".parse().unwrap());
//     }

//     #[tokio::test]
//     #[should_panic]
//     async fn test_entry_from_command_invalid_parts() {
//         let parts_invalid = vec!["add", "amp", "127.0.0.1:8080"];
//         Visualiser::entry_from_command(parts_invalid).unwrap();
//     }

//     #[tokio::test]
//     #[should_panic]
//     async fn test_entry_from_command_bad_addr() {
//         let parts_bad_addr = vec!["add", "amp", "127.0.0.1:bad", "1", "2", "3", "4"];
//         Visualiser::entry_from_command(parts_bad_addr).unwrap();
//     }

//     #[tokio::test]
//     async fn test_execute_command() {
//         let graphs = Arc::new(Mutex::new(Vec::new()));

//         let cmd_add = "add amp 127.0.0.1:8080 1 2 3 4".to_string();
//         Visualiser::execute_command(cmd_add, graphs.clone()).await;
//         assert_eq!(graphs.lock().await.len(), 1);

//         let cmd_add_2 = "add pdp 127.0.0.1:8080 1 2 3 4".to_string();
//         Visualiser::execute_command(cmd_add_2, graphs.clone()).await;
//         assert_eq!(graphs.lock().await.len(), 2);

//         let cmd_interval = "interval 0 500".to_string();
//         Visualiser::execute_command(cmd_interval, graphs.clone()).await;
//         assert_eq!(graphs.lock().await[0].time_interval, 500);

//         let cmd_interval_pdp = "interval 1 1.5".to_string();
//         Visualiser::execute_command(cmd_interval_pdp, graphs.clone()).await;
//         assert_eq!(graphs.lock().await[1].y_axis_bounds, Some([0.0, 1.5]));

//         Visualiser::execute_command("remove 0".to_string(), graphs.clone()).await;
//         assert_eq!(graphs.lock().await.len(), 1);
//         assert_eq!(graphs.lock().await[0].gtype, GraphConfig::PDP);

//         let cmd_clear = "clear".to_string();
//         Visualiser::execute_command(cmd_clear, graphs.clone()).await;
//         assert!(graphs.lock().await.is_empty());

//         let cmd_invalid = "this is not a command".to_string();
//         Visualiser::execute_command(cmd_invalid, graphs.clone()).await;
//         assert!(graphs.lock().await.is_empty());
//     }

//     fn setup_visualiser() -> Visualiser {
//         let global_config = GlobalConfig {
//             log_level: simplelog::LevelFilter::Off,
//             num_workers: 4,
//         };
//         let visualiser_config = VisualiserConfig {
//             target: "127.0.0.1:1234".parse().unwrap(),
//         };
//         Visualiser::new(global_config, visualiser_config)
//     }

//     fn setup_csi_data() -> Vec<CsiData> {
//         vec![
//             CsiData {
//                 timestamp: 100.0,
//                 csi: vec![vec![vec![Complex::new(1.0, 2.0), Complex::new(3.0, 4.0)]]],
//                 rssi: vec![0],
//                 sequence_number: 0,
//             },
//             CsiData {
//                 timestamp: 200.0,
//                 csi: vec![vec![vec![Complex::new(5.0, 6.0), Complex::new(7.0, 8.0)]]],
//                 rssi: vec![0],
//                 sequence_number: 0,
//             },
//         ]
//     }
// }
