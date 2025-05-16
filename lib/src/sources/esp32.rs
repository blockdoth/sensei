use crate::errors::{ControllerError, DataSourceError}; // Ensure ControllerError is accessible
use crate::sources::DataSourceT;
use crate::sources::controllers::esp32_controller::Esp32Command;

use std::any::Any;
use std::collections::HashMap;
use std::io::{Read as StdRead, Write as StdWrite}; // Renamed to avoid ambiguity
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering as AtomicOrdering},
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use byteorder::{LittleEndian, ReadBytesExt as _}; // Use _ to import extension methods
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, bounded};
use log::{debug, error, info, warn};
use serialport::{ClearBuffer, SerialPort};

const CMD_PREAMBLE_HOST_TO_ESP: [u8; 4] = [0xC3; 4];
const CMD_PACKET_TOTAL_SIZE_HOST_TO_ESP: usize = 128;
const ESP_PACKET_PREAMBLE_ESP_TO_HOST: [u8; 8] = [0xAA; 8];
const ESP_TO_HOST_MIN_HEADER_SIZE: usize = ESP_PACKET_PREAMBLE_ESP_TO_HOST.len() + 2;

const DEFAULT_ACK_TIMEOUT_MS: u64 = 2000; // Increased slightly
const DEFAULT_CSI_BUFFER_SIZE: usize = 100; // Reduced from 1000 to be more conservative
const SERIAL_READ_TIMEOUT_MS: u64 = 100;
const SERIAL_READ_BUFFER_SIZE: usize = 4096; // Increased for potentially larger bursts

// --- Type Aliases for `ack_waiters` ---
type AckPayload = Result<Vec<u8>, ControllerError>;
type AckSender = Sender<AckPayload>;
type AckWaiterMap = HashMap<u8, AckSender>;
type SharedAckWaiters = Arc<Mutex<AckWaiterMap>>;
// --- End Type Aliases ---

#[derive(serde::Deserialize, Debug, Clone)]
pub struct Esp32SourceConfig {
    pub port_name: String,
    pub baud_rate: u32,
    pub csi_buffer_size: Option<usize>,
    pub ack_timeout_ms: Option<u64>,
}

pub struct Esp32Source {
    config: Esp32SourceConfig,
    port: Arc<Mutex<Option<Box<dyn SerialPort>>>>,
    is_running: Arc<AtomicBool>,
    reader_handle: Option<JoinHandle<()>>,
    csi_data_rx: Receiver<Vec<u8>>,
    csi_data_tx: Sender<Vec<u8>>,
    ack_waiters: SharedAckWaiters,
}

impl Esp32Source {
    pub fn new(config: Esp32SourceConfig) -> Result<Self, DataSourceError> {
        let buffer_size = config.csi_buffer_size.unwrap_or(DEFAULT_CSI_BUFFER_SIZE);
        if buffer_size == 0 {
            return Err(DataSourceError::Controller(
                "CSI buffer size cannot be zero.".to_string(),
            ));
        }
        let (csi_data_tx, csi_data_rx) = bounded(buffer_size);
        Ok(Self {
            config,
            port: Arc::new(Mutex::new(None)),
            is_running: Arc::new(AtomicBool::new(false)),
            reader_handle: None,
            csi_data_tx,
            csi_data_rx,
            ack_waiters: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    // Method called by Esp32Controller
    pub async fn send_esp32_command(
        &mut self, // Changed to &mut self as it modifies ack_waiters
        cmd: Esp32Command,
        data: Option<Vec<u8>>,
    ) -> Result<Vec<u8>, ControllerError> {
        if !self.is_running.load(AtomicOrdering::SeqCst) {
            return Err(ControllerError::Execution(
                "ESP32 source is not running. Cannot send command.".to_string(),
            ));
        }

        let mut command_packet = vec![0u8; CMD_PACKET_TOTAL_SIZE_HOST_TO_ESP];
        command_packet[0..CMD_PREAMBLE_HOST_TO_ESP.len()]
            .copy_from_slice(&CMD_PREAMBLE_HOST_TO_ESP);

        let cmd_byte_offset = CMD_PREAMBLE_HOST_TO_ESP.len();
        command_packet[cmd_byte_offset] = cmd as u8;
        let data_offset = cmd_byte_offset + 1;

        if let Some(d) = data {
            let max_data_len = CMD_PACKET_TOTAL_SIZE_HOST_TO_ESP - data_offset;
            if d.len() > max_data_len {
                return Err(ControllerError::InvalidParams(format!(
                    "Command data too large: {} bytes (max {})",
                    d.len(),
                    max_data_len
                )));
            }
            command_packet[data_offset..data_offset + d.len()].copy_from_slice(&d);
        }

        let (ack_tx_local, ack_rx_local): (AckSender, Receiver<AckPayload>) = bounded(1);
        {
            let mut waiters = self
                .ack_waiters
                .lock()
                .map_err(|_| ControllerError::Execution("ACK waiter lock poisoned".to_string()))?;
            if waiters.insert(cmd as u8, ack_tx_local).is_some() {
                warn!(
                    "Overwriting ACK waiter for command {cmd:?}. This might indicate a logic issue or a previous command timed out without cleanup."
                );
            }
        }

        let port_clone = Arc::clone(&self.port);
        // Use tokio::task::spawn_blocking for the synchronous serial write
        tokio::task::spawn_blocking(move || {
            let mut port_guard = port_clone
                .lock()
                .map_err(|_| std::io::Error::other("Port lock poisoned"))?;
            if let Some(port_ref) = port_guard.as_mut() {
                debug!(
                    "Sending command {:?} to ESP32. Packet size: {}",
                    cmd,
                    command_packet.len()
                );
                port_ref.write_all(&command_packet)?;
                port_ref.flush()?;
                Ok(())
            } else {
                Err(std::io::Error::new(
                    std::io::ErrorKind::NotConnected,
                    "Serial port not open for sending command",
                ))
            }
        })
        .await
        .map_err(|e| {
            ControllerError::Execution(format!("Task for serial write panicked: {e}"))
        })??;
        // First ? for JoinError, second ? for std::io::Error

        let ack_timeout =
            Duration::from_millis(self.config.ack_timeout_ms.unwrap_or(DEFAULT_ACK_TIMEOUT_MS));

        let ack_result =
            tokio::task::spawn_blocking(move || ack_rx_local.recv_timeout(ack_timeout))
                .await
                .map_err(|e| {
                    ControllerError::Execution(format!("Task for ACK receive panicked: {e}"))
                })?;

        // Ensure waiter is removed
        self.ack_waiters
            .lock()
            .map_err(|_| {
                ControllerError::Execution("ACK waiter lock poisoned during cleanup".to_string())
            })?
            .remove(&(cmd as u8));

        match ack_result {
            Ok(ack_payload_result) => {
                // ack_payload_result is Result<Vec<u8>, ControllerError>
                match ack_payload_result {
                    Ok(ack_data) => {
                        info!("ACK received for command: {cmd:?}");
                        Ok(ack_data)
                    }
                    Err(e) => {
                        // Error explicitly sent from reader (e.g., ESP32 reported error)
                        error!("ESP32 reported an error in ACK for command {cmd:?}: {e}");
                        Err(e)
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                error!("Timeout waiting for ACK for command: {cmd:?}");
                Err(ControllerError::Execution(format!(
                    "ACK timeout for ESP32 command {cmd:?}"
                )))
            }
            Err(RecvTimeoutError::Disconnected) => {
                error!(
                    "ACK channel disconnected for command: {cmd:?}. Reader thread might have died."
                );
                Err(ControllerError::Execution(
                    "ESP32 ACK channel disconnected; reader thread likely terminated.".to_string(),
                ))
            }
        }
    }

    fn reader_task_loop(
        port_arc: Arc<Mutex<Option<Box<dyn SerialPort>>>>,
        is_running: Arc<AtomicBool>,
        csi_data_tx: Sender<Vec<u8>>,
        ack_waiters: SharedAckWaiters,
    ) {
        info!("ESP32Source reader task started.");
        let mut partial_buffer = Vec::with_capacity(SERIAL_READ_BUFFER_SIZE * 2);

        while is_running.load(AtomicOrdering::Relaxed) {
            let bytes_read_from_port = {
                let mut port_guard = match port_arc.lock() {
                    Ok(guard) => guard,
                    Err(_) => {
                        error!("Reader task: Port mutex poisoned. Terminating.");
                        break;
                    }
                };
                if let Some(port) = port_guard.as_mut() {
                    let mut temp_read_buf = [0u8; SERIAL_READ_BUFFER_SIZE];
                    match port.read(&mut temp_read_buf) {
                        Ok(0) => {
                            // Depending on OS/driver, 0 bytes might mean disconnect or just no data with timeout
                            // Given serialport timeout, this often means no data. Let timeout handle it.
                            // If it consistently returns 0, it might be a closed port.
                            // For robustness, treat as "no data" unless it persists.
                            // warn!("Serial port read 0 bytes. ESP32 might have disconnected or no data.");
                            thread::sleep(Duration::from_millis(10)); // Small pause if 0 bytes read
                            Ok(vec![]) // Treat as no new data
                        }
                        Ok(n) => Ok(temp_read_buf[..n].to_vec()),
                        Err(e) if e.kind() == std::io::ErrorKind::TimedOut => Ok(Vec::new()),
                        Err(e) => {
                            error!("Serial read error in reader task: {e}. Terminating.");
                            Err(e) // Fatal error for this iteration, will break loop
                        }
                    }
                } else {
                    // Port not open or has been taken.
                    if !is_running.load(AtomicOrdering::Relaxed) {
                        break;
                    } // Exit if stopping
                    thread::sleep(Duration::from_millis(200)); // Wait for port to become available or stop signal
                    continue; // Re-check conditions
                }
            };

            let mut new_data_was_read_in_this_iteration = false; // Track if new data was actually read

            match bytes_read_from_port {
                Ok(new_bytes) => {
                    if !new_bytes.is_empty() {
                        partial_buffer.extend_from_slice(&new_bytes);
                        new_data_was_read_in_this_iteration = true; // Mark that we got new data
                    }
                    // new_bytes is dropped here if it was Ok
                }
                Err(_) => {
                    // Serial read error, break the loop.
                    is_running.store(false, AtomicOrdering::Relaxed); // Signal termination
                    break;
                }
            }

            // Process buffer logic (simplified from before, but similar principle)
            loop {
                if partial_buffer.len() < ESP_TO_HOST_MIN_HEADER_SIZE {
                    break;
                }

                if let Some(pos) = partial_buffer
                    .windows(ESP_PACKET_PREAMBLE_ESP_TO_HOST.len())
                    .position(|w| w == ESP_PACKET_PREAMBLE_ESP_TO_HOST)
                {
                    if pos > 0 {
                        partial_buffer.drain(0..pos);
                    }
                    if partial_buffer.len() < ESP_TO_HOST_MIN_HEADER_SIZE {
                        break;
                    }

                    let len_bytes = &partial_buffer
                        [ESP_PACKET_PREAMBLE_ESP_TO_HOST.len()..ESP_TO_HOST_MIN_HEADER_SIZE];
                    let payload_len_signed = i16::from_le_bytes([len_bytes[0], len_bytes[1]]);

                    if payload_len_signed == 0 {
                        warn!("ESP32 packet with declared length 0. Discarding header.");
                        partial_buffer.drain(0..ESP_TO_HOST_MIN_HEADER_SIZE);
                        continue;
                    }

                    let actual_payload_len = payload_len_signed.unsigned_abs() as usize;
                    let total_packet_len_on_wire = ESP_TO_HOST_MIN_HEADER_SIZE + actual_payload_len;

                    if partial_buffer.len() < total_packet_len_on_wire {
                        break;
                    }

                    let packet_data_with_header = partial_buffer
                        .drain(0..total_packet_len_on_wire)
                        .collect::<Vec<_>>();
                    let actual_payload_content =
                        packet_data_with_header[ESP_TO_HOST_MIN_HEADER_SIZE..].to_vec();

                    if payload_len_signed < 0 {
                        // ACK
                        if actual_payload_content.is_empty() {
                            warn!("ACK packet with empty payload.");
                            continue;
                        }
                        let cmd_byte = actual_payload_content[0];
                        let ack_data = if actual_payload_content.len() > 1 {
                            actual_payload_content[1..].to_vec()
                        } else {
                            Vec::new()
                        };

                        let mut waiters_guard = match ack_waiters.lock() {
                            Ok(g) => g,
                            Err(_) => {
                                error!("ACK Waiter lock poisoned in reader");
                                continue;
                            }
                        };
                        if let Some(ack_tx_specific) = waiters_guard.remove(&cmd_byte) {
                            // Remove to consume the waiter
                            if let Err(e) = ack_tx_specific.try_send(Ok(ack_data)) {
                                warn!(
                                    "Failed to send ACK for cmd 0x{cmd_byte:02X} to specific waiter: {e}"
                                );
                            }
                        } else {
                            // This can happen if a command timed out and its waiter was removed, but ACK arrived late.
                            // Or if an unsolicited ACK is received.
                            // debug!("Received ACK for cmd 0x{:02X} but no specific waiter was registered.", cmd_byte);
                        }
                    } else {
                        // CSI Data
                        if csi_data_tx.is_full() {
                            warn!(
                                "CSI data channel full. Discarding ESP32 CSI packet. Consider increasing csi_buffer_size."
                            );
                        } else if let Err(e) = csi_data_tx.try_send(actual_payload_content) {
                            warn!("Failed to send CSI data to channel: {e}");
                        }
                    }
                } else {
                    // No preamble found
                    if partial_buffer.len() >= ESP_PACKET_PREAMBLE_ESP_TO_HOST.len() {
                        let discard_len =
                            partial_buffer.len() - (ESP_PACKET_PREAMBLE_ESP_TO_HOST.len() - 1);
                        partial_buffer.drain(0..discard_len);
                    }
                    break;
                }
            }

            // Sleep if the last read attempt yielded no new bytes AND the partial_buffer is still too small.
            // We use the boolean flag new_data_was_read_in_this_iteration
            if !new_data_was_read_in_this_iteration
                && partial_buffer.len() < ESP_TO_HOST_MIN_HEADER_SIZE
            {
                if !is_running.load(AtomicOrdering::Relaxed) {
                    break;
                } // Exit if stopping
                thread::sleep(Duration::from_millis(10));
            }
        }
        is_running.store(false, AtomicOrdering::Relaxed);
        // Clean up any remaining waiters on stop to prevent deadlocks if send_command is called later
        ack_waiters.lock().unwrap().clear();
        info!("ESP32Source reader task finished.");
    }
}

#[async_trait::async_trait]
impl DataSourceT for Esp32Source {
    async fn start(&mut self) -> Result<(), DataSourceError> {
        if self.is_running.load(AtomicOrdering::SeqCst) {
            info!("ESP32Source already running.");
            return Ok(());
        }
        info!(
            "Starting ESP32Source on port {} at {} baud.",
            self.config.port_name, self.config.baud_rate
        );

        let mut port = serialport::new(&self.config.port_name, self.config.baud_rate)
            .timeout(Duration::from_millis(SERIAL_READ_TIMEOUT_MS))
            .open()
            .map_err(|e| {
                // e is serialport::Error
                error!(
                    "Failed to open serial port {}: {}",
                    self.config.port_name, e
                );
                DataSourceError::from(e) // Automatically becomes DataSourceError::Serial(e)
            })?;

        port.clear(ClearBuffer::All)
            .map_err(DataSourceError::from)?;

        *self.port.lock().unwrap() = Some(port);
        self.is_running.store(true, AtomicOrdering::SeqCst);

        let port_clone = Arc::clone(&self.port);
        let is_running_clone = Arc::clone(&self.is_running);
        let csi_data_tx_clone = self.csi_data_tx.clone();
        let ack_waiters_clone = Arc::clone(&self.ack_waiters);

        let reader_handle = thread::Builder::new()
            .name("esp32-reader".to_string())
            .spawn(move || {
                Esp32Source::reader_task_loop(
                    port_clone,
                    is_running_clone,
                    csi_data_tx_clone,
                    ack_waiters_clone,
                );
            })
            .map_err(|e| {
                DataSourceError::Controller(format!("Failed to spawn ESP32 reader thread: {e}"))
            })?;

        self.reader_handle = Some(reader_handle);
        info!("ESP32Source started successfully.");
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), DataSourceError> {
        if !self.is_running.load(AtomicOrdering::SeqCst) && self.reader_handle.is_none() {
            info!("ESP32Source already stopped or never started.");
            return Ok(());
        }
        info!("Stopping ESP32Source...");
        self.is_running.store(false, AtomicOrdering::SeqCst);

        // Close/drop the port first to interrupt blocking reads in the reader thread
        if let Some(port_to_drop) = self.port.lock().unwrap().take() {
            drop(port_to_drop);
            debug!("Serial port explicitly dropped during stop.");
        }

        if let Some(handle) = self.reader_handle.take() {
            debug!("Waiting for ESP32 reader thread to join...");
            if let Err(e) = handle.join() {
                // This might block for a bit if thread is stuck
                error!("ESP32 reader thread panicked or failed to join cleanly: {e:?}");
            } else {
                debug!("ESP32 reader thread joined.");
            }
        }

        self.ack_waiters.lock().unwrap().clear(); // Clear any stale waiters
        info!("ESP32Source stopped.");
        Ok(())
    }

    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, DataSourceError> {
        if !self.is_running.load(AtomicOrdering::SeqCst) && self.csi_data_rx.is_empty() {
            // If explicitly stopped and no more data, could return a specific "EOS" or just Ok(0)
            debug!("ESP32Source not running and CSI buffer empty.");
            return Ok(0); // Or Err(DataSourceError::Controller("Source stopped".to_string()));
        }

        let csi_data_rx_clone = self.csi_data_rx.clone();
        let result_opt_payload = tokio::task::spawn_blocking(move || {
            match csi_data_rx_clone.recv_timeout(Duration::from_millis(1)) {
                // Very short timeout for non-blocking feel
                Ok(data_payload) => Ok(Some(data_payload)),
                Err(RecvTimeoutError::Timeout) => Ok(None),
                Err(RecvTimeoutError::Disconnected) => Err(DataSourceError::Controller(
                    "CSI data channel disconnected".to_string(),
                )),
            }
        })
        .await
        .map_err(|e| DataSourceError::Controller(format!("Read task panicked: {e}")))?; // Error from spawn_blocking join

        match result_opt_payload {
            Ok(Some(data_payload)) => {
                if data_payload.len() > buf.len() {
                    warn!(
                        "User buffer (len {}) too small for ESP32 CSI data (len {}). Data truncated.",
                        buf.len(),
                        data_payload.len()
                    );
                    buf.copy_from_slice(&data_payload[..buf.len()]);
                    Ok(buf.len())
                } else {
                    buf[..data_payload.len()].copy_from_slice(&data_payload);
                    Ok(data_payload.len())
                }
            }
            Ok(None) => Ok(0), // No data currently available
            Err(e) => Err(e),  // Error from recv (e.g. Disconnected)
        }
    }
}

impl Drop for Esp32Source {
    fn drop(&mut self) {
        // Best effort to stop the thread and release resources if not already stopped.
        if self.is_running.load(AtomicOrdering::Relaxed) || self.reader_handle.is_some() {
            info!("Dropping Esp32Source: signaling reader thread to stop.");
            self.is_running.store(false, AtomicOrdering::SeqCst);
            // Close port to help unblock reader thread
            if let Ok(mut port_guard) = self.port.lock() {
                if let Some(port_to_drop) = port_guard.take() {
                    drop(port_to_drop);
                }
            }
            // Joining thread in drop is generally discouraged as it can deadlock or panic.
            // The reader thread should observe `is_running` and exit.
            if let Some(handle) = self.reader_handle.take() {
                // Optionally, can try a timed join or just let it be if it's designed to exit quickly.
                // For now, we just take it. The thread should exit on its own.
                debug!("Reader thread handle taken in drop. Thread should self-terminate.");
            }
        }
    }
}
