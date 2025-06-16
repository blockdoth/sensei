use std::collections::VecDeque;
use std::vec;

use async_trait::async_trait;
use crossterm::event::{KeyCode, KeyEvent};
use lib::csi_types::CsiData;
use lib::sources::controllers::esp32_controller::{
    Bandwidth as EspBandwidth, CsiType as EspCsiType, CustomFrameParams, Esp32ControllerParams, Esp32DeviceConfig, EspMode,
    OperationMode as EspOperationMode, SecondaryChannel as EspSecondaryChannel,
};
use lib::tui::Tui;
use lib::tui::logs::LogEntry;
use log::{debug, error, info, warn};
use ratatui::Frame;
use tokio::sync::mpsc::{Receiver, Sender};

use super::spam_settings::SpamSettings;
use super::tui::ui;
use super::{CSI_DATA_BUFFER_CAPACITY, EspChannelCommand, LOG_BUFFER_CAPACITY};

/// Indicates which panel in the TUI currently has focus.
#[derive(Debug, PartialEq)]
pub enum FocusedPanel {
    /// The main display panel.
    Main,
    /// The spam configuration editor panel.
    SpamConfig,
}

/// Specifies the current operational mode of the tool.
#[derive(Debug, PartialEq)]
pub enum ToolMode {
    /// Listening for CSI (Channel State Information) data.
    Listen,
    /// Transmitting spam packets.
    Spam,
}

/// Represents possible user interactions or changes in the spam configuration panel.
#[derive(Debug, PartialEq)]
pub enum SpamConfigUpdate {
    /// Insert a character into the focused field.
    Edit(char),
    /// Confirm and apply the changes.
    Enter,
    /// Move focus to the next input field.
    TabRight,
    /// Move focus to the previous input field.
    TabLeft,
    /// Move the input cursor one character to the left.
    CursorLeft,
    /// Move the input cursor one character to the right.
    CursorRight,
    /// Move the cursor/input focus to the field above.
    CursorUp,
    /// Move the cursor/input focus to the field below.
    CursorDown,
    /// Exit the spam config editing panel.
    Escape,
    /// Delete the current character at the cursor.
    Delete,
}

/// Represents all possible events or updates handled by the TUI.
#[derive(Debug, PartialEq)]
pub enum EspUpdate {
    /// A user interaction within the spam configuration panel.
    SpamConfig(SpamConfigUpdate),
    /// A new log entry to display.
    Log(LogEntry),
    /// Status update for connection or operation.
    Status(String),
    /// New CSI data received.
    CsiData(CsiData),
    /// Toggle between Listen and Spam mode.
    ModeChange,
    /// Enter the spam configuration editor.
    EditSpamConfig,
    /// Trigger a single burst of spam packets.
    TriggerBurstSpam,
    /// Toggle continuous spam mode on or off.
    ToggleContinuousSpam,
    /// Increment the current WiFi channel.
    IncrementChannel,
    /// Change the bandwidth setting (e.g., 20 MHz ↔ 40 MHz).
    ChangeBandwidth,
    /// Switch between Legacy and HT CSI modes.
    ChangeCsiMode,
    /// Acknowledge a successful controller update.
    ControllerUpdateSuccess,
    /// Handle ESP device disconnection.
    ClearLogs,
    /// Clears the logs
    ClearCSI,
    /// Clears CSI log
    EspDisconnected,
    /// Exit the application.
    Exit,
}

/// Specifies which input field is currently focused during spam config editing.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum FocussedInput {
    SrcMac(usize),
    DstMac(usize),
    Reps(usize),
    PauseMs(usize),
    None,
}

/// Holds the entire state of the TUI, including configurations, logs, and mode information.
#[derive(Debug)]
pub struct TuiState {
    pub connection_status: String,
    pub should_quit: bool,
    pub focused_panel: FocusedPanel,
    pub focused_input: FocussedInput,
    pub logs: VecDeque<LogEntry>,
    pub tool_mode: ToolMode,
    pub esp_mode: EspMode,
    pub last_error_message: Option<String>,
    pub csi_data: VecDeque<CsiData>,

    pub esp_config: Esp32DeviceConfig,
    pub unsaved_esp_config: Esp32DeviceConfig,

    pub spam_settings: SpamSettings,
    pub unsaved_spam_settings: SpamSettings,
    pub unsaved_changes: bool,
    pub synced: i32,
}

#[async_trait]
impl Tui<EspUpdate, EspChannelCommand> for TuiState {
    /// Draws the UI based on the state of the TUI, should not change any state by itself
    fn draw_ui(&self, f: &mut Frame) {
        ui(f, self);
    }

    /// Handles a single keyboard event and produces and Update, should not change any state
    fn handle_keyboard_event(&self, key_event: KeyEvent) -> Option<EspUpdate> {
        let key_code = key_event.code;
        match self.focused_panel {
            FocusedPanel::SpamConfig => match key_code {
                KeyCode::Backspace => Some(EspUpdate::SpamConfig(SpamConfigUpdate::Delete)),
                KeyCode::Enter => Some(EspUpdate::SpamConfig(SpamConfigUpdate::Enter)),
                KeyCode::Tab => Some(EspUpdate::SpamConfig(SpamConfigUpdate::TabRight)),
                KeyCode::BackTab => Some(EspUpdate::SpamConfig(SpamConfigUpdate::TabLeft)),
                KeyCode::Right => Some(EspUpdate::SpamConfig(SpamConfigUpdate::CursorRight)),
                KeyCode::Left => Some(EspUpdate::SpamConfig(SpamConfigUpdate::CursorLeft)),
                KeyCode::Esc => Some(EspUpdate::SpamConfig(SpamConfigUpdate::Escape)),
                KeyCode::Up => Some(EspUpdate::SpamConfig(SpamConfigUpdate::CursorUp)),
                KeyCode::Down => Some(EspUpdate::SpamConfig(SpamConfigUpdate::CursorDown)),
                KeyCode::Char('q') | KeyCode::Char('Q') if self.focused_input == FocussedInput::None => Some(EspUpdate::Exit),
                KeyCode::Char(chr) => Some(EspUpdate::SpamConfig(SpamConfigUpdate::Edit(chr))),
                _ => None,
            },

            FocusedPanel::Main => match key_code {
                KeyCode::Char('m') | KeyCode::Char('M') => Some(EspUpdate::ModeChange),
                KeyCode::Char('e') | KeyCode::Char('E') => Some(EspUpdate::EditSpamConfig),
                KeyCode::Char('s') | KeyCode::Char('S') => Some(EspUpdate::TriggerBurstSpam),
                KeyCode::Char('t') | KeyCode::Char('T') => Some(EspUpdate::ToggleContinuousSpam),
                KeyCode::Char('c') | KeyCode::Char('C') => Some(EspUpdate::IncrementChannel),
                KeyCode::Char('l') | KeyCode::Char('L') => Some(EspUpdate::ChangeCsiMode),
                KeyCode::Char('b') | KeyCode::Char('B') => Some(EspUpdate::ChangeBandwidth),
                KeyCode::Char('q') | KeyCode::Char('Q') => Some(EspUpdate::Exit),
                KeyCode::Char(',') => Some(EspUpdate::ClearCSI),
                KeyCode::Char('.') => Some(EspUpdate::ClearLogs),
                _ => None,
            },
        }
    }

    /// Handles incoming Updates produced from any source, this is the only place where state should change
    async fn handle_update(&mut self, update: EspUpdate, command_send: &Sender<EspChannelCommand>, update_recv: &mut Receiver<EspUpdate>) {
        match update {
            EspUpdate::SpamConfig(spam_config_update) => match spam_config_update {
                SpamConfigUpdate::Edit(chr) => {
                    match self.focused_input {
                        FocussedInput::SrcMac(string_idx) if chr.is_ascii_hexdigit() => {
                            // string_idx / 2 = byte idx
                            self.unsaved_spam_settings.src_mac[string_idx / 2] =
                                SpamSettings::update_mac(self.unsaved_spam_settings.src_mac[string_idx / 2], chr, string_idx);
                            self.focused_input = self.focused_input.cursor_right();
                            self.unsaved_changes = true;
                        }
                        FocussedInput::DstMac(string_idx) if chr.is_ascii_hexdigit() => {
                            self.unsaved_spam_settings.dst_mac[string_idx / 2] =
                                SpamSettings::update_mac(self.unsaved_spam_settings.dst_mac[string_idx / 2], chr, string_idx);
                            self.focused_input = self.focused_input.cursor_right();
                            self.unsaved_changes = true;
                        }
                        FocussedInput::PauseMs(string_idx) if chr.is_numeric() => {
                            self.unsaved_spam_settings.pause_ms =
                                SpamSettings::modify_digit_at_index(self.unsaved_spam_settings.pause_ms, string_idx, Some(chr));
                            self.focused_input = self.focused_input.cursor_right();
                            self.unsaved_changes = true;
                        }
                        FocussedInput::Reps(string_idx) if chr.is_numeric() => {
                            self.unsaved_spam_settings.n_reps =
                                SpamSettings::modify_digit_at_index(self.unsaved_spam_settings.n_reps, string_idx, Some(chr));
                            self.focused_input = self.focused_input.cursor_right();
                            self.unsaved_changes = true;
                        }
                        _ => (),
                    }
                }
                SpamConfigUpdate::Delete => match self.focused_input {
                    FocussedInput::SrcMac(string_idx) => {
                        self.unsaved_spam_settings.src_mac[string_idx / 2] = 0;
                    }
                    FocussedInput::DstMac(string_idx) => {
                        self.unsaved_spam_settings.dst_mac[string_idx / 2] = 0;
                    }
                    FocussedInput::PauseMs(string_idx) => {
                        self.unsaved_spam_settings.pause_ms =
                            SpamSettings::modify_digit_at_index(self.unsaved_spam_settings.pause_ms, string_idx, None);
                    }
                    FocussedInput::Reps(string_idx) => {
                        self.unsaved_spam_settings.n_reps = SpamSettings::modify_digit_at_index(self.unsaved_spam_settings.n_reps, string_idx, None);
                    }
                    _ => (),
                },
                SpamConfigUpdate::Enter => {
                    self.apply_changes(command_send).await;
                    self.focused_input = FocussedInput::None;
                    self.focused_panel = FocusedPanel::Main;
                }
                SpamConfigUpdate::TabRight => self.focused_input = self.focused_input.tab_right(),
                SpamConfigUpdate::TabLeft => self.focused_input = self.focused_input.tab_left(),
                SpamConfigUpdate::CursorRight => match self.focused_input {
                    FocussedInput::PauseMs(i) if i >= self.unsaved_spam_settings.pause_ms.to_string().len() => {}
                    FocussedInput::Reps(i) if i >= self.unsaved_spam_settings.n_reps.to_string().len() => {}
                    _ => self.focused_input = self.focused_input.cursor_right(),
                },
                SpamConfigUpdate::CursorLeft => self.focused_input = self.focused_input.cursor_left(),
                SpamConfigUpdate::CursorUp => match self.focused_input {
                    FocussedInput::Reps(i) if i >= self.unsaved_spam_settings.n_reps.to_string().len() => {
                        self.focused_input = FocussedInput::PauseMs(self.unsaved_spam_settings.n_reps.to_string().len())
                    }
                    _ => self.focused_input = self.focused_input.cursor_up(),
                },

                SpamConfigUpdate::CursorDown => match self.focused_input {
                    FocussedInput::Reps(i) if i >= self.unsaved_spam_settings.pause_ms.to_string().len() => {
                        self.focused_input = FocussedInput::PauseMs(self.unsaved_spam_settings.pause_ms.to_string().len())
                    }
                    _ => self.focused_input = self.focused_input.cursor_down(),
                },
                SpamConfigUpdate::Escape => {
                    self.focused_input = FocussedInput::None;
                    self.focused_panel = FocusedPanel::Main;
                }
            },
            EspUpdate::EditSpamConfig if self.tool_mode == ToolMode::Spam => {
                self.focused_panel = FocusedPanel::SpamConfig;
                self.focused_input = FocussedInput::SrcMac(0);
            }
            EspUpdate::Log(log_entry) => {
                if self.logs.len() >= LOG_BUFFER_CAPACITY {
                    self.logs.pop_front();
                }
                self.logs.push_back(log_entry);
            }
            EspUpdate::Status(message) => self.connection_status = message,
            EspUpdate::CsiData(csi) => {
                debug!("Received CSI data");
                if self.csi_data.len() >= CSI_DATA_BUFFER_CAPACITY {
                    self.csi_data.pop_front();
                }
                self.csi_data.push_back(csi);
            }
            EspUpdate::ChangeBandwidth => {
                match self.esp_config.bandwidth {
                    EspBandwidth::Twenty => {
                        self.unsaved_esp_config.bandwidth = EspBandwidth::Forty;
                        self.unsaved_esp_config.secondary_channel = EspSecondaryChannel::Above;
                    }
                    EspBandwidth::Forty => {
                        self.unsaved_esp_config.bandwidth = EspBandwidth::Twenty;
                        self.unsaved_esp_config.secondary_channel = EspSecondaryChannel::None;
                    }
                };
                self.unsaved_changes = true;
                self.apply_changes(command_send).await;
            }
            EspUpdate::ChangeCsiMode => {
                self.unsaved_esp_config.csi_type = match self.esp_config.csi_type {
                    EspCsiType::HighThroughputLTF => EspCsiType::LegacyLTF,
                    EspCsiType::LegacyLTF => EspCsiType::HighThroughputLTF,
                };
                self.unsaved_changes = true;
                self.apply_changes(command_send).await;
            }
            EspUpdate::IncrementChannel => {
                self.unsaved_esp_config.channel = (self.esp_config.channel % 11) + 1;
                self.unsaved_changes = true;
                self.apply_changes(command_send).await;
            }
            EspUpdate::ModeChange => {
                match self.tool_mode {
                    ToolMode::Spam => {
                        self.unsaved_esp_config.mode = EspOperationMode::Receive;
                        self.esp_mode = EspMode::Listening;
                        self.tool_mode = ToolMode::Listen;
                    }
                    ToolMode::Listen => {
                        self.unsaved_esp_config.mode = EspOperationMode::Transmit;
                        self.esp_mode = EspMode::SendingPaused;
                        self.tool_mode = ToolMode::Spam;
                    }
                };
                self.last_error_message = None;
                self.unsaved_changes = true;
                self.apply_changes(command_send).await;
            }
            EspUpdate::TriggerBurstSpam => match self.esp_mode {
                EspMode::SendingContinuous => {
                    error!("Already continuously sending, please pause sending before starting a burst")
                }
                EspMode::SendingBurst => {
                    // There is currently no way to know when a burst has ended, so this has to do
                    warn!("Previous burst might not have ended yet, please wait until it is done before starting another burst");
                    self.esp_mode = EspMode::SendingPaused;
                }
                EspMode::SendingPaused => {
                    self.unsaved_esp_config.mode = EspOperationMode::Transmit;
                    self.esp_mode = EspMode::SendingBurst;
                    self.unsaved_changes = true;
                    self.apply_changes(command_send).await;
                }
                _ => {}
            },
            EspUpdate::ToggleContinuousSpam => match self.esp_mode {
                EspMode::SendingBurst => {
                    error!("Already in burst, please pause or wait until it is done before starting spamming")
                }
                EspMode::SendingContinuous => {
                    info!("Turned off spam mode");
                    self.esp_mode = EspMode::SendingPaused;
                    self.unsaved_changes = true;
                    self.apply_changes(command_send).await;
                }
                EspMode::SendingPaused => {
                    info!("Turning on spam mode");
                    self.unsaved_esp_config.mode = EspOperationMode::Transmit;
                    self.esp_mode = EspMode::SendingContinuous;
                    self.unsaved_changes = true;
                    self.apply_changes(command_send).await;
                }
                _ => {}
            },
            EspUpdate::Exit => {
                if self.esp_config.mode == EspOperationMode::Transmit {
                    info!("Shutdown: Requesting to PAUSE WiFi transmit task (was in Transmit mode).");
                } else {
                    info!("Shutdown: Skipping PauseWifiTransmit command (was in Receive mode, task likely not active).");
                }
                command_send.send(EspChannelCommand::Exit).await;
                self.should_quit = true;
            }
            EspUpdate::ControllerUpdateSuccess => {
                info!("Controller updated successfully");
                self.synced -= 1;
            }
            EspUpdate::EspDisconnected => {
                self.connection_status = "ESP disconnected".to_string();
                self.synced = 0;
            }
            EspUpdate::ClearCSI => {
                info!("CSI logs cleared");
                self.csi_data.clear();
            }
            EspUpdate::ClearLogs => {
                info!("Logs cleared");
                self.logs.clear();
            }
            _ => {}
        }
    }

    // Gets called each tick of the main loop, useful for updating graphs and live views, should only make small changes to state
    async fn on_tick(&mut self) {}

    // Whether the tui should quit
    fn should_quit(&self) -> bool {
        self.should_quit
    }
}

impl TuiState {
    /// Constructs a new `TuiState` with default configurations.
    pub fn new() -> Self {
        Self {
            last_error_message: None,
            connection_status: "INITIALIZING...".into(),
            should_quit: false,
            focused_panel: FocusedPanel::Main,
            focused_input: FocussedInput::None,
            logs: VecDeque::with_capacity(LOG_BUFFER_CAPACITY),
            esp_config: Esp32DeviceConfig::default(),
            unsaved_esp_config: Esp32DeviceConfig::default(),
            esp_mode: EspMode::Listening,
            tool_mode: ToolMode::Listen,
            csi_data: VecDeque::with_capacity(CSI_DATA_BUFFER_CAPACITY),
            spam_settings: SpamSettings::default(),
            unsaved_spam_settings: SpamSettings::default(),
            unsaved_changes: false,
            synced: 1,
        }
    }

    /// Applies any pending changes in the ESP or spam configuration to the controller.
    ///
    /// This function is responsible for sending updates over the command channel.
    pub async fn apply_changes(&mut self, command_send: &Sender<EspChannelCommand>) {
        if self.unsaved_changes {
            // TODO figure out a more elegant solution
            let spam_settings = if self.esp_mode != EspMode::Listening {
                Some(CustomFrameParams {
                    src_mac: self.unsaved_spam_settings.src_mac,
                    dst_mac: self.unsaved_spam_settings.dst_mac,
                    pause_ms: self.unsaved_spam_settings.pause_ms,
                    n_reps: self.unsaved_spam_settings.n_reps,
                })
            } else {
                None
            };

            let new_controller = Esp32ControllerParams {
                device_config: self.unsaved_esp_config.clone(),
                mac_filters: vec![], // TODO support mac filtering
                mode: self.esp_mode.clone(),
                synchronize_time: false,
                transmit_custom_frame: spam_settings,
            };

            let msg = EspChannelCommand::UpdatedConfig(new_controller);
            match command_send.send(msg).await {
                Ok(_) => {
                    info!("Applied update");
                    self.spam_settings = self.unsaved_spam_settings.clone();
                    self.esp_config = self.unsaved_esp_config.clone();
                    self.synced += 1;
                }
                Err(_) => {
                    error!("Failed to apply update")
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tokio::sync::mpsc;

    use super::*;

    fn create_key_event(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn test_tui_state_new() {
        let state = TuiState::new();
        assert_eq!(state.connection_status, "INITIALIZING...");
        assert!(!state.should_quit);
        assert_eq!(state.focused_panel, FocusedPanel::Main);
        assert_eq!(state.focused_input, FocussedInput::None);
        assert_eq!(state.logs.capacity(), LOG_BUFFER_CAPACITY);
        assert_eq!(state.csi_data.capacity(), CSI_DATA_BUFFER_CAPACITY);
        assert_eq!(state.tool_mode, ToolMode::Listen);
        assert_eq!(state.esp_mode, EspMode::Listening);
        assert_eq!(state.esp_config, Esp32DeviceConfig::default());
        assert_eq!(state.unsaved_esp_config, Esp32DeviceConfig::default());
        assert_eq!(state.spam_settings, SpamSettings::default());
        assert_eq!(state.unsaved_spam_settings, SpamSettings::default());
        assert!(!state.unsaved_changes);
        assert_eq!(state.synced, 1);
    }

    #[test]
    fn test_handle_keyboard_event_main_panel() {
        let state = TuiState::new(); // focused_panel is Main by default

        assert_eq!(
            state.handle_keyboard_event(create_key_event(KeyCode::Char('m'))),
            Some(EspUpdate::ModeChange)
        );
        assert_eq!(
            state.handle_keyboard_event(create_key_event(KeyCode::Char('e'))),
            Some(EspUpdate::EditSpamConfig)
        );
        assert_eq!(
            state.handle_keyboard_event(create_key_event(KeyCode::Char('s'))),
            Some(EspUpdate::TriggerBurstSpam)
        );
        assert_eq!(
            state.handle_keyboard_event(create_key_event(KeyCode::Char('t'))),
            Some(EspUpdate::ToggleContinuousSpam)
        );
        assert_eq!(
            state.handle_keyboard_event(create_key_event(KeyCode::Char('c'))),
            Some(EspUpdate::IncrementChannel)
        );
        assert_eq!(
            state.handle_keyboard_event(create_key_event(KeyCode::Char('l'))),
            Some(EspUpdate::ChangeCsiMode)
        );
        assert_eq!(
            state.handle_keyboard_event(create_key_event(KeyCode::Char('b'))),
            Some(EspUpdate::ChangeBandwidth)
        );
        assert_eq!(state.handle_keyboard_event(create_key_event(KeyCode::Char('q'))), Some(EspUpdate::Exit));
        assert_eq!(
            state.handle_keyboard_event(create_key_event(KeyCode::Char(','))),
            Some(EspUpdate::ClearCSI)
        );
        assert_eq!(
            state.handle_keyboard_event(create_key_event(KeyCode::Char('.'))),
            Some(EspUpdate::ClearLogs)
        );
        assert_eq!(state.handle_keyboard_event(create_key_event(KeyCode::Char('x'))), None); // Unmapped key
    }

    #[test]
    fn test_handle_keyboard_event_spam_config_panel() {
        let mut state = TuiState::new();
        state.focused_panel = FocusedPanel::SpamConfig;

        assert_eq!(
            state.handle_keyboard_event(create_key_event(KeyCode::Backspace)),
            Some(EspUpdate::SpamConfig(SpamConfigUpdate::Delete))
        );
        assert_eq!(
            state.handle_keyboard_event(create_key_event(KeyCode::Enter)),
            Some(EspUpdate::SpamConfig(SpamConfigUpdate::Enter))
        );
        assert_eq!(
            state.handle_keyboard_event(create_key_event(KeyCode::Tab)),
            Some(EspUpdate::SpamConfig(SpamConfigUpdate::TabRight))
        );
        assert_eq!(
            state.handle_keyboard_event(create_key_event(KeyCode::BackTab)),
            Some(EspUpdate::SpamConfig(SpamConfigUpdate::TabLeft))
        );
        assert_eq!(
            state.handle_keyboard_event(create_key_event(KeyCode::Right)),
            Some(EspUpdate::SpamConfig(SpamConfigUpdate::CursorRight))
        );
        assert_eq!(
            state.handle_keyboard_event(create_key_event(KeyCode::Left)),
            Some(EspUpdate::SpamConfig(SpamConfigUpdate::CursorLeft))
        );
        assert_eq!(
            state.handle_keyboard_event(create_key_event(KeyCode::Esc)),
            Some(EspUpdate::SpamConfig(SpamConfigUpdate::Escape))
        );
        assert_eq!(
            state.handle_keyboard_event(create_key_event(KeyCode::Up)),
            Some(EspUpdate::SpamConfig(SpamConfigUpdate::CursorUp))
        );
        assert_eq!(
            state.handle_keyboard_event(create_key_event(KeyCode::Down)),
            Some(EspUpdate::SpamConfig(SpamConfigUpdate::CursorDown))
        );
        assert_eq!(
            state.handle_keyboard_event(create_key_event(KeyCode::Char('a'))),
            Some(EspUpdate::SpamConfig(SpamConfigUpdate::Edit('a')))
        );

        // Test 'q' for exit when FocussedInput is None
        state.focused_input = FocussedInput::None;
        assert_eq!(state.handle_keyboard_event(create_key_event(KeyCode::Char('q'))), Some(EspUpdate::Exit));
        state.focused_input = FocussedInput::SrcMac(0); // Reset for next potential tests
        assert_eq!(
            state.handle_keyboard_event(create_key_event(KeyCode::Char('q'))),
            Some(EspUpdate::SpamConfig(SpamConfigUpdate::Edit('q')))
        );

        assert_eq!(state.handle_keyboard_event(create_key_event(KeyCode::F(1))), None); // Unmapped key
    }

    #[tokio::test]
    async fn test_handle_update_log() {
        let mut state = TuiState::new();
        let timestamp = chrono::Local::now();
        let log_entry = LogEntry {
            timestamp,
            level: log::Level::Info,
            message: "Test log".to_string(),
        };
        let (cmd_tx, _) = mpsc::channel(1);
        let (_, mut update_rx) = mpsc::channel(1);

        state.handle_update(EspUpdate::Log(log_entry.clone()), &cmd_tx, &mut update_rx).await;
        assert_eq!(state.logs.len(), 1);
        // Compare relevant fields, timestamp might be tricky due to exact timing
        assert_eq!(state.logs.front().unwrap().message, "Test log");
        assert_eq!(state.logs.front().unwrap().level, log::Level::Info);

        // Test log capacity
        for i in 0..LOG_BUFFER_CAPACITY + 5 {
            let entry_ts = chrono::Local::now();
            let entry = LogEntry {
                timestamp: entry_ts,
                level: log::Level::Info,
                message: format!("Log {i}"),
            };
            state.handle_update(EspUpdate::Log(entry), &cmd_tx, &mut update_rx).await;
        }
        assert_eq!(state.logs.len(), LOG_BUFFER_CAPACITY);
        assert_eq!(state.logs.back().unwrap().message, format!("Log {}", LOG_BUFFER_CAPACITY + 4)); // Corrected index
        assert_eq!(state.logs.front().unwrap().message, format!("Log {}", 5));
    }

    #[tokio::test]
    async fn test_handle_update_status() {
        let mut state = TuiState::new();
        let (cmd_tx, _) = mpsc::channel(1);
        let (_, mut update_rx) = mpsc::channel(1);
        state
            .handle_update(EspUpdate::Status("CONNECTED".to_string()), &cmd_tx, &mut update_rx)
            .await;
        assert_eq!(state.connection_status, "CONNECTED");
    }

    #[tokio::test]
    async fn test_handle_update_csi_data() {
        let mut state = TuiState::new();
        let csi_data_entry = CsiData::default();
        let (cmd_tx, _) = mpsc::channel(1);
        let (_, mut update_rx) = mpsc::channel(1);

        state
            .handle_update(EspUpdate::CsiData(csi_data_entry.clone()), &cmd_tx, &mut update_rx)
            .await;
        assert_eq!(state.csi_data.len(), 1);

        // Test CSI data capacity
        for _i in 0..CSI_DATA_BUFFER_CAPACITY + 5 {
            state.handle_update(EspUpdate::CsiData(CsiData::default()), &cmd_tx, &mut update_rx).await;
        }
        assert_eq!(state.csi_data.len(), CSI_DATA_BUFFER_CAPACITY);
    }

    #[tokio::test]
    async fn test_handle_update_mode_change() {
        let mut state = TuiState::new();
        let (cmd_tx, mut cmd_rx) = mpsc::channel(1);
        let (_, mut update_rx) = mpsc::channel(1);

        // Listen -> Spam
        state.handle_update(EspUpdate::ModeChange, &cmd_tx, &mut update_rx).await;
        assert_eq!(state.tool_mode, ToolMode::Spam);
        assert_eq!(state.esp_mode, EspMode::SendingPaused);
        assert_eq!(state.unsaved_esp_config.mode, EspOperationMode::Transmit);
        assert!(state.unsaved_changes);
        if let Ok(EspChannelCommand::UpdatedConfig(params)) = cmd_rx.try_recv() {
            assert_eq!(params.mode, EspMode::SendingPaused);
        } else {
            panic!("Command not sent");
        }

        // Spam -> Listen
        state.handle_update(EspUpdate::ModeChange, &cmd_tx, &mut update_rx).await;
        assert_eq!(state.tool_mode, ToolMode::Listen);
        assert_eq!(state.esp_mode, EspMode::Listening);
        assert_eq!(state.unsaved_esp_config.mode, EspOperationMode::Receive);
        assert!(state.unsaved_changes);
        if let Ok(EspChannelCommand::UpdatedConfig(params)) = cmd_rx.try_recv() {
            assert_eq!(params.mode, EspMode::Listening);
        } else {
            panic!("Command not sent");
        }
    }

    #[tokio::test]
    async fn test_handle_update_edit_spam_config() {
        let mut state = TuiState::new();
        state.tool_mode = ToolMode::Spam; // Prerequisite
        let (cmd_tx, _) = mpsc::channel(1);
        let (_, mut update_rx) = mpsc::channel(1);

        state.handle_update(EspUpdate::EditSpamConfig, &cmd_tx, &mut update_rx).await;
        assert_eq!(state.focused_panel, FocusedPanel::SpamConfig);
        assert_eq!(state.focused_input, FocussedInput::SrcMac(0));

        // Should not change if not in Spam mode
        state.tool_mode = ToolMode::Listen;
        state.focused_panel = FocusedPanel::Main;
        state.handle_update(EspUpdate::EditSpamConfig, &cmd_tx, &mut update_rx).await;
        assert_eq!(state.focused_panel, FocusedPanel::Main);
    }

    #[tokio::test]
    async fn test_handle_update_exit() {
        let mut state = TuiState::new();
        let (cmd_tx, mut cmd_rx) = mpsc::channel(1);
        let (_, mut update_rx) = mpsc::channel(1);

        state.handle_update(EspUpdate::Exit, &cmd_tx, &mut update_rx).await;
        assert!(state.should_quit);
        assert!(matches!(cmd_rx.try_recv(), Ok(EspChannelCommand::Exit)));
    }

    #[tokio::test]
    async fn test_handle_update_spam_config_edit() {
        let mut state = TuiState::new();
        state.focused_panel = FocusedPanel::SpamConfig;
        let (cmd_tx, _) = mpsc::channel(1);
        let (_, mut update_rx) = mpsc::channel(1);

        // Edit SrcMac
        state.focused_input = FocussedInput::SrcMac(0);
        state
            .handle_update(EspUpdate::SpamConfig(SpamConfigUpdate::Edit('A')), &cmd_tx, &mut update_rx)
            .await;
        assert_eq!(state.unsaved_spam_settings.src_mac[0], 0xA2); // 'A' replaces high nibble of 0x12, resulting in 0xA2
        assert_eq!(state.focused_input, FocussedInput::SrcMac(1));
        assert!(state.unsaved_changes);
        state.unsaved_changes = false; // Reset for next

        // Edit DstMac
        state.focused_input = FocussedInput::DstMac(2); // 2nd byte, high nibble
        state
            .handle_update(EspUpdate::SpamConfig(SpamConfigUpdate::Edit('B')), &cmd_tx, &mut update_rx)
            .await;
        assert_eq!(state.unsaved_spam_settings.dst_mac[1], 0xB2); // 'B' replaces high nibble of 0x82, resulting in 0xB2
        assert_eq!(state.focused_input, FocussedInput::DstMac(3));
        assert!(state.unsaved_changes);
        state.unsaved_changes = false;

        // Edit Reps
        state.focused_input = FocussedInput::Reps(0);
        state.unsaved_spam_settings.n_reps = 10;
        state
            .handle_update(EspUpdate::SpamConfig(SpamConfigUpdate::Edit('2')), &cmd_tx, &mut update_rx)
            .await;
        assert_eq!(state.unsaved_spam_settings.n_reps, 20); // "2" replaces "1"
        assert_eq!(state.focused_input, FocussedInput::Reps(1));
        assert!(state.unsaved_changes);
        state.unsaved_changes = false;

        // Edit PauseMs
        state.focused_input = FocussedInput::PauseMs(1);
        state.unsaved_spam_settings.pause_ms = 53;
        state
            .handle_update(EspUpdate::SpamConfig(SpamConfigUpdate::Edit('7')), &cmd_tx, &mut update_rx)
            .await;
        assert_eq!(state.unsaved_spam_settings.pause_ms, 57); // "7" replaces "3"
        assert_eq!(state.focused_input, FocussedInput::PauseMs(2));
        assert!(state.unsaved_changes);
    }

    #[tokio::test]
    async fn test_handle_update_spam_config_delete() {
        let mut state = TuiState::new();
        state.focused_panel = FocusedPanel::SpamConfig;
        let (cmd_tx, _) = mpsc::channel(1);
        let (_, mut update_rx) = mpsc::channel(1);

        // Delete in SrcMac
        state.focused_input = FocussedInput::SrcMac(0);
        state.unsaved_spam_settings.src_mac[0] = 0xAB;
        state
            .handle_update(EspUpdate::SpamConfig(SpamConfigUpdate::Delete), &cmd_tx, &mut update_rx)
            .await;
        assert_eq!(state.unsaved_spam_settings.src_mac[0], 0);

        // Delete in Reps
        state.focused_input = FocussedInput::Reps(0);
        state.unsaved_spam_settings.n_reps = 123;
        state
            .handle_update(EspUpdate::SpamConfig(SpamConfigUpdate::Delete), &cmd_tx, &mut update_rx)
            .await;
        assert_eq!(state.unsaved_spam_settings.n_reps, 23); // "1" is removed
    }

    #[tokio::test]
    async fn test_handle_update_spam_config_enter() {
        let mut state = TuiState::new();
        state.focused_panel = FocusedPanel::SpamConfig;
        state.unsaved_changes = true; // Simulate some changes
        let (cmd_tx, mut cmd_rx) = mpsc::channel(1);
        let (_, mut update_rx) = mpsc::channel(1);

        state
            .handle_update(EspUpdate::SpamConfig(SpamConfigUpdate::Enter), &cmd_tx, &mut update_rx)
            .await;
        assert_eq!(state.focused_input, FocussedInput::None);
        assert_eq!(state.focused_panel, FocusedPanel::Main);
        assert!(cmd_rx.try_recv().is_ok()); // Check if apply_changes was called
    }

    #[tokio::test]
    async fn test_handle_update_spam_config_escape() {
        let mut state = TuiState::new();
        state.focused_panel = FocusedPanel::SpamConfig;
        state.focused_input = FocussedInput::SrcMac(0);
        let (cmd_tx, _) = mpsc::channel(1);
        let (_, mut update_rx) = mpsc::channel(1);

        state
            .handle_update(EspUpdate::SpamConfig(SpamConfigUpdate::Escape), &cmd_tx, &mut update_rx)
            .await;
        assert_eq!(state.focused_input, FocussedInput::None);
        assert_eq!(state.focused_panel, FocusedPanel::Main);
    }

    #[tokio::test]
    async fn test_handle_update_change_bandwidth() {
        let mut state = TuiState::new();
        let (cmd_tx, mut cmd_rx) = mpsc::channel(1);
        let (_, mut update_rx) = mpsc::channel(1);

        // Twenty -> Forty
        state.esp_config.bandwidth = EspBandwidth::Twenty;
        state.unsaved_esp_config.bandwidth = EspBandwidth::Twenty;
        state.handle_update(EspUpdate::ChangeBandwidth, &cmd_tx, &mut update_rx).await;
        assert_eq!(state.unsaved_esp_config.bandwidth, EspBandwidth::Forty);
        assert_eq!(state.unsaved_esp_config.secondary_channel, EspSecondaryChannel::Above);
        assert!(state.unsaved_changes);
        assert!(cmd_rx.try_recv().is_ok());

        // Forty -> Twenty
        state.esp_config.bandwidth = EspBandwidth::Forty; // Simulate change applied
        state.unsaved_esp_config.bandwidth = EspBandwidth::Forty;
        state.handle_update(EspUpdate::ChangeBandwidth, &cmd_tx, &mut update_rx).await;
        assert_eq!(state.unsaved_esp_config.bandwidth, EspBandwidth::Twenty);
        assert_eq!(state.unsaved_esp_config.secondary_channel, EspSecondaryChannel::None);
        assert!(state.unsaved_changes);
        assert!(cmd_rx.try_recv().is_ok());
    }

    #[tokio::test]
    async fn test_handle_update_increment_channel() {
        let mut state = TuiState::new();
        let (cmd_tx, mut cmd_rx) = mpsc::channel(1);
        let (_, mut update_rx) = mpsc::channel(1);

        state.esp_config.channel = 1;
        state.unsaved_esp_config.channel = 1;
        state.handle_update(EspUpdate::IncrementChannel, &cmd_tx, &mut update_rx).await;
        assert_eq!(state.unsaved_esp_config.channel, 2);
        assert!(state.unsaved_changes);
        assert!(cmd_rx.try_recv().is_ok());

        state.esp_config.channel = 11; // Simulate change applied
        state.unsaved_esp_config.channel = 11;
        state.handle_update(EspUpdate::IncrementChannel, &cmd_tx, &mut update_rx).await;
        assert_eq!(state.unsaved_esp_config.channel, 1); // Wrap around
        assert!(state.unsaved_changes);
        assert!(cmd_rx.try_recv().is_ok());
    }

    #[tokio::test]
    async fn test_apply_changes() {
        let mut state = TuiState::new();
        let (cmd_tx, mut cmd_rx) = mpsc::channel(1);

        state.unsaved_changes = false;
        state.apply_changes(&cmd_tx).await;
        assert!(cmd_rx.try_recv().is_err()); // No command if no changes

        state.unsaved_changes = true;
        state.unsaved_esp_config.channel = 5;
        state.unsaved_spam_settings.n_reps = 100;
        state.esp_mode = EspMode::SendingContinuous; // To include spam settings

        state.apply_changes(&cmd_tx).await;

        if let Ok(EspChannelCommand::UpdatedConfig(params)) = cmd_rx.try_recv() {
            assert_eq!(params.device_config.channel, 5);
            assert!(params.transmit_custom_frame.is_some());
            assert_eq!(params.transmit_custom_frame.unwrap().n_reps, 100);
            assert_eq!(state.esp_config.channel, 5);
            assert_eq!(state.spam_settings.n_reps, 100);
            assert_eq!(state.synced, 2); // Initial 1 + 1 for this change
        } else {
            panic!("UpdatedConfig command not received or incorrect type");
        }
    }
}
