use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::vec;

use futures::future::pending;
use lib::handler::device_handler::CfgType::Delete;
use lib::handler::device_handler::{CfgType, DeviceHandlerConfig};
use lib::network::rpc_message::CtrlMsg::*;
use lib::network::rpc_message::DataMsg::RawFrame;
use lib::network::rpc_message::RpcMessageKind::{Ctrl, Data};
use lib::network::rpc_message::SourceType::ESP32;
use lib::network::rpc_message::{CtrlMsg, DataMsg, DeviceId};
use lib::network::tcp::ChannelMsg;
use lib::network::tcp::client::TcpClient;
use log::*;
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::watch::{Receiver, Sender};
use tokio::sync::{Mutex, watch};

use crate::services::{DEFAULT_ADDRESS, GlobalConfig, OrchestratorConfig, Run};

pub struct Orchestrator {
    client: Arc<Mutex<TcpClient>>,
    targets: Vec<SocketAddr>,
}

impl Run<OrchestratorConfig> for Orchestrator {
    fn new(global_config: GlobalConfig, config: OrchestratorConfig) -> Self {
        Orchestrator {
            client: Arc::new(Mutex::new(TcpClient::new())),
            targets: config.targets,
        }
    }

    async fn run(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        for target_addr in self.targets.clone() {
            Self::connect(&self.client, target_addr).await;
        }
        self.cli_interface().await?;
        Ok(())
    }
}

impl Orchestrator {
    async fn connect(client: &Arc<Mutex<TcpClient>>, target_addr: SocketAddr) -> Result<(), Box<dyn std::error::Error>> {
        Ok(client.lock().await.connect(target_addr).await?)
    }

    async fn disconnect(client: &Arc<Mutex<TcpClient>>, target_addr: SocketAddr) -> Result<(), Box<dyn std::error::Error>> {
        Ok(client.lock().await.disconnect(target_addr).await?)
    }

    async fn subscribe(client: &Arc<Mutex<TcpClient>>, target_addr: SocketAddr, device_id: u64) -> Result<(), Box<dyn std::error::Error>> {
        let msg = Ctrl(CtrlMsg::Subscribe { device_id });
        info!("Subscribing to {target_addr} for device id {device_id}");
        Ok(client.lock().await.send_message(target_addr, msg).await?)
    }

    async fn unsubscribe(client: &Arc<Mutex<TcpClient>>, target_addr: SocketAddr, device_id: u64) -> Result<(), Box<dyn std::error::Error>> {
        let msg = Ctrl(CtrlMsg::Unsubscribe { device_id });
        info!("Unsubscribing from {target_addr} for device id {device_id}");
        Ok(client.lock().await.send_message(target_addr, msg).await?)
    }

    // Temporary, refactor once TUI gets added
    async fn cli_interface(&self) -> Result<(), Box<dyn std::error::Error>> {
        let (send_commands_channel, recv_commands_channel) = watch::channel::<ChannelMsg>(ChannelMsg::Empty);

        let send_client = self.client.clone();
        let recv_client = self.client.clone();

        let _command_task = tokio::spawn(async move {
            // Create the input reader
            let stdin: BufReader<io::Stdin> = BufReader::new(io::stdin());
            let mut lines = stdin.lines();

            info!("Starting TUI interface");
            info!("Manual mode, type 'help' for commands");

            while let Ok(Some(line)) = lines.next_line().await {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                Self::parse_command(line, send_client.clone(), send_commands_channel.clone()).await;
                io::stdout().flush().await.unwrap(); // Ensure prompt shows up again
            }
            println!("Send loop ended (stdin closed).");
        });

        let _recv_task = tokio::spawn(async move {
            Self::recv_task(recv_commands_channel, recv_client.clone()).await;
        });

        pending::<()>().await;

        Ok(())
    }

    async fn parse_command(
        line: &str,
        send_client: Arc<Mutex<TcpClient>>,
        send_commands_channel: Sender<ChannelMsg>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut input = line.split_whitespace();
        match input.next() {
            Some("connect") => {
                // Connect the orchestrator and another target node
                let target_addr: SocketAddr = input
                    .next()
                    .unwrap() // #TODO remove unwrap
                    .parse()
                    .unwrap_or(DEFAULT_ADDRESS);
                Self::connect(&send_client, target_addr).await?;
            }
            Some("disconnect") => {
                // Disconnect the orchestrator from another target node
                let target_addr: SocketAddr = input.next().unwrap_or("").parse().unwrap_or(DEFAULT_ADDRESS);
                Self::disconnect(&send_client, target_addr).await?;
            }
            Some("sub") => {
                // Subscribe the orchestrator to the data output of a node
                let target_addr: SocketAddr = input.next().unwrap_or("").parse().unwrap_or(DEFAULT_ADDRESS);
                let device_id: DeviceId = input.next().unwrap_or("0").parse().unwrap();

                Self::subscribe(&send_client, target_addr, device_id).await?;
                send_commands_channel.send(ChannelMsg::ListenSubscribe { addr: target_addr })?;
            }
            Some("unsub") => {
                // Unsubscribe the orchestrator from the data output of another node
                let target_addr: SocketAddr = input
                    .next()
                    .unwrap_or("") // #TODO remove unwrap
                    .parse()
                    .unwrap_or(DEFAULT_ADDRESS);
                let device_id: DeviceId = input.next().unwrap_or("0").parse().unwrap();
                Self::unsubscribe(&send_client, target_addr, device_id).await?;
                send_commands_channel.send(ChannelMsg::ListenUnsubscribe { addr: target_addr })?;
            }
            Some("subto") => {
                // Tells a node to subscribe to another node
                let target_addr: SocketAddr = input.next().unwrap_or("").parse().unwrap_or(DEFAULT_ADDRESS);
                let source_addr: SocketAddr = input.next().unwrap_or("").parse().unwrap_or(DEFAULT_ADDRESS);
                let device_id: DeviceId = input.next().unwrap_or("0").parse().unwrap();

                let msg = Ctrl(SubscribeTo {
                    target: source_addr,
                    device_id,
                });

                info!("Telling {target_addr} to subscribe to {source_addr} on device id {device_id}");

                send_client.lock().await.send_message(target_addr, msg).await?;
            }
            Some("unsubfrom") => {
                // Tells a node to unsubscribe from another node
                let target_addr: SocketAddr = input.next().unwrap_or("").parse().unwrap_or(DEFAULT_ADDRESS);
                let source_addr: SocketAddr = input.next().unwrap_or("").parse().unwrap_or(DEFAULT_ADDRESS);
                let device_id: DeviceId = input.next().unwrap_or("0").parse().unwrap();

                let msg = Ctrl(UnsubscribeFrom {
                    target: source_addr,
                    device_id,
                });

                info!("Telling {target_addr} to unsubscribe from device id {device_id} from {source_addr}");

                send_client.lock().await.send_message(target_addr, msg).await?;
            }
            Some("dummydata") => {
                // To test the subscription mechanic
                let target_addr: SocketAddr = input.next().unwrap_or("").parse().unwrap_or(DEFAULT_ADDRESS);

                let msg = Data {
                    data_msg: RawFrame {
                        ts: 1234f64,
                        bytes: vec![],
                        source_type: ESP32,
                    },
                    device_id: 0,
                };

                info!("Sending dummy data to {target_addr}");
                send_client.lock().await.send_message(target_addr, msg).await?;
            }
            Some("sendstatus") => {
                let target_addr = input.next().unwrap_or("").parse().unwrap_or(DEFAULT_ADDRESS);
                let host_id = input.next().unwrap_or("").parse().unwrap_or(0);
                send_commands_channel.send(ChannelMsg::SendHostStatus {
                    reg_addr: target_addr,
                    host_id,
                })?;
            }
            Some("configure") => {
                let target_addr = input.next().unwrap_or("").parse().unwrap_or(DEFAULT_ADDRESS);
                let device_id: DeviceId = input.next().unwrap_or("0").parse().unwrap();
                match input.next() {
                    Some("create") => {
                        let config_path: PathBuf = input.next().unwrap_or("sensei/src/orchestrator/example_config.yaml").into();
                        let cfg = match DeviceHandlerConfig::from_yaml(config_path) {
                            Ok(configs) => match configs.first().cloned() {
                                Some(config) => config,
                                _ => return Ok(()),
                            },
                            _ => return Ok(()),
                        };

                        let cfg_type = CfgType::Create { cfg };
                        let msg = Ctrl(Configure { device_id, cfg_type });

                        info!("Telling {target_addr} to create a device handler");

                        send_client
                            .lock()
                            .await
                            .send_message(target_addr, msg)
                            .await
                            .expect("TODO: panic message");
                    }
                    Some("edit") => {
                        let config_path: PathBuf = input.next().unwrap_or("sensei/src/orchestrator/example_config.yaml").into();
                        let cfg = match DeviceHandlerConfig::from_yaml(config_path) {
                            Ok(configs) => match configs.first().cloned() {
                                Some(config) => config,
                                _ => return Ok(()),
                            },
                            _ => return Ok(()),
                        };

                        let cfg_type = CfgType::Edit { cfg };
                        let msg = Ctrl(Configure { device_id, cfg_type });

                        info!("Telling {target_addr} to edit a device handler");

                        send_client
                            .lock()
                            .await
                            .send_message(target_addr, msg)
                            .await
                            .expect("TODO: panic message");
                    }
                    Some("delete") => {
                        let cfg_type = Delete;
                        let msg = Ctrl(Configure { device_id, cfg_type });

                        info!("Telling {target_addr} to delete a device handler");

                        send_client
                            .lock()
                            .await
                            .send_message(target_addr, msg)
                            .await
                            .expect("TODO: panic message");
                    }
                    _ => {
                        info!("Invalid configuration type");
                    }
                }
            }
            _ => {
                info!("Failed to parse command")
            }
        }
        Ok(())
    }

    async fn recv_task(mut recv_commands_channel: Receiver<ChannelMsg>, recv_client: Arc<Mutex<TcpClient>>) {
        let mut receiving = false;
        let mut targets: Vec<SocketAddr> = vec![];
        loop {
            if recv_commands_channel.has_changed().unwrap_or(false) {
                let msg_opt = recv_commands_channel.borrow_and_update().clone();
                match msg_opt {
                    ChannelMsg::ListenSubscribe { addr } => {
                        if !targets.contains(&addr) {
                            targets.push(addr);
                        }
                        receiving = true;
                    }
                    ChannelMsg::ListenUnsubscribe { addr } => {
                        if let Some(pos) = targets.iter().position(|x| *x == addr) {
                            targets.remove(pos);
                        }
                        if targets.is_empty() {
                            receiving = false;
                        }
                    }
                    _ => (),
                }
            }
            if receiving {
                for target_addr in targets.iter() {
                    let msg = recv_client.lock().await.read_message(*target_addr).await.unwrap();
                    match msg.msg {
                        Data {
                            data_msg: DataMsg::CsiFrame { csi },
                            device_id: _,
                        } => {
                            info!("{}: {}", msg.src_addr, csi.timestamp)
                        }
                        Data {
                            data_msg:
                                DataMsg::RawFrame {
                                    ts,
                                    bytes: _,
                                    source_type: _,
                                },
                            device_id: _,
                        } => info!("{}: {ts}", msg.src_addr),
                        _ => (),
                    }
                }
            }
        }
    }
}
