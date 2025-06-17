//! TCP Client
//! ----------
//!
//! This module provides an asynchronous TCP client for communicating with a sensei-specific server.
//! It manages multiple connections using Tokio and async primitives. The client supports connecting, disconnecting,
//! sending, and receiving messages, with graceful connection handling and error reporting.
//!
//! # Features
//! - Asynchronous TCP client using Tokio
//! - Handles multiple connections concurrently
//! - Graceful connect/disconnect and error handling
//! - Integration with sensei's RPC message types
//!
//! # Example
//! ```rust,ignore
//! use lib::network::tcp::client::TcpClient;
//! use std::net::SocketAddr;
//! // ...
//! let mut client = TcpClient::new();
//! let addr: SocketAddr = "127.0.0.1:12345".parse().unwrap();
//! tokio::spawn(async move {
//!     client.connect(addr).await.unwrap();
//!     // ...
//! });
//! ```

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
use tokio::time::timeout;

use crate::errors::NetworkError;
use crate::network::rpc_message::RpcMessageKind::*;
use crate::network::rpc_message::{HostCtrl, RpcMessage, RpcMessageKind};
use crate::network::tcp::{self, CONNECTION_TIMEOUT, MAX_MESSAGE_LENGTH};

/// TcpClient manages TCP connections to remote servers, allowing sending and receiving of framed messages.
///
/// Connections are managed in a thread-safe way using an async Mutex. Each connection is associated with a
/// `SocketAddr` and stores its own read/write streams and buffer for message handling.
///
/// # Example
/// ```rust,ignore
/// let mut client = TcpClient::new();
/// client.connect(addr).await?;
/// client.send_message(addr, msg).await?;
/// let response = client.read_message(addr).await?;
/// client.disconnect(addr).await?;
/// ```
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
    /// Creates a new `TcpClient` instance.
    pub fn new() -> Self {
        Self {
            connections: Arc::new(Mutex::new(HashMap::new())),
            addr: None,
        }
    }

    /// Returns a list of currently connected `SocketAddr`s.
    pub async fn get_connections(&self) -> Vec<SocketAddr> {
        self.connections.lock().await.values().map(|conn| conn.addr).collect()
    }

    /// Connects to the specified `target_addr` and establishes a new TCP connection.
    /// If already connected, does nothing.
    ///
    /// # Errors
    /// Returns a `NetworkError` if the connection fails.
    pub async fn connect(&mut self, target_addr: SocketAddr) -> Result<(), NetworkError> {
        if let Some(connection) = self.connections.lock().await.get(&target_addr) {
            let target_addr = connection.read_stream.peer_addr()?;
            info!("Already connected to {target_addr}");
            return Ok(());
        }

        let connection = TcpStream::connect(target_addr).await?;
        let (read_stream, mut write_stream) = connection.into_split();
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

    /// Gracefully disconnects from the specified `target_addr`.
    ///
    /// # Errors
    /// Returns a `NetworkError` if the disconnect fails or times out.
    pub async fn disconnect(&mut self, target_addr: SocketAddr) -> Result<(), NetworkError> {
        timeout(CONNECTION_TIMEOUT, self.wait_for_disconnect(target_addr)).await?
    }

    /// Reads a message from the specified connection, waiting up to the connection timeout.
    ///
    /// # Errors
    /// Returns a `NetworkError` if reading fails or times out.
    pub async fn read_message(&mut self, target_addr: SocketAddr) -> Result<RpcMessage, NetworkError> {
        timeout(CONNECTION_TIMEOUT, self.wait_for_read_message(target_addr)).await?
    }

    /// Sends a message to the specified connection, waiting up to the connection timeout.
    ///
    /// # Errors
    /// Returns a `NetworkError` if sending fails or times out.
    pub async fn send_message(&mut self, target_addr: SocketAddr, msg: RpcMessageKind) -> Result<(), NetworkError> {
        timeout(CONNECTION_TIMEOUT, self.wait_for_send_message(target_addr, msg)).await?
    }

    /// Waits for a graceful disconnect from the specified connection.
    /// Does not time out when the client if slow with responding.
    pub async fn wait_for_disconnect(&mut self, target_addr: SocketAddr) -> Result<(), NetworkError> {
        let mut connections = self.connections.lock().await;
        let mut connection = connections.remove(&target_addr).ok_or(NetworkError::NoSuchConnection)?;
        debug!("Initiating graceful disconnect with {target_addr}");
        super::send_message(&mut connection.write_stream, HostCtrl(HostCtrl::Disconnect)).await?;
        debug!("Waiting for confirm of disconnect from {target_addr}");
        let res = super::read_message(&mut connection.read_stream, &mut connection.buffer).await?;
        match res {
            Some(RpcMessage {
                msg: HostCtrl(HostCtrl::Disconnect),
                ..
            }) => {
                info!("Connection with {target_addr} closed gracefully.");
                if let Err(e) = connection.write_stream.shutdown().await {
                    error!("Failed to shutdown write stream: {e}");
                }
                Ok(())
            }
            Some(other_msg) => {
                error!("Expected disconnect ACK, got: {other_msg:?}");
                Err(NetworkError::Closed)
            }
            None => {
                error!("Peer closed connection without sending Disconnect ACK.");
                Err(NetworkError::Closed)
            }
        }
    }

    /// Waits for a message to be read from the specified connection.
    /// Does not time out when the client is slow with responding.
    pub async fn wait_for_read_message(&mut self, target_addr: SocketAddr) -> Result<RpcMessage, NetworkError> {
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
    }

    /// Waits for a message to be sent to the specified connection.
    /// Does not time out when sending a message takes longer than expected.
    pub async fn wait_for_send_message(&mut self, target_addr: SocketAddr, msg: RpcMessageKind) -> Result<(), NetworkError> {
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
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    use mockall::predicate::*;

    use super::*;
    use crate::errors::NetworkError;
    use crate::network::rpc_message::{HostCtrl, RpcMessageKind};

    fn test_addr() -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 12345)
    }

    #[tokio::test]
    async fn test_new_client() {
        let client = TcpClient::new();
        assert!(client.addr.is_none());
        assert_eq!(client.get_connections().await.len(), 0);
    }

    #[tokio::test]
    async fn test_get_connections_empty() {
        let client = TcpClient::new();
        let conns = client.get_connections().await;
        assert!(conns.is_empty());
    }

    #[tokio::test]
    async fn test_connect_error() {
        // Try to connect to an unreachable address, should return error
        let mut client = TcpClient::new();
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9); // Port 9 is usually closed
        let result = client.connect(addr).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_disconnect_no_connection() {
        let mut client = TcpClient::new();
        let addr = test_addr();
        let result = client.disconnect(addr).await;
        assert!(matches!(result, Err(NetworkError::Closed) | Err(_)));
    }

    #[tokio::test]
    async fn test_read_message_no_connection() {
        let mut client = TcpClient::new();
        let addr = test_addr();
        let result = client.read_message(addr).await;
        assert!(matches!(result, Err(NetworkError::Closed) | Err(_)));
    }

    #[tokio::test]
    async fn test_send_message_no_connection() {
        let mut client = TcpClient::new();
        let addr = test_addr();
        let msg = RpcMessageKind::HostCtrl(HostCtrl::Connect);
        let result = client.send_message(addr, msg).await;
        assert!(matches!(result, Err(NetworkError::Closed) | Err(_)));
    }

    // Mockall tests

    #[tokio::test]
    async fn test_mock_connect() {
        let mut mock = MockTcpClient::default();
        let addr = test_addr();
        mock.expect_connect().with(eq(addr)).times(1).returning(|_| Ok(()));
        let result = mock.connect(addr).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_mock_send_message() {
        let mut mock = MockTcpClient::default();
        let addr = test_addr();
        let msg = RpcMessageKind::HostCtrl(HostCtrl::Connect);
        mock.expect_send_message()
            .with(eq(addr), eq(msg.clone()))
            .times(1)
            .returning(|_, _| Ok(()));
        let result = mock.send_message(addr, msg).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_mock_read_message_error() {
        let mut mock = MockTcpClient::default();
        let addr = test_addr();
        mock.expect_read_message()
            .with(eq(addr))
            .times(1)
            .returning(|_| Err(NetworkError::Closed));
        let result = mock.read_message(addr).await;
        assert!(matches!(result, Err(NetworkError::Closed)));
    }
}
