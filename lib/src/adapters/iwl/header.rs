use std::time::{SystemTime, UNIX_EPOCH};

// use std::u16;
use crate::errors::IwlAdapterError;

/// Represents the parsed header of an Intel Wireless Link (IWL) CSI (Channel State Information) packet.
///
/// This struct contains metadata extracted from the CSI header including sequence numbers,
/// antenna configuration, RSSI (Received Signal Strength Indicator), and more.
#[derive(Debug)]
pub struct IwlHeader {
    /// Wall-clock timestamp (in seconds with microsecond precision) when the header was parsed.
    pub timestamp: f64,
    /// 12-bit sequence number identifying the frame.
    pub sequence_number: u16,
    /// Number of receiving antennas (max 3).
    pub nrx: usize,
    /// Number of transmitting antennas or spatial streams (max 3)
    pub ntx: usize,
    /// Received signal strength indicators for each antenna (up to 3).
    pub rssi: Vec<u16>,
    /// Noise floor measurement (in dBm).
    pub noise: i8,
    /// Automatic gain control (AGC) level.
    pub agc: u8,
    /// Antenna permutation array. Maps raw CSI streams to actual antennas.
    pub perm: [usize; 3],
}

impl IwlHeader {
    /// Parses a raw byte buffer containing an IWL CSI header and returns a tuple of the parsed
    /// `IwlHeader` and the payload slice containing CSI matrix data.
    ///
    /// # Arguments
    /// * `buf` - A byte slice expected to contain a valid IWL header followed by payload.
    ///
    /// # Returns
    /// * `Ok((IwlHeader, &[u8]))` - On successful parsing.
    /// * `Err(IwlAdapterError)` - If the buffer is malformed or fails validation.
    ///
    /// # Errors
    /// Returns an appropriate `IwlAdapterError` if:
    /// - The buffer is too short to contain a full header.
    /// - The netlink code is not the expected value (187).
    /// - The sequence number exceeds the 12-bit 802.11 spec limit.
    /// - The antenna count exceeds hardware limits (max 3 for rx/tx).
    /// - The reported payload size does not match calculated CSI matrix size.
    ///
    /// # Notes
    /// * The original timestamp from the hardware is discarded and replaced
    ///   with the current system time at parsing for cross-platform consistency.
    /// * CSI payload matrix size is verified based on NRX and NTX using the formula:
    ///   `(30 * (nrx * ntx * 8 * 2 + 3)) / 8`, rounded up.
    pub fn parse(buf: &[u8]) -> Result<(Self, &[u8]), IwlAdapterError> {
        if buf.len() < 21 {
            return Err(IwlAdapterError::IncompleteHeader);
        }

        // This is the netlink message code. We extract that first, since
        // the actual iwl header follows after.
        let code = buf[0];
        let buf = &buf[1..];

        // Parse fields from buffer
        // NOTE: The timestamp is useless. Its some internal time that only lasts for like 60 minutes
        let _timestamp = u32::from_le_bytes(buf[0..4].try_into().expect("length checked"));

        // Use current system time for timestamp (in seconds + microseconds).
        let ts = SystemTime::now().duration_since(UNIX_EPOCH).expect("Time went backwards");
        let timestamp = ts.as_secs() as f64 + ts.subsec_micros() as f64 / 1_000_000.0;

        let sequence_number = u16::from_le_bytes(buf[6..8].try_into().expect("length checked"));
        let nrx = buf[8] as usize;
        let ntx = buf[9] as usize;
        let rssi = vec![buf[10] as u16, buf[11] as u16, buf[12] as u16];
        let noise = buf[13] as i8;
        let agc = buf[14];
        let antenna_sel = buf[15];
        let len = u16::from_le_bytes(buf[16..18].try_into().expect("length checked")) as usize;

        if code != 187 {
            return Err(IwlAdapterError::InvalidCode(code));
        }

        // Confirm maximum value of sequence number according to 802.11
        if sequence_number > 4095 {
            return Err(IwlAdapterError::InvalidSequenceNumber(sequence_number));
        }

        // IWL has three receive antennas, and accordingly can handle a maximum
        // of three streams. Check that the header contains that range.
        if (ntx > 3) || (nrx > 3) {
            return Err(IwlAdapterError::InvalidAntennaSpec {
                num_rx: nrx,
                num_streams: ntx,
            });
        }

        // Extract payload and confirm it contains the amount of bytes
        // specified in the header.
        let payload = &buf[20..];

        if payload.len() < len {
            return Err(IwlAdapterError::IncompletePacket);
        }

        // Calculate expected CSI length based on NRX and NTX to validate
        // reported length matches.
        let calc_len = (30 * (nrx * ntx * 8 * 2 + 3)).div_ceil(8);
        if len != calc_len {
            return Err(IwlAdapterError::InvalidMatrixSize(len));
        }

        // Calculate permutation array based on antenna selection. According
        // to the original docs, the driver permutes CSI values in the order
        // of signal strengths. We undo this permutation for consistency.
        let perm = [
            (antenna_sel & 0x3) as usize,
            ((antenna_sel >> 2) & 0x3) as usize,
            ((antenna_sel >> 4) & 0x3) as usize,
        ];

        // Return header and the remaining payload slice
        Ok((
            IwlHeader {
                timestamp,
                sequence_number,
                nrx,
                ntx,
                rssi,
                noise,
                agc,
                perm,
            },
            payload,
        ))
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::IwlAdapterError;
    use crate::adapters::iwl::test_utils::build_test_packet;


    // Case where everything is alright
    #[test]
    fn test_valid_header_parse() {
        let buf = build_test_packet(187, 100, 2, 2, [40, 41, 42], -92, 7, 0b00011011, None);
        let res = IwlHeader::parse(&buf);

        assert!(res.is_ok());
    }

    // Case where the header is is not long enough
    #[test]
    fn test_incomplete_header() {
        let buf = vec![0; 5];
        assert!(matches!(IwlHeader::parse(&buf).unwrap_err(), IwlAdapterError::IncompleteHeader));
    }

    // Is not 187
    #[test]
    fn test_invalid_netlink_code() {
        let buf = build_test_packet(80, 100, 2, 2, [40, 41, 42], -92, 7, 0b00011011, None);
        assert!(matches!(IwlHeader::parse(&buf).unwrap_err(), IwlAdapterError::InvalidCode(80)));
    }

    // Is not bigger than 4095
    #[test]
    fn test_invalid_sequence_number() {
        let buf = build_test_packet(187, 5000, 2, 2, [40, 41, 42], -92, 7, 0b00011011, None);
        assert!(matches!(IwlHeader::parse(&buf).unwrap_err(), IwlAdapterError::InvalidSequenceNumber(5000)));
    }

    // Less than 3 streams 
    #[test]
    fn test_invalid_antenna_spec() {
        let buf = build_test_packet(187, 100, 4, 1, [40, 41, 42], -92, 7, 0b00011011, None);
        assert!(matches!(IwlHeader::parse(&buf).unwrap_err(), IwlAdapterError::InvalidAntennaSpec { num_rx: 4, num_streams: 1 }));
    }

    // Payload does not match header
    #[test]
    fn test_incomplete_packet_payload() {
        let buf = build_test_packet(187, 100, 2, 2, [40, 41, 42], -92, 7, 0b00011011, Some(100));
        let truncated_buf = &buf[0..buf.len() - 40]; 
        assert!(matches!(IwlHeader::parse(truncated_buf).unwrap_err(), IwlAdapterError::IncompletePacket));
    }

    // Expected CSI length based on NRX and NTX 
    // does not match reported length
    #[test]
    fn test_invalid_matrix_size() {
        let buf = build_test_packet(187, 100, 2, 2, [40, 41, 42], -92, 7, 0b00011011, Some(999));
        assert!(matches!(IwlHeader::parse(&buf).unwrap_err(),  IwlAdapterError::InvalidMatrixSize(999)));
    }
}
