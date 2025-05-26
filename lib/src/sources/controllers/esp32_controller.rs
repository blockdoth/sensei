//! ESP32 Controller
//!
//! Defines parameters and logic for configuring an ESP32 device
//! through the `Esp32Source`.

use crate::errors::ControllerError;
use crate::sources::DataSourceT;
use crate::sources::controllers::Controller;
use crate::sources::esp32::Esp32Source; // Adjusted path

use async_trait::async_trait;
use log::{info, warn};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::any::Any; // Required for downcasting if using source.as_any_mut()

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
    PauseAcquisition = 0x04,
    UnpauseAcquisition = 0x05,
    ApplyDeviceConfig = 0x06,
    PauseWifiTransmit = 0x07,
    ResumeWifiTransmit = 0x08,
    TransmitCustomFrame = 0x09,
    SynchronizeTimeInit = 0x0A,
    SynchronizeTimeApply = 0x0B,
}

// --- Controller Parameter Structures (Kept as they are well-defined) ---
#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema)]
pub struct MacFilterPair {
    pub src_mac: [u8; 6],
    pub dst_mac: [u8; 6],
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema)]
pub struct CustomFrameParams {
    pub src_mac: [u8; 6],
    pub dst_mac: [u8; 6],
    pub n_reps: i32,
    pub pause_ms: i32,
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema)]
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

/// Parameters for controlling an ESP32 device.
#[derive(Serialize, Deserialize, Debug, Clone, Default, JsonSchema)]
#[serde(default)]
pub struct Esp32Controller {
    pub device_config: Esp32DeviceConfig,
    pub mac_filters_to_add: Vec<MacFilterPair>,
    pub clear_all_mac_filters: bool,
    pub control_acquisition: bool,   // true=unpause, false=pause
    pub control_wifi_transmit: bool, // true=resume, false=pause
    pub synchronize_time: bool,
    pub transmit_custom_frame: Option<CustomFrameParams>,
}

#[typetag::serde(name = "ESP32Controller")]
#[async_trait]
impl Controller for Esp32Controller {
    async fn apply(&self, source: &mut dyn DataSourceT) -> Result<(), ControllerError> {
        // Ensure your DataSourceT trait has `fn as_any_mut(&mut self) -> &mut dyn Any;`
        // and Esp32Source implements it.
        let mut esp_source = (source as &mut dyn Any)
            .downcast_mut::<Esp32Source>()
            .ok_or_else(|| ControllerError::InvalidDataSource("Esp32Controller requires an Esp32Source.".to_string()))?;

        let mut needs_acquisition_resume_after_config = false;
        if esp_source.send_esp32_command(Esp32Command::PauseAcquisition, None).await.is_err() {
            warn!("Pre-config: Failed to explicitly pause CSI acquisition. Continuing with config changes.");
        } else {
            info!("Pre-config: Paused CSI acquisition.");
        }
        if self.device_config.mode == OperationMode::Receive {
            needs_acquisition_resume_after_config = true;
        }

        let channel = &self.device_config.channel;
        if !(1..=14).contains(channel) {
            return Err(ControllerError::InvalidParams(format!(
                "Invalid WiFi channel: {channel}. Must be between 1 and 14.",
            )));
        }
        info!("Controller: Setting ESP32 channel to {channel}");
        esp_source
            .send_esp32_command(Esp32Command::SetChannel, Some(vec![*channel]))
            .await
            .map_err(|e| ControllerError::CommandFailed {
                command_name: "SetChannel".to_string(),
                details: e.to_string(),
            })?;

        let mut new_operation_mode_for_logic: Option<OperationMode> = None;

        info!("Controller: Applying ESP32 device configuration: {:?}", self.device_config);
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
        }
        let cmd_data = vec![
            self.device_config.mode as u8,
            self.device_config.bandwidth as u8,
            self.device_config.secondary_channel as u8,
            self.device_config.csi_type as u8,
            self.device_config.manual_scale,
        ];
        esp_source
            .send_esp32_command(Esp32Command::ApplyDeviceConfig, Some(cmd_data))
            .await
            .map_err(|e| ControllerError::CommandFailed {
                command_name: "ApplyDeviceConfig".to_string(),
                details: e.to_string(),
            })?;
        new_operation_mode_for_logic = Some(self.device_config.mode);

        if self.clear_all_mac_filters {
            Esp32Controller::clear_macs(esp_source).await?;
        }

        info!("Controller: Adding {} MAC filter(s) to ESP32.", self.mac_filters_to_add.len());
        for filter_pair in self.mac_filters_to_add.clone() {
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

        if self.control_acquisition {
            let cmd = if self.control_acquisition {
                Esp32Command::UnpauseAcquisition
            } else {
                Esp32Command::PauseAcquisition
            };
            info!(
                "Controller: Explicitly {} CSI acquisition.",
                if self.control_acquisition { "resuming" } else { "pausing" }
            );
            esp_source
                .send_esp32_command(cmd, None)
                .await
                .map_err(|e| ControllerError::CommandFailed {
                    command_name: format!("{cmd:?}"),
                    details: e.to_string(),
                })?;
        } else if needs_acquisition_resume_after_config || new_operation_mode_for_logic == Some(OperationMode::Receive) {
            info!("Controller: Implicitly unpausing CSI acquisition due to Receive mode.");
            esp_source
                .send_esp32_command(Esp32Command::UnpauseAcquisition, None)
                .await
                .map_err(|e| ControllerError::CommandFailed {
                    command_name: "UnpauseAcquisition (implicit)".to_string(),
                    details: e.to_string(),
                })?;
        } else if new_operation_mode_for_logic == Some(OperationMode::Transmit) {
            info!("Controller: Implicitly pausing CSI acquisition due to Transmit mode.");
            esp_source
                .send_esp32_command(Esp32Command::PauseAcquisition, None)
                .await
                .map_err(|e| ControllerError::CommandFailed {
                    command_name: "PauseAcquisition (implicit)".to_string(),
                    details: e.to_string(),
                })?;
        }

        if self.control_wifi_transmit {
            let cmd = if self.control_wifi_transmit {
                Esp32Command::ResumeWifiTransmit
            } else {
                Esp32Command::PauseWifiTransmit
            };
            info!(
                "Controller: Explicitly {} general WiFi transmit task.",
                if self.control_wifi_transmit { "resuming" } else { "pausing" }
            );
            esp_source
                .send_esp32_command(cmd, None)
                .await
                .map_err(|e| ControllerError::CommandFailed {
                    command_name: format!("{cmd:?}"),
                    details: e.to_string(),
                })?;
        } else if new_operation_mode_for_logic == Some(OperationMode::Transmit) {
            info!("Controller: Implicitly pausing general WiFi transmit task due to custom Transmit mode.");
            esp_source
                .send_esp32_command(Esp32Command::PauseWifiTransmit, None)
                .await
                .map_err(|e| ControllerError::CommandFailed {
                    command_name: "PauseWifiTransmit (implicit)".to_string(),
                    details: e.to_string(),
                })?;
        }

        if self.synchronize_time {
            info!("Controller: Initiating time synchronization with ESP32.");
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
            info!("Controller: Time synchronization sequence sent.");
        }

        if let Some(ref frame_params) = self.transmit_custom_frame {
            info!("Controller: Transmitting custom frames: {frame_params:?}");
            if new_operation_mode_for_logic.is_some() && new_operation_mode_for_logic != Some(OperationMode::Transmit) {
                warn!("Transmitting custom frames typically requires OperationMode::Transmit. Current/new mode may not be optimal.");
            }
            if frame_params.n_reps <= 0 || frame_params.pause_ms < 0 {
                return Err(ControllerError::InvalidParams(
                    "Custom frame n_reps must be > 0 and pause_ms >= 0.".to_string(),
                ));
            }

            let mut tx_data = Vec::with_capacity(20);
            tx_data.extend_from_slice(&frame_params.dst_mac);
            tx_data.extend_from_slice(&frame_params.src_mac);
            tx_data.extend_from_slice(&frame_params.n_reps.to_le_bytes());
            tx_data.extend_from_slice(&frame_params.pause_ms.to_le_bytes());

            esp_source
                .send_esp32_command(Esp32Command::TransmitCustomFrame, Some(tx_data))
                .await
                .map_err(|e| ControllerError::CommandFailed {
                    command_name: "TransmitCustomFrame".to_string(),
                    details: e.to_string(),
                })?;
        }

        info!("Controller: Esp32Controller applied successfully.");
        Ok(())
    }
}

impl Esp32Controller {
    async fn clear_macs(esp: &mut Esp32Source) -> Result<(), ControllerError> {
        info!("Controller: Clearing all MAC filters on ESP32.");
        esp.send_esp32_command(Esp32Command::WhitelistClear, None)
            .await
            .map_err(|e| ControllerError::CommandFailed {
                command_name: "WhitelistClear".to_string(),
                details: e.to_string(),
            })?;
        Ok(())
    }
}
