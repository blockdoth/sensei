use crate::cli::{GlobalConfig, OrchestratorSubcommandArgs, VisualiserSubcommandArgs};
use crate::module::{CliInit, RunsServer};
use async_trait::async_trait;
use lib::csi_types::CsiData;
use lib::errors::NetworkError;
use lib::network::rpc_message::CtrlMsg::*;
use lib::network::rpc_message::DataMsg::*;
use lib::network::rpc_message::RpcMessageKind::{Ctrl, Data};
use lib::network::rpc_message::{AdapterMode, CtrlMsg, RpcMessage};
use lib::network::tcp::client::TcpClient;
use lib::network::tcp::server::TcpServer;
use lib::network::tcp::{ChannelMsg, ConnectionHandler};
use log::{debug, info, warn};
use minifb::{Key, Window, WindowOptions};
use plotters::coord::ranged1d::ReversibleRanged;
use plotters::prelude::*;
use plotters_bitmap::BitMapBackend;
use ratatui::Terminal;
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{EnterAlternateScreen, enable_raw_mode, disable_raw_mode, LeaveAlternateScreen};
use ratatui::layout::{Constraint, Layout};
use ratatui::prelude::Direction;
use std::cell::RefCell;
use std::io::stdout;
use std::net::SocketAddr;
use std::ops::DerefMut;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tokio::io;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::sync::watch::{Receiver, Sender};
use tokio::sync::{Mutex, watch};

pub struct Visualiser {
    data: Arc<Mutex<Vec<Vec<CsiData>>>>, // Devices x CsiData over time
    width: usize,
    height: usize,
    target_addr: SocketAddr,
    ui_type: String,
}

impl CliInit<VisualiserSubcommandArgs> for Visualiser {
    fn init(config: &VisualiserSubcommandArgs, global: &GlobalConfig) -> Self {
        Visualiser {
            data: Arc::new(Mutex::new(vec![])),
            width: config.width,
            height: config.height,
            target_addr: global.socket_addr,
            ui_type: config.ui_type.clone(),
        }
    }
}

impl Visualiser {
    fn receive_data_task(
        &self,
        data: Arc<Mutex<Vec<Vec<CsiData>>>>,
        client: Arc<Mutex<TcpClient>>,
        target_addr: SocketAddr,
    ) {
        tokio::spawn(async move {
            debug!("Receive task");

            loop {
                let mut client = client.lock().await;
                debug!("Client locked");
                client.read_message(target_addr).await;
            }
        });
    }

    async fn output_data(&self) -> Vec<Vec<CsiData>> {
        self.data.lock().await.clone()
    }
    
    async fn plot_data_tui(&self) -> Result<(), Box<dyn std::error::Error>> {
        enable_raw_mode()?;
        let mut stdout = stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        let tick_rate = Duration::from_millis(200);
        let mut last_tick = Instant::now();

        
        // Shutdown process
        disable_raw_mode()?;
        execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        )?;
        terminal.show_cursor()?;

        Ok(())
    }

    async fn plot_data_gui(&self) -> Result<(), Box<dyn std::error::Error>> {
        let mut window = Window::new("Data", self.width, self.height, WindowOptions::default())?;

        window.limit_update_rate(Some(std::time::Duration::from_micros(32000)));

        let mut minifb_buffer: Vec<u32> = vec![0; self.width * self.height];

        'outer: while window.is_open() && !window.is_key_down(Key::Escape) {
            for _ in 0..50 {
                if !window.is_open() {
                    break;
                }
                if window.is_key_down(Key::Escape) {
                    break 'outer;
                }
                window.update();
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }

            let current_data = self.output_data().await;

            let max = 10f32;
            let min = -10f32;
            let len = current_data.len() as f32;

            // Plotters will draw to an RGB u8 buffer. We need a temporary buffer for that.
            // Each pixel requires 3 bytes (Red, Green, Blue).
            let mut plot_buffer = vec![0u8; self.width * self.height * 3];

            // Create a BitMapBackend. This backend wraps our `plot_buffer` and allows plotters to draw onto it.
            // The scope `{}` is used here to ensure that `root` (and its borrow of `plot_buffer`)
            // is dropped before we try to read from `plot_buffer` later.
            {
                let root_drawing_area = BitMapBackend::with_buffer(
                    &mut plot_buffer,
                    (self.width as u32, self.height as u32),
                )
                .into_drawing_area();

                // Fill the background of the drawing area with white.
                root_drawing_area.fill(&WHITE)?;

                // ChartBuilder is used to configure and build the chart.
                let mut chart =
                    ChartBuilder::on(&root_drawing_area) // Chart title and font
                        .margin(10) // Margin around the chart
                        .x_label_area_size((10).percent_height()) // Space for X-axis labels
                        .y_label_area_size((10).percent_width()) // Space for Y-axis labels
                        .build_cartesian_2d(0f32..len, min - 1f32..max + 1f32)?; // Define X and Y axis ranges

                // Configure the mesh (grid lines) for the chart.
                chart
                    .configure_mesh()
                    .x_desc("X Axis") // X-axis description
                    .y_desc("Y Axis") // Y-axis description
                    .draw()?;

                // Plot the data.
                chart
                    .draw_series(LineSeries::new(
                        (current_data.into_iter()) // Create an iterator on the data
                            .enumerate()
                            .map(|(i, x)| (i as f32, x[0].timestamp as f32)), // Map to f32 values and x and y coords
                        &RED, // Plot the line in red
                    ))?
                    .label("Data format") // Add a label for the series
                    .legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], RED)); // Configure legend

                // Configure and draw the series labels (legend).
                chart
                    .configure_series_labels()
                    .background_style(WHITE.mix(0.8)) // Legend background color
                    .border_style(BLACK) // Legend border color
                    .draw()?;

                // Present all the drawing operations.
                // This flushes the drawing to the `plot_buffer`.
                root_drawing_area.present()?;
            }

            // Convert the plot_buffer (RGB u8 format) to minifb_buffer (u32 XRGB format)
            // `plot_buffer` contains R, G, B bytes sequentially.
            // `minifb_buffer` expects one u32 per pixel. We'll pack RGB into a u32.
            for (i, pixel_u32) in minifb_buffer.iter_mut().enumerate() {
                let r_idx = i * 3; // Index for Red component
                let g_idx = i * 3 + 1; // Index for Green component
                let b_idx = i * 3 + 2; // Index for Blue component

                let r = plot_buffer[r_idx] as u32;
                let g = plot_buffer[g_idx] as u32;
                let b = plot_buffer[b_idx] as u32;

                // Combine R, G, B into a single u32 value (0x00RRGGBB)
                *pixel_u32 = (r << 16) | (g << 8) | b;
            }

            // Update the window with the contents of minifb_buffer.
            window.update_with_buffer(&minifb_buffer, self.width, self.height)?;
        }

        let output = self.output_data().await;
        println!("{output:?}");

        Ok(())
    }

    fn client_task(&self, client: Arc<Mutex<TcpClient>>, target_addr: SocketAddr) {
        tokio::spawn(async move {
            debug!("Client task");

            // Visualiser connects to the target node on startup
            {
                // Locking the client within this lifetime ensures that the receiver task
                // only starts once the lock in this lifetime has been released
                let mut client = client.lock().await;
                client.connect(target_addr).await;

                let msg = RpcMessage {
                    src_addr: client.self_addr.unwrap(),
                    target_addr: target_addr,
                    msg: Ctrl(CtrlMsg::Connect),
                };

                client.send_message(target_addr, msg).await;
            }

            let stdin: BufReader<io::Stdin> = BufReader::new(io::stdin());
            let mut lines = stdin.lines();

            while let Ok(Some(line)) = lines.next_line().await {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }

                if line.eq("help") {
                    todo!("Help message for visualiser")
                }

                let mut client = client.lock().await;
                match line.parse::<CtrlMsg>() {
                    Ok(Connect) => {
                        client.connect(target_addr).await;

                        let msg = RpcMessage {
                            src_addr: client.self_addr.unwrap(),
                            target_addr: target_addr,
                            msg: Ctrl(CtrlMsg::Connect),
                        };

                        client.send_message(target_addr, msg).await;
                    }

                    Ok(Disconnect) => {
                        client.disconnect(target_addr).await;
                    }

                    Ok(Subscribe { device_id, mode }) => {
                        let src_addr = client.self_addr.unwrap();
                        let msg = RpcMessage {
                            src_addr,
                            target_addr: target_addr,
                            msg: Ctrl(CtrlMsg::Subscribe {
                                device_id: 0,
                                mode: AdapterMode::RAW,
                            }),
                        };
                        client.send_message(target_addr, msg).await;
                        info!("Subscribed to node {}", target_addr)
                    }

                    Ok(Unsubscribe { device_id }) => {
                        let src_addr = client.self_addr.unwrap();
                        let msg = RpcMessage {
                            src_addr,
                            target_addr: target_addr,
                            msg: Ctrl(CtrlMsg::Unsubscribe { device_id: 0 }),
                        };
                        client.send_message(target_addr, msg);
                        info!("Subscribed from node {}", target_addr)
                    }

                    _ => {}
                }
            }
        });
    }
}
impl RunsServer for Visualiser {
    async fn start_server(self: Arc<Visualiser>) -> Result<(), Box<dyn std::error::Error>> {
        // Technically, the visualiser has cli tools for connecting to multiple nodes
        // At the moment, it is sufficient to connect to one target node on startup
        // Manually start the subscription by typing subscribe
        let client = Arc::new(Mutex::new(TcpClient::new().await));
        self.client_task(client.clone(), self.target_addr);
        self.receive_data_task(self.data.clone(), client.clone(), self.target_addr);

        if (self.ui_type == "tui") {
            self.plot_data_gui().await?;
        } else {
        }

        Ok(())
    }
}
