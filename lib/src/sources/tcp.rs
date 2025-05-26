use crate::errors::DataSourceError;
use crate::sources::DataSourceT;
use crate::network::tcp::client::TcpClient;
use crate::network::rpc_message::{RpcMessage, RpcMessageKind, DataMsg};
use std::net::SocketAddr;
use log::trace;

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
pub struct TCPConfig {
    target_addr: SocketAddr,
}

pub struct TCPSource {
    client: TcpClient,
    target_addr: SocketAddr,
}

impl TCPSource {
    pub fn new(config: TCPConfig) -> Result<Self, DataSourceError> {
        trace!("Creating new TCPSource for {}", config.target_addr);
        Ok(Self {
            client: TcpClient::new(),
            target_addr: config.target_addr,
        })
    }
}

#[async_trait::async_trait]
impl DataSourceT for TCPSource {
    async fn start(&mut self) -> Result<(), DataSourceError> {
        trace!("Connecting to TCP socket at {}", self.target_addr);
        self.client
            .connect(self.target_addr)
            .await
            .map_err(DataSourceError::from)?;
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), DataSourceError> {
        trace!("Disconnecting from TCP socket at {}", self.target_addr);
        self.client
            .disconnect(self.target_addr)
            .await
            .map_err(DataSourceError::from)?;
        Ok(())
    }

    async fn read_buf(&mut self, buf: &mut [u8]) -> Result<usize, DataSourceError> {
        // you can either proxy to the client or leave unimplemented
        Err(DataSourceError::ReadBuf)
    }

    async fn read(&mut self) -> Result<Option<DataMsg>, DataSourceError> {
        let rpcmsg: RpcMessage = self
            .client
            .read_message(self.target_addr)
            .await
            .map_err(DataSourceError::from)?;

        match rpcmsg.msg {
            RpcMessageKind::Ctrl(_ctrl_msg) => {
                // control messages carry no data payload
                Ok(None)
            }
            RpcMessageKind::Data { data_msg, .. } => {
                // return the actual data payload
                Ok(Some(data_msg))
            }
        }
    }

}

