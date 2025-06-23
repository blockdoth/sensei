use std::io::{Cursor, Read};

use byteorder::{LittleEndian, ReadBytesExt};

use crate::ToConfig;
use crate::adapters::{CsiDataAdapter, DataAdapterConfig};
use crate::csi_types::{Complex, CsiData};
use crate::errors::{CsiAdapterError, Esp32AdapterError, TaskError}; // Import Esp32AdapterError
use crate::network::rpc_message::DataMsg;
// ESP32 typically operates in SISO mode (1 Transmit, 1 Receive antenna).
// If future ESP32 variants support MIMO CSI and the format changes to include Ntx/Nrx,
// this might need to become configurable or be parsed from the packet.
const NUM_TX_ANTENNAS_ESP32: usize = 1;
const NUM_RX_ANTENNAS_ESP32: usize = 1;

/// Adapter for ESP32 Channel State Information (CSI) data.
///
/// This adapter parses raw byte frames, expected to conform to the ESP32's CSI packet structure,
/// into a structured `CsiData` format.
pub struct ESP32Adapter {
    /// Whether to apply scaling to CSI data.
    /// Note: The specific scaling mechanism for ESP32 might differ from other adapters like IWL.
    /// This flag is currently a placeholder, and no scaling is applied in this version.
    #[allow(dead_code)] // Remove when scaling is implemented
    scale_csi: bool,
}

impl ESP32Adapter {
    /// Creates a new `ESP32Adapter`.
    ///
    /// # Arguments
    ///
    /// * `scale_csi` - A boolean flag indicating whether CSI data should be scaled. (Currently, this parameter is a placeholder and does not affect behavior).
    pub fn new(scale_csi: bool) -> Self {
        Self { scale_csi }
    }
}

#[async_trait::async_trait]
impl CsiDataAdapter for ESP32Adapter {
    /// Attempts to consume a `DataMsg` (expected to be `RawFrame`) and produce a `CsiFrame`.
    ///
    /// The `RawFrame`'s byte payload is parsed according to the ESP32 CSI packet format.
    /// This format typically includes a timestamp, MAC addresses, sequence number, RSSI,
    /// gain values, and the CSI data itself (interleaved I/Q components for subcarriers).
    ///
    /// # Arguments
    ///
    /// * `msg` - A `DataMsg` enum. If `DataMsg::RawFrame`, its `bytes` field is parsed.
    ///           If `DataMsg::CsiFrame`, it is passed through.
    ///
    /// # Returns
    ///
    /// * `Ok(Some(DataMsg::CsiFrame))` - When a CSI frame is successfully parsed.
    /// * `Ok(None)` - If the input message does not contain enough data to form a complete frame
    ///                (This adapter currently assumes a complete ESP32 CSI payload in `msg.bytes`).
    /// * `Err(CsiAdapterError)` - On a decoding or parsing error.
    async fn produce(&mut self, msg: DataMsg) -> Result<Option<DataMsg>, CsiAdapterError> {
        match msg {
            DataMsg::RawFrame {
                ts: _frame_ts,
                bytes,
                source_type: _,
            } => {
                // The expected ESP32 CSI payload structure (after any transport framing):
                // - timestamp_us (u64, 8 bytes)
                // - src_mac ([u8; 6], 6 bytes)
                // - dst_mac ([u8; 6], 6 bytes)
                // - seq (u16, 2 bytes)
                // - rssi (i8, 1 byte)
                // - agc_gain (u8, 1 byte)
                // - fft_gain (u8, 1 byte)
                // - csi_data_len (u16, 2 bytes) - length of the csi_data field in bytes
                // - csi_data (Vec<i8>, csi_data_len bytes) - interleaved I/Q values
                const MIN_ESP32_CSI_HEADER_SIZE: usize = 8 + 6 + 6 + 2 + 1 + 1 + 1 + 2;

                if bytes.len() < MIN_ESP32_CSI_HEADER_SIZE {
                    return Err(CsiAdapterError::ESP32(Esp32AdapterError::PayloadTooShort {
                        // Corrected Error Construction
                        expected: MIN_ESP32_CSI_HEADER_SIZE,
                        actual: bytes.len(),
                    }));
                }

                let mut cursor = Cursor::new(&bytes);

                let timestamp_us = cursor.read_u64::<LittleEndian>().map_err(|e| {
                    CsiAdapterError::ESP32(Esp32AdapterError::ParseError(format!("Failed to read timestamp_us: {e}"))) // Corrected
                })?;
                let mut _src_mac = [0u8; 6];
                cursor.read_exact(&mut _src_mac).map_err(|e| {
                    // std::io::Read::read_exact
                    CsiAdapterError::ESP32(Esp32AdapterError::ParseError(format!("Failed to read src_mac: {e}"))) // Corrected
                })?;
                let mut _dst_mac = [0u8; 6];
                cursor.read_exact(&mut _dst_mac).map_err(|e| {
                    // std::io::Read::read_exact
                    CsiAdapterError::ESP32(Esp32AdapterError::ParseError(format!("Failed to read dst_mac: {e}"))) // Corrected
                })?;
                let sequence_number = cursor.read_u16::<LittleEndian>().map_err(|e| {
                    CsiAdapterError::ESP32(Esp32AdapterError::ParseError(format!("Failed to read sequence_number: {e}"))) // Corrected
                })?;
                let rssi_val = cursor.read_i8().map_err(|e| {
                    CsiAdapterError::ESP32(Esp32AdapterError::ParseError(format!("Failed to read rssi: {e}"))) // Corrected
                })?;
                let _agc_gain = cursor.read_u8().map_err(|e| {
                    CsiAdapterError::ESP32(Esp32AdapterError::ParseError(format!("Failed to read agc_gain: {e}"))) // Corrected
                })?;
                let _fft_gain = cursor.read_u8().map_err(|e| {
                    CsiAdapterError::ESP32(Esp32AdapterError::ParseError(format!("Failed to read fft_gain: {e}"))) // Corrected
                })?;
                let csi_data_len_bytes = cursor.read_u16::<LittleEndian>().map_err(|e| {
                    CsiAdapterError::ESP32(Esp32AdapterError::ParseError(format!("Failed to read csi_data_len: {e}"))) // Corrected
                })? as usize;

                let current_pos = cursor.position() as usize;
                if bytes.len() < current_pos + csi_data_len_bytes {
                    return Err(CsiAdapterError::ESP32(Esp32AdapterError::PayloadTooShort {
                        // Corrected
                        expected: current_pos + csi_data_len_bytes,
                        actual: bytes.len(),
                    }));
                }

                if csi_data_len_bytes == 0 {
                    return Ok(None); // Corrected: Added return for early exit
                } else if csi_data_len_bytes % 2 != 0 {
                    return Err(CsiAdapterError::ESP32(Esp32AdapterError::ParseError(format!(
                        // Corrected
                        "CSI data length ({csi_data_len_bytes}) must be even (I/Q pairs)."
                    ))));
                }

                let mut csi_byte_buffer = vec![0u8; csi_data_len_bytes];
                cursor.read_exact(&mut csi_byte_buffer).map_err(|e| {
                    // std::io::Read::read_exact
                    CsiAdapterError::ESP32(Esp32AdapterError::ParseError(format!("Failed to read CSI data block: {e}"))) // Corrected
                })?;

                let num_complex_values = csi_data_len_bytes / 2;
                let mut csi_subcarriers: Vec<Complex> = Vec::with_capacity(num_complex_values);

                for i in 0..num_complex_values {
                    let imag_val = csi_byte_buffer[2 * i] as i8 as f64;
                    let real_val = csi_byte_buffer[2 * i + 1] as i8 as f64;
                    csi_subcarriers.push(Complex::new(real_val, imag_val));
                }

                let csi_structured = if NUM_TX_ANTENNAS_ESP32 > 0 && NUM_RX_ANTENNAS_ESP32 > 0 {
                    vec![vec![csi_subcarriers; NUM_RX_ANTENNAS_ESP32]; NUM_TX_ANTENNAS_ESP32]
                } else {
                    vec![]
                };

                let timestamp_sec = timestamp_us as f64 / 1_000_000.0;
                // Convert i8 RSSI to u16 by adding 128 to handle negative values
                // This maps the range [-128, 127] to [0, 255]
                let rssi_vec = vec![(rssi_val as i16 + 128) as u16];

                Ok(Some(DataMsg::CsiFrame {
                    csi: CsiData {
                        timestamp: timestamp_sec,
                        sequence_number,
                        rssi: rssi_vec,
                        csi: csi_structured,
                    },
                }))
            }
            DataMsg::CsiFrame { csi } => Ok(Some(DataMsg::CsiFrame { csi })),
        }
    }
}

#[async_trait::async_trait]
impl ToConfig<DataAdapterConfig> for ESP32Adapter {
    /// Converts the current `ESP32Adapter` instance into its configuration representation.
    ///
    /// Implements the `ToConfig` trait for `ESP32Adapter`, returning a `DataAdapterConfig::Esp32`
    /// variant that encapsulates the current state of the adapter. This configuration can be
    /// serialized into formats such as JSON or YAML for storage, inspection, or transmission.
    ///
    /// # Returns
    /// - `Ok(DataAdapterConfig::Esp32)` containing the cloned `scale_csi` value.
    /// - `Err(TaskError)` if conversion fails (not applicable in this implementation).
    async fn to_config(&self) -> Result<DataAdapterConfig, TaskError> {
        Ok(DataAdapterConfig::Esp32 { scale_csi: self.scale_csi })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::rpc_message::SourceType;
    use crate::csi_types::Complex;

    /// Helper function to create a valid ESP32 CSI packet
    fn create_esp32_csi_packet(csi_data_len: u16, csi_data: Option<Vec<i8>>) -> Vec<u8> {
        let mut packet = Vec::new();
        
        // timestamp_us (u64, 8 bytes) - little endian
        packet.extend(&1234567890u64.to_le_bytes());
        
        // src_mac (6 bytes)
        packet.extend(&[0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC]);
        
        // dst_mac (6 bytes)
        packet.extend(&[0xDE, 0xF0, 0x12, 0x34, 0x56, 0x78]);
        
        // sequence_number (u16, 2 bytes) - little endian
        packet.extend(&42u16.to_le_bytes());
        
        // rssi (i8, 1 byte)
        packet.push(-50i8 as u8);
        
        // agc_gain (u8, 1 byte)
        packet.push(10);
        
        // fft_gain (u8, 1 byte)
        packet.push(5);
        
        // csi_data_len (u16, 2 bytes) - little endian
        packet.extend(&csi_data_len.to_le_bytes());
        
        // csi_data (Vec<i8>, csi_data_len bytes)
        if let Some(data) = csi_data {
            packet.extend(data.iter().map(|&x| x as u8));
        } else {
            // Default to alternating I/Q values for testing
            for i in 0..csi_data_len {
                if i % 2 == 0 {
                    packet.push(10); // I component
                } else {
                    packet.push(20); // Q component
                }
            }
        }
        
        packet
    }

    #[tokio::test]
    async fn test_produce_valid_esp32_packet() {
        let mut adapter = ESP32Adapter::new(false);
        
        // Create a packet with 4 CSI values (8 bytes: 4 I/Q pairs)
        let packet_bytes = create_esp32_csi_packet(8, None);
        
        let msg = DataMsg::RawFrame {
            ts: 123456.789,
            bytes: packet_bytes,
            source_type: SourceType::ESP32,
        };

        let result = adapter.produce(msg).await;
        assert!(result.is_ok());
        
        let output = result.unwrap();
        assert!(output.is_some());
        
        match output.unwrap() {
            DataMsg::CsiFrame { csi } => {
                assert_eq!(csi.sequence_number, 42);
                // -50 + 128 = 78 (converting i8 to u16 range)
                assert_eq!(csi.rssi, vec![78u16]);
                assert_eq!(csi.timestamp, 1234567890.0 / 1_000_000.0);
                
                // Check CSI structure (1 TX antenna, 1 RX antenna)
                assert_eq!(csi.csi.len(), 1); // NUM_TX_ANTENNAS_ESP32
                assert_eq!(csi.csi[0].len(), 1); // NUM_RX_ANTENNAS_ESP32
                assert_eq!(csi.csi[0][0].len(), 4); // 4 complex values from 8 bytes
                
                // Check first complex value (I=10, Q=20 as signed i8)
                let first_complex = &csi.csi[0][0][0];
                assert_eq!(first_complex.re, 20.0); // Second byte (Q becomes real in ESP32 format)
                assert_eq!(first_complex.im, 10.0); // First byte (I becomes imaginary in ESP32 format)
            }
            _ => panic!("Expected CsiFrame"),
        }
    }

    #[tokio::test]
    async fn test_produce_packet_too_short() {
        let mut adapter = ESP32Adapter::new(false);
        
        // Create a packet that's too short (less than minimum header size)
        let short_packet = vec![1, 2, 3, 4, 5]; // Only 5 bytes, need at least 27
        
        let msg = DataMsg::RawFrame {
            ts: 123456.789,
            bytes: short_packet,
            source_type: SourceType::ESP32,
        };

        let result = adapter.produce(msg).await;
        assert!(result.is_err());
        
        match result.unwrap_err() {
            CsiAdapterError::ESP32(Esp32AdapterError::PayloadTooShort { expected, actual }) => {
                assert_eq!(expected, 27); // MIN_ESP32_CSI_HEADER_SIZE
                assert_eq!(actual, 5);
            }
            _ => panic!("Expected PayloadTooShort error"),
        }
    }

    #[tokio::test]
    async fn test_produce_insufficient_csi_data() {
        let mut adapter = ESP32Adapter::new(false);
        
        // Create a packet with header claiming 10 bytes of CSI data but only providing 5
        let mut packet = Vec::new();
        packet.extend(&1234567890u64.to_le_bytes());
        packet.extend(&[0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC]); // src_mac
        packet.extend(&[0xDE, 0xF0, 0x12, 0x34, 0x56, 0x78]); // dst_mac
        packet.extend(&42u16.to_le_bytes()); // sequence_number
        packet.push(-50i8 as u8); // rssi
        packet.push(10); // agc_gain
        packet.push(5); // fft_gain
        packet.extend(&10u16.to_le_bytes()); // csi_data_len = 10
        packet.extend(&[1, 2, 3, 4, 5]); // Only 5 bytes of CSI data, not 10
        
        let msg = DataMsg::RawFrame {
            ts: 123456.789,
            bytes: packet,
            source_type: SourceType::ESP32,
        };

        let result = adapter.produce(msg).await;
        assert!(result.is_err());
        
        match result.unwrap_err() {
            CsiAdapterError::ESP32(Esp32AdapterError::PayloadTooShort { expected, actual }) => {
                assert_eq!(expected, 27 + 10); // header + claimed CSI data length
                assert_eq!(actual, 27 + 5); // header + actual CSI data length
            }
            _ => panic!("Expected PayloadTooShort error"),
        }
    }

    #[tokio::test]
    async fn test_produce_zero_csi_data_length() {
        let mut adapter = ESP32Adapter::new(false);
        
        // Create a packet with zero CSI data length
        let packet_bytes = create_esp32_csi_packet(0, Some(vec![]));
        
        let msg = DataMsg::RawFrame {
            ts: 123456.789,
            bytes: packet_bytes,
            source_type: SourceType::ESP32,
        };

        let result = adapter.produce(msg).await;
        assert!(result.is_ok());
        
        let output = result.unwrap();
        assert!(output.is_none()); // Should return None for zero-length CSI data
    }

    #[tokio::test]
    async fn test_produce_odd_csi_data_length() {
        let mut adapter = ESP32Adapter::new(false);
        
        // Create a packet with odd CSI data length (not divisible by 2)
        let packet_bytes = create_esp32_csi_packet(7, Some(vec![1, 2, 3, 4, 5, 6, 7]));
        
        let msg = DataMsg::RawFrame {
            ts: 123456.789,
            bytes: packet_bytes,
            source_type: SourceType::ESP32,
        };

        let result = adapter.produce(msg).await;
        assert!(result.is_err());
        
        match result.unwrap_err() {
            CsiAdapterError::ESP32(Esp32AdapterError::ParseError(msg)) => {
                assert!(msg.contains("CSI data length (7) must be even (I/Q pairs)"));
            }
            _ => panic!("Expected ParseError for odd CSI data length"),
        }
    }

    #[tokio::test]
    async fn test_produce_csi_frame_passthrough() {
        let mut adapter = ESP32Adapter::new(false);
        
        let csi_data = CsiData {
            timestamp: 123.456,
            sequence_number: 789,
            rssi: vec![100],
            csi: vec![vec![vec![Complex::new(1.0, 2.0)]]],
        };
        
        let msg = DataMsg::CsiFrame { csi: csi_data.clone() };

        let result = adapter.produce(msg).await;
        assert!(result.is_ok());
        
        let output = result.unwrap();
        assert!(output.is_some());
        
        match output.unwrap() {
            DataMsg::CsiFrame { csi } => {
                assert_eq!(csi.timestamp, csi_data.timestamp);
                assert_eq!(csi.sequence_number, csi_data.sequence_number);
                assert_eq!(csi.rssi, csi_data.rssi);
            }
            _ => panic!("Expected CsiFrame"),
        }
    }

    #[tokio::test]
    async fn test_produce_large_csi_packet() {
        let mut adapter = ESP32Adapter::new(false);
        
        // Create a packet with many CSI values (128 I/Q pairs
        let csi_data: Vec<i8> = (0..256).map(|i| (i % 128) as i8).collect();
        let packet_bytes = create_esp32_csi_packet(256, Some(csi_data));
        
        let msg = DataMsg::RawFrame {
            ts: 123456.789,
            bytes: packet_bytes,
            source_type: SourceType::ESP32,
        };

        let result = adapter.produce(msg).await;
        assert!(result.is_ok());
        
        let output = result.unwrap();
        assert!(output.is_some());
        
        match output.unwrap() {
            DataMsg::CsiFrame { csi } => {
                assert_eq!(csi.csi[0][0].len(), 128); // 128 complex values from 256 bytes
            }
            _ => panic!("Expected CsiFrame"),
        }
    }

    #[tokio::test]
    async fn test_to_config() {
        let adapter_no_scale = ESP32Adapter::new(false);
        let config_result = adapter_no_scale.to_config().await;
        assert!(config_result.is_ok());
        
        match config_result.unwrap() {
            DataAdapterConfig::Esp32 { scale_csi } => {
                assert!(!scale_csi);
            }
            _ => panic!("Expected Esp32 config"),
        }

        let adapter_with_scale = ESP32Adapter::new(true);
        let config_result = adapter_with_scale.to_config().await;
        assert!(config_result.is_ok());
        
        match config_result.unwrap() {
            DataAdapterConfig::Esp32 { scale_csi } => {
                assert!(scale_csi);
            }
            _ => panic!("Expected Esp32 config"),
        }
    }

    #[test]
    fn test_new_adapter() {
        let adapter_no_scale = ESP32Adapter::new(false);
        assert!(!adapter_no_scale.scale_csi);
        
        let adapter_with_scale = ESP32Adapter::new(true);
        assert!(adapter_with_scale.scale_csi);
    }

    #[tokio::test]
    async fn test_produce_empty_input() {
        let mut adapter = ESP32Adapter::new(false);
        
        let msg = DataMsg::RawFrame {
            ts: 123456.789,
            bytes: vec![],
            source_type: SourceType::ESP32,
        };

        let result = adapter.produce(msg).await;
        assert!(result.is_err());
        
        match result.unwrap_err() {
            CsiAdapterError::ESP32(Esp32AdapterError::PayloadTooShort { expected, actual }) => {
                assert_eq!(expected, 27); // MIN_ESP32_CSI_HEADER_SIZE
                assert_eq!(actual, 0);
            }
            _ => panic!("Expected PayloadTooShort error"),
        }
    }

    #[tokio::test]
    async fn test_produce_different_source_types() {
        let mut adapter = ESP32Adapter::new(false);
        
        // Test with IWL5300 source type (should still work as we only check RawFrame)
        let packet_bytes = create_esp32_csi_packet(8, None);
        let msg = DataMsg::RawFrame {
            ts: 123456.789,
            bytes: packet_bytes,
            source_type: SourceType::IWL5300, // Different source type
        };

        let result = adapter.produce(msg).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_some());
    }

    #[tokio::test]
    async fn test_produce_boundary_values() {
        let mut adapter = ESP32Adapter::new(false);
        
        // Test with exactly minimum required size (header only)
        let mut packet = Vec::new();
        packet.extend(&1234567890u64.to_le_bytes()); // timestamp
        packet.extend(&[0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC]); // src_mac
        packet.extend(&[0xDE, 0xF0, 0x12, 0x34, 0x56, 0x78]); // dst_mac
        packet.extend(&42u16.to_le_bytes()); // sequence_number
        packet.push(-50i8 as u8); // rssi
        packet.push(10); // agc_gain
        packet.push(5); // fft_gain
        packet.extend(&0u16.to_le_bytes()); // csi_data_len = 0
        
        let msg = DataMsg::RawFrame {
            ts: 123456.789,
            bytes: packet,
            source_type: SourceType::ESP32,
        };

        let result = adapter.produce(msg).await;
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.is_none()); // Should return None for zero-length CSI data
    }

    #[tokio::test]
    async fn test_produce_maximum_csi_data() {
        let mut adapter = ESP32Adapter::new(false);
        
        // Test with maximum reasonable CSI data size (1000 I/Q pairs = 2000 bytes)
        let csi_data: Vec<i8> = (0..2000).map(|i| (i % 256) as i8).collect();
        let packet_bytes = create_esp32_csi_packet(2000, Some(csi_data));
        
        let msg = DataMsg::RawFrame {
            ts: 123456.789,
            bytes: packet_bytes,
            source_type: SourceType::ESP32,
        };

        let result = adapter.produce(msg).await;
        assert!(result.is_ok());
        
        let output = result.unwrap();
        assert!(output.is_some());
        
        match output.unwrap() {
            DataMsg::CsiFrame { csi } => {
                assert_eq!(csi.csi[0][0].len(), 1000); // 1000 complex values
            }
            _ => panic!("Expected CsiFrame"),
        }
    }

    #[tokio::test]
    async fn test_produce_rssi_conversion_edge_cases() {
        let mut adapter = ESP32Adapter::new(false);
        
        // Test with minimum RSSI value (-128)
        let packet_bytes = create_esp32_csi_packet_with_rssi(4, Some(vec![1, 2, 3, 4]), -128);
        let msg = DataMsg::RawFrame {
            ts: 123456.789,
            bytes: packet_bytes,
            source_type: SourceType::ESP32,
        };

        let result = adapter.produce(msg).await;
        assert!(result.is_ok());
        
        match result.unwrap().unwrap() {
            DataMsg::CsiFrame { csi } => {
                assert_eq!(csi.rssi, vec![0u16]); // -128 + 128 = 0
            }
            _ => panic!("Expected CsiFrame"),
        }

        // Test with maximum RSSI value (127)
        let packet_bytes = create_esp32_csi_packet_with_rssi(4, Some(vec![1, 2, 3, 4]), 127);
        let msg = DataMsg::RawFrame {
            ts: 123456.789,
            bytes: packet_bytes,
            source_type: SourceType::ESP32,
        };

        let result = adapter.produce(msg).await;
        assert!(result.is_ok());
        
        match result.unwrap().unwrap() {
            DataMsg::CsiFrame { csi } => {
                assert_eq!(csi.rssi, vec![255u16]); // 127 + 128 = 255
            }
            _ => panic!("Expected CsiFrame"),
        }
    }

    #[tokio::test]
    async fn test_produce_csi_data_parsing() {
        let mut adapter = ESP32Adapter::new(false);
        
        // Test with known I/Q values
        let csi_data = vec![
            100i8, -50i8,  // First complex: real=100, imag=-50
            -75i8, 25i8,   // Second complex: real=-75, imag=25
        ];
        let packet_bytes = create_esp32_csi_packet(4, Some(csi_data));
        
        let msg = DataMsg::RawFrame {
            ts: 123456.789,
            bytes: packet_bytes,
            source_type: SourceType::ESP32,
        };

        let result = adapter.produce(msg).await;
        assert!(result.is_ok());
        
        match result.unwrap().unwrap() {
            DataMsg::CsiFrame { csi } => {
                assert_eq!(csi.csi[0][0].len(), 2);
                
                // Check first complex value: I[0]=100, Q[1]=-50 => real=-50, imag=100
                let first_complex = &csi.csi[0][0][0];
                assert_eq!(first_complex.re, -50.0); // Q[1] becomes real
                assert_eq!(first_complex.im, 100.0); // I[0] becomes imag
                
                // Check second complex value: I[2]=-75, Q[3]=25 => real=25, imag=-75
                let second_complex = &csi.csi[0][0][1];
                assert_eq!(second_complex.re, 25.0);  // Q[3] becomes real
                assert_eq!(second_complex.im, -75.0); // I[2] becomes imag
            }
            _ => panic!("Expected CsiFrame"),
        }
    }

    #[test]
    fn test_adapter_scale_csi_flag() {
        let adapter_no_scale = ESP32Adapter::new(false);
        assert!(!adapter_no_scale.scale_csi);
        
        let adapter_with_scale = ESP32Adapter::new(true);
        assert!(adapter_with_scale.scale_csi);
    }

    /// Helper function to create ESP32 CSI packet with specific RSSI
    fn create_esp32_csi_packet_with_rssi(csi_data_len: u16, csi_data: Option<Vec<i8>>, rssi: i8) -> Vec<u8> {
        let mut packet = Vec::new();
        
        // timestamp_us (u64, 8 bytes) - little endian
        packet.extend(&1234567890u64.to_le_bytes());
        
        // src_mac (6 bytes)
        packet.extend(&[0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC]);
        
        // dst_mac (6 bytes)
        packet.extend(&[0xDE, 0xF0, 0x12, 0x34, 0x56, 0x78]);
        
        // sequence_number (u16, 2 bytes) - little endian
        packet.extend(&42u16.to_le_bytes());
        
        // rssi (i8, 1 byte) - use provided RSSI value
        packet.push(rssi as u8);
        
        // agc_gain (u8, 1 byte)
        packet.push(10);
        
        // fft_gain (u8, 1 byte)
        packet.push(5);
        
        // csi_data_len (u16, 2 bytes) - little endian
        packet.extend(&csi_data_len.to_le_bytes());
        
        // csi_data (Vec<i8>, csi_data_len bytes)
        if let Some(data) = csi_data {
            packet.extend(data.iter().map(|&x| x as u8));
        } else {
            // Default to alternating I/Q values for testing
            for i in 0..csi_data_len {
                packet.push(if i % 2 == 0 { 10 } else { 20 });
            }
        }
        
        packet
    }
}
