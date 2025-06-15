//! TCP Networking Primitives for Sensei
//!
//! This module provides asynchronous TCP networking utilities for the Sensei system, including message framing, sending/receiving, and connection handling traits for both clients and servers.
//!
//! # Submodules
//!
//! - [`client`]: Implements the asynchronous TCP client, supporting multiple concurrent connections, message sending/receiving, and integration with Sensei's RPC message types.
//! - [`server`]: Implements the asynchronous TCP server, handling incoming connections and delegating message processing to user-defined handlers.
//!
//! # Features
//!
//! - Message framing and serialization for Sensei RPC messages
//! - Connection handler traits for extensible message processing
//! - Utilities for reading and sending messages over TCP streams
//! - Channel-based communication for commands and data
//!
//! ## Example Usage
//!
//! ```rust,ignore
//! use lib::network::tcp::client::TcpClient;
//! use lib::network::tcp::server::TcpServer;
//! // ...
//! ```

use std::net::SocketAddr;
use std::time::Duration;

use async_trait::async_trait;
use log::{error, info, trace};
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::broadcast;
use tokio::sync::watch::{self};

use super::rpc_message::{CfgType, DataMsg, DeviceId, RpcMessage, RpcMessageKind};
use crate::errors::NetworkError;
use crate::network::rpc_message::{HostId, make_msg};

pub mod client;
pub mod server;

pub const MAX_MESSAGE_LENGTH: usize = 4096;
pub const CONNECTION_TIMEOUT: Duration = Duration::from_secs(10);

/// Reads a single framed RPC message from the provided TCP read stream.
///
///
/// # Arguments
/// * `read_stream` - The owned read half of a TCP stream.
/// * `buffer` - A mutable buffer to store the incoming message bytes.
///
/// # Errors
/// Returns `NetworkError::Closed` if the stream is closed, or `NetworkError::Serialization` for invalid messages.
///
/// # Example
/// ```rust,ignore
/// let mut buffer = [0u8; MAX_MESSAGE_LENGTH];
/// if let Some(msg) = read_message(&mut read_half, &mut buffer).await? {
///     // handle msg
/// }
/// ```
pub async fn read_message(read_stream: &mut OwnedReadHalf, buffer: &mut [u8]) -> Result<Option<RpcMessage>, NetworkError> {
    let mut length_buffer = [0; 4];

    let msg_length = match read_stream.read_exact(&mut length_buffer).await {
        Ok(_) => Ok(u32::from_be_bytes(length_buffer) as usize),
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            info!("Stream closed by peer.");
            Err(NetworkError::Closed)
        }
        Err(e) => Err(NetworkError::Io(e)), //TODO: better error handling
    }?;

    trace!("Received message of length {msg_length}");

    if msg_length > MAX_MESSAGE_LENGTH {
        error!("Message of length {msg_length} is to long, max message length: {MAX_MESSAGE_LENGTH}");
        return Err(NetworkError::Serialization);
    }
    if msg_length == 0 {
        return Ok(None);
    }

    let mut bytes_read: usize = 0;
    while bytes_read < msg_length {
        let n_read: usize = read_stream.read(&mut buffer[bytes_read..msg_length]).await?;
        trace!("Read {n_read} bytes from buffer");
        if n_read == 0 {
            error!("stream closed before all bytes were read ({bytes_read}/{msg_length})");
            return Err(NetworkError::Closed);
        }
        bytes_read += n_read;
    }
    //TODO fix error handling
    Ok(Some(deserialize_rpc_message(&buffer[..msg_length])?))
}

/// Sends a single framed RPC message over the provided TCP write stream.
///
///
/// # Arguments
/// * `stream` - The owned write half of a TCP stream.
/// * `msg` - The `RpcMessageKind` to send.
///
/// # Errors
/// Returns `NetworkError::Serialization` if the message is too large or cannot be serialized,
/// or an I/O error if writing to the stream fails.
///
/// # Example
/// ```rust,ignore
/// send_message(&mut write_half, RpcMessageKind::Control(...)).await?;
/// ```
pub async fn send_message(stream: &mut OwnedWriteHalf, msg: RpcMessageKind) -> Result<(), NetworkError> {
    let msg_wrapped = make_msg(&stream, msg);
    let msg_serialized = serialize_rpc_message(msg_wrapped)?;
    let msg_length: u32 = msg_serialized.len().try_into().unwrap();

    if msg_length as usize > MAX_MESSAGE_LENGTH {
        error!("Message of length {msg_length} is to long, max message length: {MAX_MESSAGE_LENGTH}");
        return Err(NetworkError::Serialization);
    }
    let msg_length_serialized = msg_length.to_be_bytes();

    trace!("Sending message of length {msg_length:?}");

    stream.write_all(&msg_length_serialized).await?;
    stream.write_all(&msg_serialized).await?;
    stream.flush().await?;
    Ok(())
}

// TODO better error handling
fn serialize_rpc_message(msg: RpcMessage) -> Result<Vec<u8>, Box<NetworkError>> {
    bincode::serialize(&msg).map_err(|_| Box::from(NetworkError::Serialization))
}

fn deserialize_rpc_message(buf: &[u8]) -> Result<RpcMessage, Box<NetworkError>> {
    bincode::deserialize(buf).map_err(|_| Box::from(NetworkError::Serialization))
}

#[async_trait]
/// Trait for handling TCP connection events in an asynchronous network context.
///
/// Implementors of this trait are responsible for processing received messages
/// and managing outgoing data over a TCP connection. All methods are asynchronous
/// and require implementors to be both `Send` and `Sync`.
///
/// # Methods
///
/// - `handle_recv`: Handles an incoming `RpcMessage` and may send commands via the provided channel.
/// - `handle_send`: Processes outgoing messages and data, sending them over the provided write stream.
pub trait ConnectionHandler: Send + Sync {
    /// Handles an incoming `RpcMessage` received over the TCP connection.
    ///
    /// This asynchronous method is called whenever a new message is received from the peer.
    /// Implementors should process the message and may send commands or updates through the
    /// provided `send_commands_channel`.
    /// While synchronous code inside this function is executed the tokio worker is "blocked".
    /// I.e. Tokio does not insert yield points automagically, and busy looping inside handle_recv blocks the worder
    /// for the duration of the connection. Be sure to insert yield points in large blocks of code.
    /// `tokio::task::consume_budget().await;` can be used in long blocks of synchronous code.
    ///
    /// # Arguments
    ///
    /// * `request` - The received `RpcMessage` to be handled.
    /// * `send_commands_channel` - A `watch::Sender<ChannelMsg>` for sending commands or updates
    ///   to other parts of the system.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if the message was handled successfully, or a `NetworkError` if an error occurred.
    async fn handle_recv(&self, request: RpcMessage, send_commands_channel: watch::Sender<ChannelMsg>) -> Result<(), NetworkError>;

    /// Handles outgoing messages and data to be sent over the TCP connection.
    ///
    /// This asynchronous method is responsible for processing outgoing commands and data,
    /// and sending them over the provided write stream to the peer. It listens for new
    /// commands via the `recv_commands_channel` and for data messages via the `recv_data_channel`.
    /// While synchronous code inside this function is executed the tokio worker is "blocked".
    /// I.e. Tokio does not insert yield points automagically, and busy looping inside handle_recv blocks the worder
    /// for the duration of the connection. Be sure to insert yield points in large blocks of code.
    /// `tokio::task::consume_budget().await;` can be used in long blocks of synchronous code.
    ///
    /// # Arguments
    ///
    /// * `recv_commands_channel` - A `watch::Receiver<ChannelMsg>` for receiving outgoing commands.
    /// * `recv_data_channel` - A `broadcast::Receiver<(DataMsg, DeviceId)>` for receiving outgoing data messages.
    /// * `send_stream` - An `OwnedWriteHalf` representing the writable half of the TCP stream.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if all messages were sent successfully, or a `NetworkError` if an error occurred.
    async fn handle_send(
        &self,
        mut recv_commands_channel: watch::Receiver<ChannelMsg>,
        mut recv_data_channel: broadcast::Receiver<(DataMsg, DeviceId)>,
        mut send_stream: OwnedWriteHalf,
    ) -> Result<(), NetworkError>;
}

#[async_trait]
pub trait SubscribeDataChannel {
    fn subscribe_data_channel(&self) -> broadcast::Receiver<(DataMsg, DeviceId)>;
}

#[derive(Debug, Clone, Deserialize)]
pub enum ChannelMsg {
    HostChannel(HostChannel),
    RegChannel(RegChannel),
    Data { data: DataMsg },
}

#[derive(Debug, Clone, Deserialize)]
pub enum HostChannel {
    Empty,
    Disconnect,
    Subscribe { device_id: DeviceId },
    Unsubscribe { device_id: DeviceId },
    SubscribeTo { target_addr: SocketAddr, device_id: DeviceId },
    UnsubscribeFrom { target_addr: SocketAddr, device_id: DeviceId },
    ListenSubscribe { addr: SocketAddr },
    ListenUnsubscribe { addr: SocketAddr },
    Configure { device_id: DeviceId, cfg_type: CfgType },
    Pong,
}

#[derive(Debug, Clone, Deserialize)]
pub enum RegChannel {
    SendHostStatus { host_id: HostId },
    SendHostStatuses,
}

impl From<HostChannel> for ChannelMsg {
    fn from(value: HostChannel) -> Self {
        ChannelMsg::HostChannel(value)
    }
}

impl From<RegChannel> for ChannelMsg {
    fn from(value: RegChannel) -> Self {
        ChannelMsg::RegChannel(value)
    }
}
