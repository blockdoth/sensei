use std::time::{SystemTime, UNIX_EPOCH};
// use std::u16;
use errors::IwlAdapterError;

pub struct IwlHeader {
    pub timestamp: f64,
    pub sequence_number: u16,
    pub nrx: usize,
    pub ntx: usize,
    pub rssi: Vec<u16>,
    pub noise: i8,
    pub agc: u8,
    pub perm: [usize; 3],
}

impl IwlHeader {
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

        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards");
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
