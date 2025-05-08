//! A Rust ESP32 CSI client similar to the Python API
//!
//! Provides high-level methods to configure the device, stream CSI, and parse out clean CSI packets.

use serialport::SerialPort;
use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};
use byteorder::{LittleEndian, ReadBytesExt};

/// ESP32 operation mode
pub enum OperationMode {
    Rx = 0x00,
    Tx = 0x01,
}

/// Secondary channel setting for 40MHz
pub enum SecondaryChannel {
    No = 0x00,
    Below = 0x01,
    Above = 0x02,
}

/// CSI type selection
pub enum CsiType {
    LegacyLtf = 0x00,
    HtLtf = 0x01,
}

/// ESP32 commands (must match firmware)
#[derive(Clone, Copy)]
#[repr(u8)]
pub enum Command {
    SetChannel      = 0x01,
    WhitelistAdd    = 0x02,
    WhitelistClear  = 0x03,
    PauseAcq        = 0x04,
    UnpauseAcq      = 0x05,
    ApplyDeviceCfg  = 0x06,
    PauseWifiTx     = 0x07,
    ResumeWifiTx    = 0x08,
    TransmitCustom  = 0x09,
    SyncTimeInit    = 0x0A,
    SyncTimeApply   = 0x0B,
}

const PREAMBLE: [u8; 4] = [0xC3, 0xC3, 0xC3, 0xC3];
const FRAME_LEN: usize = 128;
const PACKET_PREAMBLE: [u8; 8] = [0xAA; 8];
const OUTER_HEADER_LEN: usize = 8 + 2; // preamble + i16 length

/// Parsed CSI packet
pub struct CsiPacket {
    pub timestamp_us: u64,
    pub src_mac: [u8; 6],
    pub dst_mac: [u8; 6],
    pub seq: u16,
    pub rssi: i8,
    pub agc: u8,
    pub fft_gain: u8,
    pub csi_data: Vec<i8>,
}

/// High-level ESP32 CSI client
pub struct Esp32 {
    port: Arc<Mutex<Box<dyn SerialPort + Send>>>,
    csi_rx: Receiver<CsiPacket>,
    ack_rx: Receiver<Vec<u8>>,
}

impl Esp32 {
    /// Open device on given port name at 3M baud
    pub fn open(path: &str) -> io::Result<Self> {
        let mut port: Box<dyn SerialPort + Send> = serialport::new(path, 3_000_000)
            .timeout(Duration::from_millis(50))
            .open()?;
        // use platform-neutral API for RTS/DTR
        port.write_request_to_send(false)?;
        port.write_data_terminal_ready(false)?;

        let port = Arc::new(Mutex::new(port));
        let (csi_tx, csi_rx) = mpsc::channel();
        let (ack_tx, ack_rx) = mpsc::channel();

        Self::spawn_reader(Arc::clone(&port), csi_tx, ack_tx);

        Ok(Esp32 { port, csi_rx, ack_rx })
    }

    /// Spawn background thread to parse incoming frames
    fn spawn_reader(
        port: Arc<Mutex<Box<dyn SerialPort + Send>>>,
        csi_tx: Sender<CsiPacket>,
        ack_tx: Sender<Vec<u8>>,
    ) {
        thread::spawn(move || {
            let mut buffer = Vec::new();
            let mut tmp = [0u8; 1024];
    
            loop {
                // read up to 1024 bytes
                let n = match port.lock().unwrap().read(&mut tmp) {
                    Ok(0) => continue,                                           // no data
                    Err(ref err) if err.kind() == io::ErrorKind::TimedOut => continue, // timeout
                    Ok(n) => n,                                                 // got n > 0
                    Err(_) => break,                                            // fatal error
                };
                buffer.extend_from_slice(&tmp[..n]);
    
                // look for packet preamble
                while let Some(idx) = buffer
                    .windows(PACKET_PREAMBLE.len())
                    .position(|w| w == PACKET_PREAMBLE)
                {
                    // do we have enough for the outer header?
                    if buffer.len() < idx + OUTER_HEADER_LEN {
                        break;
                    }
    
                    // grab length bytes by indexing
                    let lo = buffer[idx + PACKET_PREAMBLE.len()];
                    let hi = buffer[idx + PACKET_PREAMBLE.len() + 1];
                    let raw_len = i16::from_le_bytes([lo, hi]);
                    let total_len = raw_len.abs() as usize;
    
                    // do we have the full frame?
                    if buffer.len() < idx + total_len {
                        break;
                    }
    
                    // extract and drain
                    let frame = buffer[idx..idx + total_len].to_vec();
                    buffer.drain(..idx + total_len);
    
                    // dispatch to ack or CSI parse
                    if raw_len < 0 {
                        let _ = ack_tx.send(frame);
                    } else if let Ok(pkt) = Self::parse_csi(&frame) {
                        let _ = csi_tx.send(pkt);
                    }
                }
            }
        });
    }

    /// Parse full CSI frame bytes into structured packet
    fn parse_csi(frame: &[u8]) -> Result<CsiPacket, &'static str> {
        let mut rdr = &frame[..];
        // skip preamble
        rdr = &rdr[8..];
        let _length = rdr.read_u16::<LittleEndian>().map_err(|_| "len")?;
        let timestamp = rdr.read_u64::<LittleEndian>().map_err(|_| "ts")?;
        let mut src = [0u8; 6]; rdr.read_exact(&mut src).map_err(|_| "smac")?;
        let mut dst = [0u8; 6]; rdr.read_exact(&mut dst).map_err(|_| "dmac")?;
        let seq = rdr.read_u16::<LittleEndian>().map_err(|_| "seq")?;
        let rssi = rdr.read_i8().map_err(|_| "rssi")?;
        let agc = rdr.read_u8().map_err(|_| "agc")?;
        let fft = rdr.read_u8().map_err(|_| "fft")?;
        let csi_len = rdr.read_u16::<LittleEndian>().map_err(|_| "csi_len")?;
        let mut csi = Vec::with_capacity(csi_len as usize);
        for _ in 0..csi_len {
            csi.push(rdr.read_i8().map_err(|_| "csi data")?);
        }
        Ok(CsiPacket {
            timestamp_us: timestamp,
            src_mac: src,
            dst_mac: dst,
            seq,
            rssi,
            agc,
            fft_gain: fft,
            csi_data: csi,
        })
    }

    /// Send a command (128-byte frame)
    pub fn send_command(&self, cmd: Command, payload: &[u8]) -> io::Result<()> {
        assert!(payload.len() < 123, "Payload too large");
        let mut frame = [0u8; FRAME_LEN];
        frame[..4].copy_from_slice(&PREAMBLE);
        frame[4] = cmd as u8;
        frame[5..5 + payload.len()].copy_from_slice(payload);
        self.port.lock().unwrap().write_all(&frame)?;
        Ok(())
    }

    /// Wait for ack of a given command within timeout
    pub fn wait_ack(&self, cmd: Command, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Ok(frame) = self.ack_rx.recv_timeout(Duration::from_millis(10)) {
                if frame.get(OUTER_HEADER_LEN).map(|b| *b == cmd as u8) == Some(true) {
                    return true;
                }
            }
        }
        false
    }

    /// Configure device for CSI: channel, mode, bw, type
    pub fn setup_csi(&self, channel: u8, bw20: bool, csi_type: CsiType) -> io::Result<()> {
        let mode = OperationMode::Rx as u8;
        let bw = if bw20 { 0 } else { 1 };
        let sec = SecondaryChannel::No as u8;
        let ct = csi_type as u8;
        let manu = 0u8;
        let config = [mode, bw, sec, ct, manu];
        self.send_command(Command::ApplyDeviceCfg, &config)?;
        thread::sleep(Duration::from_millis(50));
        self.send_command(Command::SetChannel, &[channel])?;
        thread::sleep(Duration::from_millis(50));
        self.send_command(Command::WhitelistClear, &[])?;
        thread::sleep(Duration::from_millis(20));
        self.send_command(Command::UnpauseAcq, &[])?;
        Ok(())
    }

    /// Consume CSI packets via callback (takes ownership)
    pub fn stream_csi<F: Fn(CsiPacket) + Send + 'static>(self, handler: F) {
        let rx = self.csi_rx;
        thread::spawn(move || {
            while let Ok(pkt) = rx.recv() {
                handler(pkt);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_frame_build() {
        let dev = Esp32::open("/dev/null").unwrap();
        dev.send_command(Command::PauseAcq, &[]).unwrap();
        // ensure no panic
    }
}