mod cli;
mod module;
mod orchestrator;
mod registry;
mod system_node;
mod visualiser;

use crate::orchestrator::*;
use crate::registry::*;
use crate::system_node::*;
use crate::visualiser::*;
use cli::*;
use module::CliInit;
use module::RunsServer;
use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::net::SocketAddr;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Args = argh::from_env();

    match &args.subcommand {
        SubCommandsArgsEnum::One(node_args) => {
            let node = SystemNode::init(node_args, &args.global_config());
            node.start_server().await;
        }
        SubCommandsArgsEnum::Two(registry_args) => {
            let registry = Registry::init(registry_args, &args.global_config());
            registry.start_server().await;
        }
        SubCommandsArgsEnum::Three(orchestrator_args) => {
            let orchestrator = Orchestrator::init(orchestrator_args, &args.global_config());
            orchestrator.start_server().await;
        }
        SubCommandsArgsEnum::Four(visualiser_args) => {
            let visualiser = Visualiser::init(visualiser_args, &args.global_config());
            visualiser.start_server().await;
        }
    }
    Ok(())
}
