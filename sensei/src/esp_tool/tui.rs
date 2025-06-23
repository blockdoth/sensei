use chrono::{TimeZone, Utc};
use lib::sources::controllers::esp32_controller::{
    Bandwidth as EspBandwidth, CsiType as EspCsiType, EspMode, SecondaryChannel as EspSecondaryChannel,
};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Cell, Padding, Paragraph, Row, Table, Wrap};

use super::state::TuiState;
use crate::esp_tool::state::{FocusedPanel, ToolMode};
use crate::esp_tool::{CSI_DATA_BUFFER_CAPACITY, LOG_BUFFER_CAPACITY};

/// Base number of lines for the ESP32 configuration display area.
const BASE_ESP_CONFIG_LINES: u16 = 6;
/// Number of lines dedicated to displaying spam-specific configuration details.
const SPAM_DETAILS_LINES: u16 = 4;

// Renders the full TUI frame based on the current application state (TuiState).
// This function is *purely presentational* and does not mutate state.
pub fn ui(f: &mut Frame, tui_state: &TuiState) {
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
    let status_lines = vec![
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
            Span::raw(" CSI Data ("),
            Span::styled(format!("{}", tui_state.csi_data.len()), Style::default().fg(Color::Cyan)),
            Span::raw(format!("/{CSI_DATA_BUFFER_CAPACITY}) ")),
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
            format!(" Logs ({}/{LOG_BUFFER_CAPACITY}) ", tui_state.logs.len()),
            header_style,
        )),
    );

    f.render_widget(logs_widget, log_panel_area);

    // === Global Footer: Info/Error messages & Help ===
    let footer_text_str = if let Some(err_msg) = &tui_state.last_error_message {
        format!("ERROR: {err_msg} (Press 'R' to dismiss)")
    } else {
        match (&tui_state.tool_mode, &tui_state.focused_panel) {
            (ToolMode::Listen, FocusedPanel::Main) => {
                "[Q]uit | [M]ode | [C]hannel | [B]andwidth | [L]TF | [R]eset Error | [C]lear Logs | Clear [C]SI"
            }
            (ToolMode::Spam, FocusedPanel::Main) => {
                "[Q]uit | [M]ode | [E]dit Spam | [T]rigger Burst | [Space] Toggle Continuous | [R]eset Error | [C]lear Logs | Clear [C]SI"
            }
            (_, FocusedPanel::SpamConfig) => "[Esc] Cancel | [Enter] Apply | [Tab] Next | [Shift+Tab] Prev | [←→↑↓] Navigate | [Del] Delete Char",
        }
        .to_string()
    };

    let footer_style = if tui_state.last_error_message.is_some() {
        Style::default().fg(Color::Red)
    } else {
        Style::default()
    };

    let footer_paragraph = Paragraph::new(footer_text_str).wrap(Wrap { trim: true }).block(
        Block::default()
            .borders(Borders::TOP) // Only top border for the footer
            .padding(Padding::new(1, 1, 1, 0)), // Padding: left, right, top, bottom
    );

    f.render_widget(footer_paragraph, global_footer_area);
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;

    #[test]
    fn test_ui_render() {
        let backend = TestBackend::new(100, 50);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut tui_state = TuiState::new();

        // Test with default state (Listen Mode)
        terminal
            .draw(|f| {
                ui(f, &tui_state);
            })
            .unwrap();

        // Test with Spam Mode
        tui_state.tool_mode = ToolMode::Spam;
        terminal
            .draw(|f| {
                ui(f, &tui_state);
            })
            .unwrap();

        // Test with Spam Config Panel focused
        tui_state.focused_panel = FocusedPanel::SpamConfig;
        terminal
            .draw(|f| {
                ui(f, &tui_state);
            })
            .unwrap();

        // Test with an error message
        tui_state.last_error_message = Some("Test error message".to_string());
        terminal
            .draw(|f| {
                ui(f, &tui_state);
            })
            .unwrap();
    }

    #[test]
    fn test_constants() {
        assert_eq!(BASE_ESP_CONFIG_LINES, 6);
        assert_eq!(SPAM_DETAILS_LINES, 4);
    }

    #[test]
    fn test_ui_render_with_different_terminal_sizes() {
        let sizes = vec![
            (80, 24),   // Small terminal
            (100, 50),  // Medium terminal
            (120, 30),  // Wide but short
            (200, 60),  // Large terminal
        ];

        for (width, height) in sizes {
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).unwrap();
            let tui_state = TuiState::new();

            terminal
                .draw(|f| {
                    ui(f, &tui_state);
                })
                .unwrap();
        }
    }

    #[test]
    fn test_ui_render_with_different_connection_statuses() {
        let statuses = vec![
            "CONNECTED (Source Started)",
            "CONNECTED (Data Flowing)",
            "INITIALIZING",
            "DISCONNECTED (Start Fail)",
            "DISCONNECTED (Actor Stopped)",
            "ERROR: Connection failed",
        ];

        for status in statuses {
            let backend = TestBackend::new(100, 50);
            let mut terminal = Terminal::new(backend).unwrap();
            let mut tui_state = TuiState::new();
            tui_state.connection_status = status.to_string();

            terminal
                .draw(|f| {
                    ui(f, &tui_state);
                })
                .unwrap();
        }
    }

    #[test]
    fn test_ui_render_with_different_esp_modes() {
        let modes = vec![
            EspMode::SendingPaused,
            EspMode::SendingBurst,
            EspMode::SendingContinuous,
            EspMode::Listening,
        ];

        for mode in modes {
            let backend = TestBackend::new(100, 50);
            let mut terminal = Terminal::new(backend).unwrap();
            let mut tui_state = TuiState::new();
            tui_state.esp_mode = mode;

            terminal
                .draw(|f| {
                    ui(f, &tui_state);
                })
                .unwrap();
        }
    }

    #[test]
    fn test_ui_render_with_different_device_configurations() {
        let configs = vec![
            // Different channels
            (1, EspBandwidth::Twenty, EspSecondaryChannel::None, EspCsiType::LegacyLTF, 0),
            (6, EspBandwidth::Twenty, EspSecondaryChannel::None, EspCsiType::HighThroughputLTF, 1),
            (11, EspBandwidth::Forty, EspSecondaryChannel::Above, EspCsiType::LegacyLTF, 2),
            (7, EspBandwidth::Forty, EspSecondaryChannel::Below, EspCsiType::HighThroughputLTF, 3),
        ];

        for (channel, bandwidth, secondary_channel, csi_type, manual_scale) in configs {
            let backend = TestBackend::new(100, 50);
            let mut terminal = Terminal::new(backend).unwrap();
            let mut tui_state = TuiState::new();
            
            tui_state.unsaved_esp_config.channel = channel;
            tui_state.unsaved_esp_config.bandwidth = bandwidth;
            tui_state.unsaved_esp_config.secondary_channel = secondary_channel;
            tui_state.unsaved_esp_config.csi_type = csi_type;
            tui_state.unsaved_esp_config.manual_scale = manual_scale;

            terminal
                .draw(|f| {
                    ui(f, &tui_state);
                })
                .unwrap();
        }
    }

    #[test]
    fn test_ui_render_with_sync_status() {
        let sync_counts = vec![0, 1, 5, 10];

        for sync_count in sync_counts {
            let backend = TestBackend::new(100, 50);
            let mut terminal = Terminal::new(backend).unwrap();
            let mut tui_state = TuiState::new();
            tui_state.synced = sync_count;

            terminal
                .draw(|f| {
                    ui(f, &tui_state);
                })
                .unwrap();
        }
    }

    #[test]
    fn test_ui_render_with_populated_csi_data() {
        let backend = TestBackend::new(100, 50);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut tui_state = TuiState::new();

        // Add some mock CSI data
        use lib::csi_types::{CsiData, Complex};
        
        for i in 0..5 {
            let csi_data = CsiData {
                timestamp: 1000.0 + i as f64,
                sequence_number: i as u16,
                rssi: vec![50 + i as u16],
                csi: vec![vec![vec![Complex::new(1.0 + i as f64, 0.5); 10]; 1]; 1],
            };
            tui_state.csi_data.push_back(csi_data);
        }

        terminal
            .draw(|f| {
                ui(f, &tui_state);
            })
            .unwrap();
    }

    #[test]
    fn test_ui_render_with_populated_logs() {
        let backend = TestBackend::new(100, 50);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut tui_state = TuiState::new();

        // Add some mock log entries
        use lib::tui::logs::LogEntry;
        use log::Level;
        use chrono::Local;

        let log_levels = [Level::Error, Level::Warn, Level::Info, Level::Debug];
        for (i, level) in log_levels.iter().enumerate() {
            let log_entry = LogEntry {
                level: *level,
                message: format!("Test log message {}", i),
                timestamp: Local::now(),
            };
            tui_state.logs.push_back(log_entry);
        }

        terminal
            .draw(|f| {
                ui(f, &tui_state);
            })
            .unwrap();
    }

    #[test]
    fn test_ui_render_with_different_panel_focus() {
        let panels = vec![
            FocusedPanel::Main,
            FocusedPanel::SpamConfig,
        ];

        for panel in panels {
            let backend = TestBackend::new(100, 50);
            let mut terminal = Terminal::new(backend).unwrap();
            let mut tui_state = TuiState::new();
            tui_state.focused_panel = panel;

            // Test both in Listen and Spam modes
            for mode in [ToolMode::Listen, ToolMode::Spam] {
                tui_state.tool_mode = mode;
                terminal
                    .draw(|f| {
                        ui(f, &tui_state);
                    })
                    .unwrap();
            }
        }
    }

    #[test]
    fn test_ui_render_with_long_error_message() {
        let backend = TestBackend::new(100, 50);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut tui_state = TuiState::new();
        
        tui_state.last_error_message = Some("This is a very long error message that should be wrapped properly in the footer area to test the wrapping functionality".to_string());

        terminal
            .draw(|f| {
                ui(f, &tui_state);
            })
            .unwrap();
    }

    #[test]
    fn test_ui_render_with_empty_data() {
        let backend = TestBackend::new(100, 50);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut tui_state = TuiState::new();
        
        // Ensure all collections are empty
        tui_state.csi_data.clear();
        tui_state.logs.clear();

        terminal
            .draw(|f| {
                ui(f, &tui_state);
            })
            .unwrap();
    }

    #[test]
    fn test_ui_render_with_minimal_terminal_size() {
        // Test with very small terminal
        let backend = TestBackend::new(10, 5);
        let mut terminal = Terminal::new(backend).unwrap();
        let tui_state = TuiState::new();

        // Should not panic even with minimal space
        terminal
            .draw(|f| {
                ui(f, &tui_state);
            })
            .unwrap();
    }

    #[test]
    fn test_ui_render_spam_mode_with_focused_input() {
        let backend = TestBackend::new(100, 50);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut tui_state = TuiState::new();
        
        tui_state.tool_mode = ToolMode::Spam;
        tui_state.focused_panel = FocusedPanel::SpamConfig;
        
        // Test with different focused inputs using the correct FocussedInput type
        use crate::esp_tool::state::FocussedInput;
        let focused_inputs = vec![
            FocussedInput::SrcMac(0),
            FocussedInput::DstMac(0),
            FocussedInput::Reps(0),
            FocussedInput::PauseMs(0),
            FocussedInput::None,
        ];

        for focused_input in focused_inputs {
            tui_state.focused_input = focused_input;
            terminal
                .draw(|f| {
                    ui(f, &tui_state);
                })
                .unwrap();
        }
    }

    #[test]
    fn test_ui_render_with_maximum_csi_data() {
        let backend = TestBackend::new(100, 50);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut tui_state = TuiState::new();

        // Fill with maximum data to test performance
        use lib::csi_types::{CsiData, Complex};
        
        // Add many CSI frames (but not too many to avoid test timeout)
        for i in 0..100 {
            let csi_data = CsiData {
                timestamp: i as f64,
                sequence_number: i as u16,
                rssi: vec![30 + (i % 50) as u16],
                csi: vec![vec![vec![Complex::new(i as f64, 0.0); 20]; 2]; 1],
            };
            tui_state.csi_data.push_back(csi_data);
        }

        terminal
            .draw(|f| {
                ui(f, &tui_state);
            })
            .unwrap();
    }

    #[test]
    fn test_ui_layout_constraints() {
        // Test that the layout constraints are reasonable
        let backend = TestBackend::new(100, 50);
        let mut terminal = Terminal::new(backend).unwrap();
        let tui_state = TuiState::new();

        terminal
            .draw(|f| {
                ui(f, &tui_state);
                
                // Verify that the frame area is reasonable
                assert!(f.area().width > 0);
                assert!(f.area().height > 0);
            })
            .unwrap();
    }

    #[test]
    fn test_ui_render_with_unsaved_changes() {
        let backend = TestBackend::new(100, 50);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut tui_state = TuiState::new();
        
        tui_state.tool_mode = ToolMode::Spam;
        
        // Make spam settings different to show unsaved changes
        tui_state.unsaved_spam_settings.n_reps = 999;
        // Keep spam_settings at default to show difference

        terminal
            .draw(|f| {
                ui(f, &tui_state);
            })
            .unwrap();
    }
}
