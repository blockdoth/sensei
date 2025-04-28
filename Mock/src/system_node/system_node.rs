use async_trait::async_trait;

struct SystemNode {
    //Read node.toml
    //Register itself and every device with the registry
    //For each device spawn a raw source task
}

#[async_trait]
trait WifiController {
    async fn apply(&self, cfg: RadioConfig) -> anyhow::Result<()>;
}

#[async_trait]
trait DataPipeline {
    async fn subscribe(&self, mode: AdapterMode) -> anyhow::Result<FrameStream>;
}