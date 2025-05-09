use super::{ConnectionHandler, MAX_MESSAGE_LENGTH, read_message};
use crate::{
    errors::NetworkError,
    network::{rpc_message::RpcMessage, tcp::ChannelMsg},
};
use async_trait::async_trait;
use log::{debug, error, info, warn};
use std::{net::SocketAddr, sync::Arc};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::watch,
};

pub struct TcpServer {}

impl TcpServer {
    pub async fn serve(
        addr: SocketAddr,
        connection_handler: Arc<dyn ConnectionHandler>,
    ) -> Result<(), NetworkError> {
        let listener = TcpListener::bind(addr).await?;

        loop {
            match listener.accept().await {
                Ok((mut stream, peer_addr)) => {
                    let handler = Arc::clone(&connection_handler);
                    tokio::spawn(async move {
                        Self::init_connection(stream, handler).await;
                    });
                }
                Err(e) => {
                    warn!("Failed to accept connection: {e}");
                    continue;
                }
            }
        }
    }

    async fn init_connection(
        stream: TcpStream,
        connection_handler: Arc<dyn ConnectionHandler>,
    ) -> Result<(), NetworkError> {
        let mut read_buffer = vec![0; MAX_MESSAGE_LENGTH];

        let peer_addr = stream.peer_addr()?;
        let local_peer_addr = stream.peer_addr()?;

        let (mut read_stream, write_stream) = stream.into_split();

        let (send_channel, mut recv_channel) = watch::channel::<ChannelMsg>(ChannelMsg::Empty);

        let send_channel_local = send_channel.clone();
        let connection_handler_local = connection_handler.clone();

        let read_task = tokio::spawn(async move {
            debug!("Start reading task");
            loop {
                debug!("Awaiting incoming message");
                match read_message(&mut read_stream, &mut read_buffer).await {
                    Ok(Some(request)) => {
                        connection_handler
                            .handle_recv(request, send_channel_local.clone())
                            .await;
                    }
                    Ok(None) => {
                        info!("Connection with {local_peer_addr:?} closed gracefully");
                        break;
                    }
                    Err(e) => {
                        warn!("Connection with {local_peer_addr:?} closed abruptly");
                        break;
                    }
                }
            }
            debug!("Ended reading task");
        });

        let write_task = tokio::spawn(async move {
            debug!("Started writing task");
            match connection_handler_local
                .handle_send(recv_channel, write_stream)
                .await
            {
                Ok(_) => todo!(),
                Err(_) => todo!(),
            }
            debug!("Ended writing task");
        });

        info!("Started connection with {peer_addr:?}");
        tokio::join!(read_task, write_task);
        Ok(())
    }
}
