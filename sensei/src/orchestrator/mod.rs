use crate::cli::{GlobalConfig, OrchestratorSubcommandArgs};
use crate::module::*;
use lib::rpc_envelope::CtrlMsg::{Heartbeat, Subscribe, Unsubscribe};
use lib::rpc_envelope::{AdapterMode, RpcEnvelope, send_envelope};
use lib::rpc_envelope::{deserialize_envelope, serialize_envelope};
use std::arch::global_asm;
use std::sync::Arc;
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;

use std::net::SocketAddr;

pub struct Orchestrator {
    socket_addr: SocketAddr,
}

impl RunsServer for Orchestrator {
    async fn start_server(&self) -> Result<(), Box<dyn std::error::Error>> {
        println!("Starting orchestrator");

        let socket = Arc::new(UdpSocket::bind(self.socket_addr).await?);
        let recv_socket = Arc::clone(&socket);
        let send_socket = Arc::clone(&socket);

        let recv_task = self.recv_task(recv_socket);

        let send_task = self.send_task(send_socket);

        tokio::try_join!(recv_task, send_task)?;

        Ok(())
    }
}

impl CliInit<OrchestratorSubcommandArgs> for Orchestrator {
    fn init(config: &OrchestratorSubcommandArgs, global: &GlobalConfig) -> Self {
        Orchestrator {
            socket_addr: global.socket_addr,
        }
    }
}

impl Orchestrator {
    fn recv_task(&self, recv_socket: Arc<UdpSocket>) -> JoinHandle<()> {
        tokio::spawn(async move {
            //Wait for a response
            loop {
                let mut buf = [0u8; 1024];
                match recv_socket.recv_from(&mut buf).await {
                    Ok((len, addr)) => {
                        let msg = deserialize_envelope(&buf[..len]);
                        println!("{msg:?} received");
                    }
                    Err(e) => {
                        println!("Received error {e:?}");
                    }
                }
            }
        })
    }

    fn send_task(&self, send_socket: Arc<UdpSocket>) -> JoinHandle<()> {
        let socket_addr_local = self.socket_addr;

        tokio::spawn(async move {
            // Create the input reader
            let stdin: BufReader<io::Stdin> = BufReader::new(io::stdin());
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
                let addr: SocketAddr = match parts.next().unwrap().to_string().parse() {
                    Ok(socket_addr) => socket_addr,
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
                            sink_addr: socket_addr_local,
                            // TODO proper device id's
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
                            sink_addr: socket_addr_local,
                            device_id: 1,
                        });
                        let envelope = serialize_envelope(msg);
                        send_envelope(send_socket.clone(), envelope, addr).await;
                    }
                    _ => {
                        eprintln!("Unknown message type: {message_type}");
                        continue;
                    }
                }

                io::stdout().flush(); // Ensure prompt shows up again
            }

            println!("Send loop ended (stdin closed).");
        })
    }
}
