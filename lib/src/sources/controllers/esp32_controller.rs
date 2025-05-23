//! ESP32 Controller
//!
//! Defines parameters and logic for configuring an ESP32 device
//! through the `Esp32Source`.

use crate::errors::ControllerError;
use crate::sources::DataSourceT;
use crate::sources::controllers::Controller;
use crate::sources::esp32::Esp32Source; // Adjusted path

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::any::Any; // Required for downcasting if using source.as_any_mut()
use log::{info, warn};


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
pub struct Esp32DeviceConfigPayload {
    pub mode: OperationMode,
    pub bandwidth: Bandwidth,
    pub secondary_channel: SecondaryChannel,
    pub csi_type: CsiType,
    pub manual_scale: u8,
}

/// Parameters for controlling an ESP32 device.
#[derive(Serialize, Deserialize, Debug, Clone, Default, JsonSchema)]
#[serde(default)]
pub struct Esp32ControllerParams {
    pub set_channel: Option<u8>,
    pub apply_device_config: Option<Esp32DeviceConfigPayload>,
    pub mac_filters_to_add: Option<Vec<MacFilterPair>>,
    pub clear_all_mac_filters: Option<bool>,
    pub control_acquisition: Option<bool>, // true=unpause, false=pause
    pub control_wifi_transmit: Option<bool>, // true=resume, false=pause
    pub synchronize_time: Option<bool>,
    pub transmit_custom_frame: Option<CustomFrameParams>,
}

#[typetag::serde(name = "ESP32Controller")]
#[async_trait]
impl Controller for Esp32ControllerParams {
    async fn apply(&self, source: &mut dyn DataSourceT) -> Result<(), ControllerError> {
        // Ensure your DataSourceT trait has `fn as_any_mut(&mut self) -> &mut dyn Any;`
        // and Esp32Source implements it.
        let esp_source = (source as &mut dyn Any).downcast_mut::<Esp32Source>()
            .ok_or_else(|| {
                ControllerError::InvalidDataSource(
                    "Esp32ControllerParams requires an Esp32Source.".to_string(),
                )
            })?;

        let mut needs_acquisition_resume_after_config = false;
        if self.set_channel.is_some() || self.apply_device_config.is_some() {
            if esp_source.send_esp32_command(Esp32Command::PauseAcquisition, None).await.is_err() {
                 warn!("Pre-config: Failed to explicitly pause CSI acquisition. Continuing with config changes.");
            } else {
                info!("Pre-config: Paused CSI acquisition.");
            }
            if let Some(ref dev_config) = self.apply_device_config {
                if dev_config.mode == OperationMode::Receive {
                    needs_acquisition_resume_after_config = true;
                }
            }
        }

        if let Some(channel) = self.set_channel {
            if !(1..=14).contains(&channel) {
                return Err(ControllerError::InvalidParams(format!(
                    "Invalid WiFi channel: {channel}. Must be between 1 and 14.",
                )));
            }
            info!("Controller: Setting ESP32 channel to {channel}");
            esp_source
                .send_esp32_command(Esp32Command::SetChannel, Some(vec![channel]))
                .await
                .map_err(|e| ControllerError::CommandFailed {
                    command_name: "SetChannel".to_string(),
                    details: e.to_string(),
                })?;
        }

        let mut new_operation_mode_for_logic: Option<OperationMode> = None;
        if let Some(ref config_payload) = self.apply_device_config {
            info!("Controller: Applying ESP32 device configuration: {:?}", config_payload);
            if config_payload.bandwidth == Bandwidth::Forty
                && config_payload.secondary_channel == SecondaryChannel::None
            {
                return Err(ControllerError::InvalidParams(
                    "40MHz bandwidth requires a secondary channel (Above or Below) to be set."
                        .to_string(),
                ));
            }
            if config_payload.manual_scale > 3 && config_payload.csi_type == CsiType::HighThroughputLTF {
                 warn!("Manual scale {} might be too high for HT-LTF, ESP32-S3 typically expects 0-3.", config_payload.manual_scale);
            } else if config_payload.manual_scale > 1 && config_payload.csi_type == CsiType::LegacyLTF {
                 warn!("Manual scale {} might be too high for L-LTF, ESP32 typically expects 0-1.", config_payload.manual_scale);
            }

            let cmd_data = vec![
                config_payload.mode as u8,
                config_payload.bandwidth as u8,
                config_payload.secondary_channel as u8,
                config_payload.csi_type as u8,
                config_payload.manual_scale,
            ];
            esp_source
                .send_esp32_command(Esp32Command::ApplyDeviceConfig, Some(cmd_data))
                .await
                .map_err(|e| ControllerError::CommandFailed {
                    command_name: "ApplyDeviceConfig".to_string(),
                    details: e.to_string(),
                })?;
            new_operation_mode_for_logic = Some(config_payload.mode);
        }

        if let Some(true) = self.clear_all_mac_filters {
            info!("Controller: Clearing all MAC filters on ESP32.");
            esp_source
                .send_esp32_command(Esp32Command::WhitelistClear, None)
                .await
                .map_err(|e| ControllerError::CommandFailed {
                    command_name: "WhitelistClear".to_string(),
                    details: e.to_string(),
                })?;
        }
        if let Some(ref filters_to_add) = self.mac_filters_to_add {
            info!("Controller: Adding {} MAC filter(s) to ESP32.", filters_to_add.len());
            for filter_pair in filters_to_add {
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
        }

        if let Some(should_unpause_acq) = self.control_acquisition {
            let cmd = if should_unpause_acq {
                Esp32Command::UnpauseAcquisition
            } else {
                Esp32Command::PauseAcquisition
            };
            info!("Controller: Explicitly {} CSI acquisition.", if should_unpause_acq {"resuming"} else {"pausing"});
            esp_source.send_esp32_command(cmd, None).await
             .map_err(|e| ControllerError::CommandFailed {
                command_name: format!("{:?}", cmd),
                details: e.to_string(),
            })?;
        } else if needs_acquisition_resume_after_config || new_operation_mode_for_logic == Some(OperationMode::Receive) {
            info!("Controller: Implicitly unpausing CSI acquisition due to Receive mode.");
            esp_source.send_esp32_command(Esp32Command::UnpauseAcquisition, None).await
             .map_err(|e| ControllerError::CommandFailed {
                command_name: "UnpauseAcquisition (implicit)".to_string(),
                details: e.to_string(),
            })?;
        } else if new_operation_mode_for_logic == Some(OperationMode::Transmit) {
            info!("Controller: Implicitly pausing CSI acquisition due to Transmit mode.");
            esp_source.send_esp32_command(Esp32Command::PauseAcquisition, None).await
            .map_err(|e| ControllerError::CommandFailed {
                command_name: "PauseAcquisition (implicit)".to_string(),
                details: e.to_string(),
            })?;
        }

        if let Some(should_resume_tx) = self.control_wifi_transmit {
            let cmd = if should_resume_tx {
                Esp32Command::ResumeWifiTransmit
            } else {
                Esp32Command::PauseWifiTransmit
            };
            info!("Controller: Explicitly {} general WiFi transmit task.", if should_resume_tx {"resuming"} else {"pausing"});
            esp_source.send_esp32_command(cmd, None).await
             .map_err(|e| ControllerError::CommandFailed {
                command_name: format!("{:?}", cmd),
                details: e.to_string(),
            })?;
        } else if new_operation_mode_for_logic == Some(OperationMode::Transmit) {
            info!("Controller: Implicitly pausing general WiFi transmit task due to custom Transmit mode.");
            esp_source.send_esp32_command(Esp32Command::PauseWifiTransmit, None).await
             .map_err(|e| ControllerError::CommandFailed {
                command_name: "PauseWifiTransmit (implicit)".to_string(),
                details: e.to_string(),
            })?;
        }

        if let Some(true) = self.synchronize_time {
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
                .map_err(|e| {
                    ControllerError::Execution(format!("System time error for sync: {e}"))
                })?
                .as_micros() as u64;

            esp_source
                .send_esp32_command(
                    Esp32Command::SynchronizeTimeApply,
                    Some(time_us.to_le_bytes().to_vec()),
                )
                .await
                .map_err(|e| ControllerError::CommandFailed {
                    command_name: "SynchronizeTimeApply".to_string(),
                    details: e.to_string(),
                })?;
            info!("Controller: Time synchronization sequence sent.");
        }

        if let Some(ref frame_params) = self.transmit_custom_frame {
            info!("Controller: Transmitting custom frames: {:?}", frame_params);
            if new_operation_mode_for_logic.is_some() && new_operation_mode_for_logic != Some(OperationMode::Transmit) {
                 warn!("Transmitting custom frames typically requires OperationMode::Transmit. Current/new mode may not be optimal.");
            }
            if frame_params.n_reps <=0 || frame_params.pause_ms < 0 {
                return Err(ControllerError::InvalidParams(
                    "Custom frame n_reps must be > 0 and pause_ms >= 0.".to_string()
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

        info!("Controller: ESP32ControllerParams applied successfully.");
        Ok(())
    }
}