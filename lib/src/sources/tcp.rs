use crate::errors::DataSourceError;
use crate::sources::DataSourceT;
use crate::sources::controllers::Controller;
use crate::network::tcp::{TCPClient, Connection};
use crate::network::rpc_messages::{RpcMessage, RpcMessageKind, DataMsg }
use std::net::SocketAddr;

use log::trace;
use serde::{Deserialize, Serialize};

// Configuration structure for a TCP source.
///
/// This struct is deserializable from YAML config files
#[derive(Serialize, Debug, Deserialize, Clone)]
pub struct TCPConfig {
    target_addr: SocketAddr
}

/// TCP-based implementation of the [`DataSourceT`] trait.
///
/// 
pub struct TCPSource {
    // whatever needs to be there
    pub client: TCPClient,
    target_addr: SocketAddr,
}

impl TCPSource {
    /// Create a new [`NetlinkSource`] from a configuration struct.
    ///
    /// # Errors
    /// Returns [`DataSourceError`] if configuration is invalid.
    pub fn new(config: TCPConfig) -> Result<Self, DataSourceError> {
        trace!("Creating new tcp source ");
        Ok(Self {
            client: TCPClient::new(),
            target_addr: config.target_addr,
        })
    }
}

/// Implements the CSI data source trait for tcp communication
///
/// Handles startup, shutdown, and frame-by-frame payload reading from the
/// Linux netlink connector interface.
#[async_trait::async_trait]
impl DataSourceT for TCPSource {
    async fn start(&mut self) -> Result<(), DataSourceError> {
        trace!(
            "Connecting to tcp socket with Socket adress{0}", target_addr
        );
        client.connect(self.target_addr).await;
        Ok(())
        
    }

    async fn stop(&mut self) -> Result<(), DataSourceError> {
        trace!(
            "Stopping data collection from tcp",
        );
        client.disconnect(self.target_addr).await;
        Ok(())
    }

    async fn read(&mut self, msg: DataMsg) -> Result<usize, DataSourceError> {
        // whatever needs to be done to read the connection
        let rpcmsg = client.read_message(self.target_addr).await;
        match rpcmsg {
            RpcMessage{ msg , ...} {
                match msg {
                    RpcMessageKind::DataMsg
                }
            }
            
        }
    }
}
