use std::cell::RefCell;
use std::cmp::PartialEq;
use std::collections::HashMap;
use std::io::stdout;
use std::net::SocketAddr;
use std::num::ParseIntError;
use std::ops::DerefMut;
use std::str::FromStr;
use std::sync::mpsc::channel;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use std::{fmt, fs};

use async_trait::async_trait;
use charming::HtmlRenderer;
use charming::component::Title;
use charming::element::AxisType;
use charming::series::{Line, Scatter};
use charming::theme::Theme;
use lib::csi_types::{Complex, CsiData};
use lib::errors::NetworkError;
use lib::network::rpc_message::AdapterMode::SOURCE;
use lib::network::rpc_message::CtrlMsg::*;
use lib::network::rpc_message::DataMsg::*;
use lib::network::rpc_message::RpcMessageKind::{Ctrl, Data};
use lib::network::rpc_message::{AdapterMode, CtrlMsg, RpcMessage, RpcMessageKind};
use lib::network::tcp::client::TcpClient;
use lib::network::tcp::server::TcpServer;
use lib::network::tcp::{ChannelMsg, ConnectionHandler};
use log::{debug, info, warn};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::crossterm::cursor::{Hide, Show};
use ratatui::crossterm::event::{DisableMouseCapture, EnableMouseCapture, Event, KeyCode};
use ratatui::crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use ratatui::crossterm::{event, execute};
use ratatui::layout::{Constraint, Layout, Position};
use ratatui::prelude::Direction;
use ratatui::style::{Color, Style};
use ratatui::symbols::line;
use ratatui::text::{Span, ToLine};
use ratatui::widgets::{Axis, Block, Borders, Chart, Dataset};
use ratatui::{Frame, Terminal, symbols};
use tokio::io;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::sync::watch::{Receiver, Sender};
use tokio::sync::{Mutex, watch};
use warp::Filter;

use crate::cli::{GlobalConfig, OrchestratorSubcommandArgs, VisualiserSubcommandArgs};
use crate::config::{DEFAULT_ADDRESS, OrchestratorConfig, VisualiserConfig};
use crate::module::Run;
use crate::orchestrator::Orchestrator;
use crate::visualiser::GraphType::Amplitude;

pub struct Visualiser {
    // This seemed to me the best way to structure the data, as the socketaddr is a primary key for each node, and each device has a unique id only within a node
    #[allow(clippy::type_complexity)]
    data: Arc<Mutex<HashMap<SocketAddr, HashMap<u64, Vec<CsiData>>>>>, // Nodes x Devices x CsiData over time
    target_addr: SocketAddr,
    ui_type: String,
}

impl Run<VisualiserConfig> for Visualiser {
    fn new(config: VisualiserConfig) -> Self {
        Visualiser {
            data: Arc::new(Default::default()),
            target_addr: config.target,
            ui_type: config.ui_type,
        }
    }

    async fn run(&self, config: VisualiserConfig) -> Result<(), Box<dyn std::error::Error>> {
        // Technically, the visualiser has cli tools for connecting to multiple nodes
        // At the moment, it is sufficient to connect to one target node on startup
        // Manually start the subscription by typing subscribe
        let client = Arc::new(Mutex::new(TcpClient::new()));
        self.client_task(client.clone(), self.target_addr).await;
        self.receive_data_task(self.data.clone(), client.clone(), self.target_addr);

        io::stdout().flush().await;

        if (self.ui_type == "tui") {
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
    time_interval: usize,
}

impl PartialEq for Graph {
    fn eq(&self, other: &Self) -> bool {
        self.graph_type == other.graph_type
            && self.target_addr == other.target_addr
            && self.device == other.device
            && self.core == other.core
            && self.stream == other.stream
            && self.subcarrier == other.subcarrier
    }
}

#[derive(Debug, Clone, Copy)]
pub enum GraphType {
    Amplitude,
}

impl FromStr for GraphType {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "amp" => Ok(GraphType::Amplitude),
            "amplitude" => Ok(GraphType::Amplitude),
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
                        let RpcMessage { msg, src_addr, target_addr } = msg;
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

    /// Data is divided into:
    /// Nodes in outer hashmap, devices in inner hashmap.
    /// Then you get to the vec of csi data from that device.
    /// There is a timestamp for each csi data, and that data can be reduced by core, stream and subcarrier.
    ///
    /// Processing data turns each data point into a vec of tuples (timestamp, datapoint), such that it can be charted easily.
    async fn process_data(&self, graph: Graph) -> Vec<(f64, f64)> {
        // TODO: More processing types
        let target_addr = graph.target_addr;
        let device = graph.device;
        let core = graph.core;
        let stream = graph.stream;
        let subcarrier = graph.subcarrier;

        let data = match self.data.lock().await.get(&target_addr) {
            Some(data) => match data.get(&device) {
                Some(data) => data.clone(),
                None => return vec![],
            },
            None => return vec![],
        };

        data.iter().map(|x| (x.timestamp, x.csi[core][stream][subcarrier].re)).collect()
    }

    async fn plot_data_tui(&self) -> Result<(), Box<dyn std::error::Error>> {
        enable_raw_mode()?;
        let mut stdout = stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture, Hide)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        self.tui_loop(&mut terminal).await;

        // Shutdown process
        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture, Show,)?;
        terminal.show_cursor()?;

        Ok(())
    }

    async fn tui_loop<B: Backend>(&self, terminal: &mut Terminal<B>) -> io::Result<()> {
        let tick_rate = Duration::from_millis(100);
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
                let mut current_data = Vec::new();
                let mut types = Vec::new();
                let mut intervals = Vec::new();

                for graph in graphs.lock().await.iter() {
                    current_data.push(self.process_data(*graph).await);
                    types.push(graph.graph_type.clone().to_string());
                    intervals.push(graph.time_interval);
                }

                terminal.draw(|f| {
                    let size = f.area();

                    let chunks = Layout::default()
                        .direction(Direction::Vertical)
                        .margin(1)
                        .constraints([Constraint::Percentage(80), Constraint::Length(3)].as_ref())
                        .split(size);

                    let graph_count = if (current_data.is_empty()) { 1 } else { current_data.len() };
                    let constraints = vec![Constraint::Percentage(100 / graph_count as u16); graph_count];
                    let chart_area = Layout::default()
                        .direction(Direction::Horizontal)
                        .margin(1)
                        .constraints(constraints)
                        .split(chunks[0]);

                    for (i, data) in current_data.iter().enumerate() {
                        let dataset = Dataset::default()
                            .name(format!("Graph #{i}"))
                            .marker(ratatui::symbols::Marker::Braille)
                            .graph_type(ratatui::widgets::GraphType::Line)
                            .style(Style::default().fg(Color::Cyan))
                            .data(data);

                        let time_max = data.iter().max_by(|x, y| x.0.total_cmp(&y.0)).unwrap_or(&(0f64, 10000f64)).0;

                        let time_bounds = [(time_max - intervals[i] as f64 - 1f64).round(), (time_max + 1f64).round()];
                        let time_labels: Vec<Span> = time_bounds.iter().map(|n| Span::from(n.to_string())).collect();

                        let data_bounds = [
                            (data.iter().min_by(|x, y| x.1.total_cmp(&y.1)).unwrap_or(&(0f64, 10000f64)).1 - 1f64).round(),
                            (data.iter().max_by(|x, y| x.1.total_cmp(&y.1)).unwrap_or(&(0f64, 10000f64)).1 + 1f64).round(),
                        ];
                        let data_labels: Vec<Span> = data_bounds.iter().map(|n| Span::from(n.to_string())).collect();

                        let chart = Chart::new(vec![dataset])
                            .block(
                                Block::default()
                                    .title(format!("Chart {i}")) // TODO: Add descriptive title to chart
                                    .borders(Borders::ALL),
                            )
                            .x_axis(Axis::default().title("Time").bounds(time_bounds).labels(time_labels))
                            .y_axis(Axis::default().title(types[i].clone()).bounds(data_bounds).labels(data_labels));
                        f.render_widget(chart, chart_area[i]);
                    }

                    let input = ratatui::widgets::Paragraph::new(text_input.as_str()).block(Block::default().title("Command").borders(Borders::ALL));
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
        let subcarrier: usize = match parts[6].parse() {
            Ok(addr) => addr,
            Err(_) => return None, // Exit on invalid input
        };

        Some(Graph {
            graph_type,
            target_addr: addr,
            device: device_id,
            core,
            stream,
            subcarrier,
            time_interval: 1000,
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
                graphs.lock().await[parts[1].parse::<usize>().unwrap()].time_interval = parts[2].parse::<usize>().unwrap();
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

            while let Ok(Some(line)) = lines.next_line().await {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }

                Self::execute_command(line.parse().unwrap(), graphs.clone()).await;
            }
        });

        loop {
            let timeout = tick_rate.checked_sub(last_tick.elapsed()).unwrap_or_else(|| Duration::from_secs(0));

            if last_tick.elapsed() >= tick_rate {
                for (i, graph) in graphs_2.lock().await.clone().into_iter().enumerate() {
                    let chart = Self::generate_chart_from_data(self.process_data(graph).await);
                    HtmlRenderer::new("Example Chart", 800, 600)
                        .theme(Theme::Default)
                        .save(&chart, format!("{i}chart.html"))
                        .expect("TODO: panic message");
                }

                last_tick = Instant::now();
            }
        }

        Ok(())
    }

    fn generate_chart_from_data(data: Vec<(f64, f64)>) -> charming::Chart {
        let data = data.into_iter().map(|(x, y)| vec![x, y]).collect();
        charming::Chart::new()
            .title(Title::new().text("Data plot"))
            .x_axis(charming::component::Axis::new().type_(AxisType::Value))
            .y_axis(charming::component::Axis::new().type_(AxisType::Value))
            .series(Line::new().data(data))
    }

    async fn client_task(&self, client: Arc<Mutex<TcpClient>>, target_addr: SocketAddr) {
        info!("Client task");

        // Visualiser connects and subscribes to the target node on startup
        {
            // Locking the client within this lifetime ensures that the receiver task
            // only starts once the lock in this lifetime has been released
            let mut client = client.lock().await;
            client.connect(target_addr).await;

            let msg = Ctrl(CtrlMsg::Subscribe { device_id: 0 });
            client.send_message(target_addr, msg).await;
            info!("Subscribed to node {target_addr}")
        }
    }
}
