use std::ascii::Char;
use std::collections::VecDeque;

use async_trait::async_trait;
use crossterm::event::{KeyCode, KeyEvent};
use lib::csi_types::CsiData;
use lib::sources::controllers::esp32_controller::{
    Bandwidth as EspBandwidth, CsiType as EspCsiType, CustomFrameParams, Esp32Controller, Esp32DeviceConfig, OperationMode as EspOperationMode,
    SecondaryChannel as EspSecondaryChannel,
};
use lib::sources::esp32::Esp32SourceConfig;
use lib::tui::Tui;
use lib::tui::logs::LogEntry;
use log::{error, info, warn};
use ratatui::Frame;
use tokio::sync::mpsc::{self, Receiver, Sender};

use super::tui::ui;
use super::{CSI_DATA_BUFFER_CAPACITY, LOG_BUFFER_CAPACITY};


#[derive(Debug)]
pub enum FocusedPanel {
  Main,
  SpamConfig,
}

#[derive(Debug, PartialEq)]
pub enum ToolMode {
    Listen,
    Spam,
}

#[derive(Debug)]
pub enum SpamConfigUpdate {
    Delete,
    Add(char),
    Enter,
    Tab,
    BackTab,
    Escape,
    ChangeValue(i32),
}

#[derive(Debug)]
pub enum EspUpdate {
    SpamConfig(SpamConfigUpdate),
    Log(LogEntry),
    Error(String),
    CsiData(CsiData),
    ClearError,
    ModeChange,
    EditSpamConfig,
    TriggerBurstSpam,
    ToggleContinuousSpam,
    IncrementChannel,
    ToggleSendMode,
    ChangeBandwidth,
    ChangeCsiMode,
    Exit,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum FocussedInput {
    SrcMac(usize),
    DstMac(usize),
    Reps,
    PauseMs,
    None
}


#[derive(Debug)]
pub struct DeviceState {
    spamming: bool,
    listening: bool,
}

#[derive(Debug)]
pub struct TuiState {
  pub connection_status: String,
  pub current_editing_field: FocussedInput,
  pub is_editing_spam_config: bool,
  pub is_continuous_spam_active: bool,
  pub spam_input_buffer: String,
  pub current_field_has_error: bool,
  // Fields to store the state *before* an optimistic update due to a command
  pub previous_esp_config_for_revert: Option<Esp32DeviceConfig>,
  pub previous_is_continuous_spam_active_for_revert: Option<bool>,
  
  
  // New
    should_quit: bool,
    focused_panel: FocusedPanel,
    focused_input: FocussedInput,
    logs: VecDeque<LogEntry>,
    esp_config: Esp32DeviceConfig,
    tool_mode: ToolMode,
    device_state: DeviceState,
    last_error_message: Option<String>,
    csi_data: VecDeque<CsiData>,
    spam_settings: SpamSettings,
  }

#[async_trait]
impl Tui<EspUpdate, Esp32Controller> for TuiState {
    // Draws the UI based on the state of the TUI, should not change any state by itself
    fn draw_ui(&self, f: &mut Frame) {
        ui(f, self);
    }

    // Handles a single keyboard event and produces and Update, should not change any state
    fn handle_keyboard_event(&self, key_event: KeyEvent) -> Option<EspUpdate> {
        let key_code = key_event.code;
        match self.focused_panel {
            FocusedPanel::SpamConfig => {
                match key_code {
                    KeyCode::Backspace => Some(EspUpdate::SpamConfig(SpamConfigUpdate::Delete)),
                    KeyCode::Enter => Some(EspUpdate::SpamConfig(SpamConfigUpdate::Enter)),
                    KeyCode::Tab => Some(EspUpdate::SpamConfig(SpamConfigUpdate::Tab)),
                    KeyCode::BackTab => Some(EspUpdate::SpamConfig(SpamConfigUpdate::BackTab)),
                    KeyCode::Esc => Some(EspUpdate::SpamConfig(SpamConfigUpdate::Escape)),
                    KeyCode::Up => Some(EspUpdate::SpamConfig(SpamConfigUpdate::ChangeValue(1))),
                    KeyCode::Down => Some(EspUpdate::SpamConfig(SpamConfigUpdate::ChangeValue(-1))), 
                    KeyCode::Char(chr) => Some(EspUpdate::SpamConfig(SpamConfigUpdate::Add(chr))),
                    _ => None,
                }
            }

            FocusedPanel::Main => match key_code {
                KeyCode::Char('m') | KeyCode::Char('M') => Some(EspUpdate::ModeChange),
                KeyCode::Char('e') | KeyCode::Char('E') => Some(EspUpdate::EditSpamConfig),
                KeyCode::Char('s') | KeyCode::Char('S') => Some(EspUpdate::TriggerBurstSpam),
                KeyCode::Char('t') | KeyCode::Char('T') => Some(EspUpdate::ToggleContinuousSpam),
                KeyCode::Char('c') | KeyCode::Char('C') => Some(EspUpdate::IncrementChannel),
                KeyCode::Char('l') | KeyCode::Char('L') => Some(EspUpdate::ChangeCsiMode),
                KeyCode::Char('b') | KeyCode::Char('B') => Some(EspUpdate::ChangeBandwidth),
                KeyCode::Char('r') | KeyCode::Char('R') => Some(EspUpdate::ClearError),
                KeyCode::Char('q') | KeyCode::Char('Q') => Some(EspUpdate::Exit),
                _ => None,
            },
        }
    }

    // Handles incoming Updates produced from any source, this is the only place where state should change
    async fn handle_update(&mut self, update: EspUpdate, command_send: &Sender<Esp32Controller>, update_recv: &mut Receiver<EspUpdate>) {
        match update {
            EspUpdate::SpamConfig(spam_config_update) => {
              match spam_config_update {
                SpamConfigUpdate::Add(chr) => {
                  let mac_field = match self.focused_input {
                    FocussedInput::SrcMac(_) => {
                      &mut self.spam_settings.src_mac
  
                    },
                    FocussedInput::DstMac(_) => {
                      &mut self.spam_settings.dst_mac
                    }     
                    _ => return     
                  };
                  
                  if mac_field.len() < 12 && chr.is_ascii_hexdigit() {
                    mac_field.push(chr.to_ascii_uppercase());
                  }                  
                },
                SpamConfigUpdate::Delete => todo!(),
                
                
                SpamConfigUpdate::Enter => {
                  self.apply_spam_input_buffer();
                  self.focused_input = FocussedInput::None
                },
                SpamConfigUpdate::Tab => {
                  self.apply_spam_input_buffer();
                  self.focused_input.next();
                },
                SpamConfigUpdate::BackTab => {
                  self.apply_spam_input_buffer();
                  self.focused_input.prev();
                },                
                SpamConfigUpdate::Escape => match self.focused_input {
                  FocussedInput::None => match self.focused_panel {
                    FocusedPanel::SpamConfig => self.focused_panel = FocusedPanel::Main,
                    _ => {}
                  }
                  _ => self.focused_input = FocussedInput::None
                },
                SpamConfigUpdate::ChangeValue(value) => 
                  match self.focused_input {
                    FocussedInput::PauseMs => {
                      self.spam_settings.pause_ms += value
                    },     
                    FocussedInput::Reps => {
                      self.spam_settings.n_reps += value
                    },                                         
                    _ => {}                                    
                  }
              }
            },
            EspUpdate::Log(log_entry) => {
                if self.logs.len() >= LOG_BUFFER_CAPACITY {
                    self.logs.pop_front();
                }
                self.logs.push_back(log_entry);
            }
            EspUpdate::Error(message) => self.last_error_message = Some(message),
            EspUpdate::ClearError => self.last_error_message = None,
            EspUpdate::CsiData(csi) => {
                if self.csi_data.len() >= CSI_DATA_BUFFER_CAPACITY {
                    self.csi_data.pop_front();
                }
                self.csi_data.push_back(csi);
            }
            EspUpdate::ChangeBandwidth => match self.esp_config.bandwidth {
                EspBandwidth::Twenty => {
                    self.esp_config.bandwidth = EspBandwidth::Forty;
                    self.esp_config.secondary_channel = EspSecondaryChannel::Above;
                }
                EspBandwidth::Forty => {
                    self.esp_config.bandwidth = EspBandwidth::Twenty;
                    self.esp_config.secondary_channel = EspSecondaryChannel::None;
                }
            },
            EspUpdate::ChangeCsiMode => {
                self.esp_config.csi_type = match self.esp_config.csi_type {
                    EspCsiType::HighThroughputLTF => EspCsiType::LegacyLTF,
                    EspCsiType::LegacyLTF => EspCsiType::HighThroughputLTF,
                };
            }
            EspUpdate::IncrementChannel => {
                self.esp_config.channel = (self.esp_config.channel % 11) + 1;
            }
            EspUpdate::ModeChange => {
                self.tool_mode = match self.tool_mode {
                    ToolMode::Listen => {
                        self.esp_config.mode = EspOperationMode::Receive;
                        self.device_state.spamming = false;
                        ToolMode::Spam
                    }
                    ToolMode::Spam => {
                        self.esp_config.mode = EspOperationMode::Transmit;
                        self.device_state.listening = false;
                        ToolMode::Listen
                    }
                };
            }
            EspUpdate::TriggerBurstSpam => {
                if self.tool_mode == ToolMode::Spam {
                } else {
                    error!("Send burst spam ('s') for Spam mode only. Switch mode with 'm'.");
                }
            }
            EspUpdate::ToggleContinuousSpam => {
                if self.tool_mode == ToolMode::Spam {
                } else {
                    error!("Toggle continuous spam ('t') for Spam mode only. Switch mode with 'm'.");
                }
            }
            EspUpdate::ToggleSendMode => todo!(),
            EspUpdate::EditSpamConfig => {
                if self.tool_mode == ToolMode::Spam {
                    self.focused_panel = FocusedPanel::SpamConfig
                } else {
                    error!("Edit ('e') for Spam mode only. Switch mode with 'm'.");
                }
            }
            EspUpdate::Exit => {
                if self.esp_config.mode == EspOperationMode::Transmit {
                    // next_controller.control_wifi_transmit = false;
                    info!("Shutdown: Requesting to PAUSE WiFi transmit task (was in Transmit mode).");
                } else {
                    info!("Shutdown: Skipping PauseWifiTransmit command (was in Receive mode, task likely not active).");
                }
                self.should_quit = true;
            }
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
    pub fn new() -> Self {
        Self {
            ui_mode: UiMode::Csi,
            esp_config: Esp32DeviceConfig::default(),
            csi_data: VecDeque::with_capacity(CSI_DATA_BUFFER_CAPACITY),
            connection_status: "INITIALIZING...".into(),
            last_error_message: None,
            log_messages: VecDeque::with_capacity(LOG_BUFFER_CAPACITY),
            spam_settings: SpamSettings::default(),
            is_editing_spam_config: false,
            current_editing_field: FocussedInput::SrcMac(0),
            spam_input_buffer: String::new(),
            current_field_has_error: false,
            is_continuous_spam_active: false,
            should_quit: false,
            previous_esp_config_for_revert: None,
            previous_is_continuous_spam_active_for_revert: None,
        }
    }

    // Handles all incoming keyboard events
    pub async fn handle_keyboard_event(
        &mut self,
        key_event: KeyEvent,
        command_tx: &mpsc::Sender<Esp32Controller>,
        update_tx: &mpsc::Sender<EspUpdate>,
    ) -> bool {
        if self.is_editing_spam_config {
            match key_event.code {
                KeyCode::Char(ch) => match self.current_editing_field {
                    FocussedInput::SrcMac(_) | FocussedInput::DstMac(_) => {
                        if self.spam_input_buffer.len() < 2 && ch.is_ascii_hexdigit() {
                            self.spam_input_buffer.push(ch.to_ascii_uppercase());
                            self.current_field_has_error = false;
                        }
                    }
                    FocussedInput::Reps | FocussedInput::PauseMs => {
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
                    self.last_error_message = None;
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
                    self.current_editing_field = FocussedInput::SrcMac(0);
                    self.prepare_spam_input_buffer_for_current_field();
                    self.last_error_message = None;
                    info!("Entered spam config editing. Tab/Enter/Arrows to navigate/modify. Esc to exit.");
                } else {
                    let _ = update_tx.try_send(EspUpdate::Error("Edit ('e') for Spam mode only. Switch mode with 'm'.".to_string()));
                }
            }
            KeyCode::Char('s') | KeyCode::Char('S') => {
                if self.ui_mode == UiMode::Spam {
                    if self.esp_config.mode != EspOperationMode::Transmit {
                        let _ = update_tx
                            .send(EspUpdate::Error("Burst spam ('s') requires ESP Transmit mode. Use 'm'.".to_string()))
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
                    let _ = update_tx.try_send(EspUpdate::Error("Send burst spam ('s') for Spam mode only. Use 'm'.".to_string()));
                }
            }
            KeyCode::Char('t') | KeyCode::Char('T') => {
                if self.ui_mode == UiMode::Spam {
                    if self.esp_config.mode != EspOperationMode::Transmit {
                        let _ = update_tx.try_send(EspUpdate::Error("Continuous spam ('t') requires ESP Transmit mode. Use 'm'.".to_string()));
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
                    let _ = update_tx.try_send(EspUpdate::Error("Toggle continuous spam ('t') for Spam mode only. Use 'm'.".to_string()));
                }
            }
            _ => {}
        }
    }

    pub fn apply_spam_value_change(&mut self, increment: bool) {
        if !self.is_editing_spam_config {
            return;
        }
        let val_change_u8: u8 = if increment { 1 } else { 255 }; // u8::MAX for wrapping_sub(1)
        let val_change_i32: i32 = if increment { 1 } else { -1 };

        match self.current_editing_field {
            FocussedInput::SrcMac(i) => {
                self.spam_settings.src_mac[i] = self.spam_settings.src_mac[i].wrapping_add(val_change_u8);
            }
            FocussedInput::DstMac(i) => {
                self.spam_settings.dst_mac[i] = self.spam_settings.dst_mac[i].wrapping_add(val_change_u8);
            }
            FocussedInput::Reps => {
                self.spam_settings.n_reps = (self.spam_settings.n_reps + val_change_i32).max(0);
            }
            FocussedInput::PauseMs => {
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
                FocussedInput::SrcMac(_) | FocussedInput::DstMac(_) => {
                    self.last_error_message = Some("MAC octet cannot be empty. Reverted.".to_string());
                    self.current_field_has_error = true;
                    return;
                }
                FocussedInput::Reps => self.spam_settings.n_reps = 0,
                FocussedInput::PauseMs => self.spam_settings.pause_ms = 0,
            }
            self.last_error_message = None;
            return;
        }

        let parse_result: Result<(), String> = match self.current_editing_field {
            FocussedInput::SrcMac(i) => u8::from_str_radix(buffer_content, 16)
                .map_err(|e| format!("Invalid SrcMAC Octet value: '{buffer_content}' ({e})"))
                .map(|val| self.spam_settings.src_mac[i] = val),
            FocussedInput::DstMac(i) => u8::from_str_radix(buffer_content, 16)
                .map_err(|e| format!("Invalid DstMAC Octet value: '{buffer_content}' ({e})"))
                .map(|val| self.spam_settings.dst_mac[i] = val),
            FocussedInput::Reps => buffer_content
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
            FocussedInput::PauseMs => buffer_content
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
            self.last_error_message = Some(e);
            self.current_field_has_error = true;
        } else {
            self.last_error_message = None;
        }
    }

    fn prepare_spam_input_buffer_for_current_field(&mut self) {
        self.spam_input_buffer.clear();
        self.current_field_has_error = false;
        match self.current_editing_field {
            FocussedInput::SrcMac(i) => {
                self.spam_input_buffer = format!("{:02X}", self.spam_settings.src_mac[i]);
            }
            FocussedInput::DstMac(i) => {
                self.spam_input_buffer = format!("{:02X}", self.spam_settings.dst_mac[i]);
            }
            FocussedInput::Reps => {
                self.spam_input_buffer = self.spam_settings.n_reps.to_string();
            }
            FocussedInput::PauseMs => {
                self.spam_input_buffer = self.spam_settings.pause_ms.to_string();
            }
        }
    }
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

impl FocussedInput {
    fn next(self) -> Self {
        match self {
            FocussedInput::SrcMac(5) => FocussedInput::DstMac(0),
            FocussedInput::SrcMac(i) => FocussedInput::SrcMac(i + 1),
            FocussedInput::DstMac(5) => FocussedInput::Reps,
            FocussedInput::DstMac(i) => FocussedInput::DstMac(i + 1),
            FocussedInput::Reps => FocussedInput::PauseMs,
            FocussedInput::PauseMs => FocussedInput::SrcMac(0),
        }
    }

    fn prev(self) -> Self {
        match self {
            FocussedInput::SrcMac(0) => FocussedInput::PauseMs,
            FocussedInput::SrcMac(i) => FocussedInput::SrcMac(i - 1),
            FocussedInput::DstMac(0) => FocussedInput::SrcMac(5),
            FocussedInput::DstMac(i) => FocussedInput::DstMac(i - 1),
            FocussedInput::Reps => FocussedInput::DstMac(5),
            FocussedInput::PauseMs => FocussedInput::Reps,
        }
    }
}
