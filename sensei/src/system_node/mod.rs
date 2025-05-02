use crate::cli::*;
use crate::cli::{SubCommandsArgsEnum, SystemNodeSubcommandArgs};
use crate::module::*;
use argh::FromArgs;
use async_trait::async_trait;
use common::CtrlMsg::{Heartbeat, Subscribe, Unsubscribe};
use common::DataMsg::{CsiFrame, RawFrame};
use common::RpcEnvelope::{Ctrl, Data};
use common::deserialize_envelope;
use common::radio_config::RadioConfig;
use common::{AdapterMode, RpcEnvelope};
use std::env;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::thread::sleep;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;

pub struct SystemNode {
    socket_addr: SocketAddr,
    remote_addrs: Arc<Mutex<Vec<(SocketAddr, u64, AdapterMode)>>>, // Address, device id, adapter mode
    devices: Vec<u64>,
}

impl SystemNode {
    pub fn recv_task(
        &self,
        recv_socket: Arc<UdpSocket>,
        remote_addrs: Arc<Mutex<Vec<(SocketAddr, u64, AdapterMode)>>>,
    ) -> JoinHandle<()> {
        println!("receive");
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
                                    // You must lock the reference in order to edit
                                    // TODO: find a better way of doing this such that this does not potentially lock while reading (maybe not an issue)
                                    remote_addrs
                                        .lock()
                                        .unwrap()
                                        .push((sink_addr, device_id, mode));
                                    println!("Subscribed by {sink_addr}");
                                }
                                Unsubscribe {
                                    sink_addr,
                                    device_id,
                                } => {
                                    remote_addrs.lock().unwrap().retain(|(tup1, tup2, _)| {
                                        !(*tup1 == sink_addr && *tup2 == device_id)
                                    });
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

                            // TODO: Let nodes receive data messages
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

    pub fn send_task(
        &self,
        send_socket: Arc<UdpSocket>,
        remote_addrs: Arc<Mutex<Vec<(SocketAddr, u64, AdapterMode)>>>,
    ) -> JoinHandle<()> {
        println!("send task");
        tokio::spawn(async move {
            loop {
                // Periodically send data to all subscribed devices.
                // TODO: not periodically but based on input from devices
                tokio::time::sleep(Duration::from_secs(5)).await;
                println!("{}", remote_addrs.lock().unwrap().len());
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
        // Using cloning, we can create two references to the same pointer and use these in different threads.
        let send_task = self.send_task(socket.clone(), self.remote_addrs.clone());
        let recv_task = self.recv_task(socket.clone(), self.remote_addrs.clone());
        tokio::join!(send_task, recv_task);
        Ok(())
    }
}

impl CliInit<SystemNodeSubcommandArgs> for SystemNode {
    fn init(config: &SystemNodeSubcommandArgs, global: &GlobalConfig) -> Self {
        SystemNode {
            socket_addr: global.socket_addr,
            remote_addrs: Arc::new(Mutex::new(Vec::new())),
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
