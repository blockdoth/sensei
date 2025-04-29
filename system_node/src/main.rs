use async_trait::async_trait;

use common::RpcEnvelope;

struct SystemNode {
    //Read node.toml
    //Register itself and every device with the registry
    //For each device spawn a raw source task
}

impl SystemNode {
    fn new() -> Self {
        //Read node.toml
        //Register itself and devices with the registry
        //For each device spawn a raw source task
        SystemNode {}
    }
}

#[async_trait]
trait WifiController {
    async fn apply(&self, cfg: RadioConfig) -> anyhow::Result<()>;
}

#[async_trait]
trait DataPipeline {
    async fn subscribe(&self, mode: AdapterMode) -> anyhow::Result<FrameStream>;
}

fn main() {
    println!("Hello, world!");
}
