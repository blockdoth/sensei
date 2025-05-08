use crate::cli::{GlobalConfig, OrchestratorSubcommandArgs, VisualiserSubcommandArgs};
use crate::module::{CliInit, RunsServer};
use minifb::{Key, Window, WindowOptions};
use plotters::prelude::*;
use plotters_bitmap::BitMapBackend;
use std::cell::RefCell;
use tokio::sync::watch;
use tokio::sync::watch::{Receiver, Sender};

pub struct Visualiser {
    data: RefCell<Vec<u8>>,
    width: usize,
    height: usize,
}

impl CliInit<VisualiserSubcommandArgs> for Visualiser {
    fn init(config: &VisualiserSubcommandArgs, global: &GlobalConfig) -> Self {
        Visualiser {
            data: RefCell::new(vec![]),
            width: config.width,
            height: config.height,
        }
    }
}

impl Visualiser {
    pub fn receive_data_task(&self, tx: Sender<Vec<u8>>) {
        tokio::spawn(async move {
            let mut i = vec![10u8];
            loop {
                // TODO: Replace with tcp stream listening for data
                tx.send(i.clone()).expect("Channel closed");
                i[0] = (i[0] + 1) % 100;
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        });
    }

    pub fn output_data(&self) -> Vec<u8> {
        self.data.borrow().clone()
    }
}
impl RunsServer for Visualiser {
    async fn start_server(&self) -> Result<(), Box<dyn std::error::Error>> {
        let (tx, mut rx) = watch::channel::<Vec<u8>>(vec![0]);

        self.receive_data_task(tx);

        let mut window = Window::new("Data", self.width, self.height, WindowOptions::default())?;

        window.limit_update_rate(Some(std::time::Duration::from_micros(32000)));

        let mut minifb_buffer: Vec<u32> = vec![0; self.width * self.height];

        while window.is_open() && !window.is_key_down(Key::Escape) {
            rx.changed().await.expect("TODO: panic message");
            let sent_data = rx.borrow_and_update().clone();

            for point in sent_data {
                self.data.borrow_mut().push(point);
            }

            let current_data = self.data.borrow().clone();

            let max = *current_data.iter().max().unwrap() as f32;
            let min = *current_data.iter().min().unwrap() as f32;
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
                let mut chart = ChartBuilder::on(&root_drawing_area)
                    .caption("data", ("", (20).percent_height())) // Chart title and font
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
                            .map(|(i, x)| (i as f32, x as f32)), // Map to f32 values and x and y coords
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

        let output = self.output_data();
        println!("{output:?}");

        Ok(())
    }
}
