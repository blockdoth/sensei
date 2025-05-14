use anyhow::{Context, Result};
use byteorder::{LittleEndian, ReadBytesExt};
use crossbeam_channel::{bounded, Receiver, Sender, RecvTimeoutError};
use log::{debug, error, info, warn};
use serialport::SerialPort;
use std::{
    io::{Cursor, Read, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering as AtomicOrdering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

// --- Protocol Constants ---
/// Preamble for commands sent from the Host (this client) to the ESP32.
const CMD_PREAMBLE_HOST_TO_ESP: [u8; 4] = [0xC3; 4];
/// Total size of a command packet sent from the Host to the ESP32.
const CMD_PACKET_TOTAL_SIZE_HOST_TO_ESP: usize = 128;
/// Size of the data payload within a command packet from Host to ESP32.
const CMD_DATA_MAX_LEN_HOST_TO_ESP: usize =
    CMD_PACKET_TOTAL_SIZE_HOST_TO_ESP - CMD_PREAMBLE_HOST_TO_ESP.len() - 1; // 1 for command byte

/// Preamble for packets (ACK/CSI) sent from the ESP32 to the Host.
const ESP_PACKET_PREAMBLE_ESP_TO_HOST: [u8; 8] = [0xAA; 8];
/// Size of the header (Preamble + Length field) for packets from ESP32 to Host.
const ESP_TO_HOST_MIN_HEADER_SIZE: usize = ESP_PACKET_PREAMBLE_ESP_TO_HOST.len() + 2; // 2 for length field


/// Represents the WiFi channel bandwidth
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bandwidth {
    Twenty = 0x00,
    Forty = 0x01,
}

/// Represents the CSI type for extraction
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsiType {
    LegacyLTF = 0x00, // Legacy Long Training Field
    HTLTF = 0x01,     // High Throughput Long Training Field
}

/// ESP32 device operation mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationMode {
    Receive = 0x00, // Corresponds to Passive in C++
    Transmit = 0x01, // Corresponds to ActiveFREE in C++ (for sending dummy packets)
}

/// Secondary channel configuration for 40MHz bandwidth
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecondaryChannel {
    None = 0x00,
    Below = 0x01,
    Above = 0x02,
}

/// Represents the ESP32 device status (currently informational, not deeply integrated)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EspStatus {
    Disconnected,
    Connecting,
    Connected,
    Receiving,
    Transmitting,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    SetChannel = 0x01,
    WhitelistAddMacPair = 0x02,
    WhitelistClear = 0x03,
    PauseAcquisition = 0x04,
    UnpauseAcquisition = 0x05,
    ApplyDeviceConfig = 0x06,
    PauseWifiTransmit = 0x07,
    ResumeWifiTransmit = 0x08,
    TransmitCustomFrame = 0x09,
    SynchronizeTimeInit = 0x0A,
    SynchronizeTimeApply = 0x0B,
}

impl TryFrom<u8> for Command {
    type Error = EspError;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x01 => Ok(Command::SetChannel),
            0x02 => Ok(Command::WhitelistAddMacPair),
            0x03 => Ok(Command::WhitelistClear),
            0x04 => Ok(Command::PauseAcquisition),
            0x05 => Ok(Command::UnpauseAcquisition),
            0x06 => Ok(Command::ApplyDeviceConfig),
            0x07 => Ok(Command::PauseWifiTransmit),
            0x08 => Ok(Command::ResumeWifiTransmit),
            0x09 => Ok(Command::TransmitCustomFrame),
            0x0A => Ok(Command::SynchronizeTimeInit),
            0x0B => Ok(Command::SynchronizeTimeApply),
            _ => Err(EspError::InvalidCommand(value)),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CsiPacket {
    pub timestamp_us: u64,
    pub src_mac: [u8; 6],
    pub dst_mac: [u8; 6],
    pub seq: u16,
    pub rssi: i8,
    pub agc_gain: u8,
    pub fft_gain: u8,
    pub csi_data: Vec<i8>,
}

/// Device configuration
#[derive(Debug, Clone)]
pub struct DeviceConfig {
    pub mode: OperationMode,
    pub channel: u8,
    pub bandwidth: Bandwidth,
    pub secondary_channel: SecondaryChannel,
    pub csi_type: CsiType,
    pub manual_scale: u8, // Manual scale for CSI data
}

impl Default for DeviceConfig {
    fn default() -> Self {
        Self {
            mode: OperationMode::Receive, // Passive
            channel: 1,
            bandwidth: Bandwidth::Twenty,
            secondary_channel: SecondaryChannel::None,
            csi_type: CsiType::HTLTF,
            manual_scale: 0,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EspError {
    #[error("Serial port error: {0}")]
    Serial(#[from] serialport::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid command: {0}")]
    InvalidCommand(u8),
    #[error("Ack timeout for command {0:?}")]
    AckTimeout(Command),
    #[error("Protocol error: {0}")]
    Protocol(String),
    #[error("Invalid configuration: {0}")]
    Config(String),
    #[error("Device not connected or reader thread not running")]
    NotConnected,
    #[error("Reader thread panicked")]
    ThreadPanic,
}

pub struct Esp32 {
    port: Box<dyn SerialPort>,
    ack_tx: Sender<(Command, Vec<u8>)>,
    ack_rx: Receiver<(Command, Vec<u8>)>,
    csi_tx: Sender<CsiPacket>,
    csi_rx: Receiver<CsiPacket>,
    connected: Arc<AtomicBool>,
    config: DeviceConfig,
    reader_thread_handle: Option<JoinHandle<()>>,
}

impl Esp32 {
    pub fn new(port_name: &str, baud_rate: u32) -> Result<Self> {
        let port = serialport::new(port_name, baud_rate)
            .timeout(Duration::from_millis(100))
            .open()
            .context("Failed to open serial port")?;

        let (ack_tx, ack_rx) = bounded(100);
        let (csi_tx, csi_rx) = bounded(1000);

        Ok(Self {
            port,
            ack_tx,
            ack_rx,
            csi_tx,
            csi_rx,
            connected: Arc::new(AtomicBool::new(false)),
            config: DeviceConfig::default(),
            reader_thread_handle: None,
        })
    }

    pub fn connect(&mut self) -> Result<()> {
        if self.connected.load(AtomicOrdering::SeqCst) || self.reader_thread_handle.is_some() {
            warn!("ESP32 already connected or connection attempt in progress. Disconnect first if reconnect is needed.");
            return Err(EspError::Protocol("Already connected or connection pending. Call disconnect first.".to_string()).into());
        }

        let mut port_clone = self.port.try_clone().context("Failed to clone serial port for reader thread")?;
        let ack_tx_clone = self.ack_tx.clone();
        let csi_tx_clone = self.csi_tx.clone();
        let connected_clone = Arc::clone(&self.connected);

        connected_clone.store(true, AtomicOrdering::SeqCst);

        let handle = thread::spawn(move || {
            let mut partial_buffer = Vec::with_capacity(4096 * 2);
            let mut read_buf = [0u8; 2048];

            info!("ESP32 reading thread started.");
            while connected_clone.load(AtomicOrdering::Relaxed) {
                match port_clone.read(&mut read_buf) {
                    Ok(bytes_read) => {
                        if bytes_read > 0 {
                            partial_buffer.extend_from_slice(&read_buf[..bytes_read]);
                            Self::process_buffer(&mut partial_buffer, &ack_tx_clone, &csi_tx_clone);
                        } else {
                             thread::sleep(Duration::from_millis(1));
                        }
                    }
                    Err(e) => {
                        if e.kind() == std::io::ErrorKind::TimedOut {
                            continue;
                        }
                        error!("Serial read error in thread: {}. Terminating thread.", e);
                        break;
                    }
                }
            }
            connected_clone.store(false, AtomicOrdering::SeqCst);
            info!("ESP32 reading thread terminated.");
        });

        self.reader_thread_handle = Some(handle);
        thread::sleep(Duration::from_millis(200)); // Allow thread to start

        // Apply initial config (default is Receive mode) and handle acquisition state
        if let Err(e) = self.apply_device_config() {
             error!("Failed to apply initial device config during connect: {}", e);
             let _ = self.disconnect();
             return Err(e);
        }
        // Note: apply_device_config will now also handle unpausing if mode is Receive.

        if let Err(e) = self.synchronize_time() {
            error!("Failed initial time synchronization during connect: {}", e);
            let _ = self.disconnect();
            return Err(e);
        }

        info!("ESP32 connected and initial configuration applied (including acquisition state).");
        Ok(())
    }

    fn process_buffer(
        buffer: &mut Vec<u8>,
        ack_tx: &Sender<(Command, Vec<u8>)>,
        csi_tx: &Sender<CsiPacket>,
    ) {

        debug!("Processing buffer of len: {}", buffer.len());
        if !buffer.is_empty() {
            let log_len = std::cmp::min(buffer.len(), 64);
            debug!("Buffer content (first {} bytes): {:02X?}", log_len, &buffer[..log_len]);
        }

        // Check for preamble and process packets

        loop {
            if buffer.is_empty() {
                break;
            }
            if let Some(pos) = buffer.windows(ESP_PACKET_PREAMBLE_ESP_TO_HOST.len()).position(|w| w == ESP_PACKET_PREAMBLE_ESP_TO_HOST) {
                if pos > 0 {
                    debug!("Preamble found at pos {}. Discarding {} bytes before preamble.", pos, pos);
                    buffer.drain(0..pos);
                }

                if buffer.len() < ESP_TO_HOST_MIN_HEADER_SIZE {
                    debug!("Buffer (len {}) too short for full header (need {}). Waiting for more data.", buffer.len(), ESP_TO_HOST_MIN_HEADER_SIZE);
                    break;
                }

                let len_bytes = &buffer[ESP_PACKET_PREAMBLE_ESP_TO_HOST.len()..ESP_TO_HOST_MIN_HEADER_SIZE];
                let payload_len_signed_from_wire = i16::from_le_bytes([len_bytes[0], len_bytes[1]]);
                debug!("Read raw length field value from wire: {} ({:02X?})", payload_len_signed_from_wire, len_bytes);

                if payload_len_signed_from_wire == 0 {
                    warn!("Received packet with declared length 0. Problematic. Discarding header to attempt recovery. Buffer head: {:02X?}", &buffer[..std::cmp::min(buffer.len(), ESP_TO_HOST_MIN_HEADER_SIZE)]);
                    buffer.drain(0..std::cmp::min(ESP_TO_HOST_MIN_HEADER_SIZE, buffer.len()));
                    continue;
                }
                if payload_len_signed_from_wire.abs() as usize > buffer.capacity() && payload_len_signed_from_wire.abs() > 8192 {
                    warn!("Declared packet length {} is very large. Clearing buffer.", payload_len_signed_from_wire.abs());
                    buffer.clear();
                    break;
                }

                let total_packet_len_on_wire = payload_len_signed_from_wire.abs() as usize;
                debug!("Interpreted total packet length on wire: {}", total_packet_len_on_wire);

                if buffer.len() < total_packet_len_on_wire {
                    debug!("Buffer (len {}) too short for full packet (need {}). Waiting for more.", buffer.len(), total_packet_len_on_wire);
                    break;
                }

                debug!("Extracting full packet of len: {}", total_packet_len_on_wire);
                let packet_data = buffer.drain(0..total_packet_len_on_wire).collect::<Vec<_>>();
                
                Self::parse_packet_from_esp(&packet_data, payload_len_signed_from_wire, ack_tx, csi_tx);

            } else if buffer.len() >= ESP_PACKET_PREAMBLE_ESP_TO_HOST.len() {
                let keep_start = buffer.len() - (ESP_PACKET_PREAMBLE_ESP_TO_HOST.len() - 1);
                buffer.drain(0..keep_start);
                break;
            } else {
                break;
            }
        }
    }

    
    fn parse_packet_from_esp(
        full_packet_frame: &[u8],
        payload_len_signed: i16,
        ack_tx: &Sender<(Command, Vec<u8>)>,
        csi_tx: &Sender<CsiPacket>,
    ) {
        let is_ack = payload_len_signed < 0;
        let payload_offset = ESP_TO_HOST_MIN_HEADER_SIZE; // 10 bytes (8 preamble + 2 length)

        debug!("Parsing packet from ESP. is_ack: {}, payload_len_signed: {}, frame_len: {}", is_ack, payload_len_signed, full_packet_frame.len());

        if full_packet_frame.len() < payload_offset {
            warn!("Packet frame (len: {}) too small to contain minimal header + payload offset ({}). Corrupted data or logic error.", full_packet_frame.len(), payload_offset);
            return;
        }
        // actual_payload starts *after* the 8B preamble and 2B length field.
        let actual_payload = &full_packet_frame[payload_offset..];
        debug!("Actual payload to parse (len {}): {:02X?}", actual_payload.len(), &actual_payload[..std::cmp::min(actual_payload.len(), 32)]);


        if is_ack {
            if actual_payload.is_empty() {
                warn!("ACK packet payload is empty, cannot extract command byte.");
                return;
            }
            let cmd_byte = actual_payload[0];
            match Command::try_from(cmd_byte) {
                Ok(cmd) => {
                    let data = if actual_payload.len() > 1 {
                        actual_payload[1..].to_vec()
                    } else {
                        Vec::new()
                    };
                    debug!("Parsed ACK for command {:?}, data len: {}", cmd, data.len());
                    if let Err(e) = ack_tx.try_send((cmd, data)) {
                        warn!("Failed to send ACK to channel (possibly full or disconnected): {}", e);
                    }
                }
                Err(e) => {
                    warn!("Invalid command byte {} in ACK packet: {}", cmd_byte, e);
                }
            }
        } else { // CSI Packet
            match parse_csi_packet(actual_payload) {
                Ok(csi) => {
                    debug!("Successfully parsed CSI packet. Timestamp: {}, Data len: {}", csi.timestamp_us, csi.csi_data.len());
                    if let Err(e) = csi_tx.try_send(csi) {
                        warn!("Failed to send CSI packet to channel (possibly full or disconnected): {}", e);
                    }
                }
                Err(e) => {
                    warn!("Failed to parse CSI packet from payload: {}", e);
                }
            }
        }
    }

    pub fn send_command(&mut self, cmd: Command, data: Option<&[u8]>) -> Result<()> {
        if !self.connected.load(AtomicOrdering::SeqCst) && self.reader_thread_handle.is_none() {
            return Err(EspError::NotConnected.into());
        }

        let mut command_packet = [0u8; CMD_PACKET_TOTAL_SIZE_HOST_TO_ESP];
        command_packet[0..CMD_PREAMBLE_HOST_TO_ESP.len()].copy_from_slice(&CMD_PREAMBLE_HOST_TO_ESP);
        let cmd_byte_offset = CMD_PREAMBLE_HOST_TO_ESP.len();
        command_packet[cmd_byte_offset] = cmd as u8;
        let data_offset = cmd_byte_offset + 1;

        if let Some(d) = data {
            if d.len() > CMD_DATA_MAX_LEN_HOST_TO_ESP {
                return Err(EspError::Protocol(format!(
                    "Command data too large: {} bytes (max {})",
                    d.len(), CMD_DATA_MAX_LEN_HOST_TO_ESP
                )).into());
            }
            command_packet[data_offset..data_offset + d.len()].copy_from_slice(d);
        }

        debug!(
            "Sending command: {:?} with data_len {} (total packet_len {} fixed)",
            cmd,
            data.map_or(0, |d| d.len()),
            CMD_PACKET_TOTAL_SIZE_HOST_TO_ESP
        );

        for attempt in 1..=3 {
            self.port.write_all(&command_packet)?;
            self.port.flush().context("Failed to flush serial port after command write")?;

            match self.ack_rx.recv_timeout(Duration::from_millis(500 * attempt as u64)) {
                Ok((ack_cmd, _ack_data)) => { // _ack_data to silence warning, still available if needed
                    if ack_cmd == cmd {
                        info!("ACK received for command: {:?}", cmd);
                        return Ok(());
                    } else {
                        debug!(
                            "Received ACK for different command: {:?} (expected {:?}), attempt {}",
                            ack_cmd, cmd, attempt
                        );
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    debug!("ACK timeout for command {:?} on attempt {}", cmd, attempt);
                }
                Err(RecvTimeoutError::Disconnected) => {
                    error!("ACK channel disconnected while waiting for ACK for {:?}. Reader thread likely died.", cmd);
                    self.connected.store(false, AtomicOrdering::SeqCst);
                    return Err(EspError::NotConnected.into());
                }
            }
        }
        error!("AckTimeout after 3 attempts for command {:?}", cmd);
        Err(EspError::AckTimeout(cmd).into())
    }

    pub fn csi_receiver(&self) -> Receiver<CsiPacket> {
        self.csi_rx.clone()
    }

    pub fn disconnect(&mut self) -> Result<()> {
        info!("Disconnecting from ESP32...");
        if !self.connected.load(AtomicOrdering::SeqCst) && self.reader_thread_handle.is_none() {
            info!("Already disconnected or never connected.");
            return Ok(());
        }
        self.connected.store(false, AtomicOrdering::SeqCst);

        if let Some(handle) = self.reader_thread_handle.take() {
            info!("Waiting for reader thread to join...");
            match handle.join() {
                Ok(_) => info!("ESP32 reader thread joined successfully."),
                Err(_e) => { // _e to silence warning if not formatting it
                    error!("Failed to join ESP32 reader thread: (panic occurred)");
                    return Err(EspError::ThreadPanic.into());
                }
            }
        } else {
            info!("No active reader thread to join.");
        }
        info!("ESP32 disconnected.");
        Ok(())
    }

    pub fn set_channel(&mut self, channel: u8) -> Result<()> {
        if !(1..=14).contains(&channel) {
            return Err(EspError::Config(format!("Invalid WiFi channel: {}", channel)).into());
        }
        // Update local config only after ACK
        self.send_command(Command::SetChannel, Some(&[channel]))?;
        self.config.channel = channel;
        Ok(())
    }

    pub fn apply_device_config(&mut self) -> Result<()> {
        let mut cmd_data = [0u8; 5];
        cmd_data[0] = self.config.mode as u8;
        cmd_data[1] = self.config.bandwidth as u8;
        cmd_data[2] = self.config.secondary_channel as u8;
        cmd_data[3] = self.config.csi_type as u8;
        cmd_data[4] = self.config.manual_scale;

        if self.config.bandwidth == Bandwidth::Forty && self.config.secondary_channel == SecondaryChannel::None {
            return Err(EspError::Config("Must set secondary channel when using 40MHz bandwidth".into()).into());
        }

        info!("Applying device config: Mode={:?}, BW={:?}, ChanSec={:?}, CSIType={:?}, Scale={}", 
            self.config.mode, self.config.bandwidth, self.config.secondary_channel, self.config.csi_type, self.config.manual_scale);

        // Send the main configuration command
        self.send_command(Command::ApplyDeviceConfig, Some(&cmd_data))?;
        info!("ApplyDeviceConfig ACKed.");

        // After successfully applying the config, manage CSI acquisition state
        if self.config.mode == OperationMode::Receive {
            info!("Current mode is Receive. Attempting to UNPAUSE CSI acquisition.");
            // Explicitly ignore error here for now, but log it. 
            // Some firmwares might auto-unpause or not need it if already unpaused.
            if let Err(e) = self.unpause_acquisition() {
                warn!("Failed to UNPAUSE acquisition after applying config (this might be okay if already unpaused): {}", e);
            } else {
                info!("UnpauseAcquisition ACKed or sent successfully.");
            }
        } else { // For Transmit mode or any other future modes
            info!("Current mode is {:?}. Attempting to PAUSE CSI acquisition.", self.config.mode);
            if let Err(e) = self.pause_acquisition() {
                warn!("Failed to PAUSE acquisition after applying config (this might be okay if already paused): {}", e);
            } else {
                info!("PauseAcquisition ACKed or sent successfully.");
            }
        }
        Ok(())
    }

    pub fn add_mac_filter(&mut self, src_mac: &[u8; 6], dst_mac: &[u8; 6]) -> Result<()> {
        let mut filter_data = [0u8; 12];
        filter_data[0..6].copy_from_slice(src_mac);
        filter_data[6..12].copy_from_slice(dst_mac);
        self.send_command(Command::WhitelistAddMacPair, Some(&filter_data))
    }

    pub fn clear_mac_filters(&mut self) -> Result<()> {
        self.send_command(Command::WhitelistClear, None)
    }

    pub fn pause_acquisition(&mut self) -> Result<()> {
        self.send_command(Command::PauseAcquisition, None)
    }

    pub fn unpause_acquisition(&mut self) -> Result<()> {
        self.send_command(Command::UnpauseAcquisition, None)
    }

    pub fn pause_wifi_transmit(&mut self) -> Result<()> {
        self.send_command(Command::PauseWifiTransmit, None)
    }

    pub fn resume_wifi_transmit(&mut self) -> Result<()> {
        self.send_command(Command::ResumeWifiTransmit, None)
    }

    pub fn synchronize_time(&mut self) -> Result<()> {
        self.send_command(Command::SynchronizeTimeInit, None)?;
        let time_us = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| EspError::Protocol(format!("System time error: {}", e)))?
            .as_micros() as u64;
        self.send_command(Command::SynchronizeTimeApply, Some(&time_us.to_le_bytes()))
    }

    pub fn transmit_custom_frame(&mut self, src_mac: &[u8; 6], dst_mac: &[u8; 6], n_reps: i32, pause_ms: i32) -> Result<()> {
        if self.config.mode != OperationMode::Transmit {
            return Err(EspError::Config("Device must be in transmit mode (OperationMode::Transmit) for this command.".into()).into());
        }
        let mut tx_data = [0u8; 20];
        tx_data[0..6].copy_from_slice(dst_mac);
        tx_data[6..12].copy_from_slice(src_mac);
        tx_data[12..16].copy_from_slice(&n_reps.to_le_bytes());
        tx_data[16..20].copy_from_slice(&pause_ms.to_le_bytes());
        self.send_command(Command::TransmitCustomFrame, Some(&tx_data))
    }

    // --- Public getters and setters for config fields ---
    // These allow cli_test_tool.rs to modify the config and then call apply_device_config()
    pub fn get_current_config(&self) -> &DeviceConfig {
        &self.config
    }
    pub fn update_config_mode(&mut self, mode: OperationMode) { self.config.mode = mode; }
    pub fn update_config_channel_local(&mut self, channel: u8) { self.config.channel = channel; } // Renamed to avoid confusion with set_channel command
    pub fn update_config_bandwidth(&mut self, bandwidth: Bandwidth) { self.config.bandwidth = bandwidth; }
    pub fn update_config_secondary_channel(&mut self, secondary_channel: SecondaryChannel) { self.config.secondary_channel = secondary_channel; }
    pub fn update_config_csi_type(&mut self, csi_type: CsiType) { self.config.csi_type = csi_type; }
    pub fn update_config_manual_scale(&mut self, manual_scale: u8) { self.config.manual_scale = manual_scale; }

    pub fn get_subcarrier_indices(&self) -> Vec<i32> {
        match (self.config.bandwidth, self.config.csi_type) {
            (Bandwidth::Twenty, CsiType::LegacyLTF) => {
                (1..27).chain(-26..0).collect()
            },
            (Bandwidth::Twenty, CsiType::HTLTF) => {
                (1..29).chain(-28..0).collect()
            },
            (Bandwidth::Forty, CsiType::LegacyLTF) => {
                match self.config.secondary_channel {
                    SecondaryChannel::Above => {
                        (-58..-32).chain(-31..-5).collect()
                    },
                    SecondaryChannel::Below => {
                        (6..32).chain(33..59).collect()
                    },
                    _ => {
                        warn!("Invalid secondary channel for 40MHz L-LTF, returning empty subcarrier indices.");
                        Vec::new()
                    },
                }
            },
            (Bandwidth::Forty, CsiType::HTLTF) => {
                (2..59).chain(-58..-1).collect()
            },
        }
    }

    pub fn get_subcarrier_mask(&self) -> Vec<usize> {
        match (self.config.bandwidth, self.config.csi_type) {
            (Bandwidth::Twenty, CsiType::LegacyLTF) => {
                (1..27).chain(38..64).collect()
            },
            (Bandwidth::Twenty, CsiType::HTLTF) => {
                (1..29).chain(36..64).collect()
            },
            (Bandwidth::Forty, CsiType::LegacyLTF) => {
                (6..32).chain(33..59).collect()
            },
            (Bandwidth::Forty, CsiType::HTLTF) => {
                (2..59).chain(70..127).collect()
            },
        }
    }
}

impl Drop for Esp32 {
    fn drop(&mut self) {
        if self.connected.load(AtomicOrdering::SeqCst) || self.reader_thread_handle.is_some() {
            info!("Esp32 instance dropped, ensuring disconnection.");
            if let Err(e) = self.disconnect() {
                error!("Error during disconnect on drop: {}", e);
            }
        }
    }
}

fn parse_csi_packet(payload_data: &[u8]) -> Result<CsiPacket> {
    debug!("Attempting to parse CSI packet from payload_data of len: {}", payload_data.len());
    // Minimum size: 8(ts) + 6(smac) + 6(dmac) + 2(seq) + 1(rssi) + 1(agc) + 1(fft) + 2(csi_len_field) = 27
    if payload_data.len() < 27 {
        return Err(EspError::Protocol(format!(
            "CSI payload too small for header: {} bytes, expected at least 27",
            payload_data.len()
        )).into());
    }

    let mut cursor = Cursor::new(payload_data);

    let timestamp_us = cursor.read_u64::<LittleEndian>()?;
    let mut src_mac = [0u8; 6];
    cursor.read_exact(&mut src_mac)?;
    let mut dst_mac = [0u8; 6];
    cursor.read_exact(&mut dst_mac)?;
    let seq = cursor.read_u16::<LittleEndian>()?;
    let rssi = cursor.read_i8()?;
    let agc_gain = cursor.read_u8()?;
    let fft_gain = cursor.read_u8()?;
    let csi_data_len = cursor.read_u16::<LittleEndian>()? as usize;
    debug!("CSI header parsed: ts={}, smac={:02X?}, dmac={:02X?}, seq={}, rssi={}, agc={}, fft={}, declared_csi_data_len={}",
        timestamp_us, src_mac, dst_mac, seq, rssi, agc_gain, fft_gain, csi_data_len);


    let remaining_buffer_after_header = payload_data.len() - cursor.position() as usize;
    if csi_data_len > remaining_buffer_after_header {
        return Err(EspError::Protocol(format!(
            "CSI data length field ({}) exceeds remaining buffer size ({}) after parsing header. Payload start: {:02X?}",
            csi_data_len, remaining_buffer_after_header, &payload_data[..std::cmp::min(payload_data.len(), 32)]
        )).into());
    }
    if csi_data_len == 0 && remaining_buffer_after_header > 0 {
        warn!("CSI data length is 0 but buffer has {} remaining bytes. This might be an issue.", remaining_buffer_after_header);
    }


    let mut csi_data = vec![0i8; csi_data_len];
    // Read csi_data using read_exact if possible for efficiency, or loop
    // For i8, a loop is fine or read_exact into a u8 slice and transmute (unsafe) or copy.
    // Current loop is safe.
    for i in 0..csi_data_len {
        csi_data[i] = cursor.read_i8().context(format!("Failed to read CSI data byte {}/{}", i, csi_data_len))?;
    }
    debug!("Successfully read {} CSI data samples.", csi_data_len);

    Ok(CsiPacket {
        timestamp_us,
        src_mac,
        dst_mac,
        seq,
        rssi,
        agc_gain,
        fft_gain,
        csi_data,
    })
}