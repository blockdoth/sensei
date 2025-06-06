use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::vec;

use futures::future::pending;
use lib::handler::device_handler::DeviceHandlerConfig;
use lib::network::rpc_message::CtrlMsg::*;
use lib::network::rpc_message::DataMsg::RawFrame;
use lib::network::rpc_message::RpcMessageKind::{Ctrl, Data};
use lib::network::rpc_message::SourceType::ESP32;
use lib::network::rpc_message::{CfgType, CtrlMsg, DataMsg, DeviceId, HostId};
use lib::network::tcp::ChannelMsg;
use lib::network::tcp::client::TcpClient;
use log::*;
use serde::{Deserialize, Serialize};
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::watch::{Receiver, Sender};
use tokio::sync::{Mutex, watch};

use crate::orchestrator::IsRecurring::{NotRecurring, Recurring};
use crate::services::{DEFAULT_ADDRESS, GlobalConfig, OrchestratorConfig, Run};

pub struct Orchestrator {
    client: Arc<Mutex<TcpClient>>,
    experiment_config: PathBuf,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Command {
    command_types: Vec<CommandType>,
    delays: Delays,
}

impl Command {
    pub fn from_yaml(file: PathBuf) -> Result<Vec<Self>, Box<dyn std::error::Error>> {
        let yaml = std::fs::read_to_string(file.clone()).map_err(|e| format!("Failed to read YAML file: {}\n{}", file.display(), e))?;
        Ok(serde_yaml::from_str(&yaml)?)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Delays {
    init_delay: Option<u64>,
    command_delay: Option<u64>,
    is_recurring: IsRecurring,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum IsRecurring {
    Recurring {
        recurrence_delay: Option<u64>,
        iterations: Option<u64>, /* 0 is infinite */
    },
    NotRecurring,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum CommandType {
    Connect {
        target_addr: SocketAddr,
    },
    Disconnect {
        target_addr: SocketAddr,
    },
    Subscribe {
        target_addr: SocketAddr,
        device_id: DeviceId,
    },
    Unsubscribe {
        target_addr: SocketAddr,
        device_id: DeviceId,
    },
    SubscribeTo {
        target_addr: SocketAddr,
        source_addr: SocketAddr,
        device_id: DeviceId,
    },
    UnsubscribeFrom {
        target_addr: SocketAddr,
        source_addr: SocketAddr,
        device_id: DeviceId,
    },
    SendStatus {
        target_addr: SocketAddr,
        host_id: HostId,
    },
    Configure {
        target_addr: SocketAddr,
        device_id: DeviceId,
        cfg_type: CfgType,
    },
}

impl Run<OrchestratorConfig> for Orchestrator {
    fn new(global_config: GlobalConfig, config: OrchestratorConfig) -> Self {
        Orchestrator {
            client: Arc::new(Mutex::new(TcpClient::new())),
            experiment_config: config.experiment_config,
        }
    }

    async fn run(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let commands = Command::from_yaml(self.experiment_config.clone())?;

        for command in commands {
            Orchestrator::execute_command(self.client.clone(), command).await;
        }
        // for target_addr in self.targets.clone() {
        //     Self::connect(&self.client, target_addr).await;
        // }
        self.cli_interface().await?;
        Ok(())
    }
}

impl Orchestrator {
    pub async fn execute_command(client: Arc<Mutex<TcpClient>>, commands: Command) -> Result<(), Box<dyn std::error::Error>> {
        tokio::time::sleep(std::time::Duration::from_millis(commands.delays.init_delay.unwrap_or(0u64))).await;
        let command_delay = commands.delays.command_delay.unwrap_or(0u64);
        let command_types = commands.command_types;

        match commands.delays.is_recurring.clone() {
            Recurring {
                recurrence_delay,
                iterations,
            } => {
                tokio::spawn(async move {
                    let r_delay = recurrence_delay.unwrap_or(0u64);
                    let n = iterations.unwrap_or(0u64);
                    if n == 0 {
                        loop {
                            Self::match_command_types(client.clone(), command_types.clone(), command_delay).await;
                            tokio::time::sleep(std::time::Duration::from_millis(r_delay)).await;
                        }
                    } else {
                        for _ in 0..n {
                            Self::match_command_types(client.clone(), command_types.clone(), command_delay).await;
                            tokio::time::sleep(std::time::Duration::from_millis(r_delay)).await;
                        }
                    }
                });
                Ok(())
            }
            NotRecurring => {
                Self::match_command_types(client, command_types, command_delay).await;
                Ok(())
            }
        }
    }

    pub async fn match_command_types(
        client: Arc<Mutex<TcpClient>>,
        command_types: Vec<CommandType>,
        command_delay: u64,
    ) -> Result<(), Box<dyn std::error::Error>> {
        for command_type in command_types {
            Self::match_command_type(client.clone(), command_type.clone()).await;
            tokio::time::sleep(std::time::Duration::from_millis(command_delay)).await;
        }
        Ok(())
    }

    pub async fn match_command_type(client: Arc<Mutex<TcpClient>>, command_type: CommandType) -> Result<(), Box<dyn std::error::Error>> {
        match command_type {
            CommandType::Connect { target_addr } => Ok(Self::connect(&client, target_addr).await?),
            CommandType::Disconnect { target_addr } => Ok(Self::disconnect(&client, target_addr).await?),
            CommandType::Subscribe { target_addr, device_id } => Ok(Self::subscribe(&client, target_addr, device_id).await?),
            CommandType::Unsubscribe { target_addr, device_id } => Ok(Self::unsubscribe(&client, target_addr, device_id).await?),
            CommandType::SubscribeTo {
                target_addr,
                source_addr,
                device_id,
            } => Ok(Self::subscribe_to(&client, target_addr, source_addr, device_id).await?),
            CommandType::UnsubscribeFrom {
                target_addr,
                source_addr,
                device_id,
            } => Ok(Self::unsubscribe_from(&client, target_addr, source_addr, device_id).await?),
            CommandType::SendStatus { target_addr, host_id } => Ok(Self::send_status(&client, target_addr, host_id).await?),
            CommandType::Configure {
                target_addr,
                device_id,
                cfg_type,
            } => Ok(Self::configure(&client, target_addr, device_id, cfg_type).await?),
        }
    }

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

    async fn subscribe_to(
        client: &Arc<Mutex<TcpClient>>,
        target_addr: SocketAddr,
        source_addr: SocketAddr,
        device_id: u64,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let msg = Ctrl(SubscribeTo {
            target: source_addr,
            device_id,
        });

        info!("Telling {target_addr} to subscribe to {source_addr} on device id {device_id}");

        Ok(client.lock().await.send_message(target_addr, msg).await?)
    }

    async fn unsubscribe_from(
        client: &Arc<Mutex<TcpClient>>,
        target_addr: SocketAddr,
        source_addr: SocketAddr,
        device_id: u64,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let msg = Ctrl(UnsubscribeFrom {
            target: source_addr,
            device_id,
        });

        info!("Telling {target_addr} to unsubscribe from device id {device_id} from {source_addr}");

        Ok(client.lock().await.send_message(target_addr, msg).await?)
    }

    async fn send_status(client: &Arc<Mutex<TcpClient>>, target_addr: SocketAddr, host_id: HostId) -> Result<(), Box<dyn std::error::Error>> {
        let msg = Ctrl(PollHostStatus { host_id });

        Ok(client.lock().await.send_message(target_addr, msg).await?)
    }

    async fn configure(
        client: &Arc<Mutex<TcpClient>>,
        target_addr: SocketAddr,
        device_id: DeviceId,
        cfg_type: CfgType,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let msg = Ctrl(Configure { device_id, cfg_type });

        info!("Telling {target_addr} to configure the device handler");

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

                // For some reason rust/my IDE freaks out if the error type is not specific IDK why
                // Only on this specific Ok
                // This pissed me off
                // TODO: FIGURE OUT WHY
                Ok::<(), anyhow::Error>(Self::connect(&send_client, target_addr).await?)
            }
            Some("disconnect") => {
                // Disconnect the orchestrator from another target node
                let target_addr: SocketAddr = input.next().unwrap_or("").parse().unwrap_or(DEFAULT_ADDRESS);
                Ok(Self::disconnect(&send_client, target_addr).await?)
            }
            Some("sub") => {
                // Subscribe the orchestrator to the data output of a node
                let target_addr: SocketAddr = input.next().unwrap_or("").parse().unwrap_or(DEFAULT_ADDRESS);
                let device_id: DeviceId = input.next().unwrap_or("0").parse().unwrap();

                Self::subscribe(&send_client, target_addr, device_id).await?;
                Ok(send_commands_channel.send(ChannelMsg::ListenSubscribe { addr: target_addr })?)
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
                Ok(send_commands_channel.send(ChannelMsg::ListenUnsubscribe { addr: target_addr })?)
            }
            Some("subto") => {
                // Tells a node to subscribe to another node
                let target_addr: SocketAddr = input.next().unwrap_or("").parse().unwrap_or(DEFAULT_ADDRESS);
                let source_addr: SocketAddr = input.next().unwrap_or("").parse().unwrap_or(DEFAULT_ADDRESS);
                let device_id: DeviceId = input.next().unwrap_or("0").parse().unwrap();

                Ok(Self::subscribe_to(&send_client, target_addr, source_addr, device_id).await?)
            }
            Some("unsubfrom") => {
                // Tells a node to unsubscribe from another node
                let target_addr: SocketAddr = input.next().unwrap_or("").parse().unwrap_or(DEFAULT_ADDRESS);
                let source_addr: SocketAddr = input.next().unwrap_or("").parse().unwrap_or(DEFAULT_ADDRESS);
                let device_id: DeviceId = input.next().unwrap_or("0").parse().unwrap();

                Ok(Self::unsubscribe_from(&send_client, target_addr, source_addr, device_id).await?)
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
                Ok(send_client.lock().await.send_message(target_addr, msg).await?)
            }
            Some("sendstatus") => {
                let target_addr = input.next().unwrap_or("").parse().unwrap_or(DEFAULT_ADDRESS);
                let host_id = input.next().unwrap_or("").parse().unwrap_or(0);

                let msg = Ctrl(PollHostStatus { host_id });

                Ok(send_client.lock().await.send_message(target_addr, msg).await?)
            }
            Some("configure") => {
                let target_addr = input.next().unwrap_or("").parse().unwrap_or(DEFAULT_ADDRESS);
                let device_id: DeviceId = input.next().unwrap_or("0").parse().unwrap();
                let configure_type = input.next();
                let config_path: PathBuf = input.next().unwrap_or("sensei/src/orchestrator/example_config.yaml").into();
                let cfg = match DeviceHandlerConfig::from_yaml(config_path.clone()) {
                    Ok(cfgs) => match cfgs.first() {
                        Some(cfg) => cfg.clone(),
                        None => {
                            info!("There needs to be at least one config in {cfgs:?}");
                            return Ok(());
                        }
                    },
                    _ => {
                        info!("Invalid config path to read {config_path:?} to a device handler config");
                        return Ok(());
                    }
                };

                let cfg_type = match CfgType::from_string(configure_type, cfg) {
                    Ok(cfg_type) => cfg_type,
                    _ => {
                        info!("{configure_type:?} is not a valid config type, needs to be create, edit or delete");
                        return Ok(());
                    }
                };

                Ok(Self::configure(&send_client, target_addr, device_id, cfg_type).await?)
            }
            _ => {
                info!("Failed to parse command");
                Ok(())
            }
        }?;
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
