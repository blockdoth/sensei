//! TCP Client
//! ----------
//!
//! This TCP client is used to communicate with its corresponding sensei-specific
//! server counterpart, handling packets by a simple framing; Prepending a 4-byte
//! header that specifies the packet length.
//!
//! Note that a single client may connect/disconnect repeatedly, the main task of
//! this simple tokio TCP wrapper.
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use log::{debug, error, info};
#[cfg(test)]
use mockall::automock;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::Mutex;

use crate::errors::NetworkError;
use crate::network::rpc_message::RpcMessageKind::*;
use crate::network::rpc_message::{HostCtrl, RpcMessage, RpcMessageKind};
use crate::network::tcp::{self, CONNECTION_TIMEOUT, MAX_MESSAGE_LENGTH};

/// Tcp Client
pub struct TcpClient {
    //TODO look if using SocketAddr is fine to use as key
    connections: Arc<Mutex<HashMap<SocketAddr, Connection>>>,
    pub addr: Option<SocketAddr>,
}

#[derive(Debug)]
pub struct Connection {
    // A temp buffer to deal with fragmentation
    addr: SocketAddr,
    write_stream: OwnedWriteHalf,
    read_stream: OwnedReadHalf,
    pub buffer: Vec<u8>,
}

impl Default for TcpClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg_attr(test, automock)]
impl TcpClient {
    pub fn new() -> Self {
        Self {
            connections: Arc::new(Mutex::new(HashMap::new())),
            addr: None,
        }
    }

    pub async fn get_connections(&self) -> Vec<SocketAddr> {
        self.connections.lock().await.values().map(|conn| conn.addr).collect()
    }

    pub async fn connect(&mut self, target_addr: SocketAddr) -> Result<(), NetworkError> {
        if let Some(connection) = self.connections.lock().await.get(&target_addr) {
            let target_addr = connection.read_stream.peer_addr()?;
            info!("Already connected to {target_addr}");
            return Ok(());
        }

        let connection = tokio::time::timeout(CONNECTION_TIMEOUT, TcpStream::connect(target_addr)).await?;

        match connection {
            Ok(stream) => {
                let (read_stream, mut write_stream) = stream.into_split();

                let target_addr = write_stream.peer_addr()?;
                let src_addr = write_stream.local_addr()?;

                tcp::send_message(&mut write_stream, HostCtrl(HostCtrl::Connect)).await?;

                self.connections.lock().await.insert(
                    target_addr,
                    Connection {
                        addr: write_stream.peer_addr().unwrap(),
                        buffer: vec![0; MAX_MESSAGE_LENGTH],
                        write_stream,
                        read_stream,
                    },
                );
                info!("Connected to {target_addr} from {src_addr}");
                self.addr = Some(src_addr);
                Ok(())
            }
            Err(e) => {
                error!("Connection attempt to {target_addr} timed out.");
                Err(NetworkError::UnableToConnect)
            }
        }
    }

    pub async fn disconnect(&mut self, target_addr: SocketAddr) -> Result<(), NetworkError> {
        tokio::time::timeout(CONNECTION_TIMEOUT, async move {
            let connection = {
                let mut connections = self.connections.lock().await;
                connections.remove(&target_addr)
            };

            match connection {
                None => {
                    error!("No connection found with {target_addr}");
                    Err(NetworkError::Closed)
                }
                Some(mut connection) => {
                    debug!("Initiating graceful disconnect with {target_addr}");
                    match super::send_message(&mut connection.write_stream, HostCtrl(HostCtrl::Disconnect)).await {
                        Ok(_) => {
                            debug!("Waiting for confirm of disconnect from {target_addr}");
                            match super::read_message(&mut connection.read_stream, &mut connection.buffer).await {
                                Ok(Some(RpcMessage {
                                    msg: HostCtrl(HostCtrl::Disconnect),
                                    ..
                                })) => {
                                    info!("Connection with {target_addr} closed gracefully.");
                                    if let Err(e) = connection.write_stream.shutdown().await {
                                        error!("Failed to shutdown write stream: {e}");
                                    }
                                    Ok(())
                                }
                                Ok(Some(other_msg)) => {
                                    error!("Expected disconnect ACK, got: {other_msg:?}");
                                    Err(NetworkError::Closed)
                                }
                                Ok(None) => {
                                    error!("Peer closed connection without sending Disconnect ACK.");
                                    Err(NetworkError::Closed)
                                }
                                Err(e) => {
                                    error!("Error while reading Disconnect ACK: {e}");
                                    Err(e)
                                }
                            }
                        }
                        Err(e) => {
                            error!("Unable to send Disconnect to {target_addr}: {e}");
                            Err(e)
                        }
                    }
                }
            }
        })
        .await?
    }

    pub async fn read_message(&mut self, target_addr: SocketAddr) -> Result<RpcMessage, NetworkError> {
        tokio::time::timeout(CONNECTION_TIMEOUT, async move {
            match self.connections.lock().await.get_mut(&target_addr) {
                None => {
                    error!("Connection not found {target_addr}");
                    Err(NetworkError::Closed)
                }
                Some(connection) => match super::read_message(&mut connection.read_stream, &mut connection.buffer).await {
                    Ok(Some(msg)) => Ok(msg),
                    Ok(None) => {
                        error!("No message received, the connection may have been closed.");
                        Err(NetworkError::Closed)
                    }
                    Err(e) => {
                        error!("Failed to read message: {e}");
                        Err(e)
                    }
                },
            }
        })
        .await?
    }

    pub async fn send_message(&mut self, target_addr: SocketAddr, msg: RpcMessageKind) -> Result<(), NetworkError> {
        tokio::time::timeout(CONNECTION_TIMEOUT, async move {
            match self.connections.lock().await.get_mut(&target_addr) {
                None => {
                    error!("Connection not found {target_addr}");
                    Err(NetworkError::Closed)
                }
                Some(connection) => match super::send_message(&mut connection.write_stream, msg).await {
                    Ok(_) => {
                        debug!("Message sent successfully.");
                        Ok(())
                    }
                    Err(e) => {
                        error!("Failed to send message: {e}");
                        Err(e)
                    }
                },
            }
        })
        .await?
    }
}
