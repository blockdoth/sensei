use std::collections::VecDeque;

use super::{AppUpdate, CSI_DATA_BUFFER_CAPACITY, LOG_BUFFER_CAPACITY};
use crossterm::event::{KeyCode, KeyEvent};
use lib::{
    csi_types::CsiData,
    sources::{
        controllers::esp32_controller::{
            Bandwidth as EspBandwidth, CsiType as EspCsiType, CustomFrameParams, Esp32Controller, Esp32DeviceConfig,
            OperationMode as EspOperationMode, SecondaryChannel as EspSecondaryChannel,
        },
        esp32::Esp32SourceConfig,
    },
    tui::logs::LogEntry,
};
use log::{info, warn};
use tokio::sync::mpsc;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum UiMode {
    Csi,
    Spam,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SpamConfigField {
    SrcMacOctet(usize),
    DstMacOctet(usize),
    Reps,
    PauseMs,
}

#[derive(Clone, Debug)]
pub struct SpamSettings {
    pub src_mac: [u8; 6],
    pub dst_mac: [u8; 6],
    pub n_reps: i32,
    pub pause_ms: i32,
}

impl Default for SpamSettings {
    fn default() -> Self {
        Self {
            src_mac: [0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC],
            dst_mac: [0xB4, 0x82, 0xC5, 0x58, 0xA1, 0xC0],
            n_reps: 1000,
            pause_ms: 100,
        }
    }
}

impl SpamConfigField {
    fn next(self) -> Self {
        match self {
            SpamConfigField::SrcMacOctet(5) => SpamConfigField::DstMacOctet(0),
            SpamConfigField::SrcMacOctet(i) => SpamConfigField::SrcMacOctet(i + 1),
            SpamConfigField::DstMacOctet(5) => SpamConfigField::Reps,
            SpamConfigField::DstMacOctet(i) => SpamConfigField::DstMacOctet(i + 1),
            SpamConfigField::Reps => SpamConfigField::PauseMs,
            SpamConfigField::PauseMs => SpamConfigField::SrcMacOctet(0),
        }
    }

    fn prev(self) -> Self {
        match self {
            SpamConfigField::SrcMacOctet(0) => SpamConfigField::PauseMs,
            SpamConfigField::SrcMacOctet(i) => SpamConfigField::SrcMacOctet(i - 1),
            SpamConfigField::DstMacOctet(0) => SpamConfigField::SrcMacOctet(5),
            SpamConfigField::DstMacOctet(i) => SpamConfigField::DstMacOctet(i - 1),
            SpamConfigField::Reps => SpamConfigField::DstMacOctet(5),
            SpamConfigField::PauseMs => SpamConfigField::Reps,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TuiState {
    pub ui_mode: UiMode,
    pub esp_config: Esp32DeviceConfig,
    pub csi_data: VecDeque<CsiData>,
    pub connection_status: String,
    pub last_error: Option<String>,
    pub log_messages: VecDeque<LogEntry>,
    pub spam_settings: SpamSettings,
    pub is_editing_spam_config: bool,
    pub current_editing_field: SpamConfigField,
    pub is_continuous_spam_active: bool,
    pub spam_input_buffer: String,
    pub current_field_has_error: bool,
    pub should_quit: bool,
    // Fields to store the state *before* an optimistic update due to a command
    pub previous_esp_config_for_revert: Option<Esp32DeviceConfig>,
    pub previous_is_continuous_spam_active_for_revert: Option<bool>,
}

impl TuiState {
    pub fn new() -> Self {
        Self {
            ui_mode: UiMode::Csi,
            esp_config: Esp32DeviceConfig::default(),
            csi_data: VecDeque::with_capacity(CSI_DATA_BUFFER_CAPACITY),
            connection_status: "INITIALIZING...".into(),
            last_error: None,
            log_messages: VecDeque::with_capacity(LOG_BUFFER_CAPACITY),
            spam_settings: SpamSettings::default(),
            is_editing_spam_config: false,
            current_editing_field: SpamConfigField::SrcMacOctet(0),
            spam_input_buffer: String::new(),
            current_field_has_error: false,
            is_continuous_spam_active: false,
            should_quit: false,
            previous_esp_config_for_revert: None,
            previous_is_continuous_spam_active_for_revert: None,
        }
    }
    // Adds a log message to the log buffer
    pub fn add_log_message(&mut self, entry: LogEntry) {
        if self.log_messages.len() >= LOG_BUFFER_CAPACITY {
            self.log_messages.pop_front();
        }
        self.log_messages.push_back(entry);
    }

    // Adds csi data to the buffer
    pub fn add_csi_data(&mut self, data: CsiData) {
        if self.csi_data.len() >= CSI_DATA_BUFFER_CAPACITY {
            self.csi_data.pop_front();
        }
        self.csi_data.push_back(data);
    }

    pub fn apply_spam_value_change(&mut self, increment: bool) {
        if !self.is_editing_spam_config {
            return;
        }
        let val_change_u8: u8 = if increment { 1 } else { 255 }; // u8::MAX for wrapping_sub(1)
        let val_change_i32: i32 = if increment { 1 } else { -1 };

        match self.current_editing_field {
            SpamConfigField::SrcMacOctet(i) => {
                self.spam_settings.src_mac[i] = self.spam_settings.src_mac[i].wrapping_add(val_change_u8);
            }
            SpamConfigField::DstMacOctet(i) => {
                self.spam_settings.dst_mac[i] = self.spam_settings.dst_mac[i].wrapping_add(val_change_u8);
            }
            SpamConfigField::Reps => {
                self.spam_settings.n_reps = (self.spam_settings.n_reps + val_change_i32).max(0);
            }
            SpamConfigField::PauseMs => {
                self.spam_settings.pause_ms = (self.spam_settings.pause_ms + val_change_i32).max(0);
            }
        }
    }

    // Parses and applies the spam input buffer
    fn apply_spam_input_buffer(&mut self) {
        let buffer_content = self.spam_input_buffer.trim();
        self.current_field_has_error = false;

        if buffer_content.is_empty() {
            match self.current_editing_field {
                SpamConfigField::SrcMacOctet(_) | SpamConfigField::DstMacOctet(_) => {
                    self.last_error = Some("MAC octet cannot be empty. Reverted.".to_string());
                    self.current_field_has_error = true;
                    return;
                }
                SpamConfigField::Reps => self.spam_settings.n_reps = 0,
                SpamConfigField::PauseMs => self.spam_settings.pause_ms = 0,
            }
            self.last_error = None;
            return;
        }

        let parse_result: Result<(), String> = match self.current_editing_field {
            SpamConfigField::SrcMacOctet(i) => u8::from_str_radix(buffer_content, 16)
                .map_err(|e| format!("Invalid SrcMAC Octet value: '{buffer_content}' ({e})"))
                .map(|val| self.spam_settings.src_mac[i] = val),
            SpamConfigField::DstMacOctet(i) => u8::from_str_radix(buffer_content, 16)
                .map_err(|e| format!("Invalid DstMAC Octet value: '{buffer_content}' ({e})"))
                .map(|val| self.spam_settings.dst_mac[i] = val),
            SpamConfigField::Reps => buffer_content
                .parse::<i32>()
                .map_err(|e| format!("Invalid Reps value: '{buffer_content}' ({e})"))
                .and_then(|val| {
                    if val >= 0 {
                        self.spam_settings.n_reps = val;
                        Ok(())
                    } else {
                        Err("Reps must be non-negative".to_string())
                    }
                }),
            SpamConfigField::PauseMs => buffer_content
                .parse::<i32>()
                .map_err(|e| format!("Invalid PauseMs value: '{buffer_content}' ({e})"))
                .and_then(|val| {
                    if val >= 0 {
                        self.spam_settings.pause_ms = val;
                        Ok(())
                    } else {
                        Err("PauseMs must be non-negative".to_string())
                    }
                }),
        };

        if let Err(e) = parse_result {
            self.last_error = Some(e);
            self.current_field_has_error = true;
        } else {
            self.last_error = None;
        }
    }

    fn prepare_spam_input_buffer_for_current_field(&mut self) {
        self.spam_input_buffer.clear();
        self.current_field_has_error = false;
        match self.current_editing_field {
            SpamConfigField::SrcMacOctet(i) => {
                self.spam_input_buffer = format!("{:02X}", self.spam_settings.src_mac[i]);
            }
            SpamConfigField::DstMacOctet(i) => {
                self.spam_input_buffer = format!("{:02X}", self.spam_settings.dst_mac[i]);
            }
            SpamConfigField::Reps => {
                self.spam_input_buffer = self.spam_settings.n_reps.to_string();
            }
            SpamConfigField::PauseMs => {
                self.spam_input_buffer = self.spam_settings.pause_ms.to_string();
            }
        }
    }

    // Handles all incoming keyboard events
    pub async fn handle_keyboard_event(
        &mut self,
        key_event: KeyEvent,
        command_tx: &mpsc::Sender<Esp32Controller>,
        update_tx: &mpsc::Sender<AppUpdate>,
    ) -> bool {
        if key_event.code != KeyCode::Char('r') && key_event.code != KeyCode::Char('R') {
            if self.last_error.as_deref().unwrap_or("").contains("Press 'R' to dismiss") {
                // Do nothing, keep error until R is pressed
            } else if !self.last_error.as_deref().unwrap_or("").starts_with("Cmd Send Fail:") && // Don't clear cmd send fail errors this way
                !self.last_error.as_deref().unwrap_or("").contains("actor error:")
            // Don't clear actor errors this way
            {
                self.last_error = None;
            }
        }

        if self.is_editing_spam_config {
            match key_event.code {
                KeyCode::Char(ch) => match self.current_editing_field {
                    SpamConfigField::SrcMacOctet(_) | SpamConfigField::DstMacOctet(_) => {
                        if self.spam_input_buffer.len() < 2 && ch.is_ascii_hexdigit() {
                            self.spam_input_buffer.push(ch.to_ascii_uppercase());
                            self.current_field_has_error = false;
                        }
                    }
                    SpamConfigField::Reps | SpamConfigField::PauseMs => {
                        if self.spam_input_buffer.len() < 7 && ch.is_ascii_digit() {
                            self.spam_input_buffer.push(ch);
                            self.current_field_has_error = false;
                        }
                    }
                },
                KeyCode::Backspace => {
                    self.spam_input_buffer.pop();
                    self.current_field_has_error = false;
                }
                KeyCode::Enter => {
                    self.apply_spam_input_buffer();
                    if !self.current_field_has_error {
                        self.current_editing_field = self.current_editing_field.next();
                        self.prepare_spam_input_buffer_for_current_field();
                    }
                }
                KeyCode::Tab => {
                    self.apply_spam_input_buffer();
                    self.current_editing_field = self.current_editing_field.next();
                    self.prepare_spam_input_buffer_for_current_field();
                }
                KeyCode::BackTab => {
                    self.apply_spam_input_buffer();
                    self.current_editing_field = self.current_editing_field.prev();
                    self.prepare_spam_input_buffer_for_current_field();
                }
                KeyCode::Esc => {
                    self.spam_input_buffer.clear();
                    self.is_editing_spam_config = false;
                    self.last_error = None;
                    self.current_field_has_error = false;
                    info!("Exited spam config editing mode.");
                }
                KeyCode::Up => self.apply_spam_value_change(true),    // Modify value directly
                KeyCode::Down => self.apply_spam_value_change(false), // Modify value directly
                KeyCode::Left | KeyCode::Right => {}
                _ => {}
            }
            return false;
        }

        let mut next_controller = Esp32Controller::default();
        let mut action_taken = false;

        match key_event.code {
            KeyCode::Char('m') | KeyCode::Char('M') => {
                self.previous_esp_config_for_revert = Some(self.esp_config.clone());
                action_taken = true;
                let new_ui_mode = match self.ui_mode {
                    UiMode::Csi => UiMode::Spam,
                    UiMode::Spam => UiMode::Csi,
                };
                self.ui_mode = new_ui_mode;
                self.esp_config.mode = match new_ui_mode {
                    UiMode::Csi => EspOperationMode::Receive,
                    UiMode::Spam => EspOperationMode::Transmit,
                };
                next_controller.device_config = self.esp_config.clone();
                // When switching modes, ensure continuous spam is off and acquisition is handled by controller logic.
                if new_ui_mode == UiMode::Spam {
                    // Switching to Spam
                    next_controller.control_acquisition = false; // Explicitly pause acquisition
                    // If continuous spam was on, turn it off visually and command-wise
                    if self.is_continuous_spam_active {
                        self.previous_is_continuous_spam_active_for_revert = Some(self.is_continuous_spam_active);
                        self.is_continuous_spam_active = false;
                        next_controller.control_wifi_transmit = false; // Command to turn off continuous spam
                    }
                } else {
                    // Switching to CSI
                    next_controller.control_acquisition = true; // Explicitly resume acquisition
                    next_controller.control_wifi_transmit = false; // Ensure general transmit is off
                    self.is_continuous_spam_active = false; // Visually turn off
                }
            }
            KeyCode::Char('e') | KeyCode::Char('E') => {
                if self.ui_mode == UiMode::Spam {
                    self.is_editing_spam_config = true;
                    self.current_editing_field = SpamConfigField::SrcMacOctet(0);
                    self.prepare_spam_input_buffer_for_current_field();
                    self.last_error = None;
                    info!("Entered spam config editing. Tab/Enter/Arrows to navigate/modify. Esc to exit.");
                } else {
                    let _ = update_tx.try_send(AppUpdate::Error("Edit ('e') for Spam mode only. Switch mode with 'm'.".to_string()));
                }
            }
            KeyCode::Char('s') | KeyCode::Char('S') => {
                if self.ui_mode == UiMode::Spam {
                    if self.esp_config.mode != EspOperationMode::Transmit {
                        let _ = update_tx
                            .send(AppUpdate::Error("Burst spam ('s') requires ESP Transmit mode. Use 'm'.".to_string()))
                            .await;
                    } else {
                        action_taken = true;
                        next_controller.transmit_custom_frame = Some(CustomFrameParams {
                            src_mac: self.spam_settings.src_mac,
                            dst_mac: self.spam_settings.dst_mac,
                            n_reps: self.spam_settings.n_reps,
                            pause_ms: self.spam_settings.pause_ms,
                        });
                        info!("Requesting to send custom frame burst.");
                    }
                } else {
                    let _ = update_tx.try_send(AppUpdate::Error("Send burst spam ('s') for Spam mode only. Use 'm'.".to_string()));
                }
            }
            KeyCode::Char('t') | KeyCode::Char('T') => {
                if self.ui_mode == UiMode::Spam {
                    if self.esp_config.mode != EspOperationMode::Transmit {
                        let _ = update_tx.try_send(AppUpdate::Error("Continuous spam ('t') requires ESP Transmit mode. Use 'm'.".to_string()));
                    } else {
                        self.previous_is_continuous_spam_active_for_revert = Some(self.is_continuous_spam_active);
                        action_taken = true;
                        if self.is_continuous_spam_active {
                            next_controller.control_wifi_transmit = false;
                            self.is_continuous_spam_active = false;
                            info!("Requesting to PAUSE continuous WiFi transmit.");
                        } else {
                            next_controller.control_wifi_transmit = true;
                            self.is_continuous_spam_active = true;
                            info!("Requesting to RESUME continuous WiFi transmit.");
                        }
                    }
                } else {
                    let _ = update_tx.try_send(AppUpdate::Error("Toggle continuous spam ('t') for Spam mode only. Use 'm'.".to_string()));
                }
            }
            KeyCode::Char('c') | KeyCode::Char('C') => {
                self.previous_esp_config_for_revert = Some(self.esp_config.clone());
                action_taken = true;
                self.esp_config.channel = (self.esp_config.channel % 11) + 1; // Cycle 1-11
                next_controller.device_config.channel = self.esp_config.channel;
            }
            KeyCode::Char('b') | KeyCode::Char('B') => {
                self.previous_esp_config_for_revert = Some(self.esp_config.clone());
                action_taken = true;
                match self.esp_config.bandwidth {
                    EspBandwidth::Twenty => {
                        self.esp_config.bandwidth = EspBandwidth::Forty;
                        self.esp_config.secondary_channel = EspSecondaryChannel::Above;
                    }
                    EspBandwidth::Forty => {
                        self.esp_config.bandwidth = EspBandwidth::Twenty;
                        self.esp_config.secondary_channel = EspSecondaryChannel::None;
                    }
                }
                next_controller.device_config = self.esp_config.clone();
            }
            KeyCode::Char('l') | KeyCode::Char('L') => {
                self.previous_esp_config_for_revert = Some(self.esp_config.clone());
                action_taken = true;
                self.esp_config.csi_type = match self.esp_config.csi_type {
                    EspCsiType::HighThroughputLTF => EspCsiType::LegacyLTF,
                    EspCsiType::LegacyLTF => EspCsiType::HighThroughputLTF,
                };

                next_controller.device_config = self.esp_config.clone();
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                self.last_error = None;
            }
            KeyCode::Char('q') | KeyCode::Char('Q') => {
                info!("'q' pressed, initiating shutdown.");

                next_controller.control_acquisition = false;
                info!("Shutdown: Requesting to PAUSE CSI acquisition.");

                if self.esp_config.mode == EspOperationMode::Transmit {
                    next_controller.control_wifi_transmit = false;
                    info!("Shutdown: Requesting to PAUSE WiFi transmit task (was in Transmit mode).");
                } else {
                    info!("Shutdown: Skipping PauseWifiTransmit command (was in Receive mode, task likely not active).");
                }
                action_taken = true;
                self.should_quit = true;
            }
            KeyCode::Up => {
                self.csi_data.clear();
                info!("CSI data buffer cleared by user.");
            }
            KeyCode::Down => {
                action_taken = true;
                next_controller.synchronize_time = true;
            }
            _ => {}
        }

        if action_taken {
            let params_to_send = next_controller.clone(); // Clone params for analysis if send fails & for sending
            if command_tx.try_send(params_to_send).is_err() {
                let _ = update_tx.try_send(AppUpdate::Error("Cmd Send Fail: Chan full. UI reverted.".to_string()));

                // Revert optimistic changes based on the params we *tried* to send
                if let Some(old_config) = self.previous_esp_config_for_revert.take() {
                    self.esp_config = old_config;
                    info!("ESP config reverted due to command send failure (channel full).");
                }
                // This specifically handles the 't' key's optimistic update of is_continuous_spam_active
                if next_controller.control_wifi_transmit {
                    if let Some(old_spam_active) = self.previous_is_continuous_spam_active_for_revert.take() {
                        self.is_continuous_spam_active = old_spam_active;
                        info!("Continuous spam state reverted due to command send failure (channel full).");
                    }
                }
            }
        }
        false
    }
}
