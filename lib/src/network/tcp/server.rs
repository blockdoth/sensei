use super::{MAX_MESSAGE_LENGTH, RequestHandler, read_message};
use crate::{errors::NetworkError, network::rpc_message::RpcMessage};
use async_trait::async_trait;
use log::{error, info, warn};
use std::{net::SocketAddr, sync::Arc};
use tokio::net::{TcpListener, TcpStream};

pub struct TcpServer {}

impl TcpServer {
    pub async fn serve(
        addr: SocketAddr,
        request_handler: Arc<dyn RequestHandler>,
    ) -> Result<(), NetworkError> {
        info!("Binding TCP listener on {addr}...");
        let listener = TcpListener::bind(addr).await?;
        info!("Listening on {addr}");

        loop {
            match listener.accept().await {
                Ok((mut stream, peer_addr)) => {
                    let handler = Arc::clone(&request_handler);
                    tokio::spawn(async move {
                        Self::handle_connection(&mut stream, handler).await;
                    });
                }
                Err(e) => {
                    warn!("Failed to accept connection: {e}");
                    continue;
                }
            }
        }
    }

    async fn handle_connection(
        stream: &mut TcpStream,
        request_handler: Arc<dyn RequestHandler>,
    ) -> Result<(), NetworkError> {
        let mut buffer = vec![0; MAX_MESSAGE_LENGTH];
        info!("Handling connection");
        loop {
            match read_message(stream, &mut buffer).await {
                Ok(Some(request)) => {
                    request_handler.handle_request(request, stream).await;
                }
                Ok(None) => {
                    // Connection closed gracefully
                    info!("Client closed the connection.");
                    break;
                }
                Err(e) => {
                    warn!("TCP client closed the connection abruptly: {e}");
                    break;
                }
            }
        }
        Ok(())
    }
}
