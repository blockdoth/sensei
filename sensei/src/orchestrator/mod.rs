use crate::cli::{GlobalConfig, OrchestratorSubcommandArgs};
use crate::module::*;
use common::CtrlMsg::Heartbeat;
use common::RpcEnvelope;
use common::{deserialize_envelope, serialize_envelope};
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
        tokio::spawn(async move {
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
                let addr = match parts.next() {
                    Some(a) if !a.is_empty() => a,
                    _ => {
                        eprintln!("Invalid input. Format: <addr> <type>");
                        continue;
                    }
                };

                let message = match parts.next() {
                    Some(m) => m,
                    None => {
                        eprintln!("Missing message text.");
                        continue;
                    }
                };

                if message == "Heartbeat" || message.is_empty() {
                    let msg = RpcEnvelope::Ctrl(Heartbeat);
                    let data = serialize_envelope(msg);

                    match send_socket.send_to(&data, addr).await {
                        Ok(n) => println!("Sent {n} bytes to {addr}"),
                        Err(e) => eprintln!("Failed to send: {e}"),
                    }
                }

                print!("> ");
                let _ = io::stdout().flush().await; // Ensure prompt shows up again
            }

            println!("Send loop ended (stdin closed).");
        })
    }
}
