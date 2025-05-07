use crate::cli::*;
use crate::cli::{SubCommandsArgsEnum, SystemNodeSubcommandArgs};
use crate::module::*;
use argh::FromArgs;
use async_trait::async_trait;
use lib::rpc_envelope::CtrlMsg::{Heartbeat, Subscribe, Unsubscribe};
use lib::rpc_envelope::DataMsg::{CsiFrame, RawFrame};
use lib::rpc_envelope::RadioConfig;
use lib::rpc_envelope::RpcEnvelope::{Ctrl, Data};
use lib::rpc_envelope::SourceType::ESP32;
use lib::rpc_envelope::{AdapterMode, RpcEnvelope};
use lib::rpc_envelope::{deserialize_envelope, send_envelope, serialize_envelope};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::UdpSocket;
use tokio::sync::watch;
use tokio::sync::watch::{Receiver, Sender};
use tokio::task::JoinHandle;

pub struct SystemNode {
    socket_addr: SocketAddr,
    devices: Vec<u64>,
}

impl SystemNode {
    pub fn recv_task(
        &self,
        recv_socket: Arc<UdpSocket>,
        recv_addr_new: Sender<Option<(SocketAddr, u64, AdapterMode)>>,
        recv_addr_old: Sender<Option<(SocketAddr, u64, AdapterMode)>>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                let mut buf = [0u8; 1024];
                match recv_socket.recv(&mut buf).await {
                    Ok(received) => {
                        let msg = deserialize_envelope(&buf[..received]);
                        println!("{msg:?} received");
                        match msg {
                            Ctrl(command) => match command {
                                Subscribe {
                                    sink_addr,
                                    device_id,
                                    mode,
                                } => {
                                    // Changes the address across the subscribe channel
                                    recv_addr_new
                                        .send(Option::from((sink_addr, device_id, mode)))
                                        .expect("Somehow the subscribe channel got disconnected");
                                    println!("Subscribed by {sink_addr}");
                                }
                                Unsubscribe {
                                    sink_addr,
                                    device_id,
                                } => {
                                    // Changes the address across the unsubscribe channel
                                    recv_addr_old
                                        .send(Option::from((
                                            sink_addr,
                                            device_id,
                                            AdapterMode::RAW,
                                        )))
                                        .expect("Somehow the unsubscribe channel got disconnected"); // Uses raw adapter mode as the default mode, as it doesn't matter
                                    println!("Unsubscribed by {sink_addr}");
                                }
                                Heartbeat => {
                                    // More of a test message for, can add other test functionality for messages here
                                    // Nodes are supposed to send heartbeats, not receive them
                                    // TODO: Maybe nodes can be pinged using heartbeats?
                                    println!("Heartbeat");
                                }

                                _ => println!("Unknown command"),
                            },

                            // TODO: Let nodes receive data messages (and pass these on to subscribers)
                            Data(payload) => {}
                        }
                    }
                    Err(e) => {
                        println!("Received error: {e}");
                    }
                }
            }
        })
    }

    pub fn send_data_task(
        &self,
        send_socket: Arc<UdpSocket>,
        mut send_addr_new: Receiver<Option<(SocketAddr, u64, AdapterMode)>>,
        mut send_addr_old: Receiver<Option<(SocketAddr, u64, AdapterMode)>>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut targets: Vec<(SocketAddr, u64, AdapterMode)> = Vec::new();
            loop {
                // Handles changes from the receiver channel concerning adding subscribers
                if send_addr_new.has_changed().unwrap_or(false) {
                    let current = send_addr_new.borrow_and_update().clone();
                    send_addr_new.mark_unchanged();
                    let current2 = current.clone().unwrap();
                    println!("Added {current2:?}");
                    targets.push(current2);
                }

                // Handles changes from the receiver channel concerning removing subscribers
                if send_addr_old.has_changed().unwrap_or(false) {
                    let current = send_addr_old.borrow_and_update().clone();
                    send_addr_old.mark_unchanged();
                    let current2 = current.clone().unwrap();
                    println!("Removed {current2:?}");
                    targets.retain(|x| x.0 != current2.0 && x.1 != current2.1);
                }

                // Periodically send data to all subscribed devices.
                for target in &targets {
                    let msg = Data(RawFrame {
                        ts: SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .expect("Clock went backwards")
                            .as_millis(),
                        bytes: Vec::from([234u8]),
                        source_type: ESP32,
                    }); //Random placeholder data
                    let envelope = serialize_envelope(msg);
                    send_envelope(send_socket.clone(), envelope, target.0).await;
                }

                // TODO: This should not be periodic. It should prepare data packets and send them once ready (implement once we have TCP)
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        })
    }
}

impl RunsServer for SystemNode {
    async fn start_server(&self) -> Result<(), Box<dyn std::error::Error>> {
        println!("Starting system node");
        println!("{}", self.socket_addr);
        let sock = UdpSocket::bind(self.socket_addr).await.unwrap();

        let socket = Arc::new(sock);

        let (recv_addr_new, mut send_addr_new) =
            watch::channel::<Option<(SocketAddr, u64, AdapterMode)>>(None); // Channel to add new subscribers
        let (recv_addr_old, mut send_addr_old) =
            watch::channel::<Option<(SocketAddr, u64, AdapterMode)>>(None); // Channel to remove old subscribers
        // Using cloning, we can create two references to the same pointer and use these in different threads.
        let send_task = self.send_data_task(socket.clone(), send_addr_new, send_addr_old);
        let recv_task = self.recv_task(socket.clone(), recv_addr_new, recv_addr_old);
        tokio::join!(send_task, recv_task);
        Ok(())
    }
}

impl CliInit<SystemNodeSubcommandArgs> for SystemNode {
    fn init(config: &SystemNodeSubcommandArgs, global: &GlobalConfig) -> Self {
        SystemNode {
            socket_addr: global.socket_addr,
            devices: vec![],
        }
    }
}

#[async_trait]
trait WifiController {
    async fn apply(&self, cfg: RadioConfig) -> anyhow::Result<()>;
}

#[async_trait]
trait DataPipeline {
    async fn subscribe(&self, mode: AdapterMode) -> anyhow::Result<()>;
}
