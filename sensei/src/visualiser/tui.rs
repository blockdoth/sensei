use std::cmp::max;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Axis, Block, Borders, Chart, Dataset, Padding, Paragraph, Wrap};

use crate::visualiser::state::{AddGraphFocus, AmpFocusField, Focus, Graph, GraphConfig, GraphType, PDPFocusField, VisState};

const PADDING: Padding = Padding::new(1, 1, 0, 0);

pub fn ui(f: &mut Frame, tui_state: &VisState) {
    let (charts_area, control_area) = split_chart_logs(f);
    let (config_area, log_area, footer_area) = split_config_logs_footer(control_area);

    render_graphs(f, charts_area, tui_state);
    render_logs(f, log_area, tui_state);
    render_config(f, config_area, tui_state);
    render_footer(f, footer_area, tui_state);
}

fn split_chart_logs(f: &Frame) -> (Rect, Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Percentage(60), // Chart content SocketAddrarea
            Constraint::Percentage(40), // Log area
        ])
        .split(f.area());
    (chunks[0], chunks[1])
}

fn split_config_logs_footer(area: Rect) -> (Rect, Rect, Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(4), // Config area
            Constraint::Min(0),    // Log area
            Constraint::Length(3), // Footer area
        ])
        .split(area);
    (chunks[0], chunks[1], chunks[2])
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
            .padding(PADDING)
            .title(Span::from(format!(" Log ({current_log_count})"))),
    );
    f.render_widget(widget, area);
}

fn render_graphs(f: &mut Frame, area: Rect, tui_state: &VisState) {
    let graph_count = max(1, tui_state.graphs.len());

    let charts_area = split_charts(area, graph_count);

    for (i, (graph, area)) in tui_state.graphs.iter().zip(charts_area).enumerate() {
        render_graph(f, area, graph, i, &tui_state.focus)
    }
}

fn render_graph(f: &mut Frame, area: Rect, graph: &Graph, i: usize, focus: &Focus) {
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
                "[ Amplitude Plot | DeviceID: {} | Core {} | Stream {} | Subcarriers {} ]",
                graph.device_id, amplitude_config.core, amplitude_config.stream, amplitude_config.subcarrier
            )
        }
        GraphConfig::PDP(pdpconfig) => {
            format!(
                "[ PDP Plot | DeviceID: {} | Core {} | Stream {} ]",
                graph.device_id, pdpconfig.core, pdpconfig.stream
            )
        }
    };
    let border_style = if let Focus::Graph(idx) = focus
        && *idx == i
    {
        Style::default().fg(Color::Blue)
    } else {
        Style::default()
    };

    let chart = Chart::new(vec![dataset])
        .block(
            Block::default()
                .title(chart_block_title)
                .borders(Borders::ALL)
                .border_style(border_style)
                .padding(PADDING),
        )
        .x_axis(Axis::default().title(x_axis_title_str).bounds(x_bounds_arr).labels(x_labels_vec))
        .y_axis(Axis::default().title(y_axis_title_str).bounds(y_bounds_to_use).labels(y_labels_vec));
    f.render_widget(chart, area);
}

fn render_config(f: &mut Frame, area: Rect, tui_state: &VisState) {
    // "Command (add <type> <addr> <dev_id> <core> <stream> <subcarrier_or_ignored_for_pdp>"

    let mut lines: Vec<Line> = vec![];

    let selected = |field: Focus, focus: &Focus| {
        if field == *focus {
            Style::default().bg(Color::Gray).fg(Color::Black)
        } else {
            Style::default()
        }
    };

    let connection = vec![
        Span::from("Address: "),
        Span::styled(tui_state.addr_input.to_string(), selected(Focus::Address, &tui_state.focus)),
        Span::from(format!(" [{:?}]", tui_state.connection_status)),
    ];

    lines.push(Line::from(connection));

    let mut add_graph = vec![
        Span::from("Device ID: "),
        Span::styled(tui_state.device_id_input.to_string(), selected(Focus::DeviceID, &tui_state.focus)),
        Span::from(" | Type: "),
        Span::styled(format!("[{}]", tui_state.graph_type_input), selected(Focus::GraphType, &tui_state.focus)),
    ];
    let add_graph_config = match tui_state.graph_type_input {
        GraphType::Amplitude => {
            vec![
                Span::from(" | Core: "),
                Span::styled(
                    format!("[{}]", tui_state.core_input),
                    selected(Focus::AddGraph(AddGraphFocus::AmpFocus(AmpFocusField::Core)), &tui_state.focus),
                ),
                Span::from(" | Stream: "),
                Span::styled(
                    format!("[{}]", tui_state.stream_input),
                    selected(Focus::AddGraph(AddGraphFocus::AmpFocus(AmpFocusField::Stream)), &tui_state.focus),
                ),
                Span::from(" | Subcarriers: "),
                Span::styled(
                    format!("[{}]", tui_state.subcarrier_input),
                    selected(Focus::AddGraph(AddGraphFocus::AmpFocus(AmpFocusField::Subcarriers)), &tui_state.focus),
                ),
            ]
        }
        GraphType::PDP => {
            vec![
                Span::from(" | Core: "),
                Span::styled(
                    format!("[{}]", tui_state.core_input),
                    selected(Focus::AddGraph(AddGraphFocus::PDPFocus(PDPFocusField::Core)), &tui_state.focus),
                ),
                Span::from(" | Stream: "),
                Span::styled(
                    format!("[{}]", tui_state.stream_input),
                    selected(Focus::AddGraph(AddGraphFocus::PDPFocus(PDPFocusField::Stream)), &tui_state.focus),
                ),
            ]
        }
    };
    add_graph.extend(add_graph_config);
    lines.push(Line::from(add_graph));

    let border_style = match tui_state.focus {
        Focus::Address | Focus::DeviceID | Focus::GraphType | Focus::AddGraph(_) => Style::default().fg(Color::Blue),
        _ => Style::default(),
    };

    let input = Paragraph::new(Text::from(lines)).block(
        Block::default()
            .title("Add graph")
            .borders(Borders::ALL)
            .border_style(border_style)
            .padding(PADDING),
    );

    f.render_widget(input, area);
}

fn render_footer(f: &mut Frame, area: Rect, tui_state: &VisState) {
    let widget = Paragraph::new(footer_text(tui_state))
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL).padding(PADDING).title(Span::from("Keybinds")));

    f.render_widget(widget, area);
}

fn footer_text(tui_state: &VisState) -> String {
    match &tui_state.focus {
        Focus::AddGraph(add_graph_focus) => match add_graph_focus {
            AddGraphFocus::PDPFocus(_) => "[↑↓] increase/decrease | [Enter] Add Graph | [Q]uit",
            AddGraphFocus::AmpFocus(_) => "[↑↓] increase/decrease | [Enter] Add Graph | [Q]uit",
        },
        Focus::GraphType => "[↑↓] Change type | [Enter] Add Graph | [Q]uit",
        Focus::Address => "[Enter] Connect | [Q]uit",
        Focus::DeviceID => "[Enter] Add Graph | [Q]uit",
        Focus::Graph(_) => "[-+] Change Interval | [D]elete graph | [Q]uit",
    }
    .to_owned()
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
    let y_bounds_to_use = if let GraphConfig::PDP(config) = graph_type {
        config.y_axis_bounds
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
