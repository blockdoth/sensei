use core::time;
use std::fmt::format;

use chrono::{DateTime, TimeZone, Utc};
use crossterm::style;
use lib::csi_types::CsiData;
use lib::sources::controllers::esp32_controller::{
    Bandwidth as EspBandwidth, CsiType as EspCsiType, EspMode, OperationMode as EspOperationMode, SecondaryChannel as EspSecondaryChannel,
};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Cell, Padding, Paragraph, Row, Table, Wrap};

use super::state::EspTuiState;
use crate::esp_tool::state::{FocusedPanel, FocussedInput, ToolMode};
use crate::esp_tool::{CSI_DATA_BUFFER_CAPACITY, LOG_BUFFER_CAPACITY};

const BASE_ESP_CONFIG_LINES: u16 = 6; // General ESP32 config lines
const SPAM_DETAILS_LINES: u16 = 4; // Lines for spam-specific configuration details

// Renders the full TUI frame based on the current application state (TuiState).
// This function is *purely presentational* and does not mutate state.
pub fn ui(f: &mut Frame, tui_state: &EspTuiState) {
    // === Top-level layout: vertical split into main content and footer ===
    let screen_chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Min(0),    // Main content area
            Constraint::Length(3), // Footer: for info/errors
        ])
        .split(f.area());

    let content_area = screen_chunks[0];
    let global_footer_area = screen_chunks[1];

    // === Horizontal layout inside content area: Left side (config & table) and Right side (logs) ===
    let content_horizontal_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(60), // Left panel: status, config, CSI table
            Constraint::Percentage(40), // Right panel: log messages
        ])
        .split(content_area);

    let left_panel_area = content_horizontal_chunks[0];
    let log_panel_area = content_horizontal_chunks[1];

    // === Vertical split for the left panel: Status block, optional spam config, CSI data table ===
    let mut left_constraints = vec![Constraint::Length(BASE_ESP_CONFIG_LINES)];
    if tui_state.tool_mode == ToolMode::Spam {
        left_constraints.push(Constraint::Length(SPAM_DETAILS_LINES));
    }
    left_constraints.push(Constraint::Min(0)); // Remainder goes to the CSI table

    let left_vertical_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(left_constraints)
        .split(left_panel_area);

    let status_area = left_vertical_chunks[0];
    let mut next_index = 1;

    // Conditionally get area for the Spam Config block
    let spam_config_area = if tui_state.tool_mode == ToolMode::Spam {
        let area = Some(left_vertical_chunks[next_index]);
        next_index += 1;
        area
    } else {
        None
    };

    let table_area = left_vertical_chunks[next_index];

    // === Shared styles ===
    let padding = Padding::new(1, 1, 0, 0);
    let header_style = Style::default().fg(Color::White).add_modifier(Modifier::BOLD);

    // === Build ESP32 Status Paragraph ===
    let mode_str = match tui_state.esp_mode {
        EspMode::SendingPaused => "Sending (Paused)",
        EspMode::SendingBurst => "Sending Burst",
        EspMode::SendingContinuous => "Sending Continuously",
        EspMode::Listening => "Monitor",
    };
    let dev_conf = &tui_state.unsaved_esp_config;

    let bw_str = match dev_conf.bandwidth {
        EspBandwidth::Twenty => "20MHz",
        EspBandwidth::Forty => "40MHz",
    };
    let ltf_str = match dev_conf.csi_type {
        EspCsiType::HighThroughputLTF => "HT-LTF",
        EspCsiType::LegacyLTF => "L-LTF",
    };
    let sec_chan_str = match dev_conf.secondary_channel {
        EspSecondaryChannel::None => "None",
        EspSecondaryChannel::Above => "HT40+",
        EspSecondaryChannel::Below => "HT40-",
    };

    let connection_style = match tui_state.connection_status.as_str() {
        s if s.starts_with("CONNECTED") => Style::default().fg(Color::Green),
        s if s.starts_with("INITIALIZING") => Style::default().fg(Color::Yellow),
        _ => Style::default().fg(Color::Red),
    };

    // Compose multi-line status block
    let mut status_lines = vec![
        Line::from(vec![
            Span::raw("ESP32 Status: "),
            Span::styled(tui_state.connection_status.clone(), connection_style),
            Span::raw(" | Mode: "),
            Span::styled(mode_str, Style::default().add_modifier(Modifier::BOLD)),
        ]),
        Line::from({
            let mut spans = vec![
                Span::raw("WiFi Channel: "),
                Span::styled(format!("{:02}", dev_conf.channel), Style::default().fg(Color::Yellow)),
                Span::raw(" | Bandwidth: "),
                Span::styled(bw_str, Style::default().fg(Color::Yellow)),
            ];
            if dev_conf.bandwidth == EspBandwidth::Forty {
                spans.push(Span::raw(format!(" ({sec_chan_str})")));
            }
            spans
        }),
        Line::from(vec![
            Span::raw("CSI Type: "),
            Span::styled(ltf_str, Style::default().fg(Color::Yellow)),
            Span::raw(" | RSSI Scale: "),
            Span::styled(dev_conf.manual_scale.to_string(), Style::default().fg(Color::Yellow)),
        ]),
        match tui_state.synced {
            0 => Line::raw("All changes are synced"),
            i => Line::from(Span::styled(format!("Syncing {i} changes"), Style::default().fg(Color::Cyan))),
        },
    ];

    let status_paragraph = Paragraph::new(Text::from(status_lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .padding(padding)
                .title(Span::styled(" ESP32 Real-Time Status ", header_style)),
        )
        .wrap(Wrap { trim: true });

    f.render_widget(status_paragraph, status_area);

    // === Optional Spam Config Panel ===
    if let Some(spam_area) = spam_config_area {
        let spam_lines = tui_state.unsaved_spam_settings.format(tui_state.focused_input);
        let border_style = if tui_state.unsaved_spam_settings != tui_state.spam_settings {
            Style::default().fg(Color::DarkGray) // Unsaved changes
        } else {
            Style::default()
        };

        let spam_paragraph = Paragraph::new(Text::from(spam_lines))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .padding(padding)
                    .border_style(border_style)
                    .title(Span::styled(" Spam Configuration ", header_style)),
            )
            .wrap(Wrap { trim: true });

        f.render_widget(spam_paragraph, spam_area);
    }

    // === CSI Data Table ===
    let table_header = Row::new(
        ["Timestamp (s)", "Seq", "RSSI", "Subcarriers"]
            .iter()
            .map(|h| Cell::from(*h).style(Style::default().fg(Color::Yellow))),
    )
    .height(1);

    let rows: Vec<Row> = tui_state
        .csi_data
        .iter()
        .rev()
        .take(table_area.height.saturating_sub(2) as usize)
        .map(|p| {
            let num_subcarriers = p.csi.first().and_then(|rx| rx.first()).map_or(0, |sc_row| sc_row.len());
            let rssi_str = p.rssi.first().map_or_else(|| "N/A".to_string(), |r| r.to_string());

            let secs = p.timestamp.trunc() as i64;
            let nsecs = (p.timestamp.fract() * 1_000_000_000.0) as u32;
            let time_stamp = Utc.timestamp_opt(secs, nsecs).unwrap();

            Row::new(vec![
                Cell::from(format!("{}", time_stamp.format("%Y-%m-%d %H:%M:%S%.3f"))),
                Cell::from(p.sequence_number.to_string()),
                Cell::from(rssi_str),
                Cell::from(num_subcarriers.to_string()),
            ])
        })
        .collect();

    let table_widths = [Constraint::Length(28), Constraint::Length(5), Constraint::Length(7), Constraint::Min(10)];

    let csi_table = Table::new(rows, &table_widths)
        .header(table_header)
        .block(Block::default().borders(Borders::ALL).padding(padding).title(Line::from(vec![
            Span::styled("CSI Data Log ", header_style),
            Span::styled(
                format!("({}/{})", tui_state.csi_data.len(), CSI_DATA_BUFFER_CAPACITY),
                Style::default().fg(Color::Yellow),
            ),
        ])))
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol(">> ");

    f.render_widget(csi_table, table_area);

    // === Log Output Panel ===
    let log_panel_content_height = log_panel_area.height.saturating_sub(2) as usize;
    let current_log_count = tui_state.logs.len();
    let start_index = current_log_count.saturating_sub(log_panel_content_height);

    let log_lines_to_display: Vec<Line> = tui_state.logs.iter().skip(start_index).map(|entry| entry.format()).collect();

    let logs_widget = Paragraph::new(Text::from(log_lines_to_display)).wrap(Wrap { trim: true }).block(
        Block::default().borders(Borders::ALL).padding(padding).title(Span::styled(
            format!(" Log ({}/{}) ", tui_state.logs.len(), LOG_BUFFER_CAPACITY),
            header_style,
        )),
    );

    f.render_widget(logs_widget, log_panel_area);

    // === Global Footer: Info/Error messages & Help ===
    let footer_text_str = if let Some(err_msg) = &tui_state.last_error_message {
        format!("ERROR: {err_msg} (Press 'R' to dismiss)")
    } else {
        match (&tui_state.tool_mode, &tui_state.focused_panel) {
            (ToolMode::Spam, FocusedPanel::SpamConfig) if tui_state.focused_input != FocussedInput::None => {
                "[Esc] Exit Spam Config | [Tab]/[Ent] Next | [Shft+Tab] Prev | [←→↑↓] Move".to_string()
            }
            (ToolMode::Spam, _) => {
                "[Q]uit | [M]ode | [C]hannel | [B]andwidth | [L] CSI Type SpamMode: [E]dit | [S]end Burst | [T] Send Continuous".to_string()
            }
            _ => "[Q]uit | [M]ode | [C]hannel | [B]andwidth | [L] CSI Type | [.] Clear Logs | [,] Clear CSI Logs".to_string(),
        }
    };

    let footer_style = if tui_state.last_error_message.is_some() {
        Style::default().fg(Color::Red)
    } else {
        Style::default()
    };

    let footer_paragraph = Paragraph::new(footer_text_str).wrap(Wrap { trim: true }).block(
        Block::default()
            .borders(Borders::ALL)
            .padding(padding)
            .title(Span::styled("Info / Errors", header_style))
            .style(footer_style),
    );

    f.render_widget(footer_paragraph, global_footer_area);
}
