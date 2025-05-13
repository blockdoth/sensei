// esp32.rs — Robust ESP32 CSI client

use std::io::{self, Read, Write};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use bincode::de;
use byteorder::{LittleEndian, ReadBytesExt};
use log::{debug, error, info, warn};
use serialport::{ClearBuffer, SerialPort};
use thiserror::Error;

/// Operation mode: Rx or Tx
#[derive(Clone, Copy, Debug)]
#[repr(u8)]
pub enum OperationMode { Rx = 0x00, Tx = 0x01 }

/// Secondary channel for 40 MHz
#[derive(Clone, Copy, Debug)]
#[repr(u8)]
pub enum SecondaryChannel { No = 0x00, Below = 0x01, Above = 0x02 }

/// CSI type selection
#[derive(Clone, Copy, Debug)]
#[repr(u8)]
pub enum CsiType { LegacyLtf = 0x00, HtLtf = 0x01 }

/// Commands recognized by the ESP32 firmware
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(u8)]
pub enum Command {
    SetChannel = 0x01,
    PauseAcquisition = 0x04,
    UnpauseAcquisition = 0x05,
    ApplyDeviceConfig = 0x06,
    PauseWifiTx = 0x07,
    ResumeWifiTx = 0x08,
    TransmitCustomWifiFrame = 0x09,
}

const CMD_PREAMBLE: [u8; 4] = [0xC3; 4];
const PACKET_PREAMBLE: [u8; 8] = [0xAA; 8];
const OUTER_HEADER_LEN: usize = 8 + 2;
const CMD_FRAME_LEN: usize = 128;
const MAX_RETRIES: usize = 3;
const RETRY_DELAY: Duration = Duration::from_millis(200);

#[derive(Debug, Error)]
pub enum EspError {
    #[error("Serial port error: {0}")]
    Serial(#[from] serialport::Error),
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("Timeout waiting for ACK")]
    AckTimeout,
    #[error("Malformed packet: {0}")]
    MalformedPacket(&'static str),
    #[error("Channel error")]
    ChannelError,
    #[error("Mutex lock error")]
    MutexError,
}

/// Parsed CSI packet
#[derive(Clone, Debug)]
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

#[derive(Debug)]
pub enum EspStatus {
    Connected,
    Disconnected(String),
    Error(String),
}

pub struct Esp32 {
    port: Arc<Mutex<Box<dyn SerialPort + Send>>>,
    csi_rx: Receiver<CsiPacket>,
    ack_rx: Receiver<Vec<u8>>,
    status_rx: Receiver<EspStatus>,
    stop_flag: Arc<AtomicBool>,
    reader_thread: Option<thread::JoinHandle<()>>,
}

impl Esp32 {
    pub fn open(port_name: &str) -> Result<Self, EspError> {
        if !std::path::Path::new(port_name).exists() {
            return Err(EspError::Serial(serialport::Error::new(
                serialport::ErrorKind::NoDevice,
                "Port not found",
            )));
        }

        let mut raw: Box<dyn SerialPort + 'static> = serialport::new(port_name, 3_000_000)
            .timeout(Duration::from_millis(1))
            .open()?;

        raw.write_request_to_send(false)?;
        thread::sleep(Duration::from_millis(100));
        raw.write_data_terminal_ready(false)?;
        thread::sleep(Duration::from_millis(100));
        raw.clear(ClearBuffer::All)?;

        // random debug print

        let port: Arc<Mutex<Box<(dyn SerialPort + Send + 'static)>>> = Arc::new(Mutex::new(raw));
        let (csi_tx, csi_rx) = mpsc::channel();
        let (ack_tx, ack_rx) = mpsc::channel();
        let (status_tx, status_rx) = mpsc::channel();
        let stop_flag = Arc::new(AtomicBool::new(false));

        let reader = Self::spawn_reader(
            Arc::clone(&port),
            csi_tx,
            ack_tx,
            status_tx,
            Arc::clone(&stop_flag),
        );

        info!("ESP32 connected on {}", port_name);
        Ok(Esp32 {
            port,
            csi_rx,
            ack_rx,
            status_rx,
            stop_flag,
            reader_thread: Some(reader),
        })
    }

    pub fn check_status(&self) -> Option<EspStatus> {
        self.status_rx.try_recv().ok()
    }

    pub fn close(&mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        if let Some(h) = self.reader_thread.take() {
            if let Err(e) = h.join() {
                error!("Reader join error: {:?}", e);
            }
        }
        if let Ok(mut p) = self.port.lock() {
            let _ = p.clear(ClearBuffer::All);
        }
        info!("ESP32 client shut down");
    }

    pub fn send_command(&self, cmd: Command, payload: &[u8]) -> Result<(), EspError> {

        assert!(payload.len() <= CMD_FRAME_LEN - CMD_PREAMBLE.len() - 1);
        let mut frame = [0u8; CMD_FRAME_LEN];
        frame[..CMD_PREAMBLE.len()].copy_from_slice(&CMD_PREAMBLE);
        frame[CMD_PREAMBLE.len()] = cmd as u8;
        frame[CMD_PREAMBLE.len()+1..CMD_PREAMBLE.len()+1+payload.len()].copy_from_slice(payload);

        for _ in 0..MAX_RETRIES {
            // 1) flush stale RX bytes
            {
                let mut port = self.port.lock().map_err(|_| EspError::MutexError)?;
                port.clear(ClearBuffer::Input)?;
                port.write_all(&frame)?;
            }
            
            //let s = self.wait_ack(cmd, payload, COMMAND_TIMEOUT)?;
            thread::sleep(Duration::from_millis(50));

            let s = true; // TODO: remove this line
            if s {
                return Ok(())
            }
            thread::sleep(RETRY_DELAY);
        }
        Err(EspError::AckTimeout)
    }

    fn wait_ack(&self, cmd: Command, payload: &[u8], timeout: Duration)
        -> Result<bool, EspError>
    {
        let deadline = Instant::now() + timeout;
        // check every 20 ms, so we can bail quickly on a good ACK
        let per_recv = Duration::from_millis(20);
        while Instant::now() < deadline {
            match self.ack_rx.recv_timeout(per_recv) {
                Ok(frame) => {
                    if frame.len() >= OUTER_HEADER_LEN + 1
                        && frame[OUTER_HEADER_LEN] == cmd as u8
                        && frame.len() >= OUTER_HEADER_LEN + 1 + payload.len()
                        && &frame[OUTER_HEADER_LEN+1..OUTER_HEADER_LEN+1+payload.len()] == payload
                    {
                        return Ok(true);
                    }
                    // otherwise just loop again and pick up the next frame
                }
                Err(RecvTimeoutError::Timeout) => {
                    // nothing new—just try again until the deadline
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(EspError::ChannelError);
                }
            }
        }
        Ok(false)
    }

    fn spawn_reader(
        port: Arc<Mutex<Box<dyn SerialPort + Send>>>,
        csi_tx: Sender<CsiPacket>,
        ack_tx: Sender<Vec<u8>>,
        status_tx: Sender<EspStatus>,
        stop_flag: Arc<AtomicBool>,
    ) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            let mut buf = Vec::with_capacity(4096);
            let mut tmp = [0u8; 1024];

            while !stop_flag.load(Ordering::SeqCst) {
                
                // 1) lock-read in a small block
                let read_res = {
                    let mut guard = match port.lock() {
                        Ok(g) => g,
                        Err(_) => {
                            let _ = status_tx.send(EspStatus::Error("Lock failure".into()));
                            break;
                        }
                    };
                    guard.read(&mut tmp)
                };  // ← guard is dropped here, regardless of whether `read` timed out

                // 2) handle the result *after* dropping the lock
                let n = match read_res {
                    Ok(0) => {
                        status_tx.send(EspStatus::Disconnected("Closed".into())).ok();
                        thread::sleep(Duration::from_millis(50));
                        continue;
                    }
                    Ok(n) => n,
                    Err(ref e) if e.kind() == io::ErrorKind::TimedOut => continue,
                    Err(e) => {
                        status_tx.send(EspStatus::Disconnected(format!("{}", e))).ok();
                        break;
                    }
                };

                buf.extend_from_slice(&tmp[..n]);

                while buf.len() >= OUTER_HEADER_LEN {
                    if let Some(pos) = buf.windows(PACKET_PREAMBLE.len()).position(|w| w == PACKET_PREAMBLE) {
                        if buf.len() < pos + OUTER_HEADER_LEN { break; }
                        let raw_len = i16::from_le_bytes([
                            buf[pos+8], buf[pos+9]
                        ]);
                        let total = raw_len.abs() as usize;
                        if buf.len() < pos + total { break; }
                        let packet = buf.drain(..pos+total).collect::<Vec<u8>>();
                        if raw_len < 0 {
                            let _ = ack_tx.send(packet);
                        } else if let Ok(csi) = Self::parse_csi(&packet) {
                            let _ = csi_tx.send(csi);
                        }
                        continue;
                    }
                    // no preamble; drop all but tail
                    let keep = PACKET_PREAMBLE.len() - 1;
                    if buf.len() > keep { buf.drain(..buf.len()-keep); }
                    break;
                }
            }
            let _ = status_tx.send(EspStatus::Disconnected("Reader exit".into()));
        })
    }

    fn parse_csi(frame: &[u8]) -> Result<CsiPacket, EspError> {
        let mut cur = &frame[PACKET_PREAMBLE.len()..];
        let _ = cur.read_i16::<LittleEndian>()?;
        let ts = cur.read_u64::<LittleEndian>()?;
        let mut src = [0;6]; cur.read_exact(&mut src)?;
        let mut dst = [0;6]; cur.read_exact(&mut dst)?;
        let seq = cur.read_u16::<LittleEndian>()?;
        let rssi = cur.read_i8()?;
        let agc = cur.read_u8()?;
        let fft = cur.read_u8()?;
        let len = cur.read_u16::<LittleEndian>()?;
        let mut data = Vec::with_capacity(len as usize);
        for _ in 0..len {
            data.push(cur.read_i8()?);
        }
        Ok(CsiPacket {
            timestamp_us: ts,
            src_mac: src,
            dst_mac: dst,
            seq,
            rssi,
            agc,
            fft_gain: fft,
            csi_data: data,
        })
    }

    pub fn csi_rx(&self) -> &Receiver<CsiPacket> {
        &self.csi_rx
    }
}

impl Drop for Esp32 {
    fn drop(&mut self) { self.close(); }
}