use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::net::{TcpSocket, TcpStream};

#[derive(Debug, Clone)]
struct TcpClient {
    socket: Arc<Mutex<TcpSocket>>,
    socket_addr: SocketAddr,
}
