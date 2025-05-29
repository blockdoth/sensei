use std::cmp::{max, min};
use std::collections::VecDeque;
use std::vec;

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
use ratatui::prelude::Color;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use tokio::sync::mpsc::{self, Receiver, Sender};

use super::spam_settings::SpamSettings;
use super::tui::ui;
use super::{CSI_DATA_BUFFER_CAPACITY, LOG_BUFFER_CAPACITY};

#[derive(Debug, PartialEq)]
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
    Edit(char),
    Enter,
    TabRight,
    TabLeft,
    CursorLeft,
    CursorRight,
    CursorUp,
    CursorDown,
    Escape,
    Delete,
}

#[derive(Debug)]
pub enum EspUpdate {
    SpamConfig(SpamConfigUpdate),
    Log(LogEntry),
    Status(String),
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
    ControllerUpdateSuccess,
    EspDisconnected,
    Exit,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum FocussedInput {
    SrcMac(usize),
    DstMac(usize),
    Reps(usize),
    PauseMs(usize),
    None,
}

#[derive(Debug)]
pub struct DeviceState {
    pub spamming: bool,
    pub listening: bool,
}

#[derive(Debug)]
pub struct TuiState {
    pub connection_status: String,
    pub should_quit: bool,
    pub focused_panel: FocusedPanel,
    pub focused_input: FocussedInput,
    pub logs: VecDeque<LogEntry>,
    pub esp_config: Esp32DeviceConfig,
    pub tool_mode: ToolMode,
    pub device_state: DeviceState,
    pub last_error_message: Option<String>,
    pub csi_data: VecDeque<CsiData>,
    pub spam_settings: SpamSettings,
    pub last_action_caused_error: bool,
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
                KeyCode::Char('r') | KeyCode::Char('R') => Some(EspUpdate::ClearError),
                KeyCode::Char('q') | KeyCode::Char('Q') => Some(EspUpdate::Exit),
                _ => None,
            },
        }
    }

    // Handles incoming Updates produced from any source, this is the only place where state should change
    async fn handle_update(&mut self, update: EspUpdate, command_send: &Sender<Esp32Controller>, update_recv: &mut Receiver<EspUpdate>) {
        match update {
            EspUpdate::SpamConfig(spam_config_update) => match spam_config_update {
                SpamConfigUpdate::Edit(chr) => match self.focused_input {
                    FocussedInput::SrcMac(string_idx) if chr.is_ascii_hexdigit() => {
                        // string_idx / 2 = byte idx
                        self.spam_settings.src_mac[string_idx / 2] =
                            SpamSettings::update_mac(self.spam_settings.src_mac[string_idx / 2], chr, string_idx);
                        self.focused_input = self.focused_input.cursor_right();
                    }
                    FocussedInput::DstMac(string_idx) if chr.is_ascii_hexdigit() => {
                        self.spam_settings.dst_mac[string_idx / 2] =
                            SpamSettings::update_mac(self.spam_settings.dst_mac[string_idx / 2], chr, string_idx);
                        self.focused_input = self.focused_input.cursor_right();
                    }
                    FocussedInput::PauseMs(string_idx) if chr.is_numeric() => {
                        self.spam_settings.pause_ms = SpamSettings::modify_digit_at_index(self.spam_settings.pause_ms, string_idx, Some(chr));
                        self.focused_input = self.focused_input.cursor_right();
                    }
                    FocussedInput::Reps(string_idx) if chr.is_numeric() => {
                        self.spam_settings.n_reps = SpamSettings::modify_digit_at_index(self.spam_settings.n_reps, string_idx, Some(chr));
                        self.focused_input = self.focused_input.cursor_right();
                    }
                    _ => (),
                },
                SpamConfigUpdate::Delete => {
                    match self.focused_input {
                        FocussedInput::SrcMac(string_idx) => {
                            self.spam_settings.src_mac[string_idx / 2] = 0;
                        }
                        FocussedInput::DstMac(string_idx) => {
                            self.spam_settings.dst_mac[string_idx / 2] = 0;
                        }
                        FocussedInput::PauseMs(string_idx) => {
                            self.spam_settings.pause_ms = SpamSettings::modify_digit_at_index(self.spam_settings.pause_ms, string_idx, None)
                        }
                        FocussedInput::Reps(string_idx) => {
                            self.spam_settings.n_reps = SpamSettings::modify_digit_at_index(self.spam_settings.n_reps, string_idx, None)
                        }
                        _ => (),
                    }

                    self.apply_spam_input_buffer();
                }
                SpamConfigUpdate::Enter => {
                    self.apply_spam_input_buffer();
                    self.focused_input = FocussedInput::None
                }
                SpamConfigUpdate::TabRight => self.focused_input = self.focused_input.tab_right(),
                SpamConfigUpdate::TabLeft => self.focused_input = self.focused_input.tab_left(),
                SpamConfigUpdate::CursorRight => match self.focused_input {
                    FocussedInput::PauseMs(i) if i >= self.spam_settings.pause_ms.to_string().len() => {}
                    FocussedInput::Reps(i) if i >= self.spam_settings.n_reps.to_string().len() => {}
                    _ => self.focused_input = self.focused_input.cursor_right(),
                },
                SpamConfigUpdate::CursorLeft => self.focused_input = self.focused_input.cursor_left(),
                SpamConfigUpdate::CursorUp => match self.focused_input {
                    FocussedInput::Reps(i) if i >= self.spam_settings.pause_ms.to_string().len() => {
                        self.focused_input = FocussedInput::PauseMs(self.spam_settings.n_reps.to_string().len())
                    }
                    _ => self.focused_input = self.focused_input.cursor_up(),
                },

                SpamConfigUpdate::CursorDown => match self.focused_input {
                    FocussedInput::Reps(i) if i >= self.spam_settings.n_reps.to_string().len() => {
                        self.focused_input = FocussedInput::PauseMs(self.spam_settings.pause_ms.to_string().len())
                    }
                    _ => self.focused_input = self.focused_input.cursor_down(),
                },
                SpamConfigUpdate::Escape => match self.focused_input {
                    FocussedInput::None => match self.focused_panel {
                        FocusedPanel::SpamConfig => {
                            self.focused_panel = FocusedPanel::Main;
                            self.apply_spam_input_buffer();
                        }
                        _ => (),
                    },
                    _ => self.focused_input = FocussedInput::None,
                },
            },
            EspUpdate::Log(log_entry) => {
                if self.logs.len() >= LOG_BUFFER_CAPACITY {
                    self.logs.pop_front();
                }
                self.logs.push_back(log_entry);
            }
            EspUpdate::Error(message) => {
                self.last_error_message = Some(message);
                self.last_action_caused_error = true;
            }
            EspUpdate::Status(message) => self.connection_status = message,
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
                self.last_error_message = None;
            }
            EspUpdate::TriggerBurstSpam => {
                if self.tool_mode == ToolMode::Spam {
                    info!("Requesting to send custom frame burst.");
                    todo!()
                    // self.controller.transmit_custom_frame = Some(CustomFrameParams {
                    //   src_mac: self.spam_settings.src_mac,
                    //   dst_mac: self.spam_settings.dst_mac,
                    //   n_reps: self.spam_settings.n_reps,
                    //   pause_ms: self.spam_settings.pause_ms,
                    // });
                } else {
                    self.last_error_message = Some("Send burst spam ('s') for Spam mode only. Switch mode with 'm'.".into());
                }
            }
            EspUpdate::ToggleContinuousSpam if self.tool_mode == ToolMode::Spam => {
                todo!()
            }
            EspUpdate::ToggleSendMode => todo!(),
            EspUpdate::EditSpamConfig if self.tool_mode == ToolMode::Spam => {
                self.focused_panel = FocusedPanel::SpamConfig;
                self.focused_input = FocussedInput::SrcMac(0);
                // self.last_error_message = None;
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
            EspUpdate::ControllerUpdateSuccess => {
                self.connection_status = "Controller updated succesfully".to_string();
            }
            EspUpdate::EspDisconnected => {
                self.connection_status = "ESP disconnected".to_string();
            }
            _ => (),
        }
    }

    // Gets called each tick of the main loop, useful for updating graphs and live views, should only make small changes to state

    async fn on_tick(&mut self) {
        ()
    }

    // Whether the tui should quit
    fn should_quit(&self) -> bool {
        self.should_quit
    }
}

impl TuiState {
    pub fn new() -> Self {
        Self {
            last_error_message: None,
            connection_status: "INITIALIZING...".into(),
            should_quit: false,
            focused_panel: FocusedPanel::Main,
            focused_input: FocussedInput::None,
            logs: VecDeque::with_capacity(LOG_BUFFER_CAPACITY),
            esp_config: Esp32DeviceConfig::default(),
            tool_mode: ToolMode::Listen,
            device_state: DeviceState {
                spamming: false,
                listening: false,
            },
            csi_data: VecDeque::with_capacity(CSI_DATA_BUFFER_CAPACITY),
            spam_settings: SpamSettings::default(),
            last_action_caused_error: false,
        }
    }

    // Handles all incoming keyboard events
    // pub async fn handle_keyboard_event(
    //     &mut self,
    //     key_event: KeyEvent,
    //     command_tx: &mpsc::Sender<Esp32Controller>,
    //     update_tx: &mpsc::Sender<EspUpdate>,
    // ) -> bool {

    //     KeyCode::Char(ch) => match self.current_editing_field {
    //         FocussedInput::SrcMac(_) | FocussedInput::DstMac(_) => {
    //             if self.spam_input_buffer.len() < 2 && ch.is_ascii_hexdigit() {
    //                 self.spam_input_buffer.push(ch.to_ascii_uppercase());
    //                 self.current_field_has_error = false;
    //             }
    //         }
    //         FocussedInput::Reps | FocussedInput::PauseMs => {
    //             if self.spam_input_buffer.len() < 7 && ch.is_ascii_digit() {
    //                 self.spam_input_buffer.push(ch);
    //                 self.current_field_has_error = false;
    //             }
    //         }
    //     },
    // }

    // let mut next_controller = Esp32Controller::default();

    // match key_event.code {
    // KeyCode::Char('m') | KeyCode::Char('M') => {
    //     // When switching modes, ensure continuous spam is off and acquisition is handled by controller logic.
    //     if new_ui_mode == UiMode::Spam {
    //         // Switching to Spam
    //         next_controller.control_acquisition = false; // Explicitly pause acquisition
    //         // If continuous spam was on, turn it off visually and command-wise
    //         if self.is_continuous_spam_active {
    //             self.previous_is_continuous_spam_active_for_revert = Some(self.is_continuous_spam_active);
    //             self.is_continuous_spam_active = false;
    //             next_controller.control_wifi_transmit = false; // Command to turn off continuous spam
    //         }
    //     } else {
    //         // Switching to CSI
    //         next_controller.control_acquisition = true; // Explicitly resume acquisition
    //         next_controller.control_wifi_transmit = false; // Ensure general transmit is off
    //         self.is_continuous_spam_active = false; // Visually turn off
    //     }
    // }

    // KeyCode::Char('s') | KeyCode::Char('S') => {
    //     if self.ui_mode == UiMode::Spam {
    //         if self.esp_config.mode != EspOperationMode::Transmit {
    //             let _ = update_tx
    //                 .send(EspUpdate::Error("Burst spam ('s') requires ESP Transmit mode. Use 'm'.".to_string()))
    //                 .await;
    //         } else {
    //             action_taken = true;
    //             next_controller.transmit_custom_frame = Some(CustomFrameParams {
    //                 src_mac: self.spam_settings.src_mac,
    //                 dst_mac: self.spam_settings.dst_mac,
    //                 n_reps: self.spam_settings.n_reps,
    //                 pause_ms: self.spam_settings.pause_ms,
    //             });
    //             info!("Requesting to send custom frame burst.");
    //         }
    //     } else {
    //         let _ = update_tx.try_send(EspUpdate::Error("Send burst spam ('s') for Spam mode only. Use 'm'.".to_string()));
    //     }
    // }
    // KeyCode::Char('t') | KeyCode::Char('T') => {
    //     if self.ui_mode == UiMode::Spam {
    //         if self.esp_config.mode != EspOperationMode::Transmit {
    //             let _ = update_tx.try_send(EspUpdate::Error("Continuous spam ('t') requires ESP Transmit mode. Use 'm'.".to_string()));
    //         } else {
    //             self.previous_is_continuous_spam_active_for_revert = Some(self.is_continuous_spam_active);
    //             action_taken = true;
    //             if self.is_continuous_spam_active {
    //                 next_controller.control_wifi_transmit = false;
    //                 self.is_continuous_spam_active = false;
    //                 info!("Requesting to PAUSE continuous WiFi transmit.");
    //             } else {
    //                 next_controller.control_wifi_transmit = true;
    //                 self.is_continuous_spam_active = true;
    //                 info!("Requesting to RESUME continuous WiFi transmit.");
    //             }
    //         }
    //     } else {
    //         let _ = update_tx.try_send(EspUpdate::Error("Toggle continuous spam ('t') for Spam mode only. Use 'm'.".to_string()));
    //     }
    // }
    //     }
    // }

    // pub fn apply_spam_value_change(&mut self, increment: bool) {
    //     if !self.is_editing_spam_config {
    //         return;
    //     }
    //     let val_change_u8: u8 = if increment { 1 } else { 255 }; // u8::MAX for wrapping_sub(1)
    //     let val_change_i32: i32 = if increment { 1 } else { -1 };

    //     match self.current_editing_field {
    //         FocussedInput::SrcMac(i) => {
    //             self.spam_settings.src_mac[i] = self.spam_settings.src_mac[i].wrapping_add(val_change_u8);
    //         }
    //         FocussedInput::DstMac(i) => {
    //             self.spam_settings.dst_mac[i] = self.spam_settings.dst_mac[i].wrapping_add(val_change_u8);
    //         }
    //         FocussedInput::Reps => {
    //             self.spam_settings.n_reps = (self.spam_settings.n_reps + val_change_i32).max(0);
    //         }
    //         FocussedInput::PauseMs => {
    //             self.spam_settings.pause_ms = (self.spam_settings.pause_ms + val_change_i32).max(0);
    //         }
    //     }
    // }

    // // Parses and applies the spam input buffer
    fn apply_spam_input_buffer(&mut self) {
        // let buffer_content = self.spam_input_buffer.trim();
        // self.current_field_has_error = false;

        // if buffer_content.is_empty() {
        //     match self.focused_input {
        //         FocussedInput::SrcMac(_) | FocussedInput::DstMac(_) => {
        //             self.last_error_message = Some("MAC octet cannot be empty. Reverted.".to_string());
        //             self.current_field_has_error = true;
        //             return;
        //         }
        //         FocussedInput::Reps => self.spam_settings.n_reps = 0,
        //         FocussedInput::PauseMs => self.spam_settings.pause_ms = 0,
        //         _ => {}
        //     }
        //     self.last_error_message = None;
        //     return;
        //   }
    }
}

//     let parse_result: Result<(), String> = match self.current_editing_field {
//         FocussedInput::SrcMac(i) => u8::from_str_radix(buffer_content, 16)
//             .map_err(|e| format!("Invalid SrcMAC Octet value: '{buffer_content}' ({e})"))
//             .map(|val| self.spam_settings.src_mac[i] = val),
//         FocussedInput::DstMac(i) => u8::from_str_radix(buffer_content, 16)
//             .map_err(|e| format!("Invalid DstMAC Octet value: '{buffer_content}' ({e})"))
//             .map(|val| self.spam_settings.dst_mac[i] = val),
//         FocussedInput::Reps => buffer_content
//             .parse::<i32>()
//             .map_err(|e| format!("Invalid Reps value: '{buffer_content}' ({e})"))
//             .and_then(|val| {
//                 if val >= 0 {
//                     self.spam_settings.n_reps = val;
//                     Ok(())
//                 } else {
//                     Err("Reps must be non-negative".to_string())
//                 }
//             }),
//         FocussedInput::PauseMs => buffer_content
//             .parse::<i32>()
//             .map_err(|e| format!("Invalid PauseMs value: '{buffer_content}' ({e})"))
//             .and_then(|val| {
//                 if val >= 0 {
//                     self.spam_settings.pause_ms = val;
//                     Ok(())
//                 } else {
//                     Err("PauseMs must be non-negative".to_string())
//                 }
//             }),
//     };

//     if let Err(e) = parse_result {
//         self.last_error_message = Some(e);
//         self.current_field_has_error = true;
//     } else {
//         self.last_error_message = None;
//     }
// }
