use async_trait::async_trait;
use common::adapter_mode::AdapterMode;
use common::radio_config::RadioConfig;
use common::{deserialize_envelope};
use std::env;
use tokio::net::UdpSocket;

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

#[tokio::main]
pub async fn run(addr:String , port:u16) -> anyhow::Result<()> {
    //Initialize a new node
    SystemNode::new();

    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: crate_a <local_port>");
        std::process::exit(1);
    }
    let local_port = &args[1];
    let local_addr = format!("{addr}:{port}"); //Create a local address based on arguments
    let remote_addr = "127.0.0.1:8081"; //Hardcoded address for remote
    let socket = UdpSocket::bind(&local_addr).await?;

    //Respond to message
    loop {
        let mut buf = [0u8; 1024];
        match socket.recv(&mut buf).await {
            Ok(received) => {
                let msg = deserialize_envelope(&buf[..received]);
                println!("{msg:?} received");
            }
            Err(e) => {println!("Received error: {e}");}
        }
    }
}
