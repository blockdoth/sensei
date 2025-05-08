// Simple ESP32 CSI CLI in Rust
// - Opens a serial port (use the macOS "/dev/cu.usbmodemXXXX" CDC port)
// - Sends configuration commands (ApplyDeviceConfig, ChangeWifiChannel)
// - Either streams CSI or spams Wi-Fi packets to test
// - Prints raw CSI bytes or spam status

use std::env;
use std::io::{self, Read};
use std::thread;
use std::time::Duration;
use serialport::SerialPort;

// ESP32 command codes
const CMD_CLEAR_WHITELIST: u8     = 0x03;  // WhitelistClear
const CMD_UNPAUSE: u8             = 0x05;  // UnpauseAcquisition
const CMD_CHANGE_WIFI_CHANNEL: u8 = 0x01;  // ChangeWifiChannel
const CMD_APPLY_DEVICE_CONFIG: u8 = 0x06;  // ApplyDeviceConfig
const CMD_UNPAUSE_WIFI_TX: u8     = 0x08;  // UnpauseWifiTransmit

fn main() -> serialport::Result<()> {
    // Parse command-line args for CSI or Spam mode:
    // Usage for CSI: cargo run --bin test -- <port> <channel> <bw> <sec>
    // Usage for spam: cargo run --bin test -- spam <port> <channel>
    let args: Vec<String> = env::args().collect();
    let spam_mode = args.get(1).map(|s| s == "spam").unwrap_or(false);

    // Determine port and channel settings
    // Default to macOS CDC device
    let (port_name, channel, bw, sec) = if spam_mode {
        let port = args.get(2).cloned().unwrap_or_else(|| "/dev/cu.usbmodem1101".to_string());
        let ch = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(6);
        (port, ch, 0, 0)
    } else if args.len() >= 5 {
        (
            args[1].clone(),
            args[2].parse().unwrap_or(6),
            args[3].parse().unwrap_or(0),
            args[4].parse().unwrap_or(0),
        )
    } else {
        ("/dev/cu.usbmodem1234561".to_string(), 6, 0, 0)
    };

    // Open serial port
    let baud_rate = 921_600;
    let mut port = serialport::new(&port_name, baud_rate)
        .timeout(Duration::from_millis(100))
        .open()?;
    println!("Opened {} at {} baud", port_name, baud_rate);

    if spam_mode {
        // Configure ESP for transmit-only
        let mode: u8 = 2; // ActiveFREE (spam)
        let config_payload = [mode, 0, 0, 0, 0];
        send_command(&mut *port, CMD_APPLY_DEVICE_CONFIG, &config_payload)?;
        port.flush()?;
        thread::sleep(Duration::from_millis(50));

        send_command(&mut *port, CMD_CHANGE_WIFI_CHANNEL, &[channel])?;
        port.flush()?;
        thread::sleep(Duration::from_millis(50));

        send_command(&mut *port, CMD_UNPAUSE_WIFI_TX, &[])?;
        port.flush()?;
        println!("ESP32 is now spamming packets on channel {}...", channel);
        // Keep alive
        loop { thread::sleep(Duration::from_secs(1)); }
    }

    // === CSI Mode ===
    let mode: u8 = 0; // ActiveSTA
    let config_payload = [mode, bw as u8, 0, 1, 0];

    send_command(&mut *port, CMD_APPLY_DEVICE_CONFIG, &config_payload)?;
    port.flush()?;
    thread::sleep(Duration::from_millis(50));

    send_command(&mut *port, CMD_CHANGE_WIFI_CHANNEL, &[channel])?;
    port.flush()?;
    thread::sleep(Duration::from_millis(50));

    send_command(&mut *port, CMD_CLEAR_WHITELIST, &[])?;
    port.flush()?;
    thread::sleep(Duration::from_millis(20));

    send_command(&mut *port, CMD_UNPAUSE, &[])?;
    port.flush()?;
    thread::sleep(Duration::from_millis(50));

    // Read CSI data continuously
    let mut buf = [0u8; 1024];
    println!("Streaming CSI data on channel {}... Ctrl+C to stop", channel);
    loop {
        match port.read(&mut buf) {
            Ok(n) if n > 0 => print_bytes(&buf[..n]),
            Err(ref e) if e.kind() == io::ErrorKind::TimedOut => continue,
            Err(e) => { eprintln!("Serial read error: {}", e); break; },
            _ => (),
        }
    }

    Ok(())
}

/// Send a command packet: [CMD][payload...]
const PREAMBLE: [u8; 4] = [0xC3, 0xC3, 0xC3, 0xC3];
const PACKET_LEN: usize = 128;

fn send_command(port: &mut dyn SerialPort, cmd: u8, data: &[u8]) -> serialport::Result<()> {
    assert!(data.len() < 124, "payload too large");

    // Build a 128-byte frame
    let mut frame = [0u8; PACKET_LEN];
    //  0..4: preamble
    frame[0..4].copy_from_slice(&PREAMBLE);
    //      4: command
    frame[4] = cmd;
    // 5..5+data.len(): payload
    frame[5..5 + data.len()].copy_from_slice(data);
    // rest is already zero

    port.write_all(&frame)?;
    // optional: wait for ACK here before returning
    Ok(())
}

/// Helper: print bytes in hex
fn print_bytes(bytes: &[u8]) {
    for b in bytes { print!("{:02X} ", b); }
    println!();
}