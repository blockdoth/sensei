use std::net::SocketAddr;

use async_trait::async_trait;
use log::{debug, error, info};
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::broadcast;
use tokio::sync::watch::{self};

use super::rpc_message::{DataMsg, DeviceId, RpcMessage, RpcMessageKind};
use crate::errors::NetworkError;
use crate::network::rpc_message::{HostId, make_msg};
use crate::handler::device_handler::CfgType;

pub mod client;
pub mod server;

pub const MAX_MESSAGE_LENGTH: usize = 4096;

pub async fn read_message(read_stream: &mut OwnedReadHalf, buffer: &mut [u8]) -> Result<Option<RpcMessage>, NetworkError> {
    let mut length_buffer = [0; 4];

    let msg_length = match read_stream.read_exact(&mut length_buffer).await {
        Ok(_) => Ok(u32::from_be_bytes(length_buffer) as usize),
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            info!("Stream closed by peer.");
            Err(NetworkError::Closed)
        }
        Err(_) => todo!("idk"), //TODO: better error handling
    }?;

    debug!("Received message of length {msg_length}");

    if msg_length > MAX_MESSAGE_LENGTH {
        error!("Message of length {msg_length} is to long, max message length: {MAX_MESSAGE_LENGTH}");
        return Err(NetworkError::Serialization);
    }
    if msg_length == 0 {
        // todo!("handle keep alive packets");
        return Ok(None);
    }

    let mut bytes_read: usize = 0;
    while bytes_read < msg_length {
        let n_read: usize = read_stream.read(&mut buffer[bytes_read..msg_length]).await?;
        debug!("Read {n_read} bytes from buffer");
        if n_read == 0 {
            error!("stream closed before all bytes were read ({bytes_read}/{msg_length})");
            return Err(NetworkError::Closed);
        }
        bytes_read += n_read;
    }
    //TODO fix error handling
    Ok(Some(deserialize_rpc_message(&buffer[..msg_length])?))
}

pub async fn send_message(stream: &mut OwnedWriteHalf, msg: RpcMessageKind) -> Result<(), NetworkError> {
    let msg_wrapped = make_msg(&stream, msg);
    let msg_serialized = serialize_rpc_message(msg_wrapped)?;
    let msg_length: u32 = msg_serialized.len().try_into().unwrap();

    if msg_length as usize > MAX_MESSAGE_LENGTH {
        error!("Message of length {msg_length} is to long, max message length: {MAX_MESSAGE_LENGTH}");
        return Err(NetworkError::Serialization);
    }
    let msg_length_serialized = msg_length.to_be_bytes();

    debug!("Sending message of length {msg_length:?}");

    stream.write_all(&msg_length_serialized).await?;
    stream.write_all(&msg_serialized).await?;
    stream.flush().await?;
    Ok(())
}

// TODO better error handling
fn serialize_rpc_message(msg: RpcMessage) -> Result<Vec<u8>, NetworkError> {
    if let Ok(buf) = bincode::serialize(&msg) {
        Ok(buf)
    } else {
        Err(NetworkError::Serialization)
    }
}

fn deserialize_rpc_message(buf: &[u8]) -> Result<RpcMessage, NetworkError> {
    if let Ok(msg) = bincode::deserialize(buf) {
        Ok(msg)
    } else {
        Err(NetworkError::Serialization)
    }
}

#[async_trait]
pub trait ConnectionHandler: Send + Sync {
    async fn handle_recv(&self, request: RpcMessage, send_commands_channel: watch::Sender<ChannelMsg>) -> Result<(), NetworkError>;

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
    Empty,
    Disconnect,
    Subscribe { device_id: DeviceId },
    Unsubscribe { device_id: DeviceId },
    SubscribeTo { target_addr: SocketAddr, device_id: DeviceId },
    UnsubscribeFrom { target_addr: SocketAddr, device_id: DeviceId },
    ListenSubscribe { addr: SocketAddr },
    ListenUnsubscribe { addr: SocketAddr },
    Configure { device_id: DeviceId, cfg_type: CfgType },
    SendHostStatus { reg_addr: SocketAddr, host_id: HostId },
    SendHostStatuses,
    Data { data: DataMsg },
}
