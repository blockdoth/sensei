use async_trait::async_trait;
use tokio::net::UdpSocket;
use common::adapter_mode::AdapterMode;
use common::CtrlMsg::{Configure, Heartbeat};
use common::radio_config::RadioConfig;
use common::RpcEnvelope;
use common::{serialize_envelope, deserialize_envelope};

struct SystemNode {
    devices: Vec<u64>
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
async fn main() -> anyhow::Result<()> {
    SystemNode::new();
    let socket = UdpSocket::bind("127.0.0.1:8082").await?;

    //Wait until receive a message
    let mut buf  = [0u8; 1024];
    let (len, addr) = socket.recv_from(&mut buf).await?;
    let msg = deserialize_envelope(&buf[..len]);
    println!("{:?} received", msg);
    
    //Respond to message
    let response = serialize_envelope(RpcEnvelope::Ctrl(Heartbeat));
    socket.send_to(&response, &addr).await?;
    
    Ok(())
}
