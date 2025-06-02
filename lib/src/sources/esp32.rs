use std::any::Any;
use std::collections::HashMap;
use std::io::{Read as StdRead, Write as StdWrite}; // Renamed to avoid ambiguity
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use byteorder::{LittleEndian, ReadBytesExt as _}; // Use _ to import extension methods
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, bounded};
use log::{debug, error, info, trace, warn};
use serialport::{ClearBuffer, SerialPort};

use crate::ToConfig;
use crate::errors::{ControllerError, DataSourceError, TaskError}; // Ensure ControllerError is accessible
use crate::network::rpc_message::SourceType;
use crate::sources::controllers::esp32_controller::Esp32Command;
use crate::sources::{BUFSIZE, DataMsg, DataSourceConfig, DataSourceT};

const CMD_PREAMBLE_HOST_TO_ESP: [u8; 4] = [0xC3; 4];
const CMD_PACKET_TOTAL_SIZE_HOST_TO_ESP: usize = 128;
const ESP_PACKET_PREAMBLE_ESP_TO_HOST: [u8; 8] = [0xAA; 8];
const ESP_TO_HOST_MIN_HEADER_SIZE: usize = ESP_PACKET_PREAMBLE_ESP_TO_HOST.len() + 2;

const DEFAULT_ACK_TIMEOUT_MS: u64 = 2000; // Increased slightly
const DEFAULT_CSI_BUFFER_SIZE: usize = 100; // Reduced from 1000 to be more conservative
const SERIAL_READ_TIMEOUT_MS: u64 = 100;
const SERIAL_READ_BUFFER_SIZE: usize = 4096; // Increased for potentially larger bursts
const BAUDRATE: u32 = 3_000_000;

// --- Type Aliases for `ack_waiters` ---
type AckPayload = Result<Vec<u8>, ControllerError>;
type AckSender = Sender<AckPayload>;
type AckWaiterMap = HashMap<u8, AckSender>;
type SharedAckWaiters = Arc<Mutex<AckWaiterMap>>;
// --- End Type Aliases ---

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct Esp32SourceConfig {
    pub port_name: String,
    pub baud_rate: u32,
    pub csi_buffer_size: usize,
    pub ack_timeout_ms: u64,
}

impl Default for Esp32SourceConfig {
    fn default() -> Self {
        Self {
            port_name: "/dev/port".to_string(),
            baud_rate: BAUDRATE,
            csi_buffer_size: DEFAULT_CSI_BUFFER_SIZE,
            ack_timeout_ms: DEFAULT_ACK_TIMEOUT_MS,
        }
    }
}

pub struct Esp32Source {
    config: Esp32SourceConfig,
    pub port: Arc<Mutex<Option<Box<dyn SerialPort>>>>,
    pub is_running: Arc<AtomicBool>,
    reader_handle: Option<JoinHandle<()>>,
    csi_data_rx: Receiver<Vec<u8>>,
    csi_data_tx: Sender<Vec<u8>>,
    ack_waiters: SharedAckWaiters,
}

impl Esp32Source {
    pub fn new(config: Esp32SourceConfig) -> Result<Self, DataSourceError> {
        let buffer_size = config.csi_buffer_size;
        if buffer_size == 0 {
            return Err(DataSourceError::Controller("CSI buffer size cannot be zero.".to_string()));
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
        command_packet[0..CMD_PREAMBLE_HOST_TO_ESP.len()].copy_from_slice(&CMD_PREAMBLE_HOST_TO_ESP);

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
            let mut port_guard = port_clone.lock().map_err(|_| std::io::Error::other("Port lock poisoned"))?;
            if let Some(port_ref) = port_guard.as_mut() {
                debug!("Sending command {:?} to ESP32. Packet size: {}", cmd, command_packet.len());
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
        .map_err(|e| ControllerError::Execution(format!("Task for serial write panicked: {e}")))??;
        // First ? for JoinError, second ? for std::io::Error

        let ack_timeout = Duration::from_millis(self.config.ack_timeout_ms);

        let ack_result = tokio::task::spawn_blocking(move || ack_rx_local.recv_timeout(ack_timeout))
            .await
            .map_err(|e| ControllerError::Execution(format!("Task for ACK receive panicked: {e}")))?;

        // Ensure waiter is removed
        self.ack_waiters
            .lock()
            .map_err(|_| ControllerError::Execution("ACK waiter lock poisoned during cleanup".to_string()))?
            .remove(&(cmd as u8));

        match ack_result {
            Ok(ack_payload_result) => {
                // ack_payload_result is Result<Vec<u8>, ControllerError>
                match ack_payload_result {
                    Ok(ack_data) => {
                        debug!("ACK received for command: {cmd:?}");
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
                Err(ControllerError::Execution(format!("ACK timeout for ESP32 command {cmd:?}")))
            }
            Err(RecvTimeoutError::Disconnected) => {
                error!("ACK channel disconnected for command: {cmd:?}. Reader thread might have died.");
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
                    if !is_running.load(AtomicOrdering::Relaxed) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(200));
                    continue;
                }
            };

            let mut new_data_was_read_in_this_iteration = false;

            match bytes_read_from_port {
                Ok(new_bytes) => {
                    if !new_bytes.is_empty() {
                        partial_buffer.extend_from_slice(&new_bytes);
                        new_data_was_read_in_this_iteration = true;
                    }
                }
                Err(_) => {
                    is_running.store(false, AtomicOrdering::Relaxed);
                    break;
                }
            }

            loop {
                // Inner loop for processing partial_buffer
                if partial_buffer.len() < ESP_TO_HOST_MIN_HEADER_SIZE {
                    break;
                }

                if let Some(pos) = partial_buffer
                    .windows(ESP_PACKET_PREAMBLE_ESP_TO_HOST.len())
                    .position(|w| w == ESP_PACKET_PREAMBLE_ESP_TO_HOST)
                {
                    if pos > 0 {
                        debug!("Preamble found at pos {pos}. Discarding {pos} bytes before preamble.",);
                        partial_buffer.drain(0..pos);
                    }
                    if partial_buffer.len() < ESP_TO_HOST_MIN_HEADER_SIZE {
                        // Check again after drain
                        break;
                    }

                    let len_bytes = &partial_buffer[ESP_PACKET_PREAMBLE_ESP_TO_HOST.len()..ESP_TO_HOST_MIN_HEADER_SIZE];
                    let esp_length_field_val_signed = i16::from_le_bytes([len_bytes[0], len_bytes[1]]);
                    debug!("Raw length field from ESP32: {esp_length_field_val_signed}",);

                    if esp_length_field_val_signed == 0 {
                        warn!("ESP32 packet with declared length field 0. Discarding header to attempt recovery.");
                        partial_buffer.drain(0..std::cmp::min(ESP_TO_HOST_MIN_HEADER_SIZE, partial_buffer.len()));
                        continue;
                    }

                    let declared_length_from_esp = esp_length_field_val_signed.unsigned_abs() as usize;
                    debug!("Declared length from ESP32 (abs value of field): {declared_length_from_esp}",);

                    // *** ASSUMPTION CHANGE FOR THIS FIX: ***
                    // The value `declared_length_from_esp` IS the total length of the packet on the wire,
                    // including preamble and the length field itself.
                    let total_packet_len_on_wire = declared_length_from_esp;

                    // Sanity check: total length must be at least header size.
                    if total_packet_len_on_wire < ESP_TO_HOST_MIN_HEADER_SIZE {
                        error!(
                            "Declared total packet length {total_packet_len_on_wire} is less than minimum header size {ESP_TO_HOST_MIN_HEADER_SIZE}. Corrupted packet. Discarding.",
                        );
                        partial_buffer.drain(0..std::cmp::min(total_packet_len_on_wire, partial_buffer.len()));
                        continue;
                    }

                    if partial_buffer.len() < total_packet_len_on_wire {
                        debug!(
                            "Buffer (len {}) too short for full packet (need {} based on ESP field). Waiting for more.",
                            partial_buffer.len(),
                            total_packet_len_on_wire
                        );
                        break;
                    }

                    debug!("Extracting full packet of len: {total_packet_len_on_wire} from buffer",);
                    let packet_data_with_header = partial_buffer.drain(0..total_packet_len_on_wire).collect::<Vec<_>>();

                    // The actual payload is after the header
                    // This check ensures that we can safely slice the payload.
                    // If total_packet_len_on_wire was < ESP_TO_HOST_MIN_HEADER_SIZE, the previous check would have caught it.
                    // This mainly guards against total_packet_len_on_wire being exactly ESP_TO_HOST_MIN_HEADER_SIZE for non-ACKs,
                    // or if the drain somehow returned less than expected (though drain(0..X) should give X bytes if available).
                    if packet_data_with_header.len() < ESP_TO_HOST_MIN_HEADER_SIZE {
                        warn!(
                            "Drained packet (len {}) is smaller than header size ({}). Should not happen if previous checks passed. Skipping.",
                            packet_data_with_header.len(),
                            ESP_TO_HOST_MIN_HEADER_SIZE
                        );
                        continue;
                    }
                    let actual_payload_content = packet_data_with_header[ESP_TO_HOST_MIN_HEADER_SIZE..].to_vec();

                    let is_ack_packet = esp_length_field_val_signed < 0;

                    if is_ack_packet {
                        // ACK
                        if actual_payload_content.is_empty() {
                            // This implies total_packet_len_on_wire was exactly ESP_TO_HOST_MIN_HEADER_SIZE,
                            // and esp_length_field_val_signed was negative.
                            warn!(
                                "ACK packet payload is empty (command byte expected). Length field: {esp_length_field_val_signed}, Total packet len: {total_packet_len_on_wire}",
                            );
                            continue;
                        }
                        let cmd_byte = actual_payload_content[0];
                        let ack_data = if actual_payload_content.len() > 1 {
                            actual_payload_content[1..].to_vec()
                        } else {
                            Vec::new()
                        };

                        debug!("Received ACK for command byte 0x{:02X} with data len {}", cmd_byte, ack_data.len());
                        let mut waiters_guard = match ack_waiters.lock() {
                            Ok(g) => g,
                            Err(_) => {
                                error!("ACK Waiter lock poisoned in reader");
                                continue;
                            }
                        };
                        if let Some(ack_tx_specific) = waiters_guard.remove(&cmd_byte) {
                            if let Err(e) = ack_tx_specific.try_send(Ok(ack_data)) {
                                warn!("Failed to send ACK for cmd 0x{cmd_byte:02X} to specific waiter: {e}",);
                            } else {
                                debug!("Successfully sent ACK for 0x{cmd_byte:02X} to waiting task.",);
                            }
                        } else {
                            debug!("Received ACK for cmd 0x{cmd_byte:02X} but no specific waiter was registered.",);
                        }
                    } else {
                        // CSI Data
                        debug!(
                            "Received CSI Data packet. Payload len: {}. Sending to CSI channel.",
                            actual_payload_content.len()
                        );
                        if csi_data_tx.is_full() {
                            warn!("CSI data channel full. Discarding ESP32 CSI packet. Consider increasing csi_buffer_size.");
                        } else if let Err(e) = csi_data_tx.try_send(actual_payload_content) {
                            warn!("Failed to send CSI data to channel: {e}");
                        }
                    }
                } else {
                    // No preamble found
                    if partial_buffer.len() >= ESP_PACKET_PREAMBLE_ESP_TO_HOST.len() {
                        let discard_len = partial_buffer.len() - (ESP_PACKET_PREAMBLE_ESP_TO_HOST.len() - 1);
                        trace!(
                            "No preamble found, buffer len {}. Discarding {} bytes.",
                            partial_buffer.len(),
                            discard_len
                        );
                        partial_buffer.drain(0..discard_len);
                    }
                    break;
                }
            } // End inner loop

            if !new_data_was_read_in_this_iteration && partial_buffer.len() < ESP_TO_HOST_MIN_HEADER_SIZE {
                if !is_running.load(AtomicOrdering::Relaxed) {
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
        } // End while is_running

        is_running.store(false, AtomicOrdering::Relaxed);
        ack_waiters.lock().unwrap().clear();
        info!("ESP32Source reader task finished.");
    }
}

#[async_trait::async_trait]
impl DataSourceT for Esp32Source {
    async fn start(&mut self) -> Result<(), DataSourceError> {
        if self.is_running.load(AtomicOrdering::SeqCst) {
            warn!("ESP32Source already running.");
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
                error!("Failed to open serial port {}: {}", self.config.port_name, e);
                DataSourceError::from(e) // Automatically becomes DataSourceError::Serial(e)
            })?;

        port.clear(ClearBuffer::All).map_err(DataSourceError::from)?;

        *self.port.lock().unwrap() = Some(port);
        self.is_running.store(true, AtomicOrdering::SeqCst);

        let port_clone = Arc::clone(&self.port);
        let is_running_clone = Arc::clone(&self.is_running);
        let csi_data_tx_clone = self.csi_data_tx.clone();
        let ack_waiters_clone = Arc::clone(&self.ack_waiters);

        let reader_handle = thread::Builder::new()
            .name("esp32-reader".to_string())
            .spawn(move || {
                Esp32Source::reader_task_loop(port_clone, is_running_clone, csi_data_tx_clone, ack_waiters_clone);
            })
            .map_err(|e| DataSourceError::Controller(format!("Failed to spawn ESP32 reader thread: {e}")))?;

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

    async fn read_buf(&mut self, buf: &mut [u8]) -> Result<usize, DataSourceError> {
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
                Err(RecvTimeoutError::Disconnected) => Err(DataSourceError::Controller("CSI data channel disconnected".to_string())),
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

    async fn read(&mut self) -> Result<Option<DataMsg>, DataSourceError> {
        let mut temp_buf = vec![0u8; BUFSIZE];
        match self.read_buf(&mut temp_buf).await? {
            0 => Ok(None),
            n => Ok(Some(DataMsg::RawFrame {
                ts: chrono::Utc::now().timestamp_millis() as f64 / 1e3,
                bytes: temp_buf[..n].to_vec(),
                source_type: SourceType::ESP32,
            })),
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

#[async_trait::async_trait]
impl ToConfig<DataSourceConfig> for Esp32Source {
    /// Converts this `Esp32Source` instance into its configuration representation.
    ///
    /// This method creates a clone of the internal configuration and wraps it
    /// in the `DataSourceConfig::Esp32` variant, enabling serialization or
    /// saving the current source state as a configuration.
    ///
    /// # Returns
    ///
    /// * `Ok(DataSourceConfig::Esp32)` containing a clone of the `Esp32SourceConfig`.
    /// * `Err(TaskError)` if the conversion fails (unlikely as cloning should succeed).
    async fn to_config(&self) -> Result<DataSourceConfig, TaskError> {
        Ok(DataSourceConfig::Esp32(self.config.clone()))
    }
}
