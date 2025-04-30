use common::CtrlMsg::Heartbeat;
use common::RpcEnvelope;
use common::{deserialize_envelope, serialize_envelope};
use tokio::net::UdpSocket;
use std::sync::Arc;
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::task::JoinHandle;

fn recv_task(recv_socket: Arc<UdpSocket>) -> JoinHandle<()> {
    tokio::spawn(async move {
        //Wait for a response
        loop {
            let mut buf = [0u8; 1024];
            match recv_socket.recv_from(&mut buf).await {
                Ok((len, addr)) => {
                    let msg = deserialize_envelope(&buf[..len]);
                    println!("{:?} received", msg);
                }
                Err(e) => { println!("Received error {:?}", e); }
            }
        }
    })
}

fn send_task(send_socket: Arc<UdpSocket>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let stdin = BufReader::new(io::stdin());
        let mut lines = stdin.lines();

        println!("Type: <target_addr> <type>");
        println!("Example: 127.0.0.1:8082 Heartbeat");

        while let Ok(Some(line)) = lines.next_line().await {
            let input = line.trim();
            if input.is_empty() {
                continue;
            }

            let mut parts = input.splitn(2, ' ');
            let addr = match parts.next() {
                Some(a) if !a.is_empty() => a,
                _ => {
                    eprintln!("Invalid input. Format: <addr> <type>");
                    continue;
                }
            };

            let message = match parts.next() {
                Some(m) => m,
                None => {
                    eprintln!("Missing message text.");
                    continue;
                }
            };
            
            if (message == "Heartbeat" || message == "") {
                let msg = RpcEnvelope::Ctrl(Heartbeat);
                let data = serialize_envelope(msg);

                match send_socket.send_to(&data, addr).await {
                    Ok(n) => println!("Sent {} bytes to {}", n, addr),
                    Err(e) => eprintln!("Failed to send: {}", e),
                }
            }

            print!("> ");
            let _ = io::stdout().flush(); // Ensure prompt shows up again
        }

        println!("Send loop ended (stdin closed).");
    })
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let socket = Arc::new(UdpSocket::bind("127.0.0.1:8081").await?);
    let recv_socket = Arc::clone(&socket);
    let send_socket = Arc::clone(&socket);
    
    let recv_task = recv_task(recv_socket);

    let send_task = send_task(send_socket);

    tokio::try_join!(recv_task, send_task)?;

    Ok(())
}
