// Dummy packet
pub fn build_test_packet(
    code: u8,
    sequence_number: u16,
    nrx_ntx: [u8; 2],
    rssi: [u8; 3],
    noise: i8,
    agc_antenna_sel: [u8; 2],
    csi_len_override: Option<usize>,
) -> Vec<u8> {
    let mut buf = vec![];
    buf.push(code);
    buf.extend(&0u32.to_le_bytes());
    buf.extend(&0u16.to_le_bytes());
    buf.extend(&sequence_number.to_le_bytes());
    buf.push(nrx_ntx[0]);
    buf.push(nrx_ntx[1]);
    buf.extend(&rssi);
    buf.push(noise as u8);
    buf.push(agc_antenna_sel[0]);
    buf.push(agc_antenna_sel[1]);
    let nrx_usize = nrx_ntx[0] as usize;
    let ntx_usize = nrx_ntx[1] as usize;
    let csi_len = ((30 * (nrx_usize * ntx_usize * 8 * 2 + 3)) / 8) + 1;
    let len = csi_len_override.unwrap_or(csi_len);
    buf.extend(&(len as u16).to_le_bytes());

    while buf.len() < 21 {
        buf.push(0);
    }
    buf.extend(vec![0xAB; len]);

    buf
}
