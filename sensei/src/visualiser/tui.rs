use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::Color;
use ratatui::text::Span;
use ratatui::widgets::{Axis, Block, Chart, Dataset};

use crate::visualiser::state::{GraphType, VisState};

pub fn ui(f: &mut Frame, tui_state: &VisState) {
    let size = f.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([Constraint::Percentage(80), Constraint::Length(3)].as_ref())
        .split(size);

    // Use data_to_render_this_frame for graph_count and iteration
    let graph_count = tui_state.graph_data.len();

    let constraints = vec![Constraint::Percentage(100 / graph_count as u16); graph_count];

    let chart_area = Layout::default()
        .direction(Direction::Horizontal)
        .margin(1)
        .constraints(constraints)
        .split(chunks[0]);

    for (i, data_points) in tui_state.graph_data.iter().enumerate() {
        // Iterate over data_to_render_this_frame
        // graphs_snapshot still provides the spec (title, type, etc.)
        let current_graph_spec = &data_points[i];

        let dataset = Dataset::default()
            .name(format!("Graph #{i}"))
            .marker(ratatui::symbols::Marker::Braille)
            .graph_type(ratatui::widgets::GraphType::Line)
            .style(Style::default().fg(Color::Cyan))
            .data(data_points);

        let (x_axis_title_str, x_bounds_arr, x_labels_vec) =
            Self::get_x_axis_config(tui_state.graph_state.graph_type, data_points, tui_state.graph_state.time_interval);

        let (y_axis_title_str, y_bounds_to_use, y_labels_vec) =
            Self::get_y_axis_config(current_graph_spec.graph_type, data_points, tui_state.graph_state.y_axis_bounds);
        let chart_block_title = format!(
            "Chart {} - {} @ {} dev {} C{} S{}",
            i,
            tui_state.graph_state.graph_type,
            tui_state.graph_state.target_addr,
            tui_state.graph_state.device,
            tui_state.graph_state.core,
            tui_state.graph_state.stream
        );

        let chart = Chart::new(vec![dataset])
            .block(Block::default().title(chart_block_title).borders(Borders::ALL))
            .x_axis(Axis::default().title(x_axis_title_str).bounds(x_bounds_arr).labels(x_labels_vec))
            .y_axis(Axis::default().title(y_axis_title_str).bounds(y_bounds_to_use).labels(y_labels_vec));
        f.render_widget(chart, chart_area[i]);
    }

    let input = ratatui::widgets::Paragraph::new(text_input.as_str()).block(Block::default().title("Command (add <type> <addr> <dev_id> <core> <stream> <subcarrier_or_ignored_for_pdp> | remove <idx> | interval <idx> <value> | clear)").borders(Borders::ALL));
    f.render_widget(input, chunks[1]);
}

fn get_x_axis_config(graph_type: GraphType, data_points: &[(f64, f64)], time_interval: usize) -> (String, [f64; 2], Vec<Span>) {
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
            let mut labels = vec![Span::from(bounds[0].floor().to_string()), Span::from(bounds[1].floor().to_string())];
            labels.dedup_by(|a, b| a.content == b.content);
            if labels.is_empty() {
                labels.push(Span::from("0"));
            }
            ("Delay Bin".to_string(), bounds, labels)
        }
    }
}

fn calculate_dynamic_bounds(data_points: &[(f64, f64)]) -> [f64; 2] {
    let (min_val, max_val) = data_points
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(min_acc, max_acc), &(_, y)| {
            (min_acc.min(y), max_acc.max(y))
        });

    if min_val.is_infinite() || max_val.is_infinite() {
        // Handle empty data: return default bounds [0.0, 1.0] directly.
        // This fixes the test case for empty_data expecting [0.0, 1.0].
        [0.0, 1.0]
    } else if (max_val - min_val).abs() < f64::EPSILON {
        // Handle data with all same y-values: return [y - 0.5, y + 0.5] directly.
        // This fixes the test case for same_data expecting [y - 0.5, y + 0.5].
        [min_val - 0.5, max_val + 0.5] // min_val can be used as it's same as max_val
    } else {
        // Normal case: data has a range of y-values.
        // Calculate padding based on the original min_val and max_val from fold.
        let data_range = max_val - min_val;
        let padding = (data_range * 0.05).max(0.1); // Ensure a minimum padding of 0.1
        [min_val - padding, max_val + padding]
    }
}

fn get_y_axis_config(graph_type: GraphType, data_points: &[(f64, f64)], y_axis_bounds_spec: Option<[f64; 2]>) -> (String, [f64; 2], Vec<Span>) {
    let y_bounds_to_use = if graph_type == GraphType::PDP {
        if let Some(bounds) = y_axis_bounds_spec {
            bounds
        } else {
            // Default behavior for PDP if y_axis_bounds_spec is None
            Self::calculate_dynamic_bounds(data_points)
        }
    } else {
        Self::calculate_dynamic_bounds(data_points)
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
