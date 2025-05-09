use super::rpc_message::{self, DataMsg, RpcMessage};
use crate::errors::NetworkError;
use async_trait::async_trait;
use log::{error, info, trace};
use serde::Deserialize;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{
        TcpStream,
        tcp::{OwnedReadHalf, OwnedWriteHalf},
    },
    sync::watch::{Receiver, Sender},
};

pub mod client;
pub mod server;

pub const MAX_MESSAGE_LENGTH: usize = 1024;

pub async fn read_message(
    read_stream: &mut OwnedReadHalf,
    buffer: &mut [u8],
) -> Result<Option<RpcMessage>, NetworkError> {
    let mut length_buffer = [0; 4];

    // TODO error handling
    let msg_length = match read_stream.read_exact(&mut length_buffer).await {
        Ok(_) => {
            trace!("Header bytes: {length_buffer:?}");
            Ok(u32::from_be_bytes(length_buffer) as usize)
        }
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            info!("Stream closed by peer.");
            Err(NetworkError::Closed)
        }
        Err(e) => todo!("idk"),
    }?;

    trace!("Message length {msg_length}");

    if msg_length > MAX_MESSAGE_LENGTH {
        error!(
            "Message of length {msg_length} is to long, max message length: {MAX_MESSAGE_LENGTH}"
        );
        return Err(NetworkError::Serialization);
    }
    if msg_length == 0 {
        // todo!("handle keep alive packets");
        return Ok(None);
    }

    let mut bytes_read: usize = 0;

    while bytes_read < msg_length {
        let n_read: usize = read_stream
            .read(&mut buffer[bytes_read..msg_length])
            .await?;
        if n_read == 0 {
            error!("stream closed before all bytes were read ({bytes_read}/{msg_length})");
            return Err(NetworkError::Closed);
        }
        bytes_read += n_read;
    }
    info!("Read message of size {msg_length}: {buffer:?}");
    //TODO fix error handling
    Ok(Some(deserialize_rpc_message(&buffer[..msg_length])?))
}

pub async fn send_message(
    stream: &mut OwnedWriteHalf,
    msg: RpcMessage,
) -> Result<(), NetworkError> {
    let msg_serialized = serialize_rpc_message(msg)?;
    let msg_length = msg_serialized.len();

    if msg_length > MAX_MESSAGE_LENGTH {
        error!(
            "Message of length {msg_length} is to long, max message length: {MAX_MESSAGE_LENGTH}"
        );
        return Err(NetworkError::Serialization);
    }

    let msg_length_serialized = msg_length.to_be_bytes();

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
    async fn handle_recv(
        &self,
        request: RpcMessage,
        send_channel: Sender<ChannelMsg>,
    ) -> Result<(), anyhow::Error>;

    async fn handle_send(
        &self,
        mut recv_channel: Receiver<ChannelMsg>,
        mut send_stream: OwnedWriteHalf,
    ) -> Result<(), anyhow::Error>;
}

#[derive(Debug, Clone, Deserialize)]
pub enum ChannelMsg {
    Empty,
    Disconnect,
    Subscribe,
    Unsubscribe,
    Poll,
    Data(DataMsg),
}
