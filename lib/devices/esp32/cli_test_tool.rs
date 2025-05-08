//! CLI test harness for ESP32 CSI and transmit modes
//! Uses the `Esp32` client to configure, stream CSI, or spam Wi-Fi packets.

use std::env;
use std::thread;
use std::time::Duration;
mod esp32;
use esp32::{Esp32, OperationMode, SecondaryChannel, CsiType, Command};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse args: either "spam <port> <channel>
    // or `<port> <channel> <bw> <sec>` for CSI
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} [spam <port> <channel>] | <port> <channel> <bw20(true|false)> <legacy_ltf(true|false)>", args[0]);
        std::process::exit(1);
    }

    if args[1] == "spam" {
        // Spam mode
        if args.len() != 4 {
            eprintln!("Usage: {} spam <port> <channel>", args[0]);
            std::process::exit(1);
        }
        let port = &args[2];
        let channel: u8 = args[3].parse()?;

        // Open and configure for TX
        let esp = Esp32::open(port)?;
        // ActiveSend free transmit (mode=2)
        esp.send_command(Command::ApplyDeviceCfg, &[2, 0, 0, 0, 0])?;
        thread::sleep(Duration::from_millis(50));
        esp.send_command(Command::SetChannel, &[channel])?;
        thread::sleep(Duration::from_millis(50));
        esp.send_command(Command::ResumeWifiTx, &[])?;

        println!("ESP32 is now spamming on channel {}. Ctrl+C to stop.", channel);
        loop { thread::sleep(Duration::from_secs(1)); }
    }

    // CSI mode
    if args.len() != 5 {
        eprintln!("Usage: {} <port> <channel> <bw20(true|false)> <legacy_ltf(true|false)>", args[0]);
        std::process::exit(1);
    }
    let port = &args[1];
    let channel: u8 = args[2].parse()?;
    let bw20: bool = args[3].parse()?;
    let legacy: bool = args[4].parse()?;

    let esp = Esp32::open(port)?;
    // Build config payload: [mode, bw, secondary, csi_type, manual]
    let mode = OperationMode::Rx as u8;
    let bw = if bw20 { 0 } else { 1 };
    let sec = SecondaryChannel::No as u8;
    let csi_type = if legacy { CsiType::LegacyLtf } else { CsiType::HtLtf };

    // Apply config
    esp.send_command(Command::ApplyDeviceCfg, &[mode, bw, sec, csi_type as u8, 0])?;
    thread::sleep(Duration::from_millis(50));
    esp.send_command(Command::SetChannel, &[channel])?;
    thread::sleep(Duration::from_millis(50));
    esp.send_command(Command::WhitelistClear, &[])?;
    thread::sleep(Duration::from_millis(20));
    esp.send_command(Command::UnpauseAcq, &[])?;

    println!("Streaming CSI on channel {}... Ctrl+C to stop.", channel);
    esp.stream_csi(move |pkt| {
        // Print parsed CSI values, e.g.: timestamp, rssi, and length
        println!("TS={} us RSSI={} len={}",
            pkt.timestamp_us,
            pkt.rssi,
            pkt.csi_data.len()
        );
    });

    // Keep application alive
    loop { thread::sleep(Duration::from_secs(1)); }
}