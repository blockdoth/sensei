//! TCP Client
//! ----------
//!
//! This TCP client is used to communicate with its corresponding sensei-specific
//! server counterpart, handling packets by a simple framing; Prepending a 4-byte
//! header that specifies the packet length.
//!
//! Note that a single client may connect/disconnect repeatedly, the main task of
//! this simple tokio TCP wrapper.
use log::error;
use log::info;
use log::trace;
use std::net::SocketAddr;
use std::thread;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio::stream;

use crate::errors::NetworkError;
use crate::network::rpc_message::RpcMessage;

/// Initial size of internal buffer to handle fragmentation.
/// Socket reading will automatically resize this buffer if insufficient.
const MAX_BUFFER_SIZE: usize = 65536;
const ETH_HEADER_LEN: usize = 42;
const CONNECTION_TIME: u64 = 10;

/// Tcp Client
pub struct TcpClient {
    stream: Option<TcpStream>,
    buffer: Vec<u8>, // A temp buffer to deal with fragmentation
}

impl TcpClient {
    pub async fn new() -> Self {
        info!("Creating new TCP client.");
        Self {
            stream: None,
            buffer: Vec::with_capacity(MAX_BUFFER_SIZE),
        }
    }

    pub async fn connect(&mut self, target_addr: SocketAddr) -> Result<(), NetworkError> {
        if self.stream.is_some() {
            info!("Already connected to {target_addr}");
            return Ok(());
        }
        info!("Attempting to connect to {target_addr}...");

        let connection = tokio::time::timeout(
            Duration::from_secs(CONNECTION_TIME),
            TcpStream::connect(target_addr),
        )
        .await;

        match connection {
            Ok(Ok(stream)) => {
                info!("Connected to {target_addr}.");
                self.stream = Some(stream);
                Ok(())
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

    pub async fn disconnect(&mut self) -> Result<(), NetworkError> {
        todo!("Figure out how to do this properly")
    }

    pub async fn read_message(&mut self) -> Result<RpcMessage, NetworkError> {
        if let Some(stream) = self.stream.as_mut() {
            match super::read_message(stream, &mut self.buffer).await {
                Ok(Some(msg)) => {
                    return Ok(msg);
                }
                Ok(None) => {
                    error!("No message received, the connection may have been closed.");
                    return Err(NetworkError::Closed);
                }
                Err(e) => {
                    error!("Failed to read message: {e}");
                    return Err(e);
                }
            }
        }

        error!("Stream is not available");
        Err(NetworkError::Closed)
    }

    pub async fn send_message(&mut self, msg: RpcMessage) -> Result<(), NetworkError> {
        if let Some(stream) = self.stream.as_mut() {
            trace!("Sending message: {msg:?}");

            match super::send_message(stream, msg).await {
                Ok(_) => {
                    trace!("Message sent successfully.");
                    Ok(())
                }
                Err(e) => {
                    error!("Failed to send message: {e}");
                    Err(e)
                }
            }
        } else {
            error!("Failed to send message: no active stream.");
            Err(NetworkError::Closed)
        }
    }
}
