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
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

use super::{ConnectionHandler, MAX_MESSAGE_LENGTH, SubscribeDataChannel, read_message};
use crate::errors::NetworkError;
use crate::network::tcp::{ChannelMsg, HostChannel};

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
        info!("Starting node on address {addr}");
        let listener = TcpListener::bind(addr).await?;
        loop {
            match listener.accept().await {
                Ok((stream, _peer_addr)) => {
                    let local_handler = connection_handler.clone();
                    tokio::spawn(async move {
                        let _ = Self::init_connection(stream, local_handler).await;
                    });
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
        info!("Gracefully closed connection with {local_peer_addr:?}");
        Ok(())
    }

    /// The reading task.
    async fn read_task(
        connection_handler: Arc<dyn ConnectionHandler>,
        mut read_buffer: Vec<u8>,
        local_peer_addr: SocketAddr,
        mut read_stream: tokio::net::tcp::OwnedReadHalf,
        send_commands_channel_local: watch::Sender<ChannelMsg>,
    ) -> Result<(), NetworkError> {
        debug!("Start reading task");
        loop {
            match read_message(&mut read_stream, &mut read_buffer).await {
                Ok(Some(request)) => {
                    if let Err(NetworkError::Closed) = connection_handler.handle_recv(request, send_commands_channel_local.clone()).await {
                        break;
                    }
                }
                Ok(None) => {
                    info!("Connection with {local_peer_addr:?} closed gracefully");
                    break;
                }
                Err(e) => {
                    warn!("Connection with {local_peer_addr:?} closed abruptly {e}");
                    break;
                }
            }
        }
        debug!("Ended reading task");
        Ok(())
    }

    /// The writing task.
    async fn write_task<H>(
        write_stream: tokio::net::tcp::OwnedWriteHalf,
        recv_commands_channel: watch::Receiver<ChannelMsg>,
        connection_handler_local: Arc<H>,
        recv_data_channel_local: tokio::sync::broadcast::Receiver<(crate::network::rpc_message::DataMsg, u64)>,
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
