mod cli;
mod config;
mod esp_tool;
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
use config::FromYaml;
use config::SystemNodeConfig;
use log::*;
use module::Run;
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

    if !matches!(&args.subcommand, SubCommandsArgs::Five(_)) {
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
                LevelFilter::Error, // You might want this to be args.level for file logging too
                simplelog::ConfigBuilder::new()
                    // .add_filter_allow("sensei".into())
                    .set_location_level(LevelFilter::Error)
                    .build(),
                File::create("sensei.log").unwrap(),
            ),
        ])
        .unwrap();
        debug!("Parsed args and initialized CombinedLogger");
    } else {
        // For EspTest, logging will be handled by its TuiLogger.
        // You might want a minimal print or log here indicating EspTest is starting,
        // but TuiLogger in esp_tool.rs will print its own startup messages.
        println!("Starting ESP Test Tool..."); // Simple console feedback before TUI takes over
    }

    match &args.subcommand {
        SubCommandsArgs::One(node_args) => {
            let yaml_cfg = SystemNodeConfig::from_yaml(node_args.config.clone())?;
            let cfg = node_args.overlay_subcommand_args(yaml_cfg)?;
            SystemNode::new(cfg.clone()).run(cfg.clone()).await?
        }
        SubCommandsArgs::Two(registry_args) => {
            Registry::new(registry_args.parse()?)
                .run(registry_args.parse()?)
                .await?
        }
        SubCommandsArgs::Three(orchestrator_args) => {
            Orchestrator::new(orchestrator_args.parse()?)
                .run(orchestrator_args.parse()?)
                .await?
        }
        SubCommandsArgs::Four(visualiser_args) => {
            Visualiser::new(visualiser_args.parse()?)
                .run(visualiser_args.parse()?)
                .await?
        }
        SubCommandsArgs::Five(esp_tool_args) => {
            // Assuming cli.rs has `pub mod esp_tool;` and src/esp_tool.rs contains the function
            esp_tool::run_esp_test_subcommand(esp_tool_args.clone()).await?;
            // .clone() is used on esp_tool_args because the match arm borrows args.subcommand,
            // and run_esp_test_subcommand takes ownership. EspToolSubcommandArgs should be Clone.
            // Ensure EspToolSubcommandArgs in cli.rs derives Clone:
            // #[derive(FromArgs, Debug, Clone)]
            // #[argh(subcommand, name = "esp-tool")]
            // pub struct EspToolSubcommandArgs { ... }
        }
    }
    Ok(())
}
