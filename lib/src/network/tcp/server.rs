use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use log::{debug, error, info, warn};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

use super::{ConnectionHandler, MAX_MESSAGE_LENGTH, SubscribeDataChannel, read_message};
use crate::errors::NetworkError;
use crate::network::rpc_message::RpcMessage;
use crate::network::tcp::ChannelMsg;

pub struct TcpServer {}

// TODO: write docs
impl TcpServer {
    pub async fn serve<H>(addr: SocketAddr, connection_handler: Arc<H>) -> Result<(), NetworkError>
    where
        H: ConnectionHandler + SubscribeDataChannel + Clone + 'static,
    {
        info!("Starting node on address {addr}");
        let listener = TcpListener::bind(addr).await?;

        loop {
            match listener.accept().await {
                Ok((mut stream, peer_addr)) => {
                    let local_handler = connection_handler.clone();
                    tokio::spawn(async move {
                        Self::init_connection(stream, local_handler).await;
                    });
                }
                Err(e) => {
                    warn!("Failed to accept connection: {e}");
                    continue;
                }
            }
        }
    }

    async fn init_connection<H>(stream: TcpStream, connection_handler: Arc<H>) -> Result<(), NetworkError>
    where
        H: ConnectionHandler + SubscribeDataChannel + Clone + 'static,
    {
        let mut read_buffer = vec![0; MAX_MESSAGE_LENGTH];

        let peer_addr = stream.peer_addr()?;
        let local_peer_addr = stream.peer_addr()?;

        let (mut read_stream, write_stream) = stream.into_split();

        let (send_commands_channel, mut recv_commands_channel) = watch::channel::<ChannelMsg>(ChannelMsg::Empty);

        let send_commands_channel_local = send_commands_channel.clone();
        let connection_handler_local = connection_handler.clone();

        let recv_data_channel_local = connection_handler.subscribe_data_channel();

        let read_task = tokio::spawn(async move {
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
        });

        let write_task = tokio::spawn(async move {
            debug!("Started writing task");
            connection_handler_local
                .handle_send(recv_commands_channel, recv_data_channel_local, write_stream)
                .await;
            debug!("Ended writing task");
        });

        tokio::join!(read_task, write_task);
        info!("Gracefully closed connection with {local_peer_addr:?}");
        Ok(())
    }
}
