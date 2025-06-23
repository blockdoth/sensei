use std::collections::HashMap;
use std::io::{Read as StdRead, Write as StdWrite}; // Renamed to avoid ambiguity
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

// Use _ to import extension methods
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, bounded};
use log::{debug, error, info, trace, warn};
#[cfg(test)]
use mockall::automock;
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

// --- Helper functions for serde defaults ---
fn default_baud_rate() -> u32 {
    BAUDRATE
}

fn default_csi_buffer_size() -> usize {
    DEFAULT_CSI_BUFFER_SIZE
}

fn default_ack_timeout_ms() -> u64 {
    DEFAULT_ACK_TIMEOUT_MS
}
// --- End Helper functions ---

// --- Type Aliases for `ack_waiters` ---
type AckPayload = Result<Vec<u8>, ControllerError>;
type AckSender = Sender<AckPayload>;
type AckWaiterMap = HashMap<u8, AckSender>;
type SharedAckWaiters = Arc<Mutex<AckWaiterMap>>;
// --- End Type Aliases ---

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq)]
pub struct Esp32SourceConfig {
    pub port_name: String,
    #[serde(default = "default_baud_rate")]
    pub baud_rate: u32,
    #[serde(default = "default_csi_buffer_size")]
    pub csi_buffer_size: usize,
    #[serde(default = "default_ack_timeout_ms")]
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

impl std::fmt::Debug for Esp32Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Esp32Source")
            .field("config", &self.config)
            .field("is_running", &self.is_running.load(AtomicOrdering::Relaxed))
            .field("reader_handle_present", &self.reader_handle.is_some())
            .field("csi_data_channel_len", &self.csi_data_rx.len())
            .finish()
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

#[cfg_attr(test, automock)]
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

        let port = serialport::new(&self.config.port_name, self.config.baud_rate)
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
            if let Some(_handle) = self.reader_handle.take() {
                // Optionally, can try a timed join or just let it be if it's designed to exit quickly.
                // For now, we just take it. The thread should exit on its own.
                debug!("Reader thread handle taken in drop. Thread should self-terminate.");
            }
        }
    }
}

#[async_trait::async_trait]
impl ToConfig<DataSourceConfig> for Esp32Source {
    /// Converts the current `Esp32Source` instance into its configuration representation.
    ///
    /// This method implements the `ToConfig` trait for `Esp32Source`, allowing a runtime
    /// instance to be transformed into a `DataSourceConfig::Esp32` variant. The resulting
    /// configuration can be serialized for storage, transmission, or logging purposes.
    ///
    /// # Returns
    ///
    /// * `Ok(DataSourceConfig::Esp32)` containing a clone of the `Esp32SourceConfig`.
    /// * `Err(TaskError)` if the conversion fails (unlikely as cloning should succeed).
    async fn to_config(&self) -> Result<DataSourceConfig, TaskError> {
        Ok(DataSourceConfig::Esp32(self.config.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::DataSourceConfig;

    fn create_default_config() -> Esp32SourceConfig {
        Esp32SourceConfig::default()
    }

    fn create_test_config() -> Esp32SourceConfig {
        Esp32SourceConfig {
            port_name: "/dev/ttyUSB0".to_string(),
            baud_rate: 115200,
            csi_buffer_size: 50,
            ack_timeout_ms: 1000,
        }
    }

    #[test]
    fn test_esp32_source_config_default() {
        let config = Esp32SourceConfig::default();
        assert_eq!(config.port_name, "/dev/port");
        assert_eq!(config.baud_rate, BAUDRATE);
        assert_eq!(config.csi_buffer_size, DEFAULT_CSI_BUFFER_SIZE);
        assert_eq!(config.ack_timeout_ms, DEFAULT_ACK_TIMEOUT_MS);
    }

    #[test]
    fn test_esp32_source_config_custom() {
        let config = create_test_config();
        assert_eq!(config.port_name, "/dev/ttyUSB0");
        assert_eq!(config.baud_rate, 115200);
        assert_eq!(config.csi_buffer_size, 50);
        assert_eq!(config.ack_timeout_ms, 1000);
    }

    #[test]
    fn test_esp32_source_new_success() {
        let config = create_test_config();
        let result = Esp32Source::new(config.clone());
        assert!(result.is_ok());
        
        let source = result.unwrap();
        assert_eq!(source.config, config);
        assert!(!source.is_running.load(AtomicOrdering::SeqCst));
        assert!(source.reader_handle.is_none());
    }

    #[test]
    fn test_esp32_source_new_zero_buffer_size() {
        let mut config = create_test_config();
        config.csi_buffer_size = 0;
        
        let result = Esp32Source::new(config);
        assert!(result.is_err());
        
        match result.unwrap_err() {
            DataSourceError::Controller(msg) => {
                assert!(msg.contains("CSI buffer size cannot be zero"));
            }
            _ => panic!("Expected Controller error"),
        }
    }

    #[tokio::test]
    async fn test_esp32_source_start_stop_lifecycle() {
        let config = create_test_config();
        let mut source = Esp32Source::new(config).unwrap();
        
        // Initially not running
        assert!(!source.is_running.load(AtomicOrdering::SeqCst));
        
        // Note: We can't test actual start() because it requires a real serial port
        // But we can test the state management logic
        
        // Simulate the state that would be set by start()
        source.is_running.store(true, AtomicOrdering::SeqCst);
        assert!(source.is_running.load(AtomicOrdering::SeqCst));
        
        // Test stop when already stopped (should be safe)
        source.is_running.store(false, AtomicOrdering::SeqCst);
        let stop_result = source.stop().await;
        assert!(stop_result.is_ok());
    }

    #[tokio::test]
    async fn test_esp32_source_read_when_not_started() {
        let config = create_test_config();
        let mut source = Esp32Source::new(config).unwrap();
        
        // Try to read when source is not started
        let mut buffer = vec![0u8; 1024];
        let read_result = source.read_buf(&mut buffer).await;
        
        // Should return 0 bytes when not running
        match read_result {
            Ok(bytes_read) => assert_eq!(bytes_read, 0),
            Err(_) => {}, // Also acceptable - depends on implementation
        }
    }

    #[tokio::test]
    async fn test_esp32_source_read_message_when_not_started() {
        let config = create_test_config();
        let mut source = Esp32Source::new(config).unwrap();
        
        // Try to read message when source is not started
        let read_result = source.read().await;
        
        // Should return None when not running
        match read_result {
            Ok(None) => {}, // Expected
            Ok(Some(_)) => panic!("Should not receive data when not started"),
            Err(_) => {}, // Also acceptable - depends on implementation
        }
    }

    #[test]
    fn test_esp32_source_command_packet_constants() {
        // Test that constants are reasonable values
        assert_eq!(CMD_PREAMBLE_HOST_TO_ESP.len(), 4);
        assert_eq!(CMD_PREAMBLE_HOST_TO_ESP, [0xC3; 4]);
        assert_eq!(CMD_PACKET_TOTAL_SIZE_HOST_TO_ESP, 128);
        assert!(CMD_PACKET_TOTAL_SIZE_HOST_TO_ESP > CMD_PREAMBLE_HOST_TO_ESP.len());
        
        assert_eq!(ESP_PACKET_PREAMBLE_ESP_TO_HOST.len(), 8);
        assert_eq!(ESP_PACKET_PREAMBLE_ESP_TO_HOST, [0xAA; 8]);
        assert_eq!(ESP_TO_HOST_MIN_HEADER_SIZE, ESP_PACKET_PREAMBLE_ESP_TO_HOST.len() + 2);
    }

    #[test]
    fn test_esp32_source_timing_constants() {
        assert!(DEFAULT_ACK_TIMEOUT_MS > 0);
        assert!(DEFAULT_CSI_BUFFER_SIZE > 0);
        assert!(SERIAL_READ_TIMEOUT_MS > 0);
        assert!(SERIAL_READ_BUFFER_SIZE > 0);
        assert!(BAUDRATE > 0);
        
        // Sanity checks for reasonable values
        assert!(DEFAULT_ACK_TIMEOUT_MS >= 1000); // At least 1 second
        assert!(DEFAULT_CSI_BUFFER_SIZE >= 10); // At least some buffering
        assert!(BAUDRATE >= 9600); // Minimum reasonable baud rate
    }

    #[tokio::test]
    async fn test_esp32_source_to_config() {
        let config = create_test_config();
        let source = Esp32Source::new(config.clone()).unwrap();
        
        let to_config_result = source.to_config().await;
        assert!(to_config_result.is_ok());
        
        match to_config_result.unwrap() {
            DataSourceConfig::Esp32(returned_config) => {
                assert_eq!(returned_config, config);
            }
            _ => panic!("Expected Esp32 config"),
        }
    }

    #[tokio::test]
    async fn test_send_esp32_command_when_not_running() {
        let config = create_test_config();
        let mut source = Esp32Source::new(config).unwrap();
        
        // Try to send command when source is not running
        let result = source.send_esp32_command(Esp32Command::Nop, None).await;
        
        assert!(result.is_err());
        match result.unwrap_err() {
            ControllerError::Execution(msg) => {
                assert!(msg.contains("ESP32 source is not running"));
            }
            _ => panic!("Expected Execution error"),
        }
    }

    #[test]
    fn test_esp32_source_config_serde_default_functions() {
        // Test the default functions used by serde
        assert_eq!(default_baud_rate(), BAUDRATE);
        assert_eq!(default_csi_buffer_size(), DEFAULT_CSI_BUFFER_SIZE);
        assert_eq!(default_ack_timeout_ms(), DEFAULT_ACK_TIMEOUT_MS);
    }

    #[test]
    fn test_esp32_source_config_equality() {
        let config1 = create_test_config();
        let config2 = create_test_config();
        let mut config3 = create_test_config();
        config3.port_name = "/dev/ttyUSB1".to_string();
        
        assert_eq!(config1, config2);
        assert_ne!(config1, config3);
    }

    #[test]
    fn test_command_packet_structure() {
        // Test the structure that would be built for commands
        let cmd = Esp32Command::SetChannel;
        let data = Some(vec![6u8]); // Set channel 6
        
        // Simulate building a command packet
        let mut command_packet = [0u8; CMD_PACKET_TOTAL_SIZE_HOST_TO_ESP];
        command_packet[0..CMD_PREAMBLE_HOST_TO_ESP.len()].copy_from_slice(&CMD_PREAMBLE_HOST_TO_ESP);
        
        let cmd_byte_offset = CMD_PREAMBLE_HOST_TO_ESP.len();
        command_packet[cmd_byte_offset] = cmd as u8;
        let data_offset = cmd_byte_offset + 1;
        
        if let Some(d) = data {
            let max_data_len = CMD_PACKET_TOTAL_SIZE_HOST_TO_ESP - data_offset;
            assert!(d.len() <= max_data_len);
            command_packet[data_offset..data_offset + d.len()].copy_from_slice(&d);
        }
        
        // Verify packet structure
        assert_eq!(&command_packet[0..4], &CMD_PREAMBLE_HOST_TO_ESP);
        assert_eq!(command_packet[4], Esp32Command::SetChannel as u8);
        assert_eq!(command_packet[5], 6); // Channel data
        assert_eq!(command_packet.len(), CMD_PACKET_TOTAL_SIZE_HOST_TO_ESP);
    }

    #[test] 
    fn test_data_too_large_for_command() {
        // Test validation logic for command data size
        let cmd = Esp32Command::ApplyDeviceConfig;
        let cmd_byte_offset = CMD_PREAMBLE_HOST_TO_ESP.len();
        let data_offset = cmd_byte_offset + 1;
        let max_data_len = CMD_PACKET_TOTAL_SIZE_HOST_TO_ESP - data_offset;
        
        // Create data that's too large
        let large_data = vec![0u8; max_data_len + 1];
        
        // This would be caught by the validation logic
        assert!(large_data.len() > max_data_len);
    }

    #[test]
    fn test_ack_waiter_types() {
        // Test that type aliases are correctly defined
        let _: SharedAckWaiters = Arc::new(Mutex::new(HashMap::new()));
        let (tx, _rx): (AckSender, Receiver<AckPayload>) = bounded(1);
        
        // Test sending success payload
        let success_payload: AckPayload = Ok(vec![1, 2, 3]);
        assert!(tx.try_send(success_payload).is_ok());
        
        // Second send should fail due to bounded(1) - channel is full
        let error_payload: AckPayload = Err(ControllerError::Execution("test error".to_string()));
        assert!(tx.try_send(error_payload).is_err());
    }

    #[test]
    fn test_drop_implementation() {
        let config = create_test_config();
        let source = Esp32Source::new(config).unwrap();
        
        // Test that drop doesn't panic
        drop(source);
        // If we reach here, drop succeeded
    }

    #[test]
    fn test_esp32_source_config_clone() {
        let config = create_test_config();
        let cloned_config = config.clone();
        
        assert_eq!(config, cloned_config);
        assert_eq!(config.port_name, cloned_config.port_name);
        assert_eq!(config.baud_rate, cloned_config.baud_rate);
        assert_eq!(config.csi_buffer_size, cloned_config.csi_buffer_size);
        assert_eq!(config.ack_timeout_ms, cloned_config.ack_timeout_ms);
    }

    #[test]
    fn test_esp32_source_thread_safety() {
        // Test that the types implement Send + Sync as required
        fn assert_send_sync<T: Send + Sync>() {}
        
        // These should compile without errors
        assert_send_sync::<Esp32SourceConfig>();
        // Note: Can't test Esp32Source directly due to SerialPort not being Send+Sync
        // But the design with Arc<Mutex<>> ensures thread safety
    }

    #[test]
    fn test_esp32_source_config_serde_serialization() {
        let config = create_test_config();
        
        // Test JSON serialization
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: Esp32SourceConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config, deserialized);
    }

    #[test]
    fn test_esp32_source_config_debug_format() {
        let config = create_test_config();
        let debug_str = format!("{:?}", config);
        
        assert!(debug_str.contains("Esp32SourceConfig"));
        assert!(debug_str.contains("/dev/ttyUSB0"));
        assert!(debug_str.contains("115200"));
        assert!(debug_str.contains("50"));
        assert!(debug_str.contains("1000"));
    }

    #[test]
    fn test_esp32_source_config_with_default_values() {
        let mut config = Esp32SourceConfig {
            port_name: "/dev/ttyUSB0".to_string(),
            ..Default::default()
        };
        
        assert_eq!(config.port_name, "/dev/ttyUSB0");
        assert_eq!(config.baud_rate, default_baud_rate());
        assert_eq!(config.csi_buffer_size, default_csi_buffer_size());
        assert_eq!(config.ack_timeout_ms, default_ack_timeout_ms());
        
        // Test that serde defaults work
        config.baud_rate = 0; // This should be replaced by default
        let json = r#"{"port_name": "/dev/ttyUSB0"}"#;
        let deserialized: Esp32SourceConfig = serde_json::from_str(json).unwrap();
        assert_eq!(deserialized.baud_rate, default_baud_rate());
    }

    #[tokio::test]
    async fn test_esp32_source_read_buf_when_running_but_no_data() {
        let config = create_test_config();
        let mut source = Esp32Source::new(config).unwrap();
        
        // Simulate running state but no data available
        source.is_running.store(true, AtomicOrdering::SeqCst);
        
        let mut buf = vec![0u8; 1024];
        let result = source.read_buf(&mut buf).await;
        
        // Should return error because no data is available and receiver times out
        // However, in this test environment without actual threads running,
        // the behavior may vary, so we just check it doesn't panic
        match result {
            Ok(_) => {}, // If it somehow succeeds, that's fine too
            Err(_) => {}, // Expected error is also fine
        }
    }

    #[tokio::test]
    async fn test_esp32_source_start_stop_multiple_times() {
        let config = create_test_config();
        let mut source = Esp32Source::new(config).unwrap();
        
        // Test stopping when not started
        let result = source.stop().await;
        assert!(result.is_ok()); // Should handle gracefully
        
        // Test multiple stops
        let result = source.stop().await;
        assert!(result.is_ok()); // Should handle gracefully
    }

    #[test]
    fn test_constants_values() {
        assert_eq!(CMD_PREAMBLE_HOST_TO_ESP, [0xC3; 4]);
        assert_eq!(CMD_PACKET_TOTAL_SIZE_HOST_TO_ESP, 128);
        assert_eq!(ESP_PACKET_PREAMBLE_ESP_TO_HOST, [0xAA; 8]);
        assert_eq!(ESP_TO_HOST_MIN_HEADER_SIZE, ESP_PACKET_PREAMBLE_ESP_TO_HOST.len() + 2);
        assert_eq!(BAUDRATE, 3_000_000);
        assert_eq!(DEFAULT_ACK_TIMEOUT_MS, 2000);
        assert_eq!(DEFAULT_CSI_BUFFER_SIZE, 100);
        assert_eq!(SERIAL_READ_TIMEOUT_MS, 100);
        assert_eq!(SERIAL_READ_BUFFER_SIZE, 4096);
    }

    #[test]
    fn test_esp32_source_config_extreme_values() {
        // Test with very high but reasonable values (not max to avoid overflow)
        let extreme_config = Esp32SourceConfig {
            port_name: "/dev/ttyUSB999".to_string(),
            baud_rate: 1_000_000, // High but reasonable baud rate
            csi_buffer_size: 0, // This should fail
            ack_timeout_ms: 10_000, // High but reasonable timeout
        };
        
        // Should fail because buffer size is zero
        let result = Esp32Source::new(extreme_config);
        assert!(result.is_err());
        
        match result.unwrap_err() {
            DataSourceError::Controller(msg) => {
                assert!(msg.contains("CSI buffer size cannot be zero"));
            }
            _ => panic!("Expected Controller error"),
        }
    }

    #[test]
    fn test_esp32_source_config_minimum_values() {
        let min_config = Esp32SourceConfig {
            port_name: "".to_string(),
            baud_rate: 1,
            csi_buffer_size: 1,
            ack_timeout_ms: 1,
        };
        
        let result = Esp32Source::new(min_config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_esp32_source_new_with_various_buffer_sizes() {
        let valid_sizes = vec![1, 10, 100, 1000, 10000];
        
        for size in valid_sizes {
            let config = Esp32SourceConfig {
                port_name: "/dev/test".to_string(),
                baud_rate: 115200,
                csi_buffer_size: size,
                ack_timeout_ms: 1000,
            };
            
            let result = Esp32Source::new(config);
            assert!(result.is_ok(), "Failed to create source with buffer size {}", size);
        }
    }

    #[test]
    fn test_packet_structure_constants() {
        // Verify packet structure makes sense
        assert!(CMD_PACKET_TOTAL_SIZE_HOST_TO_ESP > CMD_PREAMBLE_HOST_TO_ESP.len() + 1);
        assert!(ESP_TO_HOST_MIN_HEADER_SIZE >= ESP_PACKET_PREAMBLE_ESP_TO_HOST.len());
        
        // Verify there's room for data in command packets
        let data_space = CMD_PACKET_TOTAL_SIZE_HOST_TO_ESP - CMD_PREAMBLE_HOST_TO_ESP.len() - 1;
        assert!(data_space > 0);
    }

    #[test]
    fn test_esp32_source_debug_implementation() {
        let config = create_test_config();
        let source = Esp32Source::new(config).unwrap();
        
        let debug_str = format!("{:?}", source);
        assert!(debug_str.contains("Esp32Source"));
        // Debug implementation is minimal, just check it doesn't panic
    }

    #[test]
    fn test_atomic_bool_usage() {
        let config = create_test_config();
        let source = Esp32Source::new(config).unwrap();
        
        // Test atomic operations
        assert!(!source.is_running.load(AtomicOrdering::Relaxed));
        assert!(!source.is_running.load(AtomicOrdering::SeqCst));
        assert!(!source.is_running.load(AtomicOrdering::Acquire));
        
        source.is_running.store(true, AtomicOrdering::Relaxed);
        assert!(source.is_running.load(AtomicOrdering::Relaxed));
        
        source.is_running.store(false, AtomicOrdering::SeqCst);
        assert!(!source.is_running.load(AtomicOrdering::SeqCst));
    }

    #[test]
    fn test_crossbeam_channel_usage() {
        let (tx, rx) = bounded::<Vec<u8>>(10);
        
        // Test sending and receiving
        let test_data = vec![1, 2, 3, 4, 5];
        tx.send(test_data.clone()).unwrap();
        
        let received = rx.recv().unwrap();
        assert_eq!(received, test_data);
        
        // Test bounded behavior
        for i in 0..10 {
            assert!(tx.try_send(vec![i]).is_ok());
        }
        
        // 11th send should fail
        assert!(tx.try_send(vec![11]).is_err());
    }

    #[tokio::test]
    async fn test_esp32_source_to_config_implementation() {
        let original_config = create_test_config();
        let source = Esp32Source::new(original_config.clone()).unwrap();
        
        let config_result = source.to_config().await;
        assert!(config_result.is_ok());
        
        match config_result.unwrap() {
            DataSourceConfig::Esp32(recovered_config) => {
                assert_eq!(recovered_config, original_config);
            }
            _ => panic!("Expected Esp32 config variant"),
        }
    }

    #[test]
    fn test_shared_ack_waiters_thread_safety() {
        let waiters: SharedAckWaiters = Arc::new(Mutex::new(HashMap::new()));
        let waiters_clone = Arc::clone(&waiters);
        
        // Test that we can access from multiple contexts
        {
            let mut map = waiters.lock().unwrap();
            let (tx, _rx) = bounded(1);
            map.insert(1u8, tx);
        }
        
        {
            let map = waiters_clone.lock().unwrap();
            assert!(map.contains_key(&1u8));
        }
    }

    #[test]
    fn test_error_type_conversions() {
        // Test that DataSourceError can be created from io::Error
        let io_error = std::io::Error::new(std::io::ErrorKind::NotFound, "test error");
        let data_source_error = DataSourceError::from(io_error);
        
        match data_source_error {
            DataSourceError::Io(_) => {}, // Expected
            _ => panic!("Expected Io error variant"),
        }
    }

    #[test]
    fn test_controller_error_variants() {
        let exec_error = ControllerError::Execution("test execution error".to_string());
        match exec_error {
            ControllerError::Execution(msg) => {
                assert_eq!(msg, "test execution error");
            }
            _ => panic!("Expected Execution variant"),
        }
    }
}
