use std::error::Error;
use std::fs::File;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::vec;

use futures::future::pending;
use lib::handler::device_handler::DeviceHandlerConfig;
use lib::network::rpc_message::DataMsg::RawFrame;
use lib::network::rpc_message::RpcMessageKind::Data;
use lib::network::rpc_message::SourceType::ESP32;
use lib::network::rpc_message::{CfgType, DataMsg, DeviceId, HostCtrl, HostId, RegCtrl, RpcMessageKind};
use lib::network::tcp::client::TcpClient;
use lib::network::tcp::{ChannelMsg, HostChannel};
use log::*;
use serde::{Deserialize, Serialize};
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::watch::{Receiver, Sender};
use tokio::sync::{Mutex, watch};

use crate::cli::DEFAULT_ORCHESTRATOR_CONFIG;
use crate::orchestrator::IsRecurring::{NotRecurring, Recurring};
use crate::services::{DEFAULT_ADDRESS, GlobalConfig, OrchestratorConfig, Run};

pub struct Orchestrator {
    client: Arc<Mutex<TcpClient>>,
    experiment_config: PathBuf,
    output_path: Option<PathBuf>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Experiment {
    metadata: Metadata,
    stages: Vec<Stage>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Metadata {
    name: String,
    output_path: Option<PathBuf>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Stage {
    name: String,
    command_blocks: Vec<Block>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Block {
    commands: Vec<Command>,
    delays: Delays,
}

impl Experiment {
    pub fn from_yaml(file: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
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
pub enum Command {
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
    Delay {
        delay: u64,
    },
}

impl Run<OrchestratorConfig> for Orchestrator {
    fn new(global_config: GlobalConfig, config: OrchestratorConfig) -> Self {
        Orchestrator {
            client: Arc::new(Mutex::new(TcpClient::new())),
            experiment_config: config.experiment_config,
            output_path: None,
        }
    }

    async fn run(&mut self) -> Result<(), Box<dyn Error>> {
        let experiment = Experiment::from_yaml(self.experiment_config.clone())?;

        self.load_experiment(self.client.clone(), experiment).await;

        self.cli_interface().await?;
        Ok(())
    }
}

impl Orchestrator {
    pub async fn load_experiment(&mut self, client: Arc<Mutex<TcpClient>>, experiment: Experiment) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.output_path = experiment.metadata.output_path;

        if let Some(path) = &self.output_path {
            File::create(path)?;
        }

        for (i, stage) in experiment.stages.into_iter().enumerate() {
            let name = stage.name.clone();
            info!("Executing stage {name}");
            Self::execute_stage(client.clone(), stage).await?;
            info!("Finished stage {name}");
        }

        Ok(())
    }
    pub async fn execute_stage(client: Arc<Mutex<TcpClient>>, stage: Stage) -> Result<(), Box<dyn Error + Send + Sync>> {
        let mut tasks = vec![];

        for block in stage.command_blocks {
            let clone_client = client.clone();
            let task = tokio::spawn(async move { Self::execute_command_block(clone_client, block).await.expect("Failed to execute command") });
            tasks.push(task);
        }

        let results = futures::future::join_all(tasks).await;

        for result in results {
            result?;
        }

        Ok(())
    }
    pub async fn execute_command_block(client: Arc<Mutex<TcpClient>>, block: Block) -> Result<(), Box<dyn Error + Send + Sync>> {
        tokio::time::sleep(std::time::Duration::from_millis(block.delays.init_delay.unwrap_or(0u64))).await;
        let command_delay = block.delays.command_delay.unwrap_or(0u64);
        let command_types = block.commands;

        match block.delays.is_recurring.clone() {
            Recurring {
                recurrence_delay,
                iterations,
            } => {
                let r_delay = recurrence_delay.unwrap_or(0u64);
                let n = iterations.unwrap_or(0u64);
                if n == 0 {
                    loop {
                        Self::match_commands(client.clone(), command_types.clone(), command_delay).await;
                        tokio::time::sleep(std::time::Duration::from_millis(r_delay)).await;
                    }
                } else {
                    for _ in 0..n {
                        Self::match_commands(client.clone(), command_types.clone(), command_delay).await;
                        tokio::time::sleep(std::time::Duration::from_millis(r_delay)).await;
                    }
                }
                Ok(())
            }
            NotRecurring => {
                Self::match_commands(client, command_types, command_delay).await;
                Ok(())
            }
        }
    }

    pub async fn match_commands(client: Arc<Mutex<TcpClient>>, commands: Vec<Command>, command_delay: u64) -> Result<(), Box<dyn std::error::Error>> {
        for command in commands {
            Self::match_command(client.clone(), command.clone()).await;
            tokio::time::sleep(std::time::Duration::from_millis(command_delay)).await;
        }
        Ok(())
    }

    pub async fn match_command(client: Arc<Mutex<TcpClient>>, command: Command) -> Result<(), Box<dyn std::error::Error>> {
        match command {
            Command::Connect { target_addr } => Ok(Self::connect(&client, target_addr).await?),
            Command::Disconnect { target_addr } => Ok(Self::disconnect(&client, target_addr).await?),
            Command::Subscribe { target_addr, device_id } => Ok(Self::subscribe(&client, target_addr, device_id).await?),
            Command::Unsubscribe { target_addr, device_id } => Ok(Self::unsubscribe(&client, target_addr, device_id).await?),
            Command::SubscribeTo {
                target_addr,
                source_addr,
                device_id,
            } => Ok(Self::subscribe_to(&client, target_addr, source_addr, device_id).await?),
            Command::UnsubscribeFrom {
                target_addr,
                source_addr,
                device_id,
            } => Ok(Self::unsubscribe_from(&client, target_addr, source_addr, device_id).await?),
            Command::SendStatus { target_addr, host_id } => Ok(Self::send_status(&client, target_addr, host_id).await?),
            Command::Configure {
                target_addr,
                device_id,
                cfg_type,
            } => Ok(Self::configure(&client, target_addr, device_id, cfg_type).await?),
            Command::Delay { delay } => {
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                Ok(())
            }
        }
    }

    async fn connect(client: &Arc<Mutex<TcpClient>>, target_addr: SocketAddr) -> Result<(), Box<dyn std::error::Error>> {
        Ok(client.lock().await.connect(target_addr).await?)
    }

    async fn disconnect(client: &Arc<Mutex<TcpClient>>, target_addr: SocketAddr) -> Result<(), Box<dyn std::error::Error>> {
        Ok(client.lock().await.disconnect(target_addr).await?)
    }

    async fn subscribe(client: &Arc<Mutex<TcpClient>>, target_addr: SocketAddr, device_id: u64) -> Result<(), Box<dyn std::error::Error>> {
        let msg = HostCtrl::Subscribe { device_id };
        info!("Subscribing to {target_addr} for device id {device_id}");
        Ok(client.lock().await.send_message(target_addr, RpcMessageKind::HostCtrl(msg)).await?)
    }

    async fn unsubscribe(client: &Arc<Mutex<TcpClient>>, target_addr: SocketAddr, device_id: u64) -> Result<(), Box<dyn std::error::Error>> {
        let msg = HostCtrl::Unsubscribe { device_id };
        info!("Unsubscribing from {target_addr} for device id {device_id}");
        Ok(client.lock().await.send_message(target_addr, RpcMessageKind::HostCtrl(msg)).await?)
    }

    async fn subscribe_to(
        client: &Arc<Mutex<TcpClient>>,
        target_addr: SocketAddr,
        source_addr: SocketAddr,
        device_id: u64,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let msg = RpcMessageKind::HostCtrl(HostCtrl::SubscribeTo {
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
        let msg = RpcMessageKind::HostCtrl(HostCtrl::UnsubscribeFrom {
            target: source_addr,
            device_id,
        });

        info!("Telling {target_addr} to unsubscribe from device id {device_id} from {source_addr}");

        Ok(client.lock().await.send_message(target_addr, msg).await?)
    }

    async fn send_status(client: &Arc<Mutex<TcpClient>>, target_addr: SocketAddr, host_id: HostId) -> Result<(), Box<dyn std::error::Error>> {
        let msg = RpcMessageKind::RegCtrl(RegCtrl::PollHostStatus { host_id });

        Ok(client.lock().await.send_message(target_addr, msg).await?)
    }

    async fn configure(
        client: &Arc<Mutex<TcpClient>>,
        target_addr: SocketAddr,
        device_id: DeviceId,
        cfg_type: CfgType,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let msg = RpcMessageKind::HostCtrl(HostCtrl::Configure { device_id, cfg_type });

        info!("Telling {target_addr} to configure the device handler");

        Ok(client.lock().await.send_message(target_addr, msg).await?)
    }

    // Temporary, refactor once TUI gets added
    async fn cli_interface(&self) -> Result<(), Box<dyn std::error::Error>> {
        let (send_commands_channel, recv_commands_channel) = watch::channel::<ChannelMsg>(ChannelMsg::from(HostChannel::Empty));

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
                Ok(send_commands_channel.send(ChannelMsg::from(HostChannel::ListenSubscribe { addr: target_addr }))?)
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
                Ok(send_commands_channel.send(ChannelMsg::from(HostChannel::ListenUnsubscribe { addr: target_addr }))?)
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
                let target_addr: SocketAddr = input.next().unwrap_or("").parse().unwrap_or(DEFAULT_ADDRESS);
                let host_id = input.next().unwrap_or("").parse().unwrap_or(0);

                let msg = RpcMessageKind::RegCtrl(RegCtrl::PollHostStatus { host_id });

                Ok(send_client.lock().await.send_message(target_addr, msg).await?)
            }
            Some("configure") => {
                let target_addr = input.next().unwrap_or("").parse().unwrap_or(DEFAULT_ADDRESS);
                let device_id: DeviceId = input.next().unwrap_or("0").parse().unwrap();
                let configure_type = input.next();
                let config_path: PathBuf = input.next().unwrap_or(DEFAULT_ORCHESTRATOR_CONFIG).into();
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
                    ChannelMsg::HostChannel(HostChannel::ListenSubscribe { addr }) => {
                        if !targets.contains(&addr) {
                            targets.push(addr);
                        }
                        receiving = true;
                    }
                    ChannelMsg::HostChannel(HostChannel::ListenUnsubscribe { addr }) => {
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
