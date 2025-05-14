mod cli;
mod module;
mod orchestrator;
mod registry;
mod system_node;

use crate::orchestrator::*;
use crate::registry::*;
use crate::system_node::*;
use cli::*;
use log::*;
use module::CliInit;
use module::RunsServer;
use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::net::SocketAddr;
use std::sync::Arc;

use simplelog::{ColorChoice, CombinedLogger, LevelFilter, TermLogger, TerminalMode, WriteLogger};
use std::fs::File;

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Args = argh::from_env();
    debug!("Parsed args");

    CombinedLogger::init(vec![
        TermLogger::new(
            args.level,
            simplelog::ConfigBuilder::new()
                // .add_filter_allow("sensei".into())
                .build(),
            TerminalMode::Mixed,
            ColorChoice::Auto,
        ),
        WriteLogger::new(
            LevelFilter::Error,
            simplelog::ConfigBuilder::new()
                // .add_filter_allow("sensei".into())
                .set_location_level(LevelFilter::Error)
                .build(),
            File::create("sensei.log").unwrap(),
        ),
    ])
    .unwrap();

    match &args.subcommand {
        SubCommandsArgsEnum::One(node_args) => {
            let node = Arc::new(SystemNode::init(node_args, &args.global_config()));
            node.start_server().await;
        }
        SubCommandsArgsEnum::Two(registry_args) => {
            let registry = Arc::new(Registry::init(registry_args, &args.global_config()));
            registry.start_server().await;
        }
        SubCommandsArgsEnum::Three(orchestrator_args) => {
            let orchestrator =
                Arc::new(Orchestrator::init(orchestrator_args, &args.global_config()));
            orchestrator.start_server().await;
        }
    }
    Ok(())
}
