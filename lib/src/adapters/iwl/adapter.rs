use crate::adapters::CsiDataAdapter;
use crate::adapters::iwl::header::IwlHeader;
use crate::csi_types::{Complex, CsiData};
use crate::errors::CsiAdapterError;

const NUM_SUBCARRIER: usize = 30;

/// Adapter for Intel wireless CSI data.
///
/// The `IwlAdapter` parses raw bytes received from the Intel 5300 NIC and transforms them
/// into structured CSI data, optionally applying signal scaling to convert raw complex
/// values into meaningful signal representations.
pub struct IwlAdapter {
    /// Whether to apply scaling to CSI data based on RSSI, noise, and AGC.
    scale_csi: bool,
}

impl IwlAdapter {
    /// Creates a new Intel wireless CSI adapter.
    ///
    /// # Arguments
    ///
    /// * `scale_csi` - Whether to apply signal scaling after parsing CSI.
    pub fn new(scale_csi: bool) -> Self {
        Self { scale_csi }
    }
}

#[async_trait::async_trait]
impl CsiDataAdapter for IwlAdapter {
    /// Parses raw CSI data buffer and produces structured `CsiData`.
    ///
    /// This function:
    /// - Parses the header and payload from the input buffer.
    /// - Reconstructs the CSI matrix with complex numbers.
    /// - Optionally applies scaling to the CSI based on signal characteristics.
    /// - Will only return a value when parsing is ready, otherwise all again
    ///
    /// # Arguments
    ///
    /// * `buf` - A byte slice containing the raw CSI data.
    ///
    /// # Returns
    ///
    /// * `Ok(Some(CsiData))` if parsing is successful.
    /// * 'Ok(None)' if parsing is not yet done, call the function again
    /// * `Err(CsiAdapterError)` if parsing fails.
    async fn produce(&mut self, buf: &[u8]) -> Result<Option<CsiData>, CsiAdapterError> {
        // Parse header information and extract payload slice
        let (header, payload) = IwlHeader::parse(buf)?;

        // Initialize CSI matrix based on NRX and NTX
        let mut csi = vec![vec![vec![Complex::new(0.0, 0.0); 30]; header.nrx]; header.ntx];
        let mut index = 0;

        // Populate CSI matrix with values from payload
        for i in 0..NUM_SUBCARRIER {
            index += 3;
            let remainder = index % 8;

            (0..header.ntx).for_each(|tx| {
                (0..header.nrx).for_each(|rx| {
                    let permuted_rx = header.perm[rx];

                    // Important: We need to cast the values here to u16, since rust's left-shift
                    // operator acts cyclically; if a value leaves the boundary, its shifted back
                    // in on the other side. We don't want that.
                    let p1 = payload[index / 8] as u16;
                    let p2 = payload[index / 8 + 1] as u16;
                    let p3 = payload[index / 8 + 2] as u16;

                    // Calculate real and imag directly as i8 values
                    let real = (((p1 >> remainder) | (p2 << (8 - remainder))) as i8) as f64;
                    let imag = (((p2 >> remainder) | (p3 << (8 - remainder))) as i8) as f64;

                    csi[tx][permuted_rx][i] = Complex::new(real, imag);
                    index += 16;
                });
            });
        }

        // Scale CSI values
        if self.scale_csi {
            scale_csi(
                &mut csi,
                &header.rssi,
                header.noise,
                header.agc,
                header.ntx,
                header.nrx,
            );
        }

        // Unpermute RSSI values using the header's permutation array
        let rssi: Vec<_> = header
            .perm
            .iter()
            .take(header.nrx)
            .map(|&permuted_rx| header.rssi[permuted_rx])
            .collect();

        Ok(Some(CsiData {
            timestamp: header.timestamp,
            sequence_number: header.sequence_number,
            rssi,
            csi,
        }))
    }
}

/// Converts a dB value to its corresponding linear scale.
///
/// # Arguments
///
/// * `x` - Value in dB.
///
/// # Returns
///
/// Linear-scale value.
fn dbinv(x: f64) -> f64 {
    10f64.powf(x / 10.0)
}

/// Computes total received signal strength (RSS) in dBm.
///
/// This function uses an empirical offset to match Intel 5300 measurements as described by
/// Daniel Halperin in:
/// > https://github.com/dhalperi/linux-80211n-csitool-supplementary/blob/master/matlab/get_total_rss.m
/// > https://dhalperi.github.io/linux-80211n-csitool/faq.html
///
/// # Arguments
///
/// * `rssi` - RSSI vector from the NIC.
/// * `agc` - Automatic Gain Control value.
///
/// # Returns
///
/// Total RSS in dBm.
fn get_total_rss(rssi: &[u16], agc: u8) -> f64 {
    let rssi_mag: f64 = rssi
        .iter()
        .map(|&r| if r != 0 { dbinv(r as f64) } else { 0.0 })
        .sum();
    rssi_mag.log10() * 10.0 - 44.0 - agc as f64
}

/// Scales the CSI matrix using RSSI, noise, and AGC information.
///
/// The scaling process adjusts CSI values to match their actual received power levels,
/// accounting for thermal noise, quantization error, and transmitter diversity.
///
/// # Arguments
///
/// * `csi` - Mutable reference to the CSI matrix (Tx x Rx x Subcarrier).
/// * `rssi` - RSSI values for each receive antenna.
/// * `noise` - Noise floor reported by the NIC.
/// * `agc` - AGC level for the current capture.
/// * `ntx` - Number of transmit antennas.
/// * `nrx` - Number of receive antennas.
fn scale_csi(
    csi: &mut [Vec<Vec<Complex>>],
    rssi: &[u16],
    noise: i8,
    agc: u8,
    ntx: usize,
    nrx: usize,
) {
    // Calculate the total power of the CSI matrix
    let csi_pwr: f64 = csi
        .iter()
        .flat_map(|tx| tx.iter().flat_map(|rx| rx.iter().map(|val| val.norm_sqr())))
        .sum();

    // Compute RSSI power in linear scale
    let rssi_pwr = dbinv(get_total_rss(rssi, agc));

    // Calculate the scaling factor to match RSSI power with average CSI power
    let scale = rssi_pwr / (csi_pwr / NUM_SUBCARRIER as f64);

    // Convert noise level from dB to linear scale.
    // If no noise present, default to a constant magic constant noise floor
    let noise_db = if noise == -127 { -92.0 } else { noise as f64 };
    let thermal_noise_pwr = dbinv(noise_db);

    // Quantization error for CSI values (assuming Intel 6-bit ADC with +/- 1 quantization error)
    let quant_error_pwr = scale * (nrx * ntx) as f64;

    // Calculate the total noise and error power
    let total_noise_pwr = thermal_noise_pwr + quant_error_pwr;

    // Combine overall scaling factor with NTx-based adjustment in one operation
    let ntx_adjustment = match ntx {
        2 => 2f64,
        3 => 1.6788, // this is sqrt(dbinv(4.5)), intel's approximation to sqrt(3)
        _ => 1.0,
    };
    let overall_scale = (scale * ntx_adjustment / total_noise_pwr).sqrt();

    // Apply scaling to each CSI value in place
    for tx in csi.iter_mut() {
        for rx in tx.iter_mut() {
            for val in rx.iter_mut() {
                *val *= overall_scale;
            }
        }
    }
}
