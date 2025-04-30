use tokio::net::UdpSocket;
use common::CtrlMsg::Heartbeat;
use common::RpcEnvelope;
use common::{serialize_envelope, deserialize_envelope};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let socket = UdpSocket::bind("127.0.0.1:8081").await?;
    socket.connect("127.0.0.1:8082").await?;

    //Send a ping
    let msg = RpcEnvelope::Ctrl(Heartbeat);
    let buf = serialize_envelope(msg);
    socket.send(&buf).await?;

    //Wait for a response
    let mut recv_buf = [0u8; 1024];
    let len = socket.recv(&mut recv_buf).await?;
    let msg = deserialize_envelope(&recv_buf[..len]);
    println!("{:?} received", msg);

    Ok(())
}
