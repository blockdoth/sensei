use chrono::{TimeZone, Utc};
use lib::experiments::ExperimentStatus;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Padding, Paragraph, Wrap};

use super::state::OrgTuiState;
use crate::orchestrator::state::{DeviceStatus, Focus, FocusExp, FocusHost, FocusReg, HostStatus, RegistryStatus};

const HEADER_STYLE: Style = Style {
    fg: None,                        // No foreground color
    bg: None,                        // No background color
    underline_color: None,           // No underline color
    add_modifier: Modifier::empty(), // No text modifiers
    sub_modifier: Modifier::empty(), // No modifiers to remove
};

const PADDING: Padding = Padding::new(1, 1, 0, 0);

pub fn ui(f: &mut Frame, tui_state: &OrgTuiState) {
    let (main_area, footer_area) = split_main_footer(f);
    let (info_area, config_area) = split_info_config(main_area);
    let (log_area, csi_area) = split_log_csi(info_area);
    let (registry_area, hosts_area, experiments_area) = split_registry_hosts_experiments(config_area);

    render_logs(f, tui_state, log_area);
    render_csi(f, tui_state, csi_area);
    render_registry(f, tui_state, registry_area);
    render_hosts(f, tui_state, hosts_area);
    render_experiments(f, tui_state, experiments_area);
    render_footer(f, tui_state, footer_area);
}

// === Splits ===

/// Splits the full frame into two vertical chunks:
/// - Main content area
/// - Footer
fn split_main_footer(f: &Frame) -> (Rect, Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Min(0),    // Main content area
            Constraint::Length(3), // Footer: for info/errors
        ])
        .split(f.area());
    (chunks[0], chunks[1])
}

/// Splits a given area horizontally into:
/// - Left panel: Information or status display
/// - Right panel: Configuration area
fn split_info_config(area: Rect) -> (Rect, Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(0),     // Left panel: status
            Constraint::Length(44), // Right panel: config
        ])
        .split(area);
    (chunks[0], chunks[1])
}

/// Splits a given area vertically into two equal parts:
/// - Top: Log output
/// - Bottom: CSI (e.g., system information or metrics)
fn split_log_csi(area: Rect) -> (Rect, Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(50), // Left panel: logs
            Constraint::Percentage(50), // Right panel: csi
        ])
        .split(area);
    (chunks[0], chunks[1])
}

/// Splits a given area vertically into three equal parts:
/// - Top: Registry address display
/// - Middle: Hosts listing
/// - Bottom: Experiment controls
fn split_registry_hosts_experiments(area: Rect) -> (Rect, Rect, Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0), // Registry address
            Constraint::Min(0), // Hosts
            Constraint::Min(0), // Experiment Control
        ])
        .split(area);
    (chunks[0], chunks[1], chunks[2])
}

// === Misc helpers ===

/// Creates a styled horizontal divider line to visually separate UI sections.
///
/// # Parameters
/// - `area`: The `Rect` representing the area where the line will be drawn.
///   Used to calculate the width of the divider.
///
/// # Returns
/// - A `Line` containing a dimmed horizontal rule (`─`), adjusted to fit
///   the area width minus 4 units for padding or margins.r
fn divider(area: Rect) -> Line<'static> {
    Line::from(vec![Span::styled(
        "─".repeat((area.width as usize).saturating_sub(4)),
        Style::default().add_modifier(Modifier::DIM),
    )])
}

// === Render functions ===

/// Renders the logs panel in the TUI.
/// Displays formatted log entries with scroll support and highlights the panel if focused.
fn render_logs(f: &mut Frame, tui_state: &OrgTuiState, area: Rect) {
    let current_log_count = tui_state.logs.len();
    let start_index = current_log_count
        .saturating_sub(area.height.saturating_sub(2) as usize)
        .saturating_sub(tui_state.logs_scroll_offset);

    let logs: Vec<Line> = tui_state.logs.iter().skip(start_index).map(|entry| entry.format()).collect();

    let border_style = if matches!(tui_state.focussed_panel, Focus::Logs) {
        Style::default().fg(Color::Blue)
    } else {
        Style::default()
    };
    let widget = Paragraph::new(Text::from(logs)).wrap(Wrap { trim: true }).block(
        Block::default()
            .padding(PADDING)
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(Span::styled(format!(" Log ({current_log_count})"), HEADER_STYLE)),
    );
    f.render_widget(widget, area);
}

/// Renders the CSI (Channel State Information) panel in the TUI.
/// Displays a timestamped list of CSI entries with basic metadata and subcarrier counts.
fn render_csi(f: &mut Frame, tui_state: &OrgTuiState, area: Rect) {
    let current_log_count = tui_state.logs.len();
    let start_index = current_log_count
        .saturating_sub(area.height.saturating_sub(2) as usize)
        .saturating_sub(tui_state.logs_scroll_offset);
    // let logs: Vec<Line> = tui_state.csi.iter().skip(start_index).map(|entry| entry.format()).collect();
    let mut lines = vec![];

    lines.push(Line::from("Timestamp(s) Seq RSSI Subcarriers"));

    tui_state
        .csi
        .iter()
        .rev()
        .skip(start_index)
        .take(area.height.saturating_sub(2) as usize)
        .for_each(|p| {
            let num_subcarriers = p.csi.first().and_then(|rx| rx.first()).map_or(0, |sc_row| sc_row.len());
            let rssi_str = p.rssi.first().map_or_else(|| "N/A".to_string(), |r| r.to_string());

            let secs = p.timestamp.trunc() as i64;
            let nsecs = (p.timestamp.fract() * 1_000_000_000.0) as u32;
            let time_stamp = Utc.timestamp_opt(secs, nsecs).unwrap();

            lines.push(Line::from(format!(
                "{} {} {} {}",
                time_stamp.format("%Y-%m-%d %H:%M:%S%.3f"),
                p.sequence_number,
                rssi_str,
                num_subcarriers
            )));
        });
    lines.push(divider(area));
    let border_style = if matches!(tui_state.focussed_panel, Focus::Csi) {
        Style::default().fg(Color::Blue)
    } else {
        Style::default()
    };
    let widget = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: true }).block(
        Block::default()
            .padding(PADDING)
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(Span::styled(" CSI".to_string(), HEADER_STYLE)),
    );
    f.render_widget(widget, area);
}

/// Renders the registry panel in the TUI.
/// Shows the registry address, status, list of available hosts, and input for manually adding hosts.
fn render_registry(f: &mut Frame, tui_state: &OrgTuiState, area: Rect) {
    let registry_addr_text = match tui_state.registry_addr {
        Some(addr) => format!("Address: {addr:?}"),
        None => "No registry specified".to_owned(),
    };

    let registry_status = match tui_state.registry_status {
        RegistryStatus::Disconnected => Span::styled(" [Not Connected]", Style::default().fg(Color::Red)),
        RegistryStatus::WaitingForConnection => Span::styled(" [Connecting...]", Style::default().fg(Color::Yellow)),
        RegistryStatus::Connected => Span::styled(" [Connected]", Style::default().fg(Color::Green)),
        RegistryStatus::Polling => Span::styled(" [Polling]", Style::default().fg(Color::Cyan)),
        RegistryStatus::NotSpecified => Span::raw(""),
    };
    let reg_addr = match &tui_state.focussed_panel {
        Focus::Registry(FocusReg::RegistryAddress) => Line::from(vec![
            Span::styled(registry_addr_text, Style::default().bg(Color::White).fg(Color::Black)),
            registry_status,
        ]),
        _ => Line::from(vec![Span::raw(registry_addr_text), registry_status]),
    };
    let mut lines: Vec<Line> = vec![reg_addr, divider(area), Line::from("ID  Address/Device  Status"), divider(area)];

    for (i, host) in tui_state.registry_hosts.iter().enumerate() {
        let line_text = format!(" [{}] {} [{:?}]", host.id, host.addr, host.status);

        let status_style = match host.status {
            HostStatus::Available => Style::default().fg(Color::Green),
            HostStatus::Disconnected => Style::default().fg(Color::Red),
            HostStatus::Unresponsive => Style::default().fg(Color::Yellow),
            _ => Style::default(),
        };

        let final_style = if matches!(tui_state.focussed_panel,Focus::Registry(FocusReg::AvailableHosts(idx)) if idx == i) {
            status_style.bg(Color::White).fg(Color::Black)
        } else {
            status_style
        };

        lines.push(Line::from(vec![Span::styled(line_text, final_style)]));
        lines.push(Line::from(format!("  └─ Devices Count: {} ", host.devices.len())));
    }

    let used_lines = lines.len();
    let total_lines = (area.height as usize).saturating_sub(6);
    if used_lines < total_lines {
        lines.extend(std::iter::repeat_n(Line::from(""), total_lines - used_lines));
    }

    lines.push(divider(area));
    lines.push(Line::from("Manually add Host: "));
    lines.push(divider(area));

    let ip_input = Line::from(" [IP:Port] ___.___.___.___:______");

    let mut add_addr = vec![Span::from(" IP:Port ")];
    add_addr.extend(edit_address(&tui_state.focussed_panel, tui_state.add_host_input_socket));
    lines.push(Line::from(add_addr));

    let border_style = if matches!(tui_state.focussed_panel, Focus::Registry(_)) {
        Style::default().fg(Color::Blue)
    } else {
        Style::default()
    };

    let widget = Paragraph::new(Text::from(lines)).block(
        Block::default()
            .padding(PADDING)
            .title(Span::styled("Registry", HEADER_STYLE))
            .borders(Borders::ALL)
            .border_style(border_style),
    );
    f.render_widget(widget, area);
}

/// Renders the hosts panel in the TUI.
/// Displays a tree of known hosts and their associated devices, with connection and subscription statuses.
fn render_hosts(f: &mut Frame, tui_state: &OrgTuiState, area: Rect) {
    let mut lines: Vec<Line> = vec![];
    lines.push(Line::from("ID  Address/Device  Status"));
    lines.push(divider(area));
    for (host_idx, host) in tui_state.known_hosts.iter().enumerate() {
        // Handles host style
        let host_style = {
            let mut style = match tui_state.focussed_panel {
                Focus::Hosts(FocusHost::HostTree(h, 0)) if h == host_idx => Style::default().bg(Color::Gray),
                _ => Style::default(),
            };

            match host.status {
                HostStatus::Unknown => style.fg(Color::DarkGray),
                HostStatus::Disconnected => style.fg(Color::Red),
                HostStatus::Available => style.fg(Color::Gray),
                HostStatus::Connected => style.fg(Color::Green),
                HostStatus::Sending => style.fg(Color::Cyan),
                HostStatus::Unresponsive => style.fg(Color::Yellow),
            }
        };

        lines.push(Line::from(Span::styled(
            format!("[{}] {} [{:?}]", host.id, host.addr, host.status),
            host_style,
        )));

        for (dev_idx, device) in host.devices.iter().enumerate() {
            let is_last_device = dev_idx == host.devices.len() - 1;
            let branch_prefix = if is_last_device { " └─ " } else { " ├─ " };
            // let status = if device.status == DeviceStatus::Subscribed { "Subbed" } else { "" };

            let text = format!("{}[{}] {:?}", branch_prefix, device.id, device.dev_type,);
            let mut device_style = match tui_state.focussed_panel {
                Focus::Hosts(FocusHost::HostTree(h, d)) if d != 0 && (d - 1) == dev_idx && h == host_idx => Style::default().bg(Color::Gray),
                _ => Style::default(),
            };

            device_style = match host.status {
                HostStatus::Connected | HostStatus::Sending => match device.status {
                    DeviceStatus::Subscribed => device_style.fg(Color::LightGreen),
                    DeviceStatus::NotSubscribed => device_style,
                },
                HostStatus::Unresponsive => todo!(),
                _ => device_style.fg(Color::DarkGray),
            };

            lines.push(Line::from(Span::styled(text, device_style)));
        }
    }
    let current_host = if let Some(selected) = tui_state.selected_host {
        selected.to_string()
    } else {
        "Current device".to_string()
    };

    let used_lines = lines.len();
    let total_lines = (area.height as usize).saturating_sub(4);
    if used_lines < total_lines {
        lines.extend(std::iter::repeat_n(Line::from(""), total_lines - used_lines));
    }

    lines.push(divider(area));
    lines.push(Line::from(format!("Selected: {current_host}")));

    let border_style = if matches!(tui_state.focussed_panel, Focus::Hosts(_)) {
        Style::default().fg(Color::Blue)
    } else {
        Style::default()
    };

    let widget = Paragraph::new(Text::from(lines)).block(
        Block::default()
            .padding(PADDING)
            .title(Span::styled("Hosts", HEADER_STYLE))
            .borders(Borders::ALL)
            .border_style(border_style),
    );
    f.render_widget(widget, area);
}

/// Renders the experiments panel in the TUI.
/// Displays current experiment status and metadata, as well as a list of selectable experiments.
fn render_experiments(f: &mut Frame, tui_state: &OrgTuiState, area: Rect) {
    let mut lines = if let Some(active_exp) = &tui_state.active_experiment {
        let status_color = match active_exp.info.status {
            ExperimentStatus::Running => Color::Yellow,
            ExperimentStatus::Stopped => Color::Red,
            ExperimentStatus::Done => Color::Green,
            _ => Color::White,
        };
        let stage_names: Vec<String> = active_exp.experiment.stages.iter().map(|f| f.name.clone()).collect();
        let running_on = if let Some(addr) = tui_state.selected_host {
            format!("{addr:?}")
        } else {
            "Current Device".to_owned()
        };

        let mut lines = vec![
            Line::from(vec![Span::raw("Name:         "), Span::raw(&active_exp.experiment.metadata.name)]),
            Line::from(vec![Span::raw("Running on:   "), Span::raw(running_on.to_string())]),
            Line::from(vec![
                Span::raw("Output path:  "),
                Span::styled(
                    active_exp.experiment.metadata.output_path.display().to_string(),
                    Style::default().add_modifier(Modifier::ITALIC),
                ),
            ]),
            Line::from(vec![
                Span::raw("Status:       "),
                Span::styled(format!("{:?}", active_exp.info.status), Style::default().fg(status_color)),
            ]),
            Line::from(vec![
                Span::raw("Stages:       "),
                Span::raw(format!("{}/{}", active_exp.info.current_stage, active_exp.experiment.stages.len())),
            ]),
        ];
        for (idx, stage) in active_exp.experiment.stages.iter().enumerate() {
            let style = if idx < active_exp.info.current_stage {
                Style::default().fg(Color::Green)
            } else if idx == active_exp.info.current_stage {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            lines.push(Line::from(vec![Span::raw("  - "), Span::styled(stage.name.clone(), style)]));
        }

        lines
    } else {
        vec![Line::from("No experiment selected")]
    };

    lines.push(divider(area));
    lines.push(Line::from("Select Experiment"));
    lines.push(divider(area));

    for (i, metadata) in tui_state.experiments.iter().enumerate() {
        let selected = match &tui_state.active_experiment {
            Some(active_exp) if &active_exp.experiment.metadata == metadata => "X",
            _ => " ",
        };

        let line_text = format!("  [{}] {}", selected, metadata.name);

        if matches!( tui_state.focussed_panel, Focus::Experiments(FocusExp::Select(idx)) if idx == i) {
            lines.push(Line::from(vec![Span::styled(
                line_text,
                Style::default().bg(Color::White).fg(Color::Black),
            )]));
        } else {
            lines.push(Line::from(line_text));
        }
    }

    let border_style = if matches!(tui_state.focussed_panel, Focus::Experiments(_)) {
        Style::default().fg(Color::Blue)
    } else {
        Style::default()
    };

    let widget = Paragraph::new(Text::from(lines)).block(
        Block::default()
            .padding(PADDING)
            .title(Span::styled("Experiment Control", HEADER_STYLE))
            .borders(Borders::ALL)
            .border_style(border_style),
    );
    f.render_widget(widget, area);
}

/// Renders the footer panel with keybindings or contextual help based on the current focused panel.
fn render_footer(f: &mut Frame, tui_state: &OrgTuiState, area: Rect) {
    let footer = Block::default().title("Info").borders(Borders::ALL);

    let widget = Paragraph::new(footer_text(tui_state)).wrap(Wrap { trim: true }).block(
        Block::default()
            .borders(Borders::ALL)
            .padding(PADDING)
            .title(Span::styled("Keybinds", HEADER_STYLE)),
    );

    f.render_widget(widget, area);
}

/// Returns a footer string with keybinds/help based on the currently focused panel.
fn footer_text(tui_state: &OrgTuiState) -> String {
    match &tui_state.focussed_panel {
        Focus::Main => "[R]egistry | [H]osts  | [E]xperiment | [L]ogs | [C]si | [.] Clear Logs | [,] Clear CSI | [Q]uit",
        Focus::Hosts(focused_hosts_panel) => match focused_hosts_panel {
            FocusHost::None => "[.] Clear Logs | [,] Clear CSI | [ESC]ape | [Q]uit",
            FocusHost::HostTree(_, 0) => "S[E]lect Host | [C]onnect | [D]isconnect | [S]ubscribe to all | [U]nsubscribe to all | [Tab] Next | [Shft+Tab] Prev | [↑↓] Move |  [.] Clear Logs | [ESC]ape | [Q]uit",
            FocusHost::HostTree(_, _) => "S[E]lect Host | [S]ubscribe | [U]nsubscribe | [Tab] Next | [Shft+Tab] Prev | [↑↓] Move | [.] Clear Logs | [,] Clear CSI | [ESC]ape | [Q]uit",
        },
        Focus::Registry(focused_registry_panel) => match focused_registry_panel {
          FocusReg::RegistryAddress => match &tui_state.registry_status {
              RegistryStatus::Disconnected | RegistryStatus::NotSpecified=> "[C]onnect | [M]anual Add | [Tab] Next | [↓] Move | [.] Clear Logs | [,] Clear CSI | [ESC]ape | [Q]uit",
              RegistryStatus::WaitingForConnection => "[D]isconnect | [M]anual Add | [.] Clear Logs | [,] Clear CSI | [ESC]ape | [Q]uit",
              RegistryStatus::Connected => "[D]isconnect | [A]add all hosts | [S]tart Polling | [P]oll once | [M]anual Add | [.] Clear Logs | [,] Clear CSI | [ESC]ape | [Q]uit",
              RegistryStatus::Polling => "[D]isconnect | [A]add all hosts | [S]top Polling | [M]anual Add | [.] Clear Logs | [,] Clear CSI | [ESC]ape | [Q]uit",
            },
            FocusReg::AvailableHosts(_) => "[M]anual Add | [Tab] Next | [Shft+Tab] Prev | [↑↓] Move | [.] Clear Logs | [,] Clear CSI | [ESC]ape | [Q]uit",
            FocusReg::AddHost(_) => "[Tab] Next | [Shft+Tab] Prev | [←→↑↓] Move | [Enter] | [.] Clear Logs | [,] Clear CSI | [ESC]ape | [Q]uit",
        },

        Focus::Experiments(focused_experiment_panel) => match focused_experiment_panel {
          FocusExp::Select(_) => match &tui_state.active_experiment {
            Some(experiment) => {
              match experiment.info.status {
                ExperimentStatus::Running => {
                  "[E]nd Experiment | [Tab] Next | [Shft+Tab] Prev | [↑↓] Move | [.] Clear Logs | [,] Clear CSI | [ESC]ape | [Q]uit"
                },
                _ => {
                  "[B]egin experiment | [S]elect Experiment | [Tab] Next | [Shft+Tab] Prev | [↑↓] Move | [.] Clear Logs | [,] Clear CSI | [ESC]ape | [Q]uit"
                }
              }
            },
            None => "[S]elect Experiment | [Tab] Next | [Shft+Tab] Prev | [↑↓] Move | [.] Clear Logs | [,] Clear CSI | [ESC]ape | [Q]uit"
          }
        },
        Focus::Logs | Focus::Csi => "[↑↓] Move | [.] Clear Logs | [,] Clear CSI | [ESC]ape | [Q]uit",
    }
    .to_owned()
}

/// Renders the editable IP address input box with highlighting on the selected character.
fn edit_address(focussed: &Focus, input: [char; 21]) -> Vec<Span<'static>> {
    match focussed {
        Focus::Registry(FocusReg::AddHost(selected_idx)) => {
            let mut spans: Vec<Span> = vec![];
            for (i, ch) in input.iter().enumerate() {
                let mut style = Style::default();
                if i == *selected_idx {
                    style = style
                        .fg(Color::Black)
                        .bg(Color::Yellow)
                        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
                    spans.push(Span::styled(ch.to_string(), style));
                } else {
                    spans.push(Span::raw(ch.to_string()));
                }
            }
            spans
        }
        _ => {
            vec![Span::from(input.iter().collect::<String>())]
        }
    }
}
