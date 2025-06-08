mod state;
mod tui;

use std::net::SocketAddr;
use std::sync::Arc;
use std::vec;

use lib::network::rpc_message::DataMsg;
use lib::network::rpc_message::RpcMessageKind::Data;
use lib::network::tcp::ChannelMsg;
use lib::network::tcp::client::TcpClient;
use lib::tui::TuiRunner;
use log::*;
use state::{OrgTuiState, OrgUpdate};
use tokio::sync::mpsc::Receiver;
use tokio::sync::{Mutex, mpsc};

use crate::services::{GlobalConfig, OrchestratorConfig, Run};

pub struct Orchestrator {
    client: Arc<Mutex<TcpClient>>,
    targets: Vec<SocketAddr>,
    log_level: LevelFilter,
}

impl Run<OrchestratorConfig> for Orchestrator {
    fn new(global_config: GlobalConfig, config: OrchestratorConfig) -> Self {
        Orchestrator {
            client: Arc::new(Mutex::new(TcpClient::new())),
            targets: config.targets,
            log_level: global_config.log_level,
        }
    }
    async fn run(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let (command_send, mut command_recv) = mpsc::channel::<ChannelMsg>(1000);
        let (update_send, mut update_recv) = mpsc::channel::<OrgUpdate>(1000);

        for target_addr in &self.targets {
            update_send.send(OrgUpdate::Connect(*target_addr));
        }

        let tasks = vec![Self::listen(command_recv, self.client.clone())];

        let tui = OrgTuiState::new(self.client.clone());

        let tui_runner = TuiRunner::new(tui, command_send, update_recv, update_send, self.log_level);
        tui_runner.run(tasks).await;
        Ok(())
    }
}

impl Orchestrator {
    async fn listen(mut recv_commands_channel: Receiver<ChannelMsg>, client: Arc<Mutex<TcpClient>>) {
        let mut receiving = false;
        let mut targets: Vec<SocketAddr> = vec![];
        loop {
            if !recv_commands_channel.is_empty() {
                let msg_opt = recv_commands_channel.recv().await;
                match msg_opt {
                    Some(ChannelMsg::ListenSubscribe { addr }) => {
                        if !targets.contains(&addr) {
                            targets.push(addr);
                        }
                        receiving = true;
                    }
                    Some(ChannelMsg::ListenSubscribe { addr }) => {
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
                    let msg = client.lock().await.read_message(*target_addr).await.unwrap();
                    match msg.msg {
                        Data {
                            data_msg: DataMsg::CsiFrame { csi },
                            device_id,
                        } => {
                            info!("{}: {}", msg.src_addr, csi.timestamp)
                        }
                        Data {
                            data_msg: DataMsg::RawFrame { ts, bytes, source_type },
                            device_id,
                        } => info!("{}: {ts}", msg.src_addr),
                        _ => (),
                    }
                }
            }
        }
    }

    // async fn parse_command(
    //     line: &str,
    //     send_client: Arc<Mutex<TcpClient>>,
    //     send_commands_channel: Sender<ChannelMsg>,
    // ) -> Result<(), Box<dyn std::error::Error>> {
    //     let mut input = line.split_whitespace();
    //     match input.next() {
    //         Some("connect") => {
    //             // Connect the orchestrator and another target node
    //             let target_addr: SocketAddr = input
    //                 .next()
    //                 .unwrap() // #TODO remove unwrap
    //                 .parse()
    //                 .unwrap_or(DEFAULT_ADDRESS);
    //             Self::connect(&send_client, target_addr).await?;
    //         }
    //         Some("disconnect") => {
    //             // Disconnect the orchestrator from another target node
    //             let target_addr: SocketAddr = input.next().unwrap_or("").parse().unwrap_or(DEFAULT_ADDRESS);
    //             Self::disconnect(&send_client, target_addr).await?;
    //         }
    //         Some("sub") => {
    //             // Subscribe the orchestrator to the data output of a node
    //             let target_addr: SocketAddr = input.next().unwrap_or("").parse().unwrap_or(DEFAULT_ADDRESS);
    //             let device_id: DeviceId = input.next().unwrap_or("0").parse().unwrap();

    //             Self::subscribe(&send_client, target_addr, device_id).await?;
    //             send_commands_channel.send(ChannelMsg::ListenSubscribe { addr: target_addr })?;
    //         }
    //         Some("unsub") => {
    //             // Unsubscribe the orchestrator from the data output of another node
    //             let target_addr: SocketAddr = input
    //                 .next()
    //                 .unwrap_or("") // #TODO remove unwrap
    //                 .parse()
    //                 .unwrap_or(DEFAULT_ADDRESS);
    //             let device_id: DeviceId = input.next().unwrap_or("0").parse().unwrap();
    //             Self::unsubscribe(&send_client, target_addr, device_id).await?;
    //             send_commands_channel.send(ChannelMsg::ListenUnsubscribe { addr: target_addr })?;
    //         }
    //         Some("subto") => {
    //             // Tells a node to subscribe to another node
    //             let target_addr: SocketAddr = input.next().unwrap_or("").parse().unwrap_or(DEFAULT_ADDRESS);
    //             let source_addr: SocketAddr = input.next().unwrap_or("").parse().unwrap_or(DEFAULT_ADDRESS);
    //             let device_id: DeviceId = input.next().unwrap_or("0").parse().unwrap();

    //             let msg = Ctrl(SubscribeTo {
    //                 target: source_addr,
    //                 device_id,
    //             });

    //             info!("Telling {target_addr} to subscribe to {source_addr} on device id {device_id}");

    //             send_client.lock().await.send_message(target_addr, msg).await?;
    //         }
    //         Some("unsubfrom") => {
    //             // Tells a node to unsubscribe from another node
    //             let target_addr: SocketAddr = input.next().unwrap_or("").parse().unwrap_or(DEFAULT_ADDRESS);
    //             let source_addr: SocketAddr = input.next().unwrap_or("").parse().unwrap_or(DEFAULT_ADDRESS);
    //             let device_id: DeviceId = input.next().unwrap_or("0").parse().unwrap();

    //             let msg = Ctrl(UnsubscribeFrom {
    //                 target: source_addr,
    //                 device_id,
    //             });

    //             info!("Telling {target_addr} to unsubscribe from device id {device_id} from {source_addr}");

    //             send_client.lock().await.send_message(target_addr, msg).await?;
    //         }
    //         Some("dummydata") => {
    //             // To test the subscription mechanic
    //             let target_addr: SocketAddr = input.next().unwrap_or("").parse().unwrap_or(DEFAULT_ADDRESS);

    //             let msg = Data {
    //                 data_msg: RawFrame {
    //                     ts: 1234f64,
    //                     bytes: vec![],
    //                     source_type: ESP32,
    //                 },
    //                 device_id: 0,
    //             };

    //             info!("Sending dummy data to {target_addr}");
    //             send_client.lock().await.send_message(target_addr, msg).await?;
    //         }
    //         Some("sendstatus") => {
    //             let target_addr = input.next().unwrap_or("").parse().unwrap_or(DEFAULT_ADDRESS);
    //             let host_id = input.next().unwrap_or("").parse().unwrap_or(0);
    //             send_commands_channel.send(ChannelMsg::SendHostStatus {
    //                 reg_addr: target_addr,
    //                 host_id,
    //             })?;
    //         }
    //         Some("configure") => {
    //             let target_addr = input.next().unwrap_or("").parse().unwrap_or(DEFAULT_ADDRESS);
    //             let device_id: DeviceId = input.next().unwrap_or("0").parse().unwrap();
    //             match input.next() {
    //                 Some("create") => {
    //                     let config_path: PathBuf = input.next().unwrap_or("sensei/src/orchestrator/example_config.yaml").into();
    //                     let cfg = match DeviceHandlerConfig::from_yaml(config_path) {
    //                         Ok(configs) => match configs.first().cloned() {
    //                             Some(config) => config,
    //                             _ => return Ok(()),
    //                         },
    //                         _ => return Ok(()),
    //                     };

    //                     let cfg_type = CfgType::Create { cfg };
    //                     let msg = Ctrl(Configure { device_id, cfg_type });

    //                     info!("Telling {target_addr} to create a device handler");

    //                     send_client
    //                         .lock()
    //                         .await
    //                         .send_message(target_addr, msg)
    //                         .await
    //                         .expect("TODO: panic message");
    //                 }
    //                 Some("edit") => {
    //                     let config_path: PathBuf = input.next().unwrap_or("sensei/src/orchestrator/example_config.yaml").into();
    //                     let cfg = match DeviceHandlerConfig::from_yaml(config_path) {
    //                         Ok(configs) => match configs.first().cloned() {
    //                             Some(config) => config,
    //                             _ => return Ok(()),
    //                         },
    //                         _ => return Ok(()),
    //                     };

    //                     let cfg_type = CfgType::Edit { cfg };
    //                     let msg = Ctrl(Configure { device_id, cfg_type });

    //                     info!("Telling {target_addr} to edit a device handler");

    //                     send_client
    //                         .lock()
    //                         .await
    //                         .send_message(target_addr, msg)
    //                         .await
    //                         .expect("TODO: panic message");
    //                 }
    //                 Some("delete") => {
    //                     let cfg_type = Delete;
    //                     let msg = Ctrl(Configure { device_id, cfg_type });

    //                     info!("Telling {target_addr} to delete a device handler");

    //                     send_client
    //                         .lock()
    //                         .await
    //                         .send_message(target_addr, msg)
    //                         .await
    //                         .expect("TODO: panic message");
    //                 }
    //                 _ => {
    //                     info!("Invalid configuration type");
    //                 }
    //             }
    //         }
    //         _ => {
    //             info!("Failed to parse command")
    //         }
    //     }
    //     Ok(())
    // }

    async fn recv_task(mut recv_commands_channel: Receiver<ChannelMsg>, recv_client: Arc<Mutex<TcpClient>>) {
        let mut receiving = false;
        let mut targets: Vec<SocketAddr> = vec![];
        loop {
            if !recv_commands_channel.is_empty() {
                let msg_opt = recv_commands_channel.recv().await.unwrap(); // TODO change
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
