use async_trait::async_trait;
use common::{AdapterMode, RpcEnvelope};
use common::CtrlMsg::{Heartbeat, Subscribe, Unsubscribe};
use common::RpcEnvelope::{Ctrl, Data};
use common::deserialize_envelope;
use common::radio_config::RadioConfig;
use std::env;
use std::sync::{Arc, Mutex};
use std::thread::sleep;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;
use common::DataMsg::{CsiFrame, RawFrame};

struct SystemNode {
    devices: Vec<u64>,
}

impl SystemNode {
    fn new() -> Self {
        //Read node.toml
        //Register itself and devices with the registry
        //For each device spawn a raw source task
        SystemNode {
            devices: Vec::new(),
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

fn recv_task(recv_socket: Arc<UdpSocket>, remote_addrs: Arc<Mutex<Vec<(String, u64, AdapterMode)>>>) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let mut buf = [0u8; 1024];
            match recv_socket.recv(&mut buf).await {
                Ok(received) => {
                    let msg = deserialize_envelope(&buf[..received]);
                    println!("{:?} received", msg);
                    match msg {
                        Ctrl(command) => match command {
                            Subscribe {
                                sink_addr,
                                device_id,
                                mode,
                            } => {
                                // You must lock the reference in order to edit
                                // TODO: find a better way of doing this such that this does not potentially lock while reading (maybe not an issue)
                                remote_addrs.lock().unwrap().push((sink_addr.to_string(), device_id, mode));
                                println!("Subscribed by {}", sink_addr);
                            }
                            Unsubscribe {
                                sink_addr,
                                device_id,
                            } => {
                                remote_addrs.lock().unwrap().retain(|(tup1, tup2, _)| {
                                    !(*tup1 == sink_addr && *tup2 == device_id)
                                });
                                println!("Unsubscribed by {}", sink_addr);
                            }
                            Heartbeat {} => {
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
                    println!("Received error: {}", e);
                }
            }
        }
    })
}

fn send_task(send_socket: Arc<UdpSocket>, remote_addrs: Arc<Mutex<Vec<(String, u64, AdapterMode)>>>) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            // Periodically send data to all subscribed devices. 
            // TODO: not periodically but based on input from devices
            tokio::time::sleep(Duration::from_secs(5)).await;
            println!("{}", remote_addrs.lock().unwrap().len());
        }
    })
}

#[tokio::main]
pub async fn run(addr: String, port: u16) -> anyhow::Result<()> {
    //Initialize a new node
    SystemNode::new();

    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: crate_a <local_port>");
        std::process::exit(1);
    }
    let local_port = &args[1];
    let local_addr = format!("{}:{}", addr, port); //Create a local address based on arguments
    let remote_addr = "127.0.0.1:8081"; //Hardcoded address for remote
    let socket = Arc::new(UdpSocket::bind(&local_addr).await?); // The socket based on the address for this node
    let remote_addrs: Arc<Mutex<Vec<(String, u64, AdapterMode)>>> = Arc::new(Mutex::new(Vec::new())); // List of subscribed addresses
    let mut local_devices: Vec<u64> = Vec::new();
    
    // Using cloning, we can create two references to the same pointer and use these in different threads.
    tokio::try_join!(send_task(socket.clone(), remote_addrs.clone()), recv_task(socket.clone(), remote_addrs.clone()))?;
    
    Ok(())
}
