use std::cmp::{max, min};
use std::usize;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Axis, Block, Borders, Chart, Dataset, Paragraph, Wrap};

use crate::visualiser::state::{Graph, GraphConfig, VisState};

pub fn ui(f: &mut Frame, tui_state: &VisState) {
    let (charts_area, control_area) = split_chart_logs(f);
    let (config_area, log_area) = split_config_logs(control_area);

    render_graphs(f, charts_area, tui_state);

    render_logs(f, log_area, tui_state);

    let input = Paragraph::new("balls").block(Block::default().title("Command (add <type> <addr> <dev_id> <core> <stream> <subcarrier_or_ignored_for_pdp> | remove <idx> | interval <idx> <value> | clear)").borders(Borders::ALL));
    f.render_widget(input, config_area);
}

fn split_chart_logs(f: &Frame) -> (Rect, Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Percentage(70),    // Chart content area
            Constraint::Percentage(30), // Log area
        ])
        .split(f.area());
    (chunks[0], chunks[1])
}

fn split_config_logs(area: Rect) -> (Rect, Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(3), // Log area
            Constraint::Min(0), // Config area
        ])
        .split(area);
    (chunks[0], chunks[1])
}

fn split_charts(area: Rect, graph_count: usize) -> Vec<Rect> {
    Layout::default()
        .direction(Direction::Horizontal)
        .margin(1)
        .constraints(vec![Constraint::Percentage(100 / graph_count as u16); graph_count])
        .split(area)
        .to_vec()
}

fn render_logs(f: &mut Frame, area: Rect, tui_state: &VisState) {
    let current_log_count = tui_state.logs.len();
    let start_index = current_log_count
        .saturating_sub(area.height.saturating_sub(2) as usize)
        .saturating_sub(tui_state.logs_scroll_offset);

    let logs: Vec<Line> = tui_state.logs.iter().skip(start_index).map(|entry| entry.format()).collect();

    let widget = Paragraph::new(Text::from(logs)).wrap(Wrap { trim: true }).block(
        Block::default()
            .borders(Borders::ALL)
            .title(Span::from(format!(" Log ({current_log_count})"))),
    );
    f.render_widget(widget, area);
}

fn render_graphs(f: &mut Frame, area: Rect, tui_state: &VisState) {
    let graph_count = max(1, tui_state.graphs.len());


    let charts_area = split_charts(area, graph_count);
    for (i, (graph, area)) in tui_state.graphs.iter().zip(charts_area).enumerate() {
        render_graph(f, area, graph, i)
    }
}

fn render_graph(f: &mut Frame, area: Rect, graph: &Graph, i: usize) {
    let dataset = Dataset::default()
        .name(format!("Graph #{i}"))
        .marker(ratatui::symbols::Marker::Braille)
        .graph_type(ratatui::widgets::GraphType::Line)
        .style(Style::default().fg(Color::Cyan))
        .data(&graph.data);

    let (x_axis_title_str, x_bounds_arr, x_labels_vec) = get_x_axis_config(graph.gtype, &graph.data);

    let (y_axis_title_str, y_bounds_to_use, y_labels_vec) = get_y_axis_config(graph.gtype, &graph.data);

    let chart_block_title = match graph.gtype {
        GraphConfig::Amplitude(amplitude_config) => {
            format!(
                "Chart {} - Amp @ {} dev {} C{} S{} Sub {}",
                i, graph.target_addr, graph.device_id, amplitude_config.core, amplitude_config.stream, amplitude_config.subcarrier
            )
        }
        GraphConfig::PDP(pdpconfig) => {
            format!(
                "Chart {} - PDP @ {} dev {} C{} S{}",
                i, graph.target_addr, graph.device_id, pdpconfig.core, pdpconfig.stream
            )
        }
    };

    let chart = Chart::new(vec![dataset])
        .block(Block::default().title(chart_block_title).borders(Borders::ALL))
        .x_axis(Axis::default().title(x_axis_title_str).bounds(x_bounds_arr).labels(x_labels_vec))
        .y_axis(Axis::default().title(y_axis_title_str).bounds(y_bounds_to_use).labels(y_labels_vec));
    f.render_widget(chart, area);
}

fn get_x_axis_config(graph_type: GraphConfig, data_points: &[(f64, f64)]) -> (String, [f64; 2], Vec<Span>) {
    match graph_type {
        GraphConfig::Amplitude(config) => {
            let time_max = data_points.iter().max_by(|x, y| x.0.total_cmp(&y.0)).unwrap_or(&(0f64, 0f64)).0;
            let bounds = [(time_max - config.time_range as f64 - 1f64).round(), (time_max + 1f64).round()];
            let labels = bounds.iter().map(|n| Span::from(n.to_string())).collect();
            ("Time".to_string(), bounds, labels)
        }
        GraphConfig::PDP(_) => {
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

fn get_y_axis_config(graph_type: GraphConfig, data_points: &[(f64, f64)]) -> (String, [f64; 2], Vec<Span>) {
    let y_bounds_to_use = if let GraphConfig::PDP(config) = graph_type
        && let Some(bounds) = config.y_axis_bounds
    {
        bounds
    } else {
        calculate_dynamic_bounds(data_points)
    };

    let data_labels: Vec<Span> = vec![
        Span::from(format!("{:.2}", y_bounds_to_use[0])),
        Span::from(format!("{:.2}", y_bounds_to_use[1])),
    ];

    let y_axis_title_str = match graph_type {
        GraphConfig::Amplitude(_) => "Amptitude".to_string(),
        GraphConfig::PDP(_) => "Power".to_string(),
    };

    (y_axis_title_str, y_bounds_to_use, data_labels)
}
