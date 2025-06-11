use std::cmp::PartialEq;
use std::collections::HashMap;
use std::fmt;
use std::io::stdout;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use charming::Chart as CharmingChart; // Alias for charming::Chart
use charming::HtmlRenderer;
use charming::theme::Theme;
use lib::csi_types::Complex as LibComplex; // Alias to avoid confusion
use lib::csi_types::CsiData;
use lib::network::rpc_message::DataMsg::*;
use lib::network::rpc_message::RpcMessageKind::Data;
use lib::network::rpc_message::{HostCtrl, RpcMessage, RpcMessageKind};
use lib::network::tcp::client::TcpClient;
use log::{debug, info};
use ratatui::Terminal;
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::crossterm::cursor::{Hide, Show};
use ratatui::crossterm::event::{DisableMouseCapture, EnableMouseCapture, Event, KeyCode};
use ratatui::crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use ratatui::crossterm::{event, execute};
use ratatui::layout::{Constraint, Layout};
use ratatui::prelude::Direction;
use ratatui::style::{Color, Style};
use ratatui::text::Span;
use ratatui::widgets::{Axis, Block, Borders, Chart, Dataset};
use rustfft::FftPlanner;
use rustfft::num_complex::Complex as FftComplex;
use tokio::io;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

use crate::services::{GlobalConfig, Run, VisualiserConfig};

pub struct Visualiser {
    #[allow(clippy::type_complexity)]
    data: Arc<Mutex<HashMap<SocketAddr, HashMap<u64, Vec<CsiData>>>>>,
    target: SocketAddr,
    ui_type: String,
}

impl Run<VisualiserConfig> for Visualiser {
    fn new(global_config: GlobalConfig, config: VisualiserConfig) -> Self {
        Visualiser {
            data: Arc::new(Default::default()),
            target: config.target,
            ui_type: config.ui_type,
        }
    }

    async fn run(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        // Technically, the visualiser has cli tools for connecting to multiple nodes
        // At the moment, it is sufficient to connect to one target node on startup
        // Manually start the subscription by typing subscribe
        let client = Arc::new(Mutex::new(TcpClient::new()));
        self.client_task(client.clone(), self.target).await;
        self.receive_data_task(self.data.clone(), client.clone(), self.target);

        io::stdout().flush().await?;

        if self.ui_type == "tui" {
            self.plot_data_tui().await?;
        } else {
            self.plot_data_gui().await?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
struct Graph {
    graph_type: GraphType,
    target_addr: SocketAddr,
    device: u64,
    core: usize,
    stream: usize,
    subcarrier: usize,
    time_interval: usize,            // For Amplitude plots: x-axis time window in ms
    y_axis_bounds: Option<[f64; 2]>, // For PDP plots: fixed y-axis bounds [min, max]
}

impl PartialEq for Graph {
    fn eq(&self, other: &Self) -> bool {
        self.graph_type == other.graph_type
            && self.target_addr == other.target_addr
            && self.device == other.device
            && self.core == other.core
            && self.stream == other.stream
            && self.subcarrier == other.subcarrier
        // time_interval is intentionally omitted for robust state tracking against graph spec changes
        // y_axis_bounds is also omitted for the same reason
    }
}

#[derive(Clone, Debug)]
struct GraphDisplayState {
    data_points: Vec<(f64, f64)>,
    csi_timestamp: f64,             // Timestamp of the CsiData this state is based on
    last_loop_update_time: Instant, // When this state was last updated or checked in the tui_loop
}

#[derive(Debug, Clone, Copy)]
pub enum GraphType {
    Amplitude,
    #[allow(clippy::upper_case_acronyms)]
    PDP,
}

impl FromStr for GraphType {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "amp" => Ok(GraphType::Amplitude),
            "amplitude" => Ok(GraphType::Amplitude),
            "pdp" => Ok(GraphType::PDP),
            _ => Err(()),
        }
    }
}

impl fmt::Display for GraphType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl PartialEq for GraphType {
    fn eq(&self, other: &Self) -> bool {
        self.to_string() == other.to_string()
    }
}

impl Visualiser {
    #[allow(clippy::type_complexity)]
    fn receive_data_task(
        &self,
        data: Arc<Mutex<HashMap<SocketAddr, HashMap<u64, Vec<CsiData>>>>>,
        client: Arc<Mutex<TcpClient>>,
        target_addr: SocketAddr,
    ) {
        tokio::spawn(async move {
            debug!("Receive task");
            loop {
                let mut client = client.lock().await;
                match client.read_message(target_addr).await {
                    Ok(msg) => {
                        let RpcMessage {
                            msg,
                            src_addr,
                            target_addr: _,
                        } = msg;
                        if let Data {
                            data_msg: CsiFrame { csi },
                            device_id,
                        } = msg
                        {
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
                    }
                    _ => return, // Connection with node is not established, ends the process
                }
            }
        });
    }

    async fn output_data(&self) -> HashMap<SocketAddr, HashMap<u64, Vec<CsiData>>> {
        self.data.lock().await.clone()
    }

    async fn process_amplitude_data(
        &self,
        device_data: &[CsiData],
        core: usize,
        stream: usize,
        subcarrier: usize,
    ) -> (Vec<(f64, f64)>, Option<f64>) {
        let latest_timestamp = device_data.last().map(|x| x.timestamp);
        let data = device_data
            .iter()
            .map(|x| (x.timestamp, x.csi[core][stream][subcarrier].re))
            .collect();
        (data, latest_timestamp)
    }

    async fn process_pdp_data(
        &self,
        device_data: &[CsiData],
        core: usize,
        stream: usize,
    ) -> (Vec<(f64, f64)>, Option<f64>) {
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
                    power_profile
                        .into_iter()
                        .enumerate()
                        .map(|(idx, p)| (idx as f64, p))
                        .collect(),
                    Some(csi_timestamp),
                )
            } else {
                (vec![], Some(csi_timestamp))
            }
        } else {
            (vec![], None)
        }
    }

    /// Processing data turns each data point into a vec of tuples (timestamp, datapoint), such that it can be charted easily.
    /// For PDP, it returns (delay_bin, power) for the latest CSI packet.
    /// Returns the processed data and an Option containing the timestamp of the CSI data used.
    async fn process_data(&self, graph: Graph) -> (Vec<(f64, f64)>, Option<f64>) {
        let data_map = self.data.lock().await;
        let device_data_map = match data_map
            .get(&graph.target_addr)
            .and_then(|node_data| node_data.get(&graph.device))
        {
            Some(data) => data.clone(),
            None => return (vec![], None),
        };

        if device_data_map.is_empty() {
            return (vec![], None);
        }

        match graph.graph_type {
            GraphType::Amplitude => {
                self.process_amplitude_data(&device_data_map, graph.core, graph.stream, graph.subcarrier)
                    .await
            }
            GraphType::PDP => self.process_pdp_data(&device_data_map, graph.core, graph.stream).await,
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

    async fn plot_data_tui(&self) -> Result<(), Box<dyn std::error::Error>> {
        enable_raw_mode()?;
        let mut stdout = stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture, Hide)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        let graph_display_states: Arc<Mutex<Vec<Option<GraphDisplayState>>>> = Arc::new(Mutex::new(Vec::new()));

        self.tui_loop(&mut terminal, graph_display_states.clone()).await?;

        // Shutdown process
        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture, Show,)?;
        terminal.show_cursor()?;

        Ok(())
    }

    fn update_pdp_display_state(
        current_display_state_slot: &mut Option<GraphDisplayState>,
        points_from_processor: Vec<(f64, f64)>,
        opt_timestamp_from_processor: Option<f64>,
        now: Instant,
    ) -> Vec<(f64, f64)> {
        const DECAY_RATE: f64 = 0.9;
        const STALE_THRESHOLD: Duration = Duration::from_millis(200);
        const MIN_POWER_THRESHOLD: f64 = 0.015;

        match (opt_timestamp_from_processor, current_display_state_slot.as_mut()) {
            (Some(timestamp_proc), Some(state)) => {
                if timestamp_proc > state.csi_timestamp || points_from_processor.len() != state.data_points.len() {
                    state.data_points = points_from_processor.clone();
                    state.csi_timestamp = timestamp_proc;
                    state.last_loop_update_time = now;
                    points_from_processor
                } else {
                    if now.duration_since(state.last_loop_update_time) > STALE_THRESHOLD {
                        state.data_points = state
                            .data_points
                            .iter()
                            .map(|(x, y)| {
                                let new_y = *y * DECAY_RATE;
                                if new_y.abs() < MIN_POWER_THRESHOLD { (*x, 0.0) } else { (*x, new_y) }
                            })
                            .collect();
                        state.last_loop_update_time = now;
                    }
                    state.data_points.clone()
                }
            }
            (Some(timestamp_proc), None) => {
                *current_display_state_slot = Some(GraphDisplayState {
                    data_points: points_from_processor.clone(),
                    csi_timestamp: timestamp_proc,
                    last_loop_update_time: now,
                });
                points_from_processor
            }
            (None, Some(state)) => {
                if now.duration_since(state.last_loop_update_time) > STALE_THRESHOLD {
                    state.data_points = state
                        .data_points
                        .iter()
                        .map(|(x, y)| {
                            let new_y = *y * DECAY_RATE;
                            if new_y.abs() < MIN_POWER_THRESHOLD { (*x, 0.0) } else { (*x, new_y) }
                        })
                        .collect();
                    state.last_loop_update_time = now;
                }
                state.data_points.clone()
            }
            (None, None) => {
                vec![]
            }
        }
    }

    fn get_x_axis_config(
        graph_type: GraphType,
        data_points: &[(f64, f64)],
        time_interval: usize,
    ) -> (String, [f64; 2], Vec<Span>) {
        match graph_type {
            GraphType::Amplitude => {
                let time_max = data_points.iter().max_by(|x, y| x.0.total_cmp(&y.0)).unwrap_or(&(0f64, 0f64)).0;
                let bounds = [(time_max - time_interval as f64 - 1f64).round(), (time_max + 1f64).round()];
                let labels = bounds.iter().map(|n| Span::from(n.to_string())).collect();
                ("Time".to_string(), bounds, labels)
            }
            GraphType::PDP => {
                let num_delay_bins = data_points.len();
                let max_delay_bin_idx = if num_delay_bins == 0 { 0.0 } else { (num_delay_bins - 1) as f64 };
                let bounds = [0.0, max_delay_bin_idx.max(0.0)];
                let mut labels = vec![
                    Span::from(bounds[0].floor().to_string()),
                    Span::from(bounds[1].floor().to_string()),
                ];
                labels.dedup_by(|a, b| a.content == b.content);
                if labels.is_empty() {
                    labels.push(Span::from("0"));
                }
                ("Delay Bin".to_string(), bounds, labels)
            }
        }
    }

    fn get_y_axis_config(
        graph_type: GraphType,
        data_points: &[(f64, f64)],
        y_axis_bounds_spec: Option<[f64; 2]>,
    ) -> (String, [f64; 2], Vec<Span>) {
        let y_bounds_to_use = if graph_type == GraphType::PDP && y_axis_bounds_spec.is_some() {
            y_axis_bounds_spec.unwrap()
        } else {
            let (min_val, max_val) = data_points.iter().fold(
                (f64::INFINITY, f64::NEG_INFINITY),
                |(min_acc, max_acc), &(_, y)| (min_acc.min(y), max_acc.max(y)),
            );
            let (min_val, max_val) = if min_val.is_infinite() || max_val.is_infinite() {
                (0.0, 1.0)
            } else if (max_val - min_val).abs() < f64::EPSILON {
                (min_val - 0.5, max_val + 0.5)
            } else {
                (min_val, max_val)
            };
            let data_range = max_val - min_val;
            let padding = (data_range * 0.05).max(0.1);
            [min_val - padding, max_val + padding]
        };

        let data_labels: Vec<Span> = vec![
            Span::from(format!("{:.2}", y_bounds_to_use[0])),
            Span::from(format!("{:.2}", y_bounds_to_use[1])),
        ];

        let y_axis_title_str = match graph_type {
            GraphType::Amplitude => graph_type.to_string(),
            GraphType::PDP => "Power".to_string(),
        };

        (y_axis_title_str, y_bounds_to_use, data_labels)
    }


    async fn tui_loop<B: Backend>(
        &self,
        terminal: &mut Terminal<B>,
        graph_display_states: Arc<Mutex<Vec<Option<GraphDisplayState>>>>,
    ) -> io::Result<()> {
        let tick_rate = Duration::from_millis(10);
        let mut last_tick = Instant::now();
        let mut text_input: String = String::new();

        // Source address, device id, core, stream, subcarrier
        let graphs: Arc<Mutex<Vec<Graph>>> = Arc::new(Mutex::new(Vec::new()));

        loop {
            let timeout = tick_rate.checked_sub(last_tick.elapsed()).unwrap_or_else(|| Duration::from_secs(0));

            if event::poll(timeout)? {
                if let Event::Key(key) = event::read()? {
                    match key.code {
                        KeyCode::Char(c) => {
                            text_input.push(c);
                        }
                        KeyCode::Backspace => {
                            text_input.pop();
                        }
                        KeyCode::Enter => {
                            Self::execute_command(text_input.clone(), graphs.clone()).await;
                            text_input.clear();
                        }
                        KeyCode::Esc => return Ok(()),
                        _ => {}
                    }
                }
            }

            if last_tick.elapsed() >= tick_rate {
                let graphs_snapshot: Vec<Graph> = graphs.lock().await.iter().cloned().collect();
                let mut display_states_locked = graph_display_states.lock().await;

                // Reconcile display_states_locked length with graphs_snapshot length
                if display_states_locked.len() > graphs_snapshot.len() {
                    display_states_locked.truncate(graphs_snapshot.len());
                } else if display_states_locked.len() < graphs_snapshot.len() {
                    display_states_locked.resize_with(graphs_snapshot.len(), || None);
                }

                let mut data_to_render_this_frame = Vec::new();
                let now = Instant::now();
                
                for (i, graph_spec) in graphs_snapshot.iter().enumerate() {
                    let (points_from_processor, opt_timestamp_from_processor) = self.process_data(*graph_spec).await;

                    if graph_spec.graph_type == GraphType::PDP {
                        let processed_points_for_render = Self::update_pdp_display_state(
                            &mut display_states_locked[i],
                            points_from_processor,
                            opt_timestamp_from_processor,
                            now,
                        );
                        data_to_render_this_frame.push(processed_points_for_render);
                    } else {
                        data_to_render_this_frame.push(points_from_processor);
                        if display_states_locked[i].is_some() {
                            display_states_locked[i] = None; 
                        }
                    }
                }
                // Drop the lock before drawing, as drawing slow slow slow
                drop(display_states_locked);

                terminal.draw(|f| {
                    let size = f.area();

                    let chunks = Layout::default()
                        .direction(Direction::Vertical)
                        .margin(1)
                        .constraints([Constraint::Percentage(80), Constraint::Length(3)].as_ref())
                        .split(size);

                    // Use data_to_render_this_frame for graph_count and iteration
                    let graph_count = if data_to_render_this_frame.is_empty() { 1 } else { data_to_render_this_frame.len() };
                    let constraints = vec![Constraint::Percentage(100 / graph_count as u16); graph_count];
                    let chart_area = Layout::default()
                        .direction(Direction::Horizontal)
                        .margin(1)
                        .constraints(constraints)
                        .split(chunks[0]);

                    for (i, data_points) in data_to_render_this_frame.iter().enumerate() { // Iterate over data_to_render_this_frame
                        // graphs_snapshot still provides the spec (title, type, etc.)
                        let current_graph_spec = &graphs_snapshot[i];

                        let dataset = Dataset::default()
                            .name(format!("Graph #{i}"))
                            .marker(ratatui::symbols::Marker::Braille)
                            .graph_type(ratatui::widgets::GraphType::Line)
                            .style(Style::default().fg(Color::Cyan))
                            .data(data_points);

                        let (x_axis_title_str, x_bounds_arr, x_labels_vec) = Self::get_x_axis_config(
                            current_graph_spec.graph_type,
                            data_points,
                            current_graph_spec.time_interval,
                        );

                        let (y_axis_title_str, y_bounds_to_use, y_labels_vec) = Self::get_y_axis_config(
                            current_graph_spec.graph_type,
                            data_points,
                            current_graph_spec.y_axis_bounds,
                        );
                        
                        let chart_block_title = format!(
                            "Chart {} - {} @ {} dev {} C{} S{}",
                            i,
                            current_graph_spec.graph_type,
                            current_graph_spec.target_addr,
                            current_graph_spec.device,
                            current_graph_spec.core,
                            current_graph_spec.stream
                        );

                        let chart = Chart::new(vec![dataset])
                            .block(
                                Block::default()
                                    .title(chart_block_title)
                                    .borders(Borders::ALL)
                            )
                            .x_axis(Axis::default().title(x_axis_title_str).bounds(x_bounds_arr).labels(x_labels_vec))
                            .y_axis(Axis::default().title(y_axis_title_str).bounds(y_bounds_to_use).labels(y_labels_vec));
                        f.render_widget(chart, chart_area[i]);
                    }

                    let input = ratatui::widgets::Paragraph::new(text_input.as_str()).block(Block::default().title("Command (add <type> <addr> <dev_id> <core> <stream> <subcarrier_or_ignored_for_pdp> | remove <idx> | interval <idx> <value> | clear)").borders(Borders::ALL));
                    f.render_widget(input, chunks[1]);
                })?;
                last_tick = Instant::now();
            }
        }
    }

    fn entry_from_command(parts: Vec<&str>) -> Option<Graph> {
        let graph_type: GraphType = match parts[1].parse() {
            Ok(addr) => addr,
            Err(_) => return None, // Exit on invalid input
        };
        let addr: SocketAddr = match parts[2].parse() {
            Ok(addr) => addr,
            Err(_) => return None, // Exit on invalid input
        };

        let device_id: u64 = match parts[3].parse() {
            Ok(addr) => addr,
            Err(_) => return None, // Exit on invalid input
        };
        let core: usize = match parts[4].parse() {
            Ok(addr) => addr,
            Err(_) => return None, // Exit on invalid input
        };
        let stream: usize = match parts[5].parse() {
            Ok(addr) => addr,
            Err(_) => return None, // Exit on invalid input
        };
        // For PDP, subcarrier is not strictly needed as we use all of them.
        // We'll parse it but it will be ignored in process_data for PDP.
        // If not provided for PDP, we can default it or handle the shorter command.
        // For now, assume it's always provided for simplicity of command structure.
        let subcarrier: usize = if parts.len() > 6 {
            match parts[6].parse() {
                Ok(addr) => addr,
                Err(_) => return None, // Exit on invalid input
            }
        } else {
            0 // Default or indicate all subcarriers for PDP if not provided
        };

        Some(Graph {
            graph_type,
            target_addr: addr,
            device: device_id,
            core,
            stream,
            subcarrier,
            time_interval: 1000,
            y_axis_bounds: None, // Initialize y_axis_bounds
        })
    }

    async fn execute_command(text_input: String, graphs: Arc<Mutex<Vec<Graph>>>) {
        let parts: Vec<&str> = text_input.split_whitespace().collect();
        if parts.is_empty() {
            return;
        }

        match parts[0] {
            "add" if parts.len() == 7 => {
                let entry = match Self::entry_from_command(parts) {
                    None => return,
                    Some(entry) => entry,
                };
                graphs.lock().await.push(entry);
            }
            "remove" if parts.len() == 2 => {
                let entry: usize = match parts[1].parse::<usize>() {
                    Ok(number) => number,
                    Err(_) => return,
                };
                graphs.lock().await.remove(entry);
            }
            "interval" if parts.len() == 3 => {
                let graph_idx: usize = match parts[1].parse::<usize>() {
                    Ok(number) => number,
                    Err(_) => return,
                };
                let value: f64 = match parts[2].parse::<f64>() {
                    Ok(val) => val,
                    Err(_) => return,
                };

                let mut graphs_locked = graphs.lock().await;
                if let Some(graph_to_modify) = graphs_locked.get_mut(graph_idx) {
                    match graph_to_modify.graph_type {
                        GraphType::PDP => {
                            graph_to_modify.y_axis_bounds = Some([0.0, value]);
                        }
                        GraphType::Amplitude => {
                            graph_to_modify.time_interval = value as usize;
                        }
                    }
                }
            }
            "clear" => {
                graphs.lock().await.clear();
            }
            _ => {} // Dont execute on invalid input
        }
    }

    async fn plot_data_gui(&self) -> Result<(), Box<dyn std::error::Error>> {
        let tick_rate = Duration::from_millis(2000);
        let mut last_tick = Instant::now();

        // Source address, device id, core, stream, subcarrier
        let graphs: Arc<Mutex<Vec<Graph>>> = Arc::new(Mutex::new(Vec::new()));
        let graphs_2 = graphs.clone();

        tokio::spawn(async move {
            let stdin: BufReader<io::Stdin> = BufReader::new(io::stdin());
            let mut lines = stdin.lines();
            info!("GUI command listener started. Enter commands like: add pdp <addr> <dev_id> <core> <stream> 0");

            while let Ok(Some(line)) = lines.next_line().await {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                info!("GUI Command received: {line}");
                Self::execute_command(line.parse().unwrap(), graphs.clone()).await;
            }
        });

        loop {
            if last_tick.elapsed() >= tick_rate {
                for (i, graph_spec) in graphs_2.lock().await.clone().into_iter().enumerate() {
                    let processed_data = self.process_data(graph_spec).await;
                    if processed_data.0.is_empty() {
                        info!("No data to plot for graph {i}");
                        continue;
                    }
                    let chart = generate_chart_from_data(processed_data.0, &graph_spec.graph_type.to_string());
                    let filename = format!(
                        "{}_{}_{}_{}_c{}_s{}_chart.html",
                        i,
                        graph_spec.graph_type.to_string().to_lowercase(),
                        graph_spec.target_addr.to_string().replace(':', "-"),
                        graph_spec.device,
                        graph_spec.core,
                        graph_spec.stream
                    );
                    HtmlRenderer::new(format!("{} Plot", graph_spec.graph_type), 800, 600)
                        .theme(Theme::Default)
                        .save(&chart, &filename)
                        .expect("Failed to save chart");
                    info!("Saved chart to {filename}");
                }

                last_tick = Instant::now();
            }
        }
    }

    async fn client_task(&self, client: Arc<Mutex<TcpClient>>, target_addr: SocketAddr) -> Result<(), Box<dyn std::error::Error>> {
        info!("Client task");

        // Visualiser connects and subscribes to the target node on startup
        {
            // Locking the client within this lifetime ensures that the receiver task
            // only starts once the lock in this lifetime has been released
            let mut client = client.lock().await;
            client.connect(target_addr).await?;

            let msg = HostCtrl::Subscribe { device_id: 0 };
            client.send_message(target_addr, RpcMessageKind::HostCtrl(msg)).await?;
            info!("Subscribed to node {target_addr}");
            Ok(())
        }
    }
}

fn generate_chart_from_data(data: Vec<(f64, f64)>, title_str: &str) -> CharmingChart {
    use charming::component::Title;
    use charming::element::AxisType;
    use charming::series::Line;

    let x_data: Vec<String> = data.iter().map(|(x, _)| x.to_string()).collect(); // Convert f64 to String for x_axis data
    let y_data: Vec<f64> = data.iter().map(|(_, y)| *y).collect();

    CharmingChart::new()
        .title(Title::new().text(title_str))
        .x_axis(charming::component::Axis::new().type_(AxisType::Category).data(x_data))
        .y_axis(charming::component::Axis::new().type_(AxisType::Value))
        .series(Line::new().data(y_data))
}
