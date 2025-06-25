//! CSI data types
//! Mofidied based on: wisense/sensei/lib/src/csi_types.rs
//! Originally authored by: Fabian Portner

use std::fmt;

use num_complex::Complex64;
use serde::{Deserialize, Serialize};

use crate::errors::DataSourceError;

/// Complex number type alias for CSI data representation.
pub type Complex = Complex64;
/// Alias for the CSI matrix:
/// Dimensions: `num_cores x num_streams x num_subcarriers`
type Csi = Vec<Vec<Vec<Complex>>>;

/// Definition of a single CSI data point
#[rustfmt::skip]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CsiData {
	pub timestamp       : f64,       // Timestamp (from receival of first packet fragment)
	pub sequence_number : u16,       // Extracted sequence number
	pub rssi            : Vec<u16>,  // Antenna (per core)
	pub csi             : Csi        // A num_cores x num_streams x num_subcarrier array
}

impl std::default::Default for CsiData {
    fn default() -> Self {
        CsiData {
            timestamp: 0.0,
            sequence_number: 0,
            rssi: vec![0; 4],
            csi: vec![vec![vec![Complex::new(0.0, 0.0); 30]; 2]; 4],
        }
    }
}

/// Channel Bandwidth
#[derive(Debug, Clone, Copy)]
pub enum Bandwidth {
    /// 20 Mhz bandwidth
    Bw20 = 20,
    /// 40 Mhz bandwidth
    Bw40 = 40,
    /// 40 Mhz bandwitdh
    Bw80 = 80,
    /// 160 Mhz bandwitdh
    Bw160 = 160,
}

impl Bandwidth {
    /// Construct a `Bandwidth` from its numeric MHz value.
    ///
    /// Returns `DataSourceError::ParsingError` if the value is invalid.
    pub fn new(value: u16) -> Result<Self, DataSourceError> {
        match value {
            20 => Ok(Bandwidth::Bw20),
            40 => Ok(Bandwidth::Bw40),
            80 => Ok(Bandwidth::Bw80),
            160 => Ok(Bandwidth::Bw160),
            _ => Err(DataSourceError::ParsingError(format!("Invalid collector_chanwidth value {value}"))),
        }
    }
}

impl std::fmt::Display for Bandwidth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

/// Frame Encoding
#[derive(Debug, Clone, Copy)]
pub enum FrameEncoding {
    /// Legacy 802.11a/b/g frame
    NonHt = 0,
    /// High Throughput (HT - 802.11n)
    Ht = 1,
    /// Very High Throughput (VHT - 802.11ac)
    Vht = 2,
    /// High-Efficiency (HE - 802.11ax)
    He = 3,
    /// Extremely High Throughput (EHT - 802.11be)
    Eht = 4,
}

impl FrameEncoding {
    /// Construct a `FrameEncoding` from its numeric identifier.
    ///
    /// Returns `DataSourceError::ParsingError` if the value is invalid.
    pub fn new(value: u8) -> Result<Self, DataSourceError> {
        match value {
            0 => Ok(FrameEncoding::NonHt),
            1 => Ok(FrameEncoding::Ht),
            2 => Ok(FrameEncoding::Vht),
            3 => Ok(FrameEncoding::He),
            4 => Ok(FrameEncoding::Eht),
            _ => Err(DataSourceError::ParsingError(format!("Invalid frame_encoding value {value}"))),
        }
    }
}

impl std::fmt::Display for FrameEncoding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}
