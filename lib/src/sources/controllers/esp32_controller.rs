//! ESP32 Controller
//!
//! Defines parameters and logic for configuring an ESP32 device
//! through the `Esp32Source`.

use std::any::Any;

use async_trait::async_trait;
use log::{debug, warn};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ToConfig;
use crate::errors::{ControllerError, TaskError};
use crate::sources::DataSourceT;
use crate::sources::controllers::{Controller, ControllerParams};
use crate::sources::esp32::Esp32Source; // Adjusted path // Required for downcasting if using source.as_any_mut()
// Assume your concrete ESP32 source is located here. Adjust path as needed.

// --- ESP32 Specific Enums (Kept as they are well-defined) ---
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema)]
pub enum OperationMode {
    Receive = 0x00,
    Transmit = 0x01,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema)]
pub enum Bandwidth {
    Twenty = 0x00,
    Forty = 0x01,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema)]
pub enum SecondaryChannel {
    None = 0x00,
    Below = 0x01,
    Above = 0x02,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema)]
pub enum CsiType {
    LegacyLTF = 0x00,
    HighThroughputLTF = 0x01,
}

/// ESP32 Command Codes - Must match firmware `Command` enum
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Esp32Command {
    Nop = 0x00,
    SetChannel = 0x01,
    WhitelistAddMacPair = 0x02,
    WhitelistClear = 0x03,
    ListeningPause = 0x04,
    ListeningResume = 0x05,
    ApplyDeviceConfig = 0x06,
    SendingPause = 0x07,
    SendingResume = 0x08,
    SetCustomFrame = 0x09,
    SynchronizeTimeInit = 0x0A,
    SynchronizeTimeApply = 0x0B,
}

// --- Controller Parameter Structures (Kept as they are well-defined) ---
#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema)]
pub struct MacFilterPair {
    pub src_mac: [u8; 6],
    pub dst_mac: [u8; 6],
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, Default)]
pub struct CustomFrameParams {
    pub src_mac: [u8; 6],
    pub dst_mac: [u8; 6],
    pub n_reps: u32,
    pub pause_ms: u32,
}

impl CustomFrameParams {
    pub fn to_vec(&self) -> Vec<u8> {
        vec![
            // dst_mac
            self.dst_mac[0],
            self.dst_mac[1],
            self.dst_mac[2],
            self.dst_mac[3],
            self.dst_mac[4],
            self.dst_mac[5],
            // src_mac
            self.src_mac[0],
            self.src_mac[1],
            self.src_mac[2],
            self.src_mac[3],
            self.src_mac[4],
            self.src_mac[5],
            // n_reps as little-endian bytes
            self.n_reps.to_le_bytes()[0],
            self.n_reps.to_le_bytes()[1],
            self.n_reps.to_le_bytes()[2],
            self.n_reps.to_le_bytes()[3],
            // pause_ms as little-endian bytes
            self.pause_ms.to_le_bytes()[0],
            self.pause_ms.to_le_bytes()[1],
            self.pause_ms.to_le_bytes()[2],
            self.pause_ms.to_le_bytes()[3],
        ]
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, PartialEq, Default)]
pub enum EspMode {
    #[default]
    SendingPaused,
    SendingBurst,
    SendingContinuous,
    Listening,
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, PartialEq)]
pub struct Esp32DeviceConfig {
    pub channel: u8,
    pub mode: OperationMode,
    pub bandwidth: Bandwidth,
    pub secondary_channel: SecondaryChannel,
    pub csi_type: CsiType,
    pub manual_scale: u8,
}

impl Default for Esp32DeviceConfig {
    fn default() -> Self {
        Self {
            channel: 1,
            mode: OperationMode::Receive,
            bandwidth: Bandwidth::Twenty,
            secondary_channel: SecondaryChannel::None,
            csi_type: CsiType::HighThroughputLTF,
            manual_scale: 0,
        }
    }
}

impl Esp32DeviceConfig {
    fn to_vec(&self) -> Vec<u8> {
        vec![
            self.mode as u8,
            self.bandwidth as u8,
            self.secondary_channel as u8,
            self.csi_type as u8,
            self.manual_scale,
        ]
    }
}

/// Parameters for controlling an ESP32 device.
#[derive(Serialize, Deserialize, Debug, Clone, Default, JsonSchema)]
#[serde(default)]
pub struct Esp32ControllerParams {
    pub device_config: Esp32DeviceConfig,
    pub mac_filters: Vec<MacFilterPair>,
    pub mode: EspMode,
    pub synchronize_time: bool,
    pub transmit_custom_frame: Option<CustomFrameParams>,
}

#[typetag::serde(name = "ESP32Controller")]
#[async_trait]
impl Controller for Esp32ControllerParams {
    async fn apply(&self, source: &mut dyn DataSourceT) -> Result<(), ControllerError> {
        // Ensure your DataSourceT trait has `fn as_any_mut(&mut self) -> &mut dyn Any;`
        // and Esp32Source implements it.
        let mut esp_source = (source as &mut dyn Any)
            .downcast_mut::<Esp32Source>()
            .ok_or_else(|| ControllerError::InvalidDataSource("Esp32Controller requires an Esp32Source.".to_string()))?;

        // Pauses the relevant modes, be aware that double pausing could cause problems
        match self.mode {
            EspMode::Listening => {
                debug!("Pausing sending to prepare for listening");
                esp_source
                    .send_esp32_command(Esp32Command::SendingPause, None)
                    .await
                    .map_err(|e| ControllerError::CommandFailed {
                        command_name: format!("Failed to pause sending"),
                        details: e.to_string(),
                    })?;
            }
            EspMode::SendingContinuous | EspMode::SendingBurst => {
                debug!("Pausing listening to prepare for sending");
                esp_source
                    .send_esp32_command(Esp32Command::ListeningPause, None)
                    .await
                    .map_err(|e| ControllerError::CommandFailed {
                        command_name: format!("Failed to pause listening"),
                        details: e.to_string(),
                    })?;
            }
            _ => {}
        };

        debug!("Controller: Checking ESP32 device configuration: {:?}", self.device_config);
        let channel = &self.device_config.channel;
        if !(1..=11).contains(channel) {
            return Err(ControllerError::InvalidParams(format!(
                "Invalid WiFi channel: {channel}. Must be between 1 and 11.",
            )));
        }

        if self.device_config.bandwidth == Bandwidth::Forty && self.device_config.secondary_channel == SecondaryChannel::None {
            return Err(ControllerError::InvalidParams(
                "40MHz bandwidth requires a secondary channel (Above or Below) to be set.".to_string(),
            ));
        }
        if self.device_config.manual_scale > 3 && self.device_config.csi_type == CsiType::HighThroughputLTF {
            warn!(
                "Manual scale {} might be too high for HT-LTF, ESP32-S3 typically expects 0-3.",
                self.device_config.manual_scale
            );
        } else if self.device_config.manual_scale > 1 && self.device_config.csi_type == CsiType::LegacyLTF {
            warn!(
                "Manual scale {} might be too high for L-LTF, ESP32 typically expects 0-1.",
                self.device_config.manual_scale
            );
        };

        debug!("Controller: Applying ESP32 device configuration: {:?}", self.device_config);
        esp_source
            .send_esp32_command(Esp32Command::ApplyDeviceConfig, Some(self.device_config.to_vec()))
            .await
            .map_err(|e| ControllerError::CommandFailed {
                command_name: "ApplyDeviceConfig".to_string(),
                details: e.to_string(),
            })?;

        debug!("Controller: Clearing all MAC filters on ESP32.");
        esp_source
            .send_esp32_command(Esp32Command::WhitelistClear, None)
            .await
            .map_err(|e| ControllerError::CommandFailed {
                command_name: "WhitelistClear".to_string(),
                details: e.to_string(),
            })?;

        debug!("Controller: Adding {} MAC filter(s) to ESP32.", self.mac_filters.len());
        for filter_pair in self.mac_filters.clone() {
            let mut filter_data = Vec::with_capacity(12);
            filter_data.extend_from_slice(&filter_pair.src_mac);
            filter_data.extend_from_slice(&filter_pair.dst_mac);
            esp_source
                .send_esp32_command(Esp32Command::WhitelistAddMacPair, Some(filter_data))
                .await
                .map_err(|e| ControllerError::CommandFailed {
                    command_name: "WhitelistAddMacPair".to_string(),
                    details: e.to_string(),
                })?;
        }

        if self.synchronize_time {
            debug!("Controller: Initiating time synchronization with ESP32.");
            esp_source
                .send_esp32_command(Esp32Command::SynchronizeTimeInit, None)
                .await
                .map_err(|e| ControllerError::CommandFailed {
                    command_name: "SynchronizeTimeInit".to_string(),
                    details: e.to_string(),
                })?;

            let time_us = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| ControllerError::Execution(format!("System time error for sync: {e}")))?
                .as_micros() as u64;

            esp_source
                .send_esp32_command(Esp32Command::SynchronizeTimeApply, Some(time_us.to_le_bytes().to_vec()))
                .await
                .map_err(|e| ControllerError::CommandFailed {
                    command_name: "SynchronizeTimeApply".to_string(),
                    details: e.to_string(),
                })?;
            debug!("Controller: Time synchronization sequence sent.");
        }

        // Resumes the relevant modes
        match self.mode {
            EspMode::Listening => {
                debug!("Changed mode to [Listening]");
                esp_source
                    .send_esp32_command(Esp32Command::ListeningResume, None)
                    .await
                    .map_err(|e| ControllerError::CommandFailed {
                        command_name: format!("UnpauseAcquisition"),
                        details: e.to_string(),
                    })?;
            }
            EspMode::SendingContinuous => {
                if let Some(frame) = &self.transmit_custom_frame {
                    debug!("Changed mode to [Sending]");
                    esp_source
                        .send_esp32_command(Esp32Command::SendingResume, Some(frame.to_vec()))
                        .await
                        .map_err(|e| ControllerError::CommandFailed {
                            command_name: format!("ResumeWifiTransmit"),
                            details: e.to_string(),
                        })?;
                } else {
                    return Err(ControllerError::InvalidParams("No custom frame type for sending specifed".to_string()));
                }
            }
            EspMode::SendingBurst => {
                if let Some(frame) = &self.transmit_custom_frame {
                    debug!("Controller: Transmitting custom frames: {:?}", self.transmit_custom_frame);
                    esp_source
                        .send_esp32_command(Esp32Command::SetCustomFrame, Some(frame.to_vec()))
                        .await
                        .map_err(|e| ControllerError::CommandFailed {
                            command_name: "TransmitCustomFrame".to_string(),
                            details: e.to_string(),
                        })?;
                } else {
                    return Err(ControllerError::InvalidParams("No custom frame type for sending specifed".to_string()));
                }
            }
            _ => {}
        };

        debug!("Controller: Esp32Controller applied successfully.");
        Ok(())
    }
}

#[async_trait::async_trait]
impl ToConfig<ControllerParams> for Esp32ControllerParams {
    /// Converts the current `Esp32ControllerParams` instance into its configuration representation.
    ///
    /// This method implements the `ToConfig` trait for `Esp32ControllerParams`, allowing a runtime
    /// instance to be transformed into a `ControllerParams::Esp32` variant. This is useful for tasks
    /// like saving the controller configuration to disk or exporting it for reproducibility.
    ///
    /// # Returns
    /// - `Ok(ControllerParams::Esp32)` containing a cloned version of the controller parameters.
    /// - `Err(TaskError)` if an error occurs during conversion (not applicable in this implementation).
    async fn to_config(&self) -> Result<ControllerParams, TaskError> {
        Ok(ControllerParams::Esp32(self.clone()))
    }
}
