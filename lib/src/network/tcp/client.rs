//! TCP Client
//! ----------
//!
//! This TCP client is used to communicate with its corresponding sensei-specific
//! server counterpart, handling packets by a simple framing; Prepending a 4-byte
//! header that specifies the packet length.
//!
//! Note that a single client may connect/disconnect repeatedly, the main task of
//! this simple tokio TCP wrapper.
use crate::errors::NetworkError;
use crate::network::rpc_message::RpcMessage;
use crate::network::rpc_message::RpcMessageKind;
use crate::network::rpc_message::RpcMessageKind::*;
use crate::network::rpc_message::*;
use log::debug;
use log::error;
use log::info;
use log::trace;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tcpserver::ConnectEventType;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio::net::tcp::OwnedReadHalf;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::tcp::ReadHalf;
use tokio::net::tcp::WriteHalf;
use tokio::stream;
use tokio::sync::Mutex;

/// Initial size of internal buffer to handle fragmentation.
/// Socket reading will automatically resize this buffer if insufficient.
const MAX_BUFFER_SIZE: usize = 65536;
const ETH_HEADER_LEN: usize = 42;
const CONNECTION_TIME: u64 = 10;

/// Tcp Client
pub struct TcpClient {
    //TODO look if using SocketAddr is fine to use as key
    pub self_addr: Option<SocketAddr>,
    connections: Arc<Mutex<HashMap<SocketAddr, Connection>>>,
}

pub struct Connection {
    // A temp buffer to deal with fragmentation
    write_stream: OwnedWriteHalf,
    read_stream: OwnedReadHalf,
    buffer: Vec<u8>,
}

impl TcpClient {
    pub async fn new() -> Self {
        Self {
            self_addr: None,
            connections: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn connect(&mut self, target_addr: SocketAddr) -> Result<(SocketAddr), NetworkError> {
        if self.connections.lock().await.contains_key(&target_addr) {
            info!("Already connected to {target_addr}");
            return Ok(self.self_addr.unwrap());
        }

        let connection = tokio::time::timeout(
            Duration::from_secs(CONNECTION_TIME),
            TcpStream::connect(target_addr),
        )
        .await;

        match connection {
            Ok(Ok(stream)) => {
                self.self_addr = Some(stream.local_addr()?); // TODO better
                let (read_stream, write_stream) = stream.into_split();
                self.connections.lock().await.insert(
                    target_addr,
                    Connection {
                        buffer: vec![],
                        write_stream,
                        read_stream,
                    },
                );
                let self_addr = self.self_addr.unwrap();
                info!("Connected to {target_addr} from {self_addr}");
                Ok(self_addr)
            }
            Ok(Err(e)) => {
                error!("Failed to connect to {target_addr}: {e}");
                Err(NetworkError::UnableToConnect)
            }
            Err(e) => {
                error!("Connection attempt to {target_addr} timed out.");
                Err(NetworkError::Timeout(e))
            }
        }
    }

    pub async fn disconnect(&mut self, target_addr: SocketAddr) -> Result<(), NetworkError> {
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
                let msg = RpcMessage {
                    msg: Ctrl(CtrlMsg::Disconnect),
                    src_addr: self.self_addr.unwrap(), // TODO improve this
                    target_addr,
                };
                debug!("Initiating graceful disconnect");
                match super::send_message(&mut connection.write_stream, msg).await {
                    Ok(_) => {
                        if let Err(e) = connection.write_stream.shutdown().await {
                            error!("Failed to shutdown write stream: {e}");
                        }

                        match super::read_message(
                            &mut connection.read_stream,
                            &mut connection.buffer,
                        )
                        .await
                        {
                            Ok(Some(RpcMessage {
                                msg: Ctrl(CtrlMsg::Disconnect),
                                ..
                            })) => {
                                info!("Connection with {target_addr} closed gracefully.");
                                Ok(())
                            }
                            Ok(Some(other_msg)) => {
                                error!("Expected Disconnect ACK, got: {other_msg:?}");
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
    }

    pub async fn read_message(
        &mut self,
        connection_addr: SocketAddr,
    ) -> Result<RpcMessage, NetworkError> {
        match self.connections.lock().await.get_mut(&connection_addr) {
            None => {
                error!("Connection not found {connection_addr}");
                Err(NetworkError::Closed)
            }
            Some(connection) => {
                match super::read_message(&mut connection.read_stream, &mut connection.buffer).await
                {
                    Ok(Some(msg)) => Ok(msg),
                    Ok(None) => {
                        error!("No message received, the connection may have been closed.");
                        Err(NetworkError::Closed)
                    }
                    Err(e) => {
                        error!("Failed to read message: {e}");
                        Err(e)
                    }
                }
            }
        }
    }

    pub async fn send_message(
        &mut self,
        connection_addr: SocketAddr,
        msg: RpcMessage,
    ) -> Result<(), NetworkError> {
        match self.connections.lock().await.get_mut(&connection_addr) {
            None => {
                error!("Connection not found {connection_addr}");
                Err(NetworkError::Closed)
            }
            Some(connection) => {
                match super::send_message(&mut connection.write_stream, msg).await {
                    Ok(_) => {
                        trace!("Message sent successfully.");
                        Ok(())
                    }
                    Err(e) => {
                        error!("Failed to send message: {e}");
                        Err(e)
                    }
                }
            }
        }
    }
}
