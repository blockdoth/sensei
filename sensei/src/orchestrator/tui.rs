use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Padding, Paragraph, Wrap};

use super::state::OrgTuiState;
use crate::orchestrator::state::{DeviceStatus, Focused, FocusedAddHostField, FocusedHosts, FocusedRegistry, HostStatus, RegistryStatus};

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

    let log_panel_content_height = status_area.height.saturating_sub(2) as usize;
    let current_log_count = tui_state.logs.len();

    let start_index = current_log_count.saturating_sub(log_panel_content_height);

    let logs_to_display: Vec<Line> = tui_state.logs.iter().skip(start_index).map(|entry| entry.format()).collect();

    let logs_widget = Paragraph::new(Text::from(logs_to_display)).wrap(Wrap { trim: true }).block(
        Block::default()
            .padding(padding)
            .borders(Borders::ALL)
            .border_style(if matches!(tui_state.focussed_panel, Focused::Logs) {
                Style::default().fg(Color::Blue)
            } else {
                Style::default()
            })
            .title(Span::styled(format!(" Log ({current_log_count})"), header_style)),
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

    let control_divider = Line::from(Span::styled(
        "─".repeat(control_chunks[0].width.into()), // horizontal divider
        Style::default().add_modifier(Modifier::DIM),
    ));

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
    let mut lines: Vec<Line> = vec![Line::from(vec![Span::raw(registry_addr_text), registry_status]), control_divider.clone()];

    // Append known addresses as Lines
    for addr in &tui_state.host_available_from_reg {
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

    let current_host = if let Some(selected) = tui_state.selected_host {
        selected.to_string()
    } else {
        "Current device".to_string()
    };

    let mut h_lines: Vec<Line> = vec![];
    h_lines.push(Line::from(format!("ID  Address/Device  Status   Selected Host: [{current_host}]")));
    h_lines.push(control_divider.clone());
    for (host_idx, host) in tui_state.known_hosts.iter().enumerate() {
        // Handles host style
        let host_style = {
            let mut style = match tui_state.focussed_panel {
                Focused::Hosts(FocusedHosts::HostTree(h, 0)) if h == host_idx => Style::default().bg(Color::Gray),
                _ => Style::default(),
            };

            match host.status {
                HostStatus::Available => style.fg(Color::DarkGray),
                HostStatus::Connected => style.fg(Color::Yellow),
                HostStatus::Disconnected => style.fg(Color::DarkGray),
                HostStatus::Sending => style.fg(Color::Green),
                HostStatus::Unresponsive => style.fg(Color::Red),
            }
        };

        h_lines.push(Line::from(Span::styled(
            format!("[{}] {} [{:?}]", host.id, host.addr, host.status),
            host_style,
        )));

        for (dev_idx, device) in host.devices.iter().enumerate() {
            let is_last_device = dev_idx == host.devices.len() - 1;
            let branch_prefix = if is_last_device { " └─ " } else { " ├─ " };
            // let status = if device.status == DeviceStatus::Subscribed { "Subbed" } else { "" };

            let text = format!("{}[{}] {:?}", branch_prefix, device.id, device.dev_type,);
            let mut device_style = match tui_state.focussed_panel {
                Focused::Hosts(FocusedHosts::HostTree(h, d)) if d != 0 && (d - 1) == dev_idx && h == host_idx => Style::default().bg(Color::Gray),
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

            h_lines.push(Line::from(Span::styled(text, device_style)));
        }
    }

    h_lines.push(control_divider.clone());
    h_lines.push(Line::from("Manually add Host: "));
    h_lines.push(control_divider.clone());

    let ip_input = Line::from(" [IP:Port] ___.___.___.___:______");

    let mut add_addr = vec![Span::from(" IP:Port ")];
    add_addr.extend(edit_number(
        &tui_state.focussed_panel,
        tui_state.add_host_input_socket,
        FocusedAddHostField::Address,
    ));
    h_lines.push(Line::from(add_addr));
    let mut add_id = vec![Span::from(" ID      ")];
    add_id.extend(edit_number(
        &tui_state.focussed_panel,
        tui_state.add_host_input_id,
        FocusedAddHostField::ID,
    ));
    h_lines.push(Line::from(add_id));
    h_lines.push(control_divider.clone());

    let hosts_tree_view = Paragraph::new(Text::from(h_lines)).block(
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

    let session = &tui_state.experiment_session;
    let mut exp_lines: Vec<Line> = if let Some(idx) = session.active_idx {
        let metadata = session.metadata.clone().unwrap();
        vec![
            Line::from(format!("Name:         {}", metadata.name)),
            Line::from(format!("Status:       {:?}", session.status)),
            Line::from(format!("Output path:  {:?}", metadata.output_path.unwrap())),
            Line::from(format!(
                "Stage:        {:?}/{:?}",
                session.progression.current_stage, session.progression.total_stages
            )),
        ]
    } else {
        vec![Line::from("No experiment selected")]
    };

    exp_lines.push(control_divider.clone());
    exp_lines.push(Line::from("Select Experiment"));
    exp_lines.push(control_divider.clone());

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
        Focused::Main => "[R]egistry | [H]osts  | [E]xperiment | [.] Clear Logs | [Q]uit",
        Focused::Hosts(focused_hosts_panel) => match focused_hosts_panel {
            FocusedHosts::None => "[A]dd host |  [.] Clear Logs | [ESC]ape | [Q]uit",
            FocusedHosts::AddHost(_,_) => "| [Tab]/[Ent] Next | [Shft+Tab] Prev | [←→↑↓] Move | [Enter] | [.] Clear Logs | [ESC]ape | [Q]uit",
            FocusedHosts::HostTree(_, 0) => "[A]dd host | S[E]lect Host | [C]onnect | [D]isconnect | [S]ubscribe to all | [U]nsubscribe to all | [Tab] Next | [Shft+Tab] Prev | [←→↑↓] Move |  [.] Clear Logs | [ESC]ape | [Q]uit",
            FocusedHosts::HostTree(_, _) => "[A]dd host | S[E]lect Host | [S]ubscribe | [U]nsubscribe | [Tab] Next | [Shft+Tab] Prev | [←→↑↓] Move | [.] Clear Logs | [ESC]ape | [Q]uit",
        },
        Focused::Registry(focused_registry_panel) => match focused_registry_panel {
            FocusedRegistry::RegistryAddress(_) => "[.] Clear Logs | [ESC]ape | [Q]uit",
            FocusedRegistry::AvailableHosts(_) => " [.] Clear Logs | [ESC]ape | [Q]uit",
        },

        Focused::Experiments => "[B]egin experiment | [E]nd Experiment | [S]elect Experiment | [.] Clear Logs | [ESC]ape | [Q]uit",
        Focused::Logs => todo!(),
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

/// Reusable edit IP function
fn edit_number(focussed: &Focused, input: [char; 21], selected: FocusedAddHostField) -> Vec<Span<'static>> {
    match focussed {
        Focused::Hosts(FocusedHosts::AddHost(selected_idx, f)) if *f == selected => {
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
