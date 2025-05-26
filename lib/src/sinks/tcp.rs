use crate::errors::SinkError;
use crate::network::rpc_message::{DataMsg, RpcMessage, RpcMessageKind};
use crate::network::tcp::client::TcpClient;
use crate::sinks::Sink;
use async_trait::async_trait;
use log::trace;
use std::net::SocketAddr;

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct TCPConfig {
    pub target_addr: SocketAddr,
    pub device_id: u64,
}

pub struct TCPSink {
    client: TcpClient,
    target_addr: SocketAddr,
    device_id: u64,
}

impl TCPSink {
    pub async fn new(config: TCPConfig) -> Result<Self, SinkError> {
        trace!("Creating new TCPSink for {}", config.target_addr);
        Ok(Self {
            client: TcpClient::new(),
            target_addr: config.target_addr,
            device_id: config.device_id,
        })
    }
}

#[async_trait]
impl Sink for TCPSink {
    async fn open(&mut self, _data: DataMsg) -> Result<(), SinkError> {
        trace!("Connecting to TCP socket at {}", self.target_addr);
        self.client
            .connect(self.target_addr)
            .await
            .map_err(SinkError::from)?;
        Ok(())
    }

    async fn close(&mut self, _data: DataMsg) -> Result<(), SinkError> {
        trace!("Disconnecting from TCP socket at {}", self.target_addr);
        self.client
            .disconnect(self.target_addr)
            .await
            .map_err(SinkError::from)?;
        Ok(())
    }

    async fn provide(&mut self, data: DataMsg) -> Result<(), SinkError> {
        let ret = RpcMessageKind::Data {
            data_msg: data,
            device_id: self.device_id,
        };

        self.client
            .send_message(self.target_addr, ret)
            .await
            .map_err(SinkError::from)?;
        Ok(())
    }
}
