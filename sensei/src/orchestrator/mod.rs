mod state;
mod tui;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use std::vec;

use lib::errors::NetworkError;
use lib::experiments::{ActiveExperiment, Command, ExperimentInfo, ExperimentSession, ExperimentStatus};
use lib::network::rpc_message::RpcMessageKind::Data;
use lib::network::rpc_message::{CfgType, DataMsg, DeviceId, HostCtrl, HostId, HostStatus, RegCtrl, RpcMessage, RpcMessageKind};
use lib::network::tcp::client::TcpClient;
use lib::tui::TuiRunner;
use log::*;
use tokio::signal;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::{Mutex, mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::sleep;

use crate::orchestrator::state::{OrgTuiState, OrgUpdate};
use crate::services::{GlobalConfig, OrchestratorConfig, Run};

#[derive(Debug)]
pub enum OrgChannelMsg {
    // === Host ===
    Connect(SocketAddr),
    Disconnect(SocketAddr),
    Subscribe(SocketAddr, Option<SocketAddr>, DeviceId),
    Unsubscribe(SocketAddr, Option<SocketAddr>, DeviceId),
    SubscribeAll(SocketAddr, Option<SocketAddr>),
    UnsubscribeAll(SocketAddr, Option<SocketAddr>),
    Start(SocketAddr, DeviceId),
    StartAll(SocketAddr),
    Stop(SocketAddr, DeviceId),
    StopAll(SocketAddr),

    // === Experiments ===
    SelectExperiment(usize),
    StartExperiment,
    StartRemoteExperiment(SocketAddr),
    StopExperiment,
    ReloadExperimentConfigs,
    // === Registry ===
    StopRemoteExperiment(SocketAddr),
    ConnectRegistry(SocketAddr),
    GetHostStatuses(SocketAddr),
    SendStatus(SocketAddr, HostId),
    DisconnectRegistry,
    StartPolling,
    StopPolling,
    Poll,
    // === Misc ===
    Configure(SocketAddr, DeviceId, CfgType),
    Delay(u64),
    Shutdown,
    Ping(SocketAddr),
}

impl From<Command> for OrgChannelMsg {
    /// Converts a `Command` into a corresponding `OrgChannelMsg` for orchestrator control.
    fn from(cmd: Command) -> Self {
        match cmd {
            Command::Connect { target_addr } => OrgChannelMsg::Connect(target_addr),
            Command::Disconnect { target_addr } => OrgChannelMsg::Disconnect(target_addr),
            Command::Subscribe { target_addr, device_id } => OrgChannelMsg::Subscribe(target_addr, None, device_id),
            Command::Unsubscribe { target_addr, device_id } => OrgChannelMsg::Unsubscribe(target_addr, None, device_id),
            Command::SubscribeTo {
                target_addr,
                source_addr,
                device_id,
            } => OrgChannelMsg::Subscribe(target_addr, Some(source_addr), device_id),
            Command::UnsubscribeFrom {
                target_addr,
                source_addr,
                device_id,
            } => OrgChannelMsg::Unsubscribe(target_addr, Some(source_addr), device_id),
            Command::SendStatus { target_addr, host_id } => OrgChannelMsg::SendStatus(target_addr, host_id),
            Command::Configure {
                target_addr,
                device_id,
                cfg_type,
            } => OrgChannelMsg::Configure(target_addr, device_id, cfg_type),
            Command::Delay { delay } => OrgChannelMsg::Delay(delay),
            Command::GetHostStatuses { target_addr } => OrgChannelMsg::GetHostStatuses(target_addr),
            Command::Ping { target_addr } => OrgChannelMsg::Ping(target_addr),
            Command::Start { target_addr, device_id } => OrgChannelMsg::Start(target_addr, device_id),
            Command::StartAll { target_addr } => OrgChannelMsg::StartAll(target_addr),
            Command::Stop { target_addr, device_id } => OrgChannelMsg::Stop(target_addr, device_id),
            Command::StopAll { target_addr } => OrgChannelMsg::StopAll(target_addr),
            Command::DummyData {} => todo!(),
            Command::SubscribeAll { target_addr } => OrgChannelMsg::SubscribeAll(target_addr, None),
            Command::UnsubscribeAll { target_addr } => OrgChannelMsg::UnsubscribeAll(target_addr, None),
            Command::SubscribeToAll { target_addr, source_addr } => OrgChannelMsg::SubscribeAll(target_addr, Some(source_addr)),
            Command::UnsubscribeFromAll { target_addr, source_addr } => OrgChannelMsg::UnsubscribeAll(target_addr, Some(source_addr)),
        }
    }
}

#[derive(Debug)]
pub enum RegistryChannelMsg {
    Connect(SocketAddr),
    Disconnect,
    Poll,
    StartPolling,
    StopPolling,
}

#[derive(Debug)]
pub enum ExperimentChannelMsg {
    Start,
    Stop,
    StartRemote(SocketAddr),
    StopRemote(SocketAddr),
    Select(usize),
    ReloadExperimentConfigs,
}
pub struct Orchestrator {
    log_level: LevelFilter,
    experiments_folder: PathBuf,
    tui: bool,
    polling_interval: u64,
}

impl Run<OrchestratorConfig> for Orchestrator {
    fn new(global_config: GlobalConfig, config: OrchestratorConfig) -> Self {
        Orchestrator {
            log_level: global_config.log_level,
            experiments_folder: config.experiments_folder,
            tui: config.tui,
            polling_interval: config.polling_interval,
        }
    }
    async fn run(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let (command_send, mut command_recv) = mpsc::channel::<OrgChannelMsg>(100);
        let (update_send, mut update_recv) = mpsc::channel::<OrgUpdate>(100);
        let (experiment_send, mut experiment_recv) = mpsc::channel::<ExperimentChannelMsg>(5);
        let (registry_send, mut registry_recv) = mpsc::channel::<RegistryChannelMsg>(5);

        let client = Arc::new(Mutex::new(TcpClient::new()));

        // Tasks needs to be boxed and pinned in order to make the type checker happy
        let tasks: Vec<Pin<Box<dyn Future<Output = ()> + Send>>> = vec![
            Box::pin(Self::command_handler(
                command_recv,
                update_send.clone(),
                experiment_send,
                registry_send,
                client.clone(),
            )),
            Box::pin(Self::experiment_handler(
                client.clone(),
                self.experiments_folder.clone(),
                experiment_recv,
                update_send.clone(),
            )),
            Box::pin(Self::registry_handler(
                client.clone(),
                registry_recv,
                update_send.clone(),
                self.polling_interval,
            )),
            Box::pin(Self::init(update_send.clone())),
        ];

        if self.tui {
            let tui = OrgTuiState::new();
            let tui_runner = TuiRunner::new(tui, command_send, update_recv, update_send, self.log_level);

            tui_runner.run(tasks).await;
        } else {
            let mut handles: Vec<JoinHandle<()>> = vec![];
            for task in tasks {
                handles.push(tokio::spawn(task));
            }
            // Create a future that resolves when Ctrl+C is received
            let ctrl_c = async {
                signal::ctrl_c().await.expect("Failed to listen for ctrl + c");
                println!(" Received Ctrl+c, shutting down...");
            };

            tokio::select! {
                _ = ctrl_c => {}
                _ = futures::future::join_all(handles) => {
                    println!("All tasks completed");
                }
            }
        }

        Ok(())
    }
}

impl Orchestrator {
    /// Initialization function that is able to set everything up using the proper channels (pun intended)
    async fn init(update_send: Sender<OrgUpdate>) {
        update_send.send(OrgUpdate::ConnectRegistry).await;
        sleep(Duration::from_millis(100)).await;
        update_send.send(OrgUpdate::TogglePolling).await;
        sleep(Duration::from_millis(100)).await;
        update_send.send(OrgUpdate::AddAllHosts).await;
    }

    /// Continuously running task responsible for managing registry related functionality
    async fn registry_handler(
        client: Arc<Mutex<TcpClient>>,
        mut registry_recv: Receiver<RegistryChannelMsg>,
        update_send: Sender<OrgUpdate>,
        polling_interval: u64,
    ) {
        let mut current_registry_addr: Option<SocketAddr> = None;
        let (poll_signal_send, mut poll_signal_recv) = watch::channel(false);

        while let Some(msg) = registry_recv.recv().await {
            match msg {
                RegistryChannelMsg::Connect(addr) => match client.lock().await.connect(addr).await {
                    Ok(_) => {
                        debug!("Connected to registry");
                        current_registry_addr = Some(addr);
                        update_send.send(OrgUpdate::RegistryIsConnected(true)).await;
                    }
                    Err(e) => {
                        debug!("Failed to connect {e}");
                        update_send.send(OrgUpdate::RegistryIsConnected(false)).await;
                    }
                },
                RegistryChannelMsg::Disconnect => {
                    if let Some(registry_addr) = current_registry_addr {
                        poll_signal_send.send(true);
                        if client.lock().await.disconnect(registry_addr).await.is_ok() {
                            update_send.send(OrgUpdate::RegistryIsConnected(false)).await;
                            debug!("Disconnected registry");
                        }
                    }
                }
                RegistryChannelMsg::Poll => {
                    if let Some(registry_addr) = current_registry_addr {
                        if let Ok(statuses) = Self::poll(client.clone(), registry_addr).await {
                            update_send.send(OrgUpdate::UpdateHostStatuses(statuses)).await;
                        }
                    }
                }
                RegistryChannelMsg::StartPolling => {
                    if let Some(registry_addr) = current_registry_addr {
                        poll_signal_send.send(false);
                        poll_signal_recv.changed().await; // Clear signals

                        let mut poll_signal_recv = poll_signal_recv.clone();
                        let mut update_send = update_send.clone();
                        let client = client.clone();
                        tokio::spawn(async move {
                            debug!("Started polling registry");
                            tokio::select! {
                              _ = async move {
                                loop {
                                  if let Ok(statuses) = Self::poll(client.clone(), registry_addr).await {
                                    update_send.send(OrgUpdate::UpdateHostStatuses(statuses)).await;
                                    debug!("Polled registry");
                                  }else{
                                    update_send.send(OrgUpdate::RegistryIsConnected(false)).await;
                                    break;
                                  }
                                  sleep(Duration::from_secs(polling_interval)).await;
                                }
                                debug!("Stopped polling registry");
                              } => {}
                              _ = poll_signal_recv.changed() => {
                                if *poll_signal_recv.borrow() {
                                  debug!("Stopped polling registry");
                                }
                                }
                            }
                        });
                    }
                }
                RegistryChannelMsg::StopPolling => {
                    poll_signal_send.send(true);
                }
            }
        }
    }

    /// Poll a device for status updates
    async fn poll(client: Arc<Mutex<TcpClient>>, registry_addr: SocketAddr) -> Result<Vec<HostStatus>, NetworkError> {
        // Lock client for send and response cycle
        let mut client = client.lock().await;

        let msg: RpcMessageKind = RpcMessageKind::RegCtrl(RegCtrl::PollHostStatuses);

        client.send_message(registry_addr, msg).await?;
        match client.read_message(registry_addr).await {
            Ok(RpcMessage { msg, src_addr, target_addr }) => {
                if let RpcMessageKind::RegCtrl(RegCtrl::HostStatuses { host_statuses }) = msg {
                    Ok(host_statuses)
                } else {
                    error!("Received wrong message");
                    Err(NetworkError::MessageError)
                }
            }
            Err(e) => {
                error!("Failed to receive host updates");
                Err(e)
            }
        }
    }

    /// Continuously running task responsible for managing experiment related functionality
    async fn experiment_handler(
        client: Arc<Mutex<TcpClient>>,
        experiment_config_path: PathBuf,
        mut experiment_recv: Receiver<ExperimentChannelMsg>,
        update_send: Sender<OrgUpdate>,
    ) {
        info!("Started experiment handler task");
        let (cancel_signal_send, cancel_signal_recv) = watch::channel(false);
        let mut session = ExperimentSession::new(update_send.clone(), cancel_signal_recv);

        session.load_experiments(experiment_config_path.clone());
        info!("Loaded {} experiments from {experiment_config_path:?}", session.experiments.len());
        update_send
            .send(OrgUpdate::UpdateExperimentList(
                session.experiments.iter().map(|f| f.metadata.clone()).collect(),
            ))
            .await;

        while let Some(msg) = experiment_recv.recv().await {
            match msg {
                ExperimentChannelMsg::Start => {
                    if let Some(active) = &session.active_experiment {
                        match active.info.status {
                            ExperimentStatus::Running => {
                                debug!("Can't start experiment while another is running");
                            }
                            _ => {
                                cancel_signal_send.send(false);
                                let mut session = session.clone();
                                let client = client.clone();
                                let update_send = update_send.clone();

                                let handler = Arc::new(move |command: Command, update_send: Sender<OrgUpdate>| {
                                    let client = client.clone(); // clone *inside* closure body
                                    async move {
                                        Orchestrator::handle_msg(client, command.into(), update_send, None, None).await;
                                    }
                                });

                                let converter = |exp| OrgUpdate::ActiveExperiment(exp);

                                tokio::spawn(async move {
                                    session.run(update_send, converter, handler).await;
                                });
                            }
                        }
                    }
                }
                ExperimentChannelMsg::StartRemote(target_addr) => {
                    if let Some(active) = &session.active_experiment {
                        // TODO make better
                        let msg = RpcMessageKind::HostCtrl(HostCtrl::StartExperiment {
                            experiment: active.experiment.clone(),
                        });
                        client.lock().await.send_message(target_addr, msg).await;
                        info!("Starting experiment on remote")
                    }
                }
                ExperimentChannelMsg::Stop => {
                    cancel_signal_send.send(true);
                }
                ExperimentChannelMsg::StopRemote(target_addr) => {
                    let msg = RpcMessageKind::HostCtrl(HostCtrl::StopExperiment);
                    client.lock().await.send_message(target_addr, msg).await;
                }
                ExperimentChannelMsg::Select(i) => {
                    if let Some(exp) = session.experiments.get(i) {
                        session.active_experiment = Some(ActiveExperiment {
                            experiment: exp.clone(),
                            info: ExperimentInfo {
                                status: ExperimentStatus::Ready,
                                current_stage: 0,
                            },
                        })
                    }
                }
                ExperimentChannelMsg::ReloadExperimentConfigs => {
                    info!("Reloading experiments from {experiment_config_path:?}");
                    session.load_experiments(experiment_config_path.clone());
                    update_send
                        .send(OrgUpdate::UpdateExperimentList(
                            session.experiments.iter().map(|e| e.metadata.clone()).collect(),
                        ))
                        .await;
                }
            }
            if session.active_experiment.is_some() {
                update_send
                    .send(OrgUpdate::ActiveExperiment(session.active_experiment.clone().unwrap()))
                    .await;
            }
        }
    }

    /// Asynchronous task responsible for handling messages from the TUI and orchestrating communication
    /// with the TCP client, experiment manager, and registry.
    ///
    /// This task listens for incoming control messages via a channel and also optionally listens
    /// to messages from the TCP client, particularly CSI and raw frame data, if receiving is enabled.
    async fn command_handler(
        mut recv_commands_channel: Receiver<OrgChannelMsg>,
        update_send: Sender<OrgUpdate>,
        experiment_send: Sender<ExperimentChannelMsg>,
        registry_send: Sender<RegistryChannelMsg>,
        client: Arc<Mutex<TcpClient>>,
    ) {
        info!("Started stream handler task");
        let mut receiving = false;
        let mut targets: Vec<SocketAddr> = vec![];
        loop {
            tokio::select! {
                // Commands from channel
                msg_opt = recv_commands_channel.recv() => {
                    debug!("Received channel message {msg_opt:?}");
                    if let Some(msg) = msg_opt {
                        Self::handle_msg(client.clone(), msg, update_send.clone(), Some(experiment_send.clone()),Some(registry_send.clone())).await;
                    }
                }

                // TCP Client messages if in receiving mode
                _ = async {
                    if receiving {
                        for target_addr in &targets {
                            if let Ok(msg) = client.lock().await.wait_for_read_message(*target_addr).await {
                                match msg.msg {
                                    Data { data_msg: DataMsg::CsiFrame { csi }, .. } => {
                                        info!("{}: {}", msg.src_addr, csi.timestamp)
                                    }
                                    Data { data_msg: DataMsg::RawFrame { ts, .. }, .. } => {
                                        info!("{}: {ts}", msg.src_addr)
                                    }
                                    _ => (),
                                }
                            }
                        }
                    } else {
                      sleep(Duration::from_millis(10)).await; // Prevents this tasks from starving the other one
                    }
                } => {}
            }
        }
    }

    /// Processes a single `OrgChannelMsg` control message and dispatches the appropriate command
    /// to the TCP client, experiment manager, or registry.
    ///
    /// Used both inside the `command_handler` loop and externally when triggering individual commands.
    pub async fn handle_msg(
        client: Arc<Mutex<TcpClient>>,
        msg: OrgChannelMsg,
        update_send: Sender<OrgUpdate>,
        experiment_send: Option<Sender<ExperimentChannelMsg>>, // Option allows reuse of this function in experiment.rs
        registry_send: Option<Sender<RegistryChannelMsg>>,     // Option allows reuse of this function in experiment.rs
    ) {
        match msg {
            OrgChannelMsg::Connect(target_addr) => {
                client.lock().await.connect(target_addr).await;
            }
            OrgChannelMsg::Disconnect(target_addr) => {
                client.lock().await.disconnect(target_addr).await;
            }
            OrgChannelMsg::Subscribe(target_addr, msg_origin_addr, device_id) => {
                if let Some(msg_origin_addr) = msg_origin_addr {
                    info!("Subscribing to {target_addr} for device id {device_id}");
                    let msg = HostCtrl::SubscribeTo { target_addr, device_id };
                    client.lock().await.send_message(msg_origin_addr, RpcMessageKind::HostCtrl(msg)).await;
                } else {
                    info!("Subscribing to {target_addr} for device id {device_id}");
                    let msg = HostCtrl::Subscribe { device_id };
                    client.lock().await.send_message(target_addr, RpcMessageKind::HostCtrl(msg)).await;
                }
            }
            OrgChannelMsg::Unsubscribe(target_addr, msg_origin_addr, device_id) => {
                if let Some(msg_origin_addr) = msg_origin_addr {
                    info!("Unsubscribing from {target_addr} for device id {device_id}");
                    let msg = HostCtrl::UnsubscribeFrom { target_addr, device_id };
                    client.lock().await.send_message(msg_origin_addr, RpcMessageKind::HostCtrl(msg)).await;
                } else {
                    info!("Unsubscribing from {target_addr} for device id {device_id}");
                    let msg = HostCtrl::Unsubscribe { device_id };
                    client.lock().await.send_message(target_addr, RpcMessageKind::HostCtrl(msg)).await;
                }
            }
            OrgChannelMsg::SubscribeAll(target_addr, msg_origin_addr) => {
                if let Some(msg_origin_addr) = msg_origin_addr {
                    todo!("");
                    info!("Subscribing {msg_origin_addr} to all devices of {target_addr}");

                    let msg = HostCtrl::SubscribeToAll { target_addr };
                    client.lock().await.send_message(msg_origin_addr, RpcMessageKind::HostCtrl(msg)).await;
                } else {
                    info!("Subscribing to all devices of {target_addr}");
                    let msg = HostCtrl::SubscribeAll;
                    client.lock().await.send_message(target_addr, RpcMessageKind::HostCtrl(msg)).await;
                }
            }
            OrgChannelMsg::UnsubscribeAll(target_addr, msg_origin_addr) => {
                if let Some(msg_origin_addr) = msg_origin_addr {
                    todo!("");
                    info!("Unsubscribing {msg_origin_addr} from all devices of {target_addr}");
                    let msg = HostCtrl::UnsubscribeFromAll { target_addr };
                    client.lock().await.send_message(msg_origin_addr, RpcMessageKind::HostCtrl(msg)).await;
                } else {
                    info!("Unsubscribing from all devices of {target_addr}");
                    let msg = HostCtrl::UnsubscribeAll;
                    client.lock().await.send_message(target_addr, RpcMessageKind::HostCtrl(msg)).await;
                }
            }
            OrgChannelMsg::SendStatus(target_addr, host_id) => {
                let msg = RpcMessageKind::RegCtrl(RegCtrl::PollHostStatus { host_id });
                client.lock().await.send_message(target_addr, msg).await;
            }
            OrgChannelMsg::Configure(target_addr, device_id, cfg_type) => {
                let msg = RpcMessageKind::HostCtrl(HostCtrl::Configure { device_id, cfg_type });

                info!("Telling {target_addr} to configure the device handler");

                client.lock().await.send_message(target_addr, msg).await;
            }
            OrgChannelMsg::Delay(ms_delay) => sleep(Duration::from_millis(ms_delay)).await,
            OrgChannelMsg::Shutdown => todo!(),
            OrgChannelMsg::SelectExperiment(idx) => {
                if let Some(experiment_send) = experiment_send {
                    experiment_send.send(ExperimentChannelMsg::Select(idx)).await;
                }
            }
            OrgChannelMsg::StartExperiment => {
                if let Some(experiment_send) = experiment_send {
                    experiment_send.send(ExperimentChannelMsg::Start).await;
                }
            }
            OrgChannelMsg::StartRemoteExperiment(addr) => {
                if let Some(experiment_send) = experiment_send {
                    experiment_send.send(ExperimentChannelMsg::StartRemote(addr)).await;
                }
            }
            OrgChannelMsg::StopExperiment => {
                if let Some(experiment_send) = experiment_send {
                    experiment_send.send(ExperimentChannelMsg::Stop).await;
                }
            }
            OrgChannelMsg::StopRemoteExperiment(addr) => {
                if let Some(experiment_send) = experiment_send {
                    experiment_send.send(ExperimentChannelMsg::StopRemote(addr)).await;
                }
            }
            OrgChannelMsg::GetHostStatuses(target_addr) => {
                if let Some(experiment_send) = experiment_send {
                    experiment_send.send(ExperimentChannelMsg::Stop).await;
                }
            }
            OrgChannelMsg::ReloadExperimentConfigs => {
                if let Some(experiment_send) = experiment_send {
                    experiment_send.send(ExperimentChannelMsg::ReloadExperimentConfigs).await;
                }
            }
            OrgChannelMsg::Ping(target_addr) => {
                let msg = RpcMessageKind::HostCtrl(HostCtrl::Ping);
                let mut client = client.lock().await;
                client.send_message(target_addr, msg).await;
                if let Ok(response) = client.read_message(target_addr).await {
                    if let RpcMessageKind::HostCtrl(HostCtrl::Pong) = response.msg {
                        debug!("idk");
                    } else {
                        error!("Expected HostStatuses response")
                    }
                } else {
                    error!("Channel error")
                }
            }

            OrgChannelMsg::Start(target_addr, device_id) => {
                let msg = RpcMessageKind::HostCtrl(HostCtrl::Start { device_id });

                info!("Telling {target_addr} to start the device handler {device_id}");

                client.lock().await.send_message(target_addr, msg).await;
            }
            OrgChannelMsg::StartAll(target_addr) => {
                let msg = RpcMessageKind::HostCtrl(HostCtrl::StartAll);

                info!("Telling {target_addr} to start all device handlers on the node");

                client.lock().await.send_message(target_addr, msg).await;
            }
            OrgChannelMsg::Stop(target_addr, device_id) => {
                let msg = RpcMessageKind::HostCtrl(HostCtrl::Stop { device_id });

                info!("Telling {target_addr} to stop the device handler {device_id}");

                client.lock().await.send_message(target_addr, msg).await;
            }
            OrgChannelMsg::StopAll(target_addr) => {
                let msg = RpcMessageKind::HostCtrl(HostCtrl::StopAll);

                info!("Telling {target_addr} to stop all device handlers on the node");

                client.lock().await.send_message(target_addr, msg).await;
            }
            // Kinda inefficient, but tech debt
            OrgChannelMsg::ConnectRegistry(socket_addr) => {
                if let Some(registry_send) = registry_send {
                    registry_send.send(RegistryChannelMsg::Connect(socket_addr)).await;
                }
            }
            OrgChannelMsg::DisconnectRegistry => {
                if let Some(registry_send) = registry_send {
                    registry_send.send(RegistryChannelMsg::Disconnect).await;
                }
            }
            OrgChannelMsg::StartPolling => {
                if let Some(registry_send) = registry_send {
                    registry_send.send(RegistryChannelMsg::StartPolling).await;
                }
            }
            OrgChannelMsg::StopPolling => {
                if let Some(registry_send) = registry_send {
                    registry_send.send(RegistryChannelMsg::StopPolling).await;
                }
            }
            OrgChannelMsg::Poll => {
                if let Some(registry_send) = registry_send {
                    registry_send.send(RegistryChannelMsg::Poll).await;
                }
            }
        }
    }
}

// #[cfg(test)]
// mod tests {
//     use std::fs::File;
//     use std::io::Write;

//     use lib::network::experiment_config::IsRecurring;
//     use lib::network::rpc_message::{HostCtrl, RpcMessage, RpcMessageKind};
//     use tempfile::tempdir;
//     use tokio::io::{AsyncReadExt, AsyncWriteExt};
//     use tokio::time::Duration;

//     use crate::orchestrator::experiment::Experiment;

//     use super::*;

//     fn create_dummy_experiment_file(dir_path: &std::path::Path, file_name: &str, content: &str) -> PathBuf {
//         let file_path = dir_path.join(file_name);
//         let mut file = File::create(&file_path).unwrap();
//         writeln!(file, "{content}").unwrap();
//         file_path
//     }

//     #[tokio::test]
//     async fn test_orchestrator_new() {
//         let temp_dir = tempdir().unwrap();
//         let dummy_config_path = create_dummy_experiment_file(
//             temp_dir.path(),
//             "exp.yaml",
//             "- metadata:\n    name: test\n    experiment_host: !Orchestrator\n  stages: []",
//         );
//         // OrchestratorConfig does not derive Clone, so we consume it here.
//         let config = OrchestratorConfig {
//             experiments_folder: temp_dir.path().to_path_buf(),
//             tui: false,
//         };

//         let global_config = GlobalConfig {
//             log_level: log::LevelFilter::Debug,
//         };
//         let _orchestrator = Orchestrator::new(global_config, config);
//     }

//     #[tokio::test]
//     async fn test_experiment_from_yaml_valid() {
//         let temp_dir = tempdir().unwrap();
//         let yaml_content = r#"
// - metadata:
//     name: Test Experiment
//     experiment_host: !Orchestrator
//     output_path: /tmp/output.log
//   stages:
//     - name: Stage 1
//       command_blocks:
//         - commands:
//             - !Connect
//               target_addr: "127.0.0.1:8080"
//           delays:
//             init_delay: 100
//             command_delay: 50
//             is_recurring: !NotRecurring
// "#;
//         let file_path = create_dummy_experiment_file(temp_dir.path(), "valid_exp.yaml", yaml_content);
//         let experiments = Experiment::from_yaml(file_path).unwrap();
//         assert_eq!(experiments.len(), 1);
//         let experiment = &experiments[0];
//         assert_eq!(experiment.metadata.name, "Test Experiment");
//         assert_eq!(experiment.metadata.output_path, Some(PathBuf::from("/tmp/output.log")));
//         assert_eq!(experiment.stages.len(), 1);
//         assert_eq!(experiment.stages[0].name, "Stage 1");
//         assert_eq!(experiment.stages[0].command_blocks.len(), 1);
//     }

//     #[tokio::test]
//     async fn test_experiment_from_yaml_invalid_path() {
//         let result = Experiment::from_yaml(PathBuf::from("non_existent.yaml"));
//         assert!(result.is_err());
//     }

//     #[tokio::test]
//     async fn test_experiment_from_yaml_malformed_content() {
//         let temp_dir = tempdir().unwrap();
//         let file_path = create_dummy_experiment_file(temp_dir.path(), "malformed_exp.yaml", "metadata: { name: test, stages: }");
//         let result = Experiment::from_yaml(file_path);
//         assert!(result.is_err());
//     }

//     #[tokio::test]
//     async fn test_execute_command_block_simple_delay() {
//         let client = Arc::new(Mutex::new(TcpClient::new()));
//         let block = Block {
//             commands: vec![Command::Delay { delay: 10 }],
//             delays: Delays {
//                 init_delay: Some(5),
//                 command_delay: Some(1),
//                 is_recurring: IsRecurring::NotRecurring,
//             },
//         };
//         let start_time = tokio::time::Instant::now();
//         Orchestrator::execute_command_block(client, block).await.unwrap();
//         let duration = start_time.elapsed();
//         assert!(duration >= Duration::from_millis(15) && duration < Duration::from_millis(100));
//     }

//     #[tokio::test]
//     async fn test_load_experiment_and_run_empty_stages() {
//         let client = Arc::new(Mutex::new(TcpClient::new()));
//         // Correctly initialize Metadata based on its actual fields
//         let experiment = Experiment {
//             metadata: lib::network::experiment_config::Metadata {
//                 name: "empty_test".to_string(),
//                 experiment_host: ExperimentHost::Orchestrator,
//                 output_path: None,
//             },
//             stages: vec![],
//         };
//         let mut orchestrator = Orchestrator {
//             client: client.clone(),
//             experiment_config: PathBuf::new(),
//             output_path: None,
//         };
//         let result = orchestrator.load_experiment(client, experiment).await;
//         assert!(result.is_ok());
//     }

//     async fn run_simple_echo_server(addr: SocketAddr) -> tokio::task::JoinHandle<()> {
//         tokio::spawn(async move {
//             let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
//             loop {
//                 let (mut socket, _) = listener.accept().await.unwrap();
//                 tokio::spawn(async move {
//                     let mut buf = [0; 1024];
//                     loop {
//                         match socket.read(&mut buf).await {
//                             Ok(0) => return,
//                             Ok(n) => {
//                                 if socket.write_all(&buf[0..n]).await.is_err() {
//                                     return;
//                                 }
//                             }
//                             Err(_) => return,
//                         }
//                     }
//                 });
//             }
//         })
//     }

//     #[tokio::test]
//     async fn test_orchestrator_connect_command() {
//         let server_addr: SocketAddr = "127.0.0.1:34567".parse().unwrap();
//         let server_handle = run_simple_echo_server(server_addr).await;
//         tokio::time::sleep(Duration::from_millis(100)).await;

//         let client = Arc::new(Mutex::new(TcpClient::new()));
//         let command = Command::Connect { target_addr: server_addr };

//         let result = Orchestrator::match_command(client.clone(), command).await;
//         assert!(result.is_ok());

//         server_handle.abort();
//     }

//     #[tokio::test]
//     async fn test_orchestrator_ping_command() {
//         let server_addr: SocketAddr = "127.0.0.1:34568".parse().unwrap();
//         let server_handle = tokio::spawn(async move {
//             let listener = tokio::net::TcpListener::bind(server_addr).await.unwrap();
//             let (mut socket, _) = listener.accept().await.unwrap();
//             let mut buf = [0u8; 4096];
//             loop {
//                 // Read the 4-byte length prefix
//                 let mut length_buf = [0u8; 4];
//                 match socket.read_exact(&mut length_buf).await {
//                     Ok(_) => {}
//                     Err(_) => return,
//                 }
//                 let msg_length = u32::from_be_bytes(length_buf) as usize;

//                 if msg_length == 0 || msg_length > 4096 {
//                     return;
//                 }

//                 // Read the message payload
//                 match socket.read_exact(&mut buf[..msg_length]).await {
//                     Ok(_) => {}
//                     Err(_) => return,
//                 }

//                 // Deserialize using bincode
//                 if let Ok(rpc_msg) = bincode::deserialize::<RpcMessage>(&buf[..msg_length]) {
//                     if matches!(rpc_msg.msg, RpcMessageKind::HostCtrl(HostCtrl::Connect)) {
//                         // Respond to Connect first
//                         let connect_response = RpcMessage {
//                             msg: RpcMessageKind::HostCtrl(HostCtrl::Connect),
//                             src_addr: server_addr,
//                             target_addr: rpc_msg.src_addr,
//                         };
//                         let response_bytes = bincode::serialize(&connect_response).unwrap();
//                         let length_prefix = (response_bytes.len() as u32).to_be_bytes();
//                         socket.write_all(&length_prefix).await.unwrap();
//                         socket.write_all(&response_bytes).await.unwrap();
//                         socket.flush().await.unwrap();
//                     } else if matches!(rpc_msg.msg, RpcMessageKind::HostCtrl(HostCtrl::Ping)) {
//                         // Construct RpcMessage directly
//                         let pong_msg = RpcMessage {
//                             msg: RpcMessageKind::HostCtrl(HostCtrl::Pong),
//                             src_addr: server_addr,
//                             target_addr: rpc_msg.src_addr,
//                         };
//                         let response_bytes = bincode::serialize(&pong_msg).unwrap();
//                         let length_prefix = (response_bytes.len() as u32).to_be_bytes();
//                         socket.write_all(&length_prefix).await.unwrap();
//                         socket.write_all(&response_bytes).await.unwrap();
//                         socket.flush().await.unwrap();
//                     }
//                 }
//             }
//         });
//         tokio::time::sleep(Duration::from_millis(100)).await;

//         let client = Arc::new(Mutex::new(TcpClient::new()));
//         client.lock().await.connect(server_addr).await.unwrap();

//         // Consume the Connect response from the server
//         let _connect_response = client.lock().await.read_message(server_addr).await.unwrap();

//         let command = Command::Ping { target_addr: server_addr };
//         let result = Orchestrator::match_command(client.clone(), command).await;
//         assert!(result.is_ok(), "Ping command failed: {:?}", result.err());

//         server_handle.abort();
//     }
// }
