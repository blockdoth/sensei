use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Padding, Paragraph, Wrap};

use super::state::OrgTuiState;
use crate::orchestrator::state::{DeviceStatus, Focused, FocusedHosts, FocusedRegistry, RegistryStatus};

pub fn ui(f: &mut Frame, tui_state: &OrgTuiState) {
    let screen_chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Min(0),    // Main content area
            Constraint::Length(3), // Footer: for info/errors
        ])
        .split(f.area());

    let header_style = Style::default().fg(Color::White).add_modifier(Modifier::BOLD);
    let padding = Padding::new(1, 1, 0, 0);

    let content_area = screen_chunks[0];
    let footer_area = screen_chunks[1];

    let content_horizontal_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(60), // Left panel: status
            Constraint::Percentage(40), // Right panel: config
        ])
        .split(content_area);

    let status_area = content_horizontal_chunks[0];
    let control_area = content_horizontal_chunks[1];

    let logs_to_display: Vec<Line> = tui_state.logs.iter().map(|entry| entry.format()).collect();

    let logs_widget = Paragraph::new(Text::from(logs_to_display)).wrap(Wrap { trim: true }).block(
        Block::default()
            .padding(padding)
            .borders(Borders::ALL)
            .border_style(if matches!(tui_state.focussed_panel, Focused::Status) {
                Style::default().fg(Color::Blue)
            } else {
                Style::default()
            })
            .title(Span::styled(format!(" Log ({}) ", tui_state.logs.len()), header_style)),
    );

    f.render_widget(logs_widget, status_area);

    let control_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(8), // Registry address
            Constraint::Min(10),   // Known addresses
            Constraint::Min(10),   // Experiment Control
        ])
        .split(control_area);

    let registry_addr_text = match tui_state.registry_addr {
        Some(addr) => addr.to_string(),
        None => "".to_owned(),
    };

    let registry_status = match tui_state.registry_status {
        RegistryStatus::Connected => Span::raw(" [Connected]"),
        RegistryStatus::Disconnected => Span::raw(" [Disconnected]"),
        RegistryStatus::NotSpecified => Span::raw(""),
    };

    // First line: registry address + status
    let mut lines: Vec<Line> = vec![
        Line::from(vec![Span::raw(registry_addr_text), registry_status]),
        Line::from(vec![Span::styled(
            "─".repeat(control_chunks[0].width.into()), // horizontal divider
            Style::default().add_modifier(Modifier::DIM),
        )]),
        // Line::from(Span::raw("Available")),
    ];

    // Append known addresses as Lines
    for addr in &tui_state.known_hosts {
        lines.push(Line::from(Span::raw(addr.to_string())));
    }

    let registry_widget = Paragraph::new(Text::from(lines)).block(
        Block::default()
            .padding(padding)
            .title(Span::styled("Registry", header_style))
            .borders(Borders::ALL)
            .border_style(if matches!(tui_state.focussed_panel, Focused::Registry(_)) {
                Style::default().fg(Color::Blue)
            } else {
                Style::default()
            }),
    );
    f.render_widget(registry_widget, control_chunks[0]);

    let mut tree_lines: Vec<Line> = vec![];
    for (host_idx, host) in tui_state.connected_hosts.iter().enumerate() {
        // Handles host style
        let host_style = if host.devices.iter().all(|d| d.status == DeviceStatus::Subscribed) {
            match &tui_state.focussed_panel {
                Focused::Hosts(FocusedHosts::HostTree(h, 0)) if *h == host_idx => {
                    if host.devices.is_empty() {
                        Style::default().fg(Color::Yellow).bg(Color::Gray)
                    } else {
                        Style::default().fg(Color::Green).bg(Color::Gray)
                    }
                }
                _ => {
                    if host.devices.is_empty() {
                        Style::default().fg(Color::Yellow)
                    } else {
                        Style::default().fg(Color::Green)
                    }
                }
            }
        } else {
            match &tui_state.focussed_panel {
                Focused::Hosts(FocusedHosts::HostTree(h, 0)) if *h == host_idx => Style::default().bg(Color::Gray),
                _ => Style::default(),
            }
        };

        tree_lines.push(Line::from(Span::styled(format!("[{}] {}", host.id, host.addr), host_style)));

        for (dev_idx, device) in host.devices.iter().enumerate() {
            let is_last_device = dev_idx == host.devices.len() - 1;
            let branch_prefix = if is_last_device { " └─ " } else { " ├─ " };
            // let status = if device.status == DeviceStatus::Subscribed { "Subbed" } else { "" };

            let text = format!("{}[{}] {:?}", branch_prefix, device.id, device.dev_type,);

            let device_style = if let Focused::Hosts(FocusedHosts::HostTree(host_idx, device_idx)) = &tui_state.focussed_panel {
                match device.status {
                    DeviceStatus::Subscribed => Style::default().fg(Color::Green).bg(Color::Gray),
                    _ => Style::default().fg(Color::White).bg(Color::Gray),
                }
            } else if device.status == DeviceStatus::Subscribed {
                Style::default().fg(Color::Green)
            } else {
                Style::default()
            };

            let device_style = if device.status == DeviceStatus::Subscribed {
                match &tui_state.focussed_panel {
                    Focused::Hosts(FocusedHosts::HostTree(h, d)) if *d != 0 && (*d - 1) == dev_idx && *h == host_idx => {
                        Style::default().fg(Color::Green).bg(Color::Gray)
                    }
                    _ => Style::default().fg(Color::Green),
                }
            } else {
                match &tui_state.focussed_panel {
                    Focused::Hosts(FocusedHosts::HostTree(h, d)) if *d != 0 && (*d - 1) == dev_idx && *h == host_idx => {
                        Style::default().bg(Color::Gray)
                    }
                    _ => Style::default(),
                }
            };

            tree_lines.push(Line::from(Span::styled(text, device_style)));
        }
    }

    // let host_text = if let FocusedPanel::Hosts(FocusedHostsPanel::Host(h, d)) = tui_state.focussed_panel {
    //     format!("Hosts: {h} / {d}")
    // } else {
    //     "Hosts".to_string()
    // };

    let hosts_divider = Line::from(Span::styled(
        "─".repeat(control_chunks[0].width.into()), // horizontal divider
        Style::default().add_modifier(Modifier::DIM),
    ));
    tree_lines.push(hosts_divider.clone());

    let add_host = vec![Line::from("Add host"), Line::from(" [IP:Port] ___.___.___.___:______")];

    tree_lines.extend(add_host);

    tree_lines.push(hosts_divider);

    let hosts_tree_view = Paragraph::new(Text::from(tree_lines)).block(
        Block::default()
            .padding(padding)
            .title(Span::styled("Hosts", header_style))
            .borders(Borders::ALL)
            .border_style(if matches!(tui_state.focussed_panel, Focused::Hosts(_)) {
                Style::default().fg(Color::Blue)
            } else {
                Style::default()
            }),
    );

    f.render_widget(hosts_tree_view, control_chunks[1]);

    let mut exp_lines: Vec<Line> = vec![
        Line::from("Status: [BALLS]"),
        Line::from("Participating devices:"),
        Line::from(" -[Device ID's]"),
        Line::from("Progress"),
        Line::from(" - Status bar / Time"),
    ];

    let experiment_widget = Paragraph::new(Text::from(exp_lines)).block(
        Block::default()
            .padding(padding)
            .borders(Borders::ALL)
            .border_style(if matches!(tui_state.focussed_panel, Focused::Experiments) {
                Style::default().fg(Color::Blue)
            } else {
                Style::default()
            })
            .title(Span::styled("Experiment Control", header_style)),
    );

    f.render_widget(experiment_widget, control_chunks[2]);

    let footer_text = (match &tui_state.focussed_panel {
        Focused::Main => "[R]egistry | [H]osts  | [E]xperiment | [Q]uit",
        Focused::Hosts(focused_hosts_panel) => match focused_hosts_panel {
            FocusedHosts::None => "[A]dd host | [ESC]ape | [Q]uit",
            FocusedHosts::AddHost(_) => "[ESC]ape | [Q]uit",
            FocusedHosts::HostTree(_, 0) => "[A]dd host | [C]onnect | [D]isconnect | [S]ubscribe to all |  [U]nsubscribe to all | [ESC]ape | [Q]uit",
            FocusedHosts::HostTree(_, _) => "[A]dd host | [S]ubscribe | [U]nsubscribe | [ESC]ape | [Q]uit",
        },
        Focused::Registry(focused_registry_panel) => match focused_registry_panel {
            FocusedRegistry::RegistryAddress(_) => "[ESC]ape | [Q]uit",
            FocusedRegistry::AvailableHosts(_) => "[ESC]ape | [Q]uit",
        },

        Focused::Experiments => "[B]egin experiment | [E]nd Experiment | [P]ause |[ESC]ape | [Q]uit",
        Focused::Status => todo!(),
    })
    .to_owned();

    let footer = Block::default().title("Info").borders(Borders::ALL);

    let footer = Paragraph::new(footer_text).wrap(Wrap { trim: true }).block(
        Block::default()
            .borders(Borders::ALL)
            .padding(padding)
            .title(Span::styled("Info / Errors", header_style)), // .style(footer_style),
    );

    f.render_widget(footer, footer_area);
}
