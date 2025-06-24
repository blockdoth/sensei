//! TCP Server for Sensei
//!
//! This module implements the asynchronous TCP server for the Sensei system. It listens for incoming TCP connections,
//! spawns tasks to handle each connection, and delegates message processing to user-defined connection handlers.
//!
//! # Features
//!
//! - Asynchronous TCP server using Tokio
//! - Handles multiple concurrent client connections
//! - Delegates message processing to `ConnectionHandler` trait implementations
//! - Graceful connection handling and error reporting
//! - Channel-based communication for commands and data
//!
//! ## Example Usage
//!
//! ```rust,ignore
//! use lib::network::tcp::server::TcpServer;
//! use std::net::SocketAddr;
//! use std::sync::Arc;
//! // ...
//! let handler = Arc::new(MyHandler::new());
//! TcpServer::serve("127.0.0.1:12345".parse().unwrap(), handler).await.unwrap();
//! ```

use std::net::SocketAddr;
use std::sync::Arc;

use log::{debug, info, trace, warn};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpSocket, TcpStream};
use tokio::sync::broadcast;
use tokio::sync::watch::{self, Sender};

use super::{ConnectionHandler, MAX_MESSAGE_LENGTH, SubscribeDataChannel, read_message};
use crate::errors::NetworkError;
use crate::network::tcp::{ChannelMsg, HostChannel};

const MAX_CONNECTIONS: u32 = 512;

/// Asynchronous TCP server for handling Sensei network connections.
///
/// The `TcpServer` struct provides methods to start a TCP server that listens for incoming connections,
/// spawns tasks to handle each connection, and delegates message processing to a user-provided handler implementing
/// the [`ConnectionHandler`] and [`SubscribeDataChannel`] traits.
///
/// # Example
/// ```rust,ignore
/// use lib::network::tcp::server::TcpServer;
/// use std::net::SocketAddr;
/// use std::sync::Arc;
/// // ...
/// let handler = Arc::new(MyHandler::new());
/// TcpServer::serve("127.0.0.1:12345".parse().unwrap(), handler).await.unwrap();
/// ```
///
/// The server will accept multiple concurrent connections and spawn a read/write task for each.
/// Message processing and outgoing data are handled by the provided handler.
pub struct TcpServer;

impl TcpServer {
    pub async fn serve<H>(addr: SocketAddr, connection_handler: Arc<H>) -> Result<(), NetworkError>
    where
        H: ConnectionHandler + SubscribeDataChannel + Clone + 'static,
    {
        info!("Started node on address {addr}");

        let socket = match addr {
            std::net::SocketAddr::V4(_) => TcpSocket::new_v4()?,
            std::net::SocketAddr::V6(_) => TcpSocket::new_v6()?,
        };
        socket.set_reuseaddr(true)?;
        socket.set_reuseport(true)?;
        socket.set_keepalive(true)?;
        socket.bind(addr)?;
        let listener = socket.listen(MAX_CONNECTIONS)?;

        loop {
            match listener.accept().await {
                Ok((stream, peer_addr)) => {
                    let local_handler = connection_handler.clone();
                    tokio::spawn(Self::init_connection(stream, local_handler));
                }
                Err(e) => {
                    warn!("Failed to accept connection: {e}");
                    continue;
                }
            }
        }
    }

    /// Called when a new connection is initiated.
    async fn init_connection<H>(stream: TcpStream, connection_handler: Arc<H>) -> Result<(), NetworkError>
    where
        H: ConnectionHandler + SubscribeDataChannel + Clone + 'static,
    {
        let mut read_buffer = vec![0; MAX_MESSAGE_LENGTH];

        let _peer_addr = stream.peer_addr()?;
        let local_peer_addr = stream.peer_addr()?;

        let (mut read_stream, write_stream) = stream.into_split();

        let (send_commands_channel, recv_commands_channel) = watch::channel::<ChannelMsg>(ChannelMsg::HostChannel(HostChannel::Empty));

        let send_commands_channel_local = send_commands_channel.clone();
        let connection_handler_local = connection_handler.clone();

        let recv_data_channel_local = connection_handler.subscribe_data_channel();

        let read_task = tokio::spawn(TcpServer::read_task(
            connection_handler,
            read_buffer,
            local_peer_addr,
            read_stream,
            send_commands_channel_local,
        ));

        let write_task = tokio::spawn(TcpServer::write_task::<H>(
            write_stream,
            recv_commands_channel,
            connection_handler_local,
            recv_data_channel_local,
        ));

        let _ = tokio::join!(read_task, write_task);
        debug!("Cleaned up all resources for connection {local_peer_addr:?}");
        Ok(())
    }

    /// The reading task.
    async fn read_task(
        connection_handler: Arc<dyn ConnectionHandler>,
        mut read_buffer: Vec<u8>,
        local_peer_addr: SocketAddr,
        mut read_stream: OwnedReadHalf,
        send_commands_channel_local: Sender<ChannelMsg>,
    ) -> Result<(), NetworkError> {
        debug!("Start reading task");
        loop {
            match read_message(&mut read_stream, &mut read_buffer).await {
                Ok(Some(request)) => {
                    if let Err(NetworkError::Closed) = connection_handler.handle_recv(request, send_commands_channel_local.clone()).await {
                        break;
                    }
                }
                Ok(None) | Err(NetworkError::Closed) => {
                    info!("Connection with {local_peer_addr:?} closed gracefully");
                    break;
                }
                Err(e) => {
                    debug!("Connection with {local_peer_addr:?} closed abruptly {e}");
                    break;
                }
            }
        }
        debug!("Ended reading task");
        Ok(())
    }

    /// The writing task.
    async fn write_task<H>(
        write_stream: OwnedWriteHalf,
        recv_commands_channel: watch::Receiver<ChannelMsg>,
        connection_handler_local: Arc<H>,
        recv_data_channel_local: broadcast::Receiver<(crate::network::rpc_message::DataMsg, u64)>,
    ) -> Result<(), NetworkError>
    where
        H: ConnectionHandler + SubscribeDataChannel + Clone + 'static,
    {
        trace!("Started writing task");
        connection_handler_local
            .handle_send(recv_commands_channel, recv_data_channel_local, write_stream)
            .await?;
        trace!("Ended writing task");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::time::Duration;

    use mockall::automock;
    use mockall::predicate::*;
    use tokio::net::tcp::OwnedWriteHalf;
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::{broadcast, watch};
    use tokio::time::sleep;

    use super::*;
    use crate::network::rpc_message::{HostCtrl, RpcMessage};

    #[automock]
    #[async_trait::async_trait]
    trait MockConnectionHandler: Send + Sync {
        async fn handle_recv(
            &self,
            _request: crate::network::rpc_message::RpcMessage,
            _send_commands_channel: watch::Sender<ChannelMsg>,
        ) -> Result<(), NetworkError>;
        async fn handle_send(
            &self,
            _recv_commands_channel: watch::Receiver<ChannelMsg>,
            _recv_data_channel: broadcast::Receiver<(crate::network::rpc_message::DataMsg, u64)>,
            _send_stream: OwnedWriteHalf,
        ) -> Result<(), NetworkError>;
    }

    #[automock]
    trait MockSubscribeDataChannel: Send + Sync {
        fn subscribe_data_channel(&self) -> broadcast::Receiver<(crate::network::rpc_message::DataMsg, u64)>;
    }

    #[derive(Clone)]
    struct DummyHandler;
    #[async_trait::async_trait]
    impl ConnectionHandler for DummyHandler {
        async fn handle_recv(&self, _request: RpcMessage, _send_commands_channel: watch::Sender<ChannelMsg>) -> Result<(), NetworkError> {
            Ok(())
        }
        async fn handle_send(
            &self,
            _recv_commands_channel: watch::Receiver<ChannelMsg>,
            _recv_data_channel: broadcast::Receiver<(crate::network::rpc_message::DataMsg, u64)>,
            _send_stream: OwnedWriteHalf,
        ) -> Result<(), NetworkError> {
            Ok(())
        }
    }
    impl SubscribeDataChannel for DummyHandler {
        fn subscribe_data_channel(&self) -> broadcast::Receiver<(crate::network::rpc_message::DataMsg, u64)> {
            let (_tx, rx) = broadcast::channel(1);
            rx
        }
    }

    #[tokio::test]
    async fn test_server_startup_and_shutdown() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let handler = Arc::new(DummyHandler);
        let listener = TcpListener::bind(addr).await.unwrap();
        let local_addr = listener.local_addr().unwrap();
        let handler_clone = handler.clone();
        let server = tokio::spawn(async move {
            TcpServer::serve(local_addr, handler_clone).await.ok();
        });
        sleep(Duration::from_millis(100)).await;
        let _client = TcpStream::connect(local_addr).await.unwrap();
        sleep(Duration::from_millis(100)).await;
        server.abort();
    }

    #[tokio::test]
    async fn test_mock_connection_handler() {
        let mut mock = MockMockConnectionHandler::new();
        mock.expect_handle_recv().returning(|_, _| Ok(()));
        mock.expect_handle_send().returning(|_, _, _| Ok(()));
        // This test just checks that the mock can be called as expected
        let (tx, _rx) = watch::channel(ChannelMsg::HostChannel(HostChannel::Empty));
        let (data_tx, data_rx) = broadcast::channel(1);
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let listener = TcpListener::bind(addr).await.unwrap();
        let local_addr = listener.local_addr().unwrap();
        let client = TcpStream::connect(local_addr).await.unwrap();
        let (server, _) = listener.accept().await.unwrap();
        let (_read_half, write_half) = server.into_split();
        let msg = crate::network::rpc_message::RpcMessage {
            msg: crate::network::rpc_message::RpcMessageKind::HostCtrl(HostCtrl::Ping),
            src_addr: "127.0.0.1:0".parse().unwrap(),
            target_addr: "127.0.0.1:0".parse().unwrap(),
        };
        let _ = mock.handle_recv(msg, tx).await;
        let _ = mock.handle_send(_rx, data_rx, write_half).await;
    }

    #[tokio::test]
    async fn test_handle_recv_error_propagation() {
        use mockall::mock;

        use crate::errors::NetworkError;
        use crate::network::rpc_message::RpcMessageKind;
        use crate::network::tcp::RpcMessage;

        mock! {
            Handler {}

            #[async_trait::async_trait]
            impl ConnectionHandler for Handler {
                async fn handle_recv(&self, _request: crate::network::rpc_message::RpcMessage, _send_commands_channel: watch::Sender<ChannelMsg>) -> Result<(), NetworkError>;
                async fn handle_send(
                    &self,
                    _recv_commands_channel: watch::Receiver<ChannelMsg>,
                    _recv_data_channel: broadcast::Receiver<(crate::network::rpc_message::DataMsg, u64)>,
                    _send_stream: tokio::net::tcp::OwnedWriteHalf,
                ) -> Result<(), NetworkError>;
            }

            impl SubscribeDataChannel for Handler {
                fn subscribe_data_channel(&self) -> broadcast::Receiver<(crate::network::rpc_message::DataMsg, u64)>;
            }
        }

        let mut mock = MockHandler::new();
        mock.expect_handle_recv()
            .returning(|_request, _send_commands_channel| Err(NetworkError::Closed));
        mock.expect_subscribe_data_channel().returning(|| {
            let (_tx, rx) = broadcast::channel(1);
            rx
        });
        let (tx, _rx) = watch::channel(ChannelMsg::HostChannel(HostChannel::Empty));
        let msg = RpcMessage {
            msg: RpcMessageKind::HostCtrl(HostCtrl::Ping),
            src_addr: "127.0.0.1:0".parse().unwrap(),
            target_addr: "127.0.0.1:0".parse().unwrap(),
        };
        let res = mock.handle_recv(msg, tx).await;
        assert!(matches!(res, Err(NetworkError::Closed)));
    }

    #[tokio::test]
    async fn test_subscribe_data_channel_returns_receiver() {
        let handler = DummyHandler;
        let rx = handler.subscribe_data_channel();
        // The returned value should be a broadcast::Receiver
        assert_eq!(rx.len(), 0);
    }
}
