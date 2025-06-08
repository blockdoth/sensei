use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use super::state::OrgTuiState;
use crate::orchestrator::state::{DeviceStatus, RegistryStatus};
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
            .borders(Borders::ALL)
            .title(Span::styled(format!(" Log ({}) ", tui_state.logs.len()), header_style)),
    );

    f.render_widget(logs_widget, status_area);

    let control_panel = Block::default().title("Control").borders(Borders::ALL);
    f.render_widget(control_panel, control_area);

    let control_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(8), // Registry address
            Constraint::Min(3),    // Known addresses
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

    let registry_widget = Paragraph::new(Text::from(lines)).block(Block::default().title("Registry").borders(Borders::ALL));
    f.render_widget(registry_widget, control_chunks[0]);

    let mut tree_lines: Vec<Line> = vec![];

    for (host_idx, host) in tui_state.connected_hosts.iter().enumerate() {
        let host_text = format!("[{}] {}", host.id, host.addr);
        let styled_span = if host.devices.iter().all(|d| d.status == DeviceStatus::Subscribed) {
            if host.devices.is_empty() {
                Span::styled(host_text, Style::default().fg(Color::Yellow))
            } else {
                Span::styled(host_text, Style::default().fg(Color::Green))
            }
        } else {
            Span::raw(host_text)
        };

        tree_lines.push(Line::from(styled_span));

        for (dev_idx, device) in host.devices.iter().enumerate() {
            let is_last_device = dev_idx == host.devices.len() - 1;
            let branch_prefix = if is_last_device { " └─ " } else { " ├─ " };
            let status = if device.status == DeviceStatus::Subscribed { "Subbed" } else { "" };

            let text = format!(
                "{}[{}] {:?} {}",
                branch_prefix,
                device.id,
                device.dev_type,
                if device.status == DeviceStatus::Subscribed { "Subbed" } else { "" }
            );

            let styled_span = if device.status == DeviceStatus::Subscribed {
                Span::styled(text, Style::default().fg(Color::Green))
            } else {
                Span::raw(text)
            };

            tree_lines.push(Line::from(styled_span));
        }
    }
    let hosts_tree_view = Paragraph::new(Text::from(tree_lines)).block(Block::default().title("Connected Hosts").borders(Borders::ALL));

    f.render_widget(hosts_tree_view, control_chunks[1]);

    let footer = Block::default().title("Info").borders(Borders::ALL);
    f.render_widget(footer, footer_area);
}
