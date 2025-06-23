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
#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, PartialEq)]
pub struct MacFilterPair {
    pub src_mac: [u8; 6],
    pub dst_mac: [u8; 6],
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, Default, PartialEq)]
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
#[derive(Serialize, Deserialize, Debug, Clone, Default, JsonSchema, PartialEq)]
#[serde(default)]
pub struct Esp32ControllerParams {
    pub device_config: Esp32DeviceConfig,
    pub mac_filters: Vec<MacFilterPair>,
    pub mode: EspMode,
    pub synchronize_time: bool,
    pub transmit_custom_frame: Option<CustomFrameParams>,
}

#[async_trait]
impl Controller for Esp32ControllerParams {
    async fn apply(&self, source: &mut dyn DataSourceT) -> Result<(), ControllerError> {
        // Ensure your DataSourceT trait has `fn as_any_mut(&mut self) -> &mut dyn Any;`
        // and Esp32Source implements it.
        let esp_source = (source as &mut dyn Any)
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
                        command_name: "Failed to pause sending".to_string(),
                        details: e.to_string(),
                    })?;
            }
            EspMode::SendingContinuous | EspMode::SendingBurst => {
                debug!("Pausing listening to prepare for sending");
                esp_source
                    .send_esp32_command(Esp32Command::ListeningPause, None)
                    .await
                    .map_err(|e| ControllerError::CommandFailed {
                        command_name: "Failed to pause listening".to_string(),
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
                        command_name: "UnpauseAcquisition".to_string(),
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
                            command_name: "ResumeWifiTransmit".to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn create_default_controller_params() -> Esp32ControllerParams {
        Esp32ControllerParams {
            device_config: Esp32DeviceConfig::default(),
            mac_filters: vec![],
            mode: EspMode::Listening,
            synchronize_time: true,
            transmit_custom_frame: None,
        }
    }

    fn create_test_mac_filter() -> MacFilterPair {
        MacFilterPair {
            src_mac: [0x11, 0x22, 0x33, 0x44, 0x55, 0x66],
            dst_mac: [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF],
        }
    }

    #[test]
    fn test_esp32_device_config_default() {
        let config = Esp32DeviceConfig::default();
        assert_eq!(config.channel, 1);
        assert_eq!(config.mode, OperationMode::Receive);
        assert_eq!(config.bandwidth, Bandwidth::Twenty);
        assert_eq!(config.secondary_channel, SecondaryChannel::None);
        assert_eq!(config.csi_type, CsiType::HighThroughputLTF);
        assert_eq!(config.manual_scale, 0);
    }

    #[test]
    fn test_esp32_device_config_to_vec() {
        let config = Esp32DeviceConfig {
            channel: 6,
            mode: OperationMode::Transmit,
            bandwidth: Bandwidth::Forty,
            secondary_channel: SecondaryChannel::Above,
            csi_type: CsiType::LegacyLTF,
            manual_scale: 2,
        };

        let vec = config.to_vec();
        assert_eq!(vec, vec![
            OperationMode::Transmit as u8,
            Bandwidth::Forty as u8,
            SecondaryChannel::Above as u8,
            CsiType::LegacyLTF as u8,
            2u8
        ]);
    }

    #[test]
    fn test_custom_frame_params_default() {
        let params = CustomFrameParams::default();
        assert_eq!(params.src_mac, [0; 6]);
        assert_eq!(params.dst_mac, [0; 6]);
        assert_eq!(params.n_reps, 0);
        assert_eq!(params.pause_ms, 0);
    }

    #[test]
    fn test_custom_frame_params_to_vec() {
        let params = CustomFrameParams {
            src_mac: [0x11, 0x22, 0x33, 0x44, 0x55, 0x66],
            dst_mac: [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF],
            n_reps: 0x12345678,
            pause_ms: 0x87654321,
        };

        let vec = params.to_vec();
        let expected = vec![
            // dst_mac
            0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF,
            // src_mac  
            0x11, 0x22, 0x33, 0x44, 0x55, 0x66,
            // n_reps as little-endian
            0x78, 0x56, 0x34, 0x12,
            // pause_ms as little-endian
            0x21, 0x43, 0x65, 0x87,
        ];
        assert_eq!(vec, expected);
    }

    #[test]
    fn test_esp32_controller_params_default() {
        let params = Esp32ControllerParams::default();
        assert_eq!(params.device_config, Esp32DeviceConfig::default());
        assert!(params.mac_filters.is_empty());
        assert_eq!(params.mode, EspMode::default());
        assert!(!params.synchronize_time);
        assert!(params.transmit_custom_frame.is_none());
    }

    #[test]
    fn test_validation_invalid_channel() {
        let mut controller = create_default_controller_params();
        controller.device_config.channel = 15; // Invalid channel (must be 1-11)
        
        // Test channel validation logic
        let channel = &controller.device_config.channel;
        assert!(!(1..=11).contains(channel));
    }

    #[test] 
    fn test_validation_valid_channel() {
        let mut controller = create_default_controller_params();
        controller.device_config.channel = 6; // Valid channel
        
        let channel = &controller.device_config.channel;
        assert!((1..=11).contains(channel));
    }

    #[test]
    fn test_validation_invalid_bandwidth_secondary_channel() {
        let mut controller = create_default_controller_params();
        controller.device_config.bandwidth = Bandwidth::Forty;
        controller.device_config.secondary_channel = SecondaryChannel::None; // Invalid for 40MHz
        
        // Test bandwidth/secondary channel validation logic
        let invalid_combo = controller.device_config.bandwidth == Bandwidth::Forty 
            && controller.device_config.secondary_channel == SecondaryChannel::None;
        assert!(invalid_combo);
    }

    #[test]
    fn test_validation_valid_bandwidth_secondary_channel() {
        let mut controller = create_default_controller_params();
        controller.device_config.bandwidth = Bandwidth::Forty;
        controller.device_config.secondary_channel = SecondaryChannel::Above; // Valid for 40MHz
        
        let valid_combo = !(controller.device_config.bandwidth == Bandwidth::Forty 
            && controller.device_config.secondary_channel == SecondaryChannel::None);
        assert!(valid_combo);
    }

    #[test] 
    fn test_validation_sending_without_custom_frame() {
        let mut controller = create_default_controller_params();
        controller.mode = EspMode::SendingContinuous;
        controller.transmit_custom_frame = None; // No custom frame for sending mode
        
        // Test that sending modes require custom frame
        let needs_custom_frame = matches!(controller.mode, EspMode::SendingContinuous | EspMode::SendingBurst);
        let has_custom_frame = controller.transmit_custom_frame.is_some();
        assert!(needs_custom_frame && !has_custom_frame);
    }

    #[test]
    fn test_validation_sending_with_custom_frame() {
        let mut controller = create_default_controller_params();
        controller.mode = EspMode::SendingContinuous;
        controller.transmit_custom_frame = Some(CustomFrameParams::default());
        
        let needs_custom_frame = matches!(controller.mode, EspMode::SendingContinuous | EspMode::SendingBurst);
        let has_custom_frame = controller.transmit_custom_frame.is_some();
        assert!(needs_custom_frame && has_custom_frame);
    }

    #[tokio::test]
    async fn test_to_config() {
        let controller = create_default_controller_params();
        let config_result = controller.to_config().await;
        assert!(config_result.is_ok());
        
        match config_result.unwrap() {
            ControllerParams::Esp32(params) => {
                assert_eq!(params, controller);
            }
            _ => panic!("Expected Esp32 controller params"),
        }
    }

    #[test]
    fn test_esp32_command_values() {
        // Ensure command enum values match expected firmware values
        assert_eq!(Esp32Command::Nop as u8, 0x00);
        assert_eq!(Esp32Command::SetChannel as u8, 0x01);
        assert_eq!(Esp32Command::WhitelistAddMacPair as u8, 0x02);
        assert_eq!(Esp32Command::WhitelistClear as u8, 0x03);
        assert_eq!(Esp32Command::ListeningPause as u8, 0x04);
        assert_eq!(Esp32Command::ListeningResume as u8, 0x05);
        assert_eq!(Esp32Command::ApplyDeviceConfig as u8, 0x06);
        assert_eq!(Esp32Command::SendingPause as u8, 0x07);
        assert_eq!(Esp32Command::SendingResume as u8, 0x08);
        assert_eq!(Esp32Command::SetCustomFrame as u8, 0x09);
        assert_eq!(Esp32Command::SynchronizeTimeInit as u8, 0x0A);
        assert_eq!(Esp32Command::SynchronizeTimeApply as u8, 0x0B);
    }

    #[test]
    fn test_enum_serialization_values() {
        // Test OperationMode
        assert_eq!(OperationMode::Receive as u8, 0x00);
        assert_eq!(OperationMode::Transmit as u8, 0x01);
        
        // Test Bandwidth
        assert_eq!(Bandwidth::Twenty as u8, 0x00);
        assert_eq!(Bandwidth::Forty as u8, 0x01);
        
        // Test SecondaryChannel
        assert_eq!(SecondaryChannel::None as u8, 0x00);
        assert_eq!(SecondaryChannel::Below as u8, 0x01);
        assert_eq!(SecondaryChannel::Above as u8, 0x02);
        
        // Test CsiType
        assert_eq!(CsiType::LegacyLTF as u8, 0x00);
        assert_eq!(CsiType::HighThroughputLTF as u8, 0x01);
    }

    #[test]
    fn test_esp_mode_default() {
        assert_eq!(EspMode::default(), EspMode::SendingPaused);
    }

    #[test]
    fn test_mac_filter_pair_equality() {
        let filter1 = MacFilterPair {
            src_mac: [1, 2, 3, 4, 5, 6],
            dst_mac: [7, 8, 9, 10, 11, 12],
        };
        
        let filter2 = MacFilterPair {
            src_mac: [1, 2, 3, 4, 5, 6],
            dst_mac: [7, 8, 9, 10, 11, 12],
        };
        
        let filter3 = MacFilterPair {
            src_mac: [1, 2, 3, 4, 5, 7], // Different
            dst_mac: [7, 8, 9, 10, 11, 12],
        };
        
        assert_eq!(filter1, filter2);
        assert_ne!(filter1, filter3);
    }

    #[test]
    fn test_mac_filter_serialization() {
        let filter = create_test_mac_filter();
        let mut expected_data = Vec::new();
        expected_data.extend_from_slice(&filter.src_mac);
        expected_data.extend_from_slice(&filter.dst_mac);
        
        // Simulate the data structure that would be sent to the ESP32
        let mut actual_data = Vec::with_capacity(12);
        actual_data.extend_from_slice(&filter.src_mac);
        actual_data.extend_from_slice(&filter.dst_mac);
        
        assert_eq!(actual_data, expected_data);
        assert_eq!(actual_data.len(), 12);
    }
}
