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
                let rssi_vec = vec![rssi_val as u16];

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
    ///
    /// # Example
    /// ```
    /// let config = esp32_adapter.to_config().await?;
    /// // Save or transmit the config as needed
    /// ```
    async fn to_config(&self) -> Result<DataAdapterConfig, TaskError> {
        Ok(DataAdapterConfig::Esp32 { scale_csi: self.scale_csi })
    }
}
