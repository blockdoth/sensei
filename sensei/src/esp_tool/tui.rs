use super::state::TuiState;
use crate::esp_tool::{
    CSI_DATA_BUFFER_CAPACITY, LOG_BUFFER_CAPACITY,
    state::{SpamConfigField, UiMode},
};
use lib::{
    csi_types::CsiData,
    sources::controllers::esp32_controller::{
        Bandwidth as EspBandwidth, CsiType as EspCsiType, OperationMode as EspOperationMode, SecondaryChannel as EspSecondaryChannel,
    },
};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, Wrap},
};

// Renders the UI based on the state, should not contain any state changing logic
pub fn ui(f: &mut Frame, tui_state: &TuiState) {
    // --- Main Layout: Content Area and Global Footer ---
    let screen_chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1) // Margin around the whole screen
        .constraints([
            Constraint::Min(0),    // Content area (everything else)
            Constraint::Length(4), // Global Footer for Info/Errors
        ])
        .split(f.area());

    let content_area = screen_chunks[0];
    let global_footer_area = screen_chunks[1];

    // --- Split Content Area: Left Panel and Right Log Panel ---
    let content_horizontal_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(65), // Left panel width
            Constraint::Percentage(35), // Right panel (logs) width
        ])
        .split(content_area);

    let left_panel_area = content_horizontal_chunks[0];
    let log_panel_area = content_horizontal_chunks[1];

    // --- Calculate height for the Status Block ---
    const BASE_ESP_CONFIG_LINES: u16 = 6; // General ESP32 config lines
    const SPAM_DETAILS_LINES: u16 = 5; // Lines for spam-specific configuration details

    let status_area_height = if tui_state.ui_mode == UiMode::Spam {
        BASE_ESP_CONFIG_LINES + SPAM_DETAILS_LINES
    } else {
        BASE_ESP_CONFIG_LINES
    };

    // --- Vertical layout for the Left Panel (Status, CSI Table) ---
    let left_vertical_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(status_area_height), // Status block
            Constraint::Min(0),                     // CSI Data Table
        ])
        .split(left_panel_area);

    let status_area = left_vertical_chunks[0];
    let table_area = left_vertical_chunks[1];

    // --- Build Status Block Content ---
    let mode_str = match tui_state.esp_config.mode {
        // Read from esp_config
        EspOperationMode::Receive => "CSI RX",
        EspOperationMode::Transmit => "WiFi SPAM TX",
    };
    let dev_conf = &tui_state.esp_config;
    let bw_str = match dev_conf.bandwidth {
        EspBandwidth::Twenty => "20MHz",
        EspBandwidth::Forty => "40MHz",
    };
    let ltf_str = match dev_conf.csi_type {
        // This should correctly provide HT-LTF or L-LTF
        EspCsiType::HighThroughputLTF => "HT-LTF",
        EspCsiType::LegacyLTF => "L-LTF",
    };
    let sec_chan_str = match dev_conf.secondary_channel {
        EspSecondaryChannel::None => "None", // Should only be None if BW is 20MHz
        EspSecondaryChannel::Above => "HT40+",
        EspSecondaryChannel::Below => "HT40-",
    };
    let connection_style = match tui_state.connection_status.as_str() {
        s if s.starts_with("CONNECTED") => Style::default().fg(Color::Green),
        s if s.starts_with("INITIALIZING") => Style::default().fg(Color::Yellow),
        _ => Style::default().fg(Color::Red),
    };

    let mut status_lines = vec![Line::from(vec![
        Span::raw("ESP32 Status: "),
        Span::styled(tui_state.connection_status.clone(), connection_style),
        Span::raw(" | Mode: "),
        Span::styled(mode_str, Style::default().add_modifier(Modifier::BOLD)),
    ])];

    // WiFi Channel, Bandwidth, and Secondary Channel line
    let mut wifi_line_spans = vec![
        Span::raw("WiFi Channel: "),
        Span::styled(tui_state.esp_config.channel.to_string(), Style::default().fg(Color::Yellow)),
        Span::raw(" | Bandwidth: "),
        Span::styled(bw_str, Style::default().fg(Color::Yellow)),
    ];
    if dev_conf.bandwidth == EspBandwidth::Forty {
        // Only show secondary channel info if BW is 40MHz
        wifi_line_spans.push(Span::raw(format!(" ({sec_chan_str})")));
    }
    status_lines.push(Line::from(wifi_line_spans));

    // CSI Type and RSSI Scale line
    status_lines.push(Line::from(vec![
        Span::raw("CSI Type: "),
        Span::styled(ltf_str, Style::default().fg(Color::Yellow)), // ltf_str used here
        Span::raw(" | RSSI Scale: "),
        Span::styled(dev_conf.manual_scale.to_string(), Style::default().fg(Color::Yellow)),
    ]));

    // CSI Buffer line
    status_lines.push(Line::from(vec![
        Span::raw("CSI Data Rx Buffer: "),
        Span::styled(
            format!("{}/{}", tui_state.csi_data.len(), CSI_DATA_BUFFER_CAPACITY),
            Style::default().fg(Color::Cyan),
        ),
    ]));

    if tui_state.ui_mode == UiMode::Spam {
        // Check app_state.ui_mode for spam section
        let spam = &tui_state.spam_settings;
        status_lines.push(Line::from(Span::styled(
            "Spam Configuration:",
            Style::default().add_modifier(Modifier::UNDERLINED),
        )));

        let get_field_style = |app_state: &TuiState, _field_type: SpamConfigField, is_active: bool| {
            if is_active {
                if app_state.current_field_has_error {
                    Style::default().fg(Color::Black).bg(Color::LightRed)
                } else {
                    Style::default().fg(Color::Black).bg(Color::Cyan)
                }
            } else {
                Style::default()
            }
        };

        let mut src_mac_spans = vec![Span::raw("  Src MAC: ")];
        for i in 0..6 {
            let current_field_type = SpamConfigField::SrcMacOctet(i);
            let is_active_field = tui_state.is_editing_spam_config && tui_state.current_editing_field == current_field_type;
            let val_str = if is_active_field {
                tui_state.spam_input_buffer.clone() + "_"
            } else {
                format!("{:02X}", spam.src_mac[i])
            };
            let style = get_field_style(tui_state, current_field_type, is_active_field);
            src_mac_spans.push(Span::styled(val_str, style));
            if i < 5 {
                src_mac_spans.push(
                    Span::raw(if is_active_field && tui_state.spam_input_buffer.len() == 2 {
                        ""
                    } else {
                        ":"
                    })
                    .style(Style::default()),
                );
            }
        }
        status_lines.push(Line::from(src_mac_spans));

        let mut dst_mac_spans = vec![Span::raw("  Dst MAC: ")];
        for i in 0..6 {
            let current_field_type = SpamConfigField::DstMacOctet(i);
            let is_active_field = tui_state.is_editing_spam_config && tui_state.current_editing_field == current_field_type;
            let val_str = if is_active_field {
                tui_state.spam_input_buffer.clone() + "_"
            } else {
                format!("{:02X}", spam.dst_mac[i])
            };
            let style = get_field_style(tui_state, current_field_type, is_active_field);
            dst_mac_spans.push(Span::styled(val_str, style));
            if i < 5 {
                dst_mac_spans.push(
                    Span::raw(if is_active_field && tui_state.spam_input_buffer.len() == 2 {
                        ""
                    } else {
                        ":"
                    })
                    .style(Style::default()),
                );
            }
        }
        status_lines.push(Line::from(dst_mac_spans));

        let reps_field_type = SpamConfigField::Reps;
        let is_reps_active = tui_state.is_editing_spam_config && tui_state.current_editing_field == reps_field_type;
        let reps_str = if is_reps_active {
            tui_state.spam_input_buffer.clone() + "_"
        } else {
            spam.n_reps.to_string()
        };
        let reps_style = get_field_style(tui_state, reps_field_type, is_reps_active);

        let pause_field_type = SpamConfigField::PauseMs;
        let is_pause_active = tui_state.is_editing_spam_config && tui_state.current_editing_field == pause_field_type;
        let pause_str = if is_pause_active {
            tui_state.spam_input_buffer.clone() + "_"
        } else {
            spam.pause_ms.to_string()
        };
        let pause_style = get_field_style(tui_state, pause_field_type, is_pause_active);

        status_lines.push(Line::from(vec![
            Span::raw("  Reps: "),
            Span::styled(reps_str, reps_style),
            Span::raw(" | Pause: "),
            Span::styled(pause_str, pause_style),
            Span::raw("ms"),
        ]));

        status_lines.push(Line::from(Span::styled(
            format!(
                "  Continuous Spam ('t'): {}",
                if tui_state.is_continuous_spam_active { "ON" } else { "OFF" }
            ),
            if tui_state.is_continuous_spam_active {
                Style::default().fg(Color::LightRed).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            },
        )));
    }

    let status_paragraph = Paragraph::new(Text::from(status_lines))
        .block(Block::default().borders(Borders::ALL).title(" ESP32 Real-Time Status "))
        .wrap(Wrap { trim: true }); // Added wrap for status paragraph
    f.render_widget(status_paragraph, status_area);

    // --- CSI Data Table ---
    let table_header_cells = ["Timestamp (s)", "Seq", "RSSI", "Subcarriers"]
        .iter()
        .map(|h| Cell::from(*h).style(Style::default().fg(Color::Yellow)));
    let table_header = Row::new(table_header_cells).height(1).bottom_margin(0);

    let rows: Vec<Row> = tui_state
        .csi_data
        .iter()
        .rev()
        .take(table_area.height.saturating_sub(2) as usize)
        .map(|p: &CsiData| {
            let num_subcarriers = p.csi.first().and_then(|rx| rx.first()).map_or(0, |sc_row| sc_row.len());
            let rssi_str = p.rssi.first().map_or_else(|| "N/A".to_string(), |r| r.to_string());
            Row::new(vec![
                Cell::from(format!("{:.6}", p.timestamp)),
                Cell::from(p.sequence_number.to_string()),
                Cell::from(rssi_str),
                Cell::from(num_subcarriers.to_string()),
            ])
        })
        .collect();

    let table_widths = [Constraint::Length(18), Constraint::Length(7), Constraint::Length(6), Constraint::Min(10)];
    let csi_table = Table::new(rows, &table_widths)
        .header(table_header)
        .block(Block::default().borders(Borders::ALL).title(" CSI Data Packets "))
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol(">> ");
    f.render_widget(csi_table, table_area);

    // --- Log Output (Right Panel) ---
    let log_panel_content_height = log_panel_area.height.saturating_sub(2) as usize;

    let log_lines_to_display: Vec<Line> = {
        let current_log_count = tui_state.log_messages.len();
        let start_index = current_log_count.saturating_sub(log_panel_content_height);

        tui_state
            .log_messages
            .iter()
            .skip(start_index)
            .map(|entry| {
                let timestamp_str = entry.timestamp.format("%H:%M:%S").to_string();
                let level_str = format!("[{}]", entry.level);
                let message_str = &entry.message;

                let style = match entry.level {
                    log::Level::Error => Style::default().fg(Color::Red),
                    log::Level::Warn => Style::default().fg(Color::Yellow),
                    log::Level::Info => Style::default().fg(Color::Cyan),
                    log::Level::Debug => Style::default().fg(Color::Blue),
                    log::Level::Trace => Style::default().fg(Color::Magenta),
                };
                Line::from(vec![
                    Span::raw(format!("{timestamp_str} ")),
                    Span::styled(level_str, style),
                    Span::styled(format!(" {message_str}"), style),
                ])
            })
            .collect()
    };

    let log_text = Text::from(log_lines_to_display);

    let logs_widget = Paragraph::new(log_text)
        .wrap(Wrap { trim: true })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" Log ({}/{}) ", tui_state.log_messages.len(), LOG_BUFFER_CAPACITY)),
        );
    f.render_widget(logs_widget, log_panel_area);

    // --- Global Footer Info/Errors ---
    let footer_text_str = if let Some(err_msg) = &tui_state.last_error {
        format!("ERROR: {err_msg} (Press 'R' to dismiss)")
    } else if tui_state.is_editing_spam_config && tui_state.ui_mode == UiMode::Spam {
        let field_name = match tui_state.current_editing_field {
            SpamConfigField::SrcMacOctet(i) => format!("Src MAC[{}]", i + 1),
            SpamConfigField::DstMacOctet(i) => format!("Dst MAC[{}]", i + 1),
            SpamConfigField::Reps => "Repetitions".to_string(),
            SpamConfigField::PauseMs => "Pause (ms)".to_string(),
        };
        format!("[Esc] Exit Edit | [Tab]/[Ent] Next | [Shft+Tab] Prev | [↑↓] Modify Val | Editing: {field_name}")
    } else {
        "Controls: [Q]uit | [M]ode | [C]h | [B]W | [L]TF | [↑]ClrCSI | [↓]SyncTime | [R]ClrErr | SpamMode: [E]dit | [S]Burst | [T]Cont.".to_string()
    };
    let footer_style = if tui_state.last_error.is_some() {
        Style::default().fg(Color::Red)
    } else if tui_state.is_editing_spam_config {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let footer_paragraph = Paragraph::new(footer_text_str)
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL).title(" Info/Errors "));
    f.render_widget(footer_paragraph, global_footer_area);
}
