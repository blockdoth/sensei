use crate::errors::{ControllerError, TaskError};
use crate::sources::DataSourceT;
use crate::sources::controllers::{Controller, ControllerParams}; // The controller trait
use crate::ToConfig;

// Assume your concrete ESP32 source is located here. Adjust path as needed.
use crate::sources::esp32::Esp32Source;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::any::Any; // For downcasting

// --- ESP32 Specific Enums ---
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

/// ESP32 Command Codes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Esp32Command {
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

// --- Controller Parameter Structures ---

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

#[derive(Serialize, Deserialize, Debug, Clone, Default, JsonSchema)]
#[serde(default)]
pub struct Esp32ControllerParams {
    pub set_channel: Option<u8>,
    pub apply_device_config: Option<Esp32DeviceConfigPayload>,
    pub mac_filters_to_add: Option<Vec<MacFilterPair>>,
    pub clear_all_mac_filters: Option<bool>,
    pub pause_acquisition: Option<bool>,
    pub pause_wifi_transmit: Option<bool>,
    pub synchronize_time: Option<bool>,
    pub transmit_custom_frame: Option<CustomFrameParams>,
}

#[typetag::serde(name = "ESP32Controller")]
#[async_trait]
impl Controller for Esp32ControllerParams {
    async fn apply(&self, source: &mut dyn DataSourceT) -> Result<(), ControllerError> {
        // Downcast `dyn DataSourceT` to `&mut Esp32Source`.
        // This requires `DataSourceT: Any` and `Esp32Source` to be the concrete type.
        let esp_source = (source as &mut dyn Any)
            .downcast_mut::<Esp32Source>()
            .ok_or_else(|| {
                ControllerError::InvalidParams(
                    "Controller expected an ESP32Source compatible type.".to_string(),
                )
            })?;

        // 1. Set Channel
        if let Some(channel) = self.set_channel {
            if !(1..=14).contains(&channel) {
                return Err(ControllerError::InvalidParams(format!(
                    "Invalid WiFi channel: {channel}. Must be between 1 and 14.",
                )));
            }
            // The send_esp32_command in Esp32Source returns Result<Vec<u8>, ControllerError>
            // We can ignore the ACK payload Vec<u8> if not needed for control logic here.
            esp_source
                .send_esp32_command(Esp32Command::SetChannel, Some(vec![channel]))
                .await
                .map(|_| ())?;
        }

        // 2. Apply Device Configuration
        let mut mode_for_acquisition_logic: Option<OperationMode> = None;
        if let Some(ref config_payload) = self.apply_device_config {
            if config_payload.bandwidth == Bandwidth::Forty
                && config_payload.secondary_channel == SecondaryChannel::None
            {
                return Err(ControllerError::InvalidParams(
                    "40MHz bandwidth requires a secondary channel (Above or Below) to be set."
                        .to_string(),
                ));
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
                .map(|_| ())?;
            mode_for_acquisition_logic = Some(config_payload.mode);
        }

        // 3. Acquisition Control
        if let Some(paused) = self.pause_acquisition {
            let cmd = if paused {
                Esp32Command::PauseAcquisition
            } else {
                Esp32Command::UnpauseAcquisition
            };
            esp_source.send_esp32_command(cmd, None).await.map(|_| ())?;
        } else if let Some(mode) = mode_for_acquisition_logic {
            let cmd = if mode == OperationMode::Receive {
                Esp32Command::UnpauseAcquisition
            } else {
                Esp32Command::PauseAcquisition
            };
            esp_source.send_esp32_command(cmd, None).await.map(|_| ())?;
        }

        // 4. MAC Filters
        if let Some(true) = self.clear_all_mac_filters {
            esp_source
                .send_esp32_command(Esp32Command::WhitelistClear, None)
                .await
                .map(|_| ())?;
        }
        if let Some(ref filters_to_add) = self.mac_filters_to_add {
            for filter_pair in filters_to_add {
                let mut filter_data = Vec::with_capacity(12);
                filter_data.extend_from_slice(&filter_pair.src_mac);
                filter_data.extend_from_slice(&filter_pair.dst_mac);
                esp_source
                    .send_esp32_command(Esp32Command::WhitelistAddMacPair, Some(filter_data))
                    .await
                    .map(|_| ())?;
            }
        }

        // 5. WiFi Transmission Control
        if let Some(paused) = self.pause_wifi_transmit {
            let cmd = if paused {
                Esp32Command::PauseWifiTransmit
            } else {
                Esp32Command::ResumeWifiTransmit
            };
            esp_source.send_esp32_command(cmd, None).await.map(|_| ())?;
        }

        // 6. Time Synchronization
        if let Some(true) = self.synchronize_time {
            esp_source
                .send_esp32_command(Esp32Command::SynchronizeTimeInit, None)
                .await
                .map(|_| ())?;
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
                .map(|_| ())?;
        }

        // 7. Custom Frame Transmission
        if let Some(ref frame_params) = self.transmit_custom_frame {
            let mut tx_data = Vec::with_capacity(20);
            tx_data.extend_from_slice(&frame_params.dst_mac);
            tx_data.extend_from_slice(&frame_params.src_mac);
            tx_data.extend_from_slice(&frame_params.n_reps.to_le_bytes());
            tx_data.extend_from_slice(&frame_params.pause_ms.to_le_bytes());
            esp_source
                .send_esp32_command(Esp32Command::TransmitCustomFrame, Some(tx_data))
                .await
                .map(|_| ())?;
        }

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
    ///
    /// # Example
    /// ```
    /// let controller_config = esp32_controller.to_config().await?;
    /// // You can now serialize `controller_config` or reuse it elsewhere
    /// ```
    async fn to_config(&self) -> Result<ControllerParams, TaskError> {
        Ok(ControllerParams::Esp32(self.clone()))
    }
}

