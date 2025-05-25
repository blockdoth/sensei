//! ESP32 Data Source
//!
//! Handles serial communication with an ESP32 device to send commands
//! and receive CSI (Channel State Information) data.

use crate::errors::{ControllerError, DataSourceError};
use crate::sources::DataSourceT;
use crate::sources::controllers::esp32_controller::Esp32Command; // Assuming this path

use async_trait::async_trait;
use byteorder::{LittleEndian, ReadBytesExt};
use crossbeam_channel::{
    Receiver as CrossbeamReceiver, Sender as CrossbeamSender, TryRecvError, bounded,
};
use log::{debug, error, info, trace, warn};
use serialport::{ClearBuffer, SerialPort};
use std::collections::HashMap;
use std::io::{ErrorKind as IoErrorKind, Read as StdRead, Write as StdWrite};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex as StdMutex, PoisonError}; // Standard Mutex for port and ack_waiters
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tokio::sync::Mutex as TokioMutex; // For async methods if needed, but serial ops are blocking

// Constants for ESP32 communication protocol
const CMD_PREAMBLE_HOST_TO_ESP: [u8; 4] = [0xC3; 4]; // As per input_component.h
const CMD_PACKET_TOTAL_SIZE_HOST_TO_ESP: usize = 128; // As per input_component.h
const ESP_PACKET_PREAMBLE_ESP_TO_HOST: [u8; 8] = [0xAA; 8]; // As per esp32.rs (original)
const ESP_TO_HOST_MIN_HEADER_SIZE: usize = ESP_PACKET_PREAMBLE_ESP_TO_HOST.len() + 2; // Preamble + Length (i16)

const DEFAULT_ACK_TIMEOUT_MS: u64 = 2500; // Increased slightly
const DEFAULT_CSI_BUFFER_SIZE: usize = 4096; // Increased slightly
const SERIAL_READ_TIMEOUT_MS: u64 = 50; // Short timeout for non-blocking feel
const SERIAL_PORT_OPERATION_TIMEOUT_MS: u64 = 1000; // Timeout for port open/write/flush
const SERIAL_READ_BUFFER_SIZE: usize = 4096; // Buffer for serial reads

const BAUDRATE: u32 = 3_000_000; // Common high speed for ESP32

type AckPayload = Result<Vec<u8>, ControllerError>;
type AckSender = CrossbeamSender<AckPayload>; // Using crossbeam for sync sender from reader thread
type AckWaiterMap = HashMap<u8, AckSender>; // Command ID to ACK sender

/// Configuration for the `Esp32Source`.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct Esp32SourceConfig {
    pub port_name: String,
    pub baud_rate: u32,
    #[serde(default = "default_csi_buffer_size")]
    pub csi_buffer_size: usize,
    #[serde(default = "default_ack_timeout_ms")]
    pub ack_timeout_ms: u64,
}

fn default_csi_buffer_size() -> usize {
    DEFAULT_CSI_BUFFER_SIZE
}
fn default_ack_timeout_ms() -> u64 {
    DEFAULT_ACK_TIMEOUT_MS
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

/// Represents a data source connected to an ESP32 device.
pub struct Esp32Source {
    config: Esp32SourceConfig,
    serial_port: Arc<StdMutex<Option<Box<dyn SerialPort>>>>,
    is_running: Arc<AtomicBool>,
    reader_thread_handle: Option<JoinHandle<()>>,
    csi_data_receiver: CrossbeamReceiver<Vec<u8>>, // For Esp32Source::read()
    csi_data_sender: CrossbeamSender<Vec<u8>>,     // For reader_task
    ack_waiters: Arc<StdMutex<AckWaiterMap>>,
}

impl Esp32Source {
    /// Creates a new `Esp32Source`.
    pub fn new(config: Esp32SourceConfig) -> Result<Self, DataSourceError> {
        if config.port_name.is_empty() {
            return Err(DataSourceError::InvalidConfig(
                "Serial port name cannot be empty.".to_string(),
            ));
        }
        if config.csi_buffer_size == 0 {
            return Err(DataSourceError::InvalidConfig(
                "CSI buffer size cannot be zero.".to_string(),
            ));
        }

        let (csi_data_sender, csi_data_receiver) = bounded(config.csi_buffer_size);

        Ok(Self {
            config,
            serial_port: Arc::new(StdMutex::new(None)),
            is_running: Arc::new(AtomicBool::new(false)),
            reader_thread_handle: None,
            csi_data_receiver,
            csi_data_sender,
            ack_waiters: Arc::new(StdMutex::new(HashMap::new())),
        })
    }

    pub fn is_source_running(&self) -> bool {
        self.is_running.load(AtomicOrdering::Relaxed)
    }

    pub fn port_name(&self) -> &str {
        &self.config.port_name
    }

    /// Sends a command to the ESP32 and waits for an acknowledgment.
    /// This method is intended to be called by the `Esp32Controller`.
    /// It's made public for the controller but not part of `DataSourceT`.
    pub async fn send_esp32_command(
        // Changed to async fn
        &self,
        command: Esp32Command,
        data: Option<Vec<u8>>,
    ) -> Result<Vec<u8>, ControllerError> {
        if !self.is_running.load(AtomicOrdering::SeqCst) {
            return Err(ControllerError::Execution(
                "ESP32 source is not running. Cannot send command.".to_string(),
            ));
        }

        let mut command_packet_builder = vec![0u8; CMD_PACKET_TOTAL_SIZE_HOST_TO_ESP];
        command_packet_builder[0..CMD_PREAMBLE_HOST_TO_ESP.len()]
            .copy_from_slice(&CMD_PREAMBLE_HOST_TO_ESP);

        let cmd_byte_offset = CMD_PREAMBLE_HOST_TO_ESP.len();
        command_packet_builder[cmd_byte_offset] = command as u8;

        let data_offset = cmd_byte_offset + 1;
        if let Some(d) = data {
            let max_data_len = CMD_PACKET_TOTAL_SIZE_HOST_TO_ESP - data_offset;
            if d.len() > max_data_len {
                return Err(ControllerError::InvalidParams(format!(
                    "Command data too large: {} bytes (max {}) for command {:?}.",
                    d.len(),
                    max_data_len,
                    command
                )));
            }
            command_packet_builder[data_offset..data_offset + d.len()].copy_from_slice(&d);
        }
        // Use Arc for the command_packet to move into spawn_blocking
        let command_packet = Arc::new(command_packet_builder);
        trace!("Prepared command packet for {command:?}: {command_packet:02X?}");

        let (ack_tx, ack_rx) = bounded(1);
        {
            let mut waiters_guard = self.ack_waiters.lock().map_err(|e: PoisonError<_>| {
                ControllerError::Execution(format!("ACK waiter lock poisoned: {e}"))
            })?;
            if waiters_guard.insert(command as u8, ack_tx).is_some() {
                warn!(
                    "Overwriting ACK waiter for command {command:?}. Previous command might have timed out."
                );
            }
        }

        // --- Asynchronously perform the blocking serial write ---
        let port_clone = Arc::clone(&self.serial_port);
        let command_packet_clone = Arc::clone(&command_packet);
        let command_debug_clone = command; // for logging

        tokio::task::spawn_blocking(move || {
            let mut port_guard = port_clone.lock().map_err(|e: PoisonError<_>| {
                std::io::Error::other(format!("Serial port lock poisoned for send: {e}"))
            })?;

            if let Some(port) = port_guard.as_mut() {
                debug!(
                    "Sending command {:?} to ESP32 ({} bytes).",
                    command_debug_clone,
                    command_packet_clone.len()
                );
                port.write_all(&command_packet_clone)?;
                port.flush()?;
                Ok(())
            } else {
                Err(std::io::Error::new(
                    IoErrorKind::NotConnected,
                    "Serial port not open for sending command.",
                ))
            }
        })
        .await // Wait for spawn_blocking to complete
        .map_err(|e| ControllerError::Execution(format!("Task for serial write panicked: {e}")))? // Handle JoinError
        .map_err(ControllerError::Io)?; // Handle IO Error from the closure

        // --- Asynchronously wait for the ACK ---
        let ack_timeout = Duration::from_millis(self.config.ack_timeout_ms);
        let ack_wait_result =
            tokio::task::spawn_blocking(move || ack_rx.recv_timeout(ack_timeout)).await; // Wait for spawn_blocking to complete

        // Remove waiter regardless of outcome, needs to be done carefully
        // Best to remove *after* recv attempt or timeout, but ensure it's always removed
        let ack_payload_result = match ack_wait_result {
            Ok(Ok(payload)) => Ok(payload), // recv_timeout succeeded, got AckPayload
            Ok(Err(crossbeam_channel::RecvTimeoutError::Timeout)) => {
                error!("Timeout waiting for ACK for command: {command:?}");
                Err(ControllerError::AckTimeout(format!(
                    "Timeout waiting for ACK for ESP32 command {command:?}"
                )))
            }
            Ok(Err(crossbeam_channel::RecvTimeoutError::Disconnected)) => {
                error!(
                    "ACK channel disconnected for command: {command:?}. Reader thread might have terminated."
                );
                Err(ControllerError::Execution(
                    "ESP32 ACK channel disconnected; reader thread likely terminated.".to_string(),
                ))
            }
            Err(join_err) => {
                // JoinError from spawn_blocking
                error!("Task for ACK receive panicked for command {command:?}: {join_err}");
                Err(ControllerError::Execution(format!(
                    "Task for ACK receive panicked: {join_err}"
                )))
            }
        };

        // Ensure waiter is removed
        self.ack_waiters
            .lock()
            .map_err(|e: PoisonError<_>| {
                ControllerError::Execution(format!("ACK waiter lock poisoned on cleanup: {e}"))
            })?
            .remove(&(command as u8));

        match ack_payload_result {
            Ok(Ok(ack_data)) => {
                // Inner Ok is from AckPayload = Result<Vec<u8>, ControllerError>
                info!("ACK received for command: {command:?}");
                Ok(ack_data)
            }
            Ok(Err(controller_err)) => {
                // Inner Err is from AckPayload
                error!("ESP32 reported error in ACK for command {command:?}: {controller_err}");
                Err(controller_err)
            }
            Err(controller_err) => {
                // Error from recv_timeout handling (Timeout, Disconnected, Panic)
                Err(controller_err)
            }
        }
    }

    /// The main loop for the reader thread.
    fn reader_task_loop(
        serial_port_arc: Arc<StdMutex<Option<Box<dyn SerialPort>>>>,
        is_running_arc: Arc<AtomicBool>,
        csi_data_sender: CrossbeamSender<Vec<u8>>,
        ack_waiters_arc: Arc<StdMutex<AckWaiterMap>>,
    ) {
        info!("ESP32Source reader thread started.");
        let mut current_read_buffer = Vec::with_capacity(SERIAL_READ_BUFFER_SIZE * 2);
        let mut temp_serial_buf = [0u8; SERIAL_READ_BUFFER_SIZE]; // Reusable buffer for serial reads

        while is_running_arc.load(AtomicOrdering::Relaxed) {
            let bytes_read_this_iteration = {
                let mut port_guard = match serial_port_arc.lock() {
                    Ok(guard) => guard,
                    Err(p_err) => {
                        error!("Reader thread: Serial port mutex poisoned: {p_err}. Terminating.");
                        break;
                    }
                };

                if let Some(port) = port_guard.as_mut() {
                    match port.read(&mut temp_serial_buf) {
                        Ok(0) => {
                            // Should not happen with timeout, but handle defensively
                            drop(port_guard); // Release lock before sleep
                            thread::sleep(Duration::from_millis(5)); // Brief pause
                            0
                        }
                        Ok(n) => {
                            current_read_buffer.extend_from_slice(&temp_serial_buf[..n]);
                            n
                        }
                        Err(e) if e.kind() == IoErrorKind::TimedOut => 0, // Expected with timeout
                        Err(e) if e.kind() == IoErrorKind::Interrupted => 0, // Can happen
                        Err(e) => {
                            error!("Serial read error in reader thread: {e}. Terminating reader.");
                            is_running_arc.store(false, AtomicOrdering::Relaxed); // Signal shutdown
                            break; // Exit loop
                        }
                    }
                } else {
                    // Port not open, or closed during operation
                    if !is_running_arc.load(AtomicOrdering::Relaxed) {
                        break;
                    } // Exit if shutting down
                    drop(port_guard); // Release lock before sleep
                    thread::sleep(Duration::from_millis(100)); // Wait for port to potentially become available
                    0
                }
            };

            if bytes_read_this_iteration > 0 {
                trace!(
                    "Reader thread: read {} bytes from serial. Buffer size: {}",
                    bytes_read_this_iteration,
                    current_read_buffer.len()
                );
            }

            // Process all complete packets in current_read_buffer
            loop {
                if current_read_buffer.len() < ESP_TO_HOST_MIN_HEADER_SIZE {
                    break; // Not enough data for even a header
                }

                // Search for preamble
                if let Some(preamble_start_idx) = current_read_buffer
                    .windows(ESP_PACKET_PREAMBLE_ESP_TO_HOST.len())
                    .position(|window| window == ESP_PACKET_PREAMBLE_ESP_TO_HOST)
                {
                    if preamble_start_idx > 0 {
                        debug!("Discarding {preamble_start_idx} bytes before preamble.");
                        current_read_buffer.drain(0..preamble_start_idx);
                        // Continue to re-check buffer length after drain
                        if current_read_buffer.len() < ESP_TO_HOST_MIN_HEADER_SIZE {
                            break;
                        }
                    }

                    // Preamble is at the start of current_read_buffer now.
                    // Read packet length (i16, Little Endian)
                    let mut len_bytes_cursor = std::io::Cursor::new(
                        &current_read_buffer
                            [ESP_PACKET_PREAMBLE_ESP_TO_HOST.len()..ESP_TO_HOST_MIN_HEADER_SIZE],
                    );
                    let esp_length_field = match len_bytes_cursor.read_i16::<LittleEndian>() {
                        Ok(len) => len,
                        Err(e) => {
                            error!(
                                "Failed to read packet length field: {e}. Discarding minimal header to attempt recovery."
                            );
                            current_read_buffer.drain(0..ESP_TO_HOST_MIN_HEADER_SIZE);
                            continue; // Try to find next packet
                        }
                    };

                    // Total length of the packet on the wire *including* preamble and length field.
                    // The firmware sends the length of (preamble + length_field + payload_data) as positive/negative.
                    // So, the `esp_length_field.unsigned_abs()` is the total size of this full ESP-to-Host frame.
                    let total_packet_on_wire_len = esp_length_field.unsigned_abs() as usize;

                    if total_packet_on_wire_len < ESP_TO_HOST_MIN_HEADER_SIZE {
                        error!(
                            "ESP32 reported invalid packet length: {total_packet_on_wire_len} (must be >= {ESP_TO_HOST_MIN_HEADER_SIZE}). Discarding preamble and length field."
                        );
                        current_read_buffer.drain(0..ESP_TO_HOST_MIN_HEADER_SIZE);
                        continue;
                    }

                    if current_read_buffer.len() < total_packet_on_wire_len {
                        trace!(
                            "Partial packet: Have {}, need {}. Waiting for more data.",
                            current_read_buffer.len(),
                            total_packet_on_wire_len
                        );
                        break; // Need more data for the full packet
                    }

                    // We have a full packet
                    let packet_with_header_and_len_field = current_read_buffer
                        .drain(0..total_packet_on_wire_len)
                        .collect::<Vec<_>>();
                    let actual_payload_data =
                        packet_with_header_and_len_field[ESP_TO_HOST_MIN_HEADER_SIZE..].to_vec();

                    let is_ack_packet = esp_length_field < 0;

                    if is_ack_packet {
                        if actual_payload_data.is_empty() {
                            warn!(
                                "ACK packet received with empty payload (expected command byte). Skipping."
                            );
                            continue;
                        }
                        let cmd_byte_acked = actual_payload_data[0];
                        let ack_data_payload = if actual_payload_data.len() > 1 {
                            actual_payload_data[1..].to_vec()
                        } else {
                            Vec::new()
                        };
                        debug!(
                            "Received ACK for command 0x{:02X} with data len {}",
                            cmd_byte_acked,
                            ack_data_payload.len()
                        );

                        let mut waiters_guard = match ack_waiters_arc.lock() {
                            Ok(g) => g,
                            Err(p_err) => {
                                error!(
                                    "ACK Waiter lock poisoned in reader: {p_err}. Cannot deliver ACK."
                                );
                                continue;
                            }
                        };
                        if let Some(ack_sender_channel) = waiters_guard.remove(&cmd_byte_acked) {
                            if let Err(e) = ack_sender_channel.try_send(Ok(ack_data_payload)) {
                                warn!(
                                    "Failed to send ACK payload for cmd 0x{cmd_byte_acked:02X} to waiting task: {e:?}. Channel might be closed (timeout)."
                                );
                            } else {
                                trace!(
                                    "Successfully delivered ACK for 0x{cmd_byte_acked:02X} to waiter."
                                );
                            }
                        } else {
                            warn!(
                                "Received ACK for command 0x{cmd_byte_acked:02X}, but no waiter was registered or it already timed out."
                            );
                        }
                    } else {
                        // CSI Data Packet
                        trace!(
                            "Received CSI Data packet (payload len {}). Forwarding.",
                            actual_payload_data.len()
                        );
                        if csi_data_sender.is_full() {
                            warn!(
                                "CSI data channel is full. Discarding incoming ESP32 CSI packet. CLI might be blocked or slow."
                            );
                        } else if let Err(e) = csi_data_sender.try_send(actual_payload_data) {
                            error!("Failed to send CSI data to processing channel: {e}.");
                            if e.is_disconnected() {
                                info!("CSI data channel disconnected. Reader thread terminating.");
                                is_running_arc.store(false, AtomicOrdering::Relaxed);
                                break; // Break inner, outer will also break.
                            }
                        }
                    }
                } else {
                    // No preamble found in the buffer. If buffer is large, discard some to avoid unbounded growth with garbage data.
                    if current_read_buffer.len() > SERIAL_READ_BUFFER_SIZE {
                        // Heuristic
                        let discard_len = current_read_buffer.len()
                            - (ESP_PACKET_PREAMBLE_ESP_TO_HOST.len().saturating_sub(1));
                        debug!(
                            "No preamble found in {} bytes of buffer. Discarding {} bytes to search again.",
                            current_read_buffer.len(),
                            discard_len
                        );
                        current_read_buffer.drain(0..discard_len);
                    }
                    break; // Need more data to find a preamble
                }
            } // End inner loop (processing current_read_buffer)

            // If no new data was read and buffer is small, pause briefly
            if bytes_read_this_iteration == 0
                && current_read_buffer.len() < ESP_TO_HOST_MIN_HEADER_SIZE
            {
                if !is_running_arc.load(AtomicOrdering::Relaxed) {
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
        } // End while is_running_arc

        // Cleanup before exiting thread
        is_running_arc.store(false, AtomicOrdering::Relaxed); // Ensure state is false
        if let Ok(mut waiters) = ack_waiters_arc.lock() {
            // Notify any pending waiters that the source is stopping
            for (_, sender) in waiters.drain() {
                let _ = sender.try_send(Err(ControllerError::Execution(
                    "ESP32Source stopped; command aborted.".to_string(),
                )));
            }
        } else {
            error!("ACK Waiters lock poisoned during reader thread shutdown cleanup.");
        }
        info!("ESP32Source reader thread finished.");
    }
}

#[async_trait]
impl DataSourceT for Esp32Source {
    async fn start(&mut self) -> Result<(), DataSourceError> {
        if self
            .is_running
            .compare_exchange(false, true, AtomicOrdering::SeqCst, AtomicOrdering::Relaxed)
            .is_err()
        {
            info!("ESP32Source already running or starting.");
            return Ok(());
        }
        info!(
            "Starting ESP32Source on port {} at {} baud.",
            self.config.port_name, self.config.baud_rate
        );

        let port_result = serialport::new(&self.config.port_name, self.config.baud_rate)
            .timeout(Duration::from_millis(SERIAL_READ_TIMEOUT_MS)) // Timeout for port.read()
            .open();

        let mut port = match port_result {
            Ok(p) => p,
            Err(e) => {
                error!(
                    "Failed to open serial port {}: {}",
                    self.config.port_name, e
                );
                self.is_running.store(false, AtomicOrdering::SeqCst);
                return Err(DataSourceError::from(e));
            }
        };

        if let Err(e) = port.clear(ClearBuffer::All) {
            error!("Failed to clear serial port buffers: {e}");
            self.is_running.store(false, AtomicOrdering::SeqCst);
            return Err(DataSourceError::from(e));
        }

        // Configure write timeout for port operations like flush
        if let Err(e) = port.set_timeout(Duration::from_millis(SERIAL_PORT_OPERATION_TIMEOUT_MS)) {
            error!("Failed to set serial port write timeout: {e}");
            self.is_running.store(false, AtomicOrdering::SeqCst);
            return Err(DataSourceError::from(e));
        }

        *self.serial_port.lock().map_err(|p_err| {
            DataSourceError::Initialization(format!("Serial port lock poisoned on start: {p_err}"))
        })? = Some(port);

        let port_clone = Arc::clone(&self.serial_port);
        let is_running_clone = Arc::clone(&self.is_running);
        let csi_sender_clone = self.csi_data_sender.clone();
        let ack_waiters_clone = Arc::clone(&self.ack_waiters);

        let reader_handle = thread::Builder::new()
            .name(format!(
                "esp32-reader-{}",
                self.config.port_name.replace("/", "_")
            ))
            .spawn(move || {
                Esp32Source::reader_task_loop(
                    port_clone,
                    is_running_clone,
                    csi_sender_clone,
                    ack_waiters_clone,
                );
            })
            .map_err(|e| {
                DataSourceError::Initialization(format!("Failed to spawn ESP32 reader thread: {e}"))
            })?;

        self.reader_thread_handle = Some(reader_handle);
        info!(
            "ESP32Source started successfully on {}.",
            self.config.port_name
        );
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), DataSourceError> {
        if self
            .is_running
            .compare_exchange(true, false, AtomicOrdering::SeqCst, AtomicOrdering::Relaxed)
            .is_err()
        {
            info!("ESP32Source already stopped or not running.");
            // Still try to join thread if handle exists, in case of partial start/stop
            if let Some(handle) = self.reader_thread_handle.take() {
                debug!(
                    "ESP32Source was not marked as running, but reader thread handle exists. Attempting join."
                );
                match handle.join() {
                    Ok(_) => debug!(
                        "Reader thread joined successfully after stop (was not marked running)."
                    ),
                    Err(e) => warn!(
                        "Error joining reader thread after stop (was not marked running): {e:?}"
                    ),
                }
            }
            return Ok(());
        }

        info!("Stopping ESP32Source on {}...", self.config.port_name);

        // Close and drop the serial port. This should interrupt blocking reads in the reader thread.
        if let Ok(mut port_guard) = self.serial_port.lock() {
            if let Some(mut port_instance) = port_guard.take() {
                // Attempt to flush before closing, ignore error if it fails during shutdown
                let _ = port_instance.flush();
                drop(port_instance);
                debug!("Serial port explicitly closed and dropped during stop.");
            }
        } else {
            error!("Serial port lock poisoned during stop. Unable to ensure clean port closure.");
        }

        if let Some(handle) = self.reader_thread_handle.take() {
            debug!("Waiting for ESP32 reader thread to join...");
            match handle.join() {
                Ok(_) => debug!("ESP32 reader thread joined successfully."),
                Err(e) => {
                    error!("ESP32 reader thread panicked or failed to join cleanly: {e:?}");
                    return Err(DataSourceError::Shutdown(
                        "Reader thread did not join cleanly.".to_string(),
                    ));
                }
            }
        } else {
            debug!("No reader thread handle found, was it already joined or never started?");
        }

        // Clear any outstanding ACK waiters
        if let Ok(mut waiters) = self.ack_waiters.lock() {
            if !waiters.is_empty() {
                warn!(
                    "{} ACK waiters were still pending during stop. Notifying them of shutdown.",
                    waiters.len()
                );
                for (_, sender) in waiters.drain() {
                    let _ = sender.try_send(Err(ControllerError::Execution(
                        "ESP32Source stopped; command aborted during shutdown.".to_string(),
                    )));
                }
            }
        } else {
            error!("ACK Waiters lock poisoned during stop cleanup.");
        }

        info!("ESP32Source stopped successfully.");
        Ok(())
    }

    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, DataSourceError> {
        // Non-blocking read from the internal CSI data channel
        match self.csi_data_receiver.try_recv() {
            Ok(data_payload) => {
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
            Err(TryRecvError::Empty) => {
                if !self.is_running.load(AtomicOrdering::Relaxed)
                    && self.csi_data_receiver.is_empty()
                {
                    trace!("ESP32Source not running and CSI buffer empty, Ok(0) for read.");
                    Ok(0) // Source stopped and no more data
                } else {
                    Ok(0) // No data currently available, but source might still be running
                }
            }
            Err(TryRecvError::Disconnected) => {
                info!("CSI data channel disconnected in read(). Reader thread likely terminated.");
                self.is_running.store(false, AtomicOrdering::Relaxed);
                Err(DataSourceError::NotConnected(
                    "CSI data channel disconnected; ESP32 source reader stopped.".to_string(),
                ))
            }
        }
    }
}

impl Drop for Esp32Source {
    fn drop(&mut self) {
        if self.is_running.load(AtomicOrdering::Relaxed) || self.reader_thread_handle.is_some() {
            info!(
                "Dropping Esp32Source for {}: ensuring resources are cleaned up.",
                self.config.port_name
            );
            // Set running to false to signal the thread
            self.is_running.store(false, AtomicOrdering::SeqCst);

            // Attempt to close port
            if let Ok(mut port_guard) = self.serial_port.lock() {
                if let Some(port_to_drop) = port_guard.take() {
                    drop(port_to_drop);
                    debug!("Serial port dropped in Esp32Source::drop");
                }
            } else {
                // If poisoned, we can't do much about the port, but the thread should still see is_running=false
                error!(
                    "Serial port lock poisoned during Esp32Source::drop for {}.",
                    self.config.port_name
                );
            }

            // Join the reader thread. This is blocking, which is generally discouraged in drop,
            // but necessary for resource cleanup if stop() wasn't called.
            // A timeout could be added here if deadlocks are a concern.
            if let Some(handle) = self.reader_thread_handle.take() {
                debug!(
                    "Esp32Source::drop waiting for reader thread to join for {}.",
                    self.config.port_name
                );
                if let Err(e) = handle.join() {
                    error!(
                        "Error joining reader thread in Esp32Source::drop for {}: {:?}",
                        self.config.port_name, e
                    );
                } else {
                    debug!(
                        "Reader thread joined in Esp32Source::drop for {}.",
                        self.config.port_name
                    );
                }
            }
        }
    }
}
