use common::AdapterMode;
use common::CtrlMsg::{Heartbeat, Subscribe, Unsubscribe};
use common::RpcEnvelope;
use common::{deserialize_envelope, serialize_envelope};
use std::sync::Arc;
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;

fn recv_task(recv_socket: Arc<UdpSocket>) -> JoinHandle<()> {
    tokio::spawn(async move {
        //Wait for a response
        loop {
            let mut buf = [0u8; 1024];
            match recv_socket.recv_from(&mut buf).await {
                Ok((len, addr)) => {
                    let msg = deserialize_envelope(&buf[..len]);
                    println!("{:?} received", msg);
                }
                Err(e) => {
                    println!("Received error {:?}", e);
                }
            }
        }
    })
}

async fn send_envelope(send_socket: Arc<UdpSocket>, envelope: Vec<u8>, addr: &str) {
    match send_socket.send_to(&envelope, addr).await {
        Ok(n) => println!("Sent {} bytes to {}", n, addr),
        Err(e) => eprintln!("Failed to send: {}", e),
    }
}

fn send_task(send_socket: Arc<UdpSocket>, send_addr: String, port: u16) -> JoinHandle<()> {
    tokio::spawn(async move {
        // Create the input reader
        let stdin = BufReader::new(io::stdin());
        let mut lines = stdin.lines();

        println!("Type: <target_addr> <type>");
        println!("Example: 127.0.0.1:8082 Heartbeat");

        while let Ok(Some(line)) = lines.next_line().await {
            let input = line.trim();
            if input.is_empty() {
                continue;
            }

            let mut parts = input.splitn(2, ' ');

            // Get address (with port) from input
            let addr = match parts.next() {
                Some(a) if !a.is_empty() => a,
                _ => {
                    eprintln!("Invalid input. Format: <addr> <type> <param1> <param2> <etc>");
                    continue;
                }
            };

            // Get the type of message from input
            let message_type = match parts.next() {
                Some(m) => m,
                None => {
                    eprintln!("Missing type.");
                    continue;
                }
            };
            match message_type.to_lowercase().as_str() {
                "heartbeat" => {
                    let msg = RpcEnvelope::Ctrl(Heartbeat);
                    let envelope = serialize_envelope(msg);
                    send_envelope(send_socket.clone(), envelope, addr).await;
                }
                "subscribe" => {
                    let msg = RpcEnvelope::Ctrl(Subscribe {
                        sink_addr: (send_addr).parse::<String>().unwrap() + ":" + &port.to_string(),
                        device_id: parts
                            .next()
                            .and_then(|s| s.parse::<u64>().ok())
                            .unwrap_or(1),
                        mode: match parts.next() {
                            Some("target") => AdapterMode::TARGET,
                            Some("source") => AdapterMode::SOURCE,
                            _ => AdapterMode::RAW,
                        },
                    });
                    let envelope = serialize_envelope(msg);
                    send_envelope(send_socket.clone(), envelope, addr).await;
                }
                "unsubscribe" => {
                    let msg = RpcEnvelope::Ctrl(Unsubscribe {
                        sink_addr: (send_addr).parse::<String>().unwrap() + ":" + &port.to_string(),
                        device_id: 1,
                    });
                    let envelope = serialize_envelope(msg);
                    send_envelope(send_socket.clone(), envelope, addr).await;
                }
                _ => {
                    eprintln!("Unknown message type: {}", message_type);
                    continue;
                }
            }

            let _ = io::stdout().flush(); // Ensure prompt shows up again
        }

        println!("Send loop ended (stdin closed).");
    })
}

#[tokio::main]
pub async fn run(addr: String, port: u16) -> anyhow::Result<()> {
    let socket = Arc::new(UdpSocket::bind(format!("{}:{}", addr, port)).await?);
    let recv_socket = Arc::clone(&socket);
    let send_socket = Arc::clone(&socket);

    let recv_task = recv_task(recv_socket);

    let send_task = send_task(send_socket, addr, port);

    tokio::try_join!(recv_task, send_task)?;

    Ok(())
}
