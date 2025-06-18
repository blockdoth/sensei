//! # Sensei Application Main Entry Point
//!
//! This module serves as the primary entry point for the Sensei application.
//! It handles parsing of command-line arguments, initializes logging, and
//! dispatches execution to the appropriate subcommand handlers (SystemNode,
//! Orchestrator, Visualiser, or EspTool).
//!
//! ## Modules
//!
//! - `cli`: Command-line interface parsing and argument handling.
//! - `esp_tool`: Integration with the ESP tool for firmware flashing and monitoring.
//! - `orchestrator`: High-level orchestration of system components.
//! - `registry`: Manages the status and discovery of hosts within the Sensei network.
//! - `services`: Defines configurations and core traits for all Sensei services.
//! - `system_node`: Representation and management of individual system nodes.
//! - `visualiser`: Visualization tools for representing system status and logs.

mod cli;
#[cfg(feature = "esp_tool")]
mod esp_tool;
#[cfg(feature = "orchestrator")]
mod orchestrator;
mod registry;
mod services;
#[cfg(feature = "sys_node")]
mod system_node;
#[cfg(feature = "visualiser")]
mod visualiser;

use std::fs::File;

use cli::*;
#[cfg(feature = "esp_tool")]
use esp_tool::EspTool;
use log::*;
#[cfg(feature = "sys_node")]
use services::FromYaml;
use services::Run;
#[cfg(feature = "sys_node")]
use services::SystemNodeConfig;
use simplelog::{ColorChoice, CombinedLogger, LevelFilter, TermLogger, TerminalMode, WriteLogger};

#[cfg(feature = "orchestrator")]
use crate::orchestrator::*;
#[cfg(feature = "sys_node")]
use crate::system_node::*;
#[cfg(feature = "visualiser")]
use crate::visualiser::*;

#[tokio::main(flavor = "multi_thread", worker_threads = 1)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Args = argh::from_env();

    let is_esp_tool = {
        #[cfg(feature = "esp_tool")]
        {
            matches!(&args.subcommand, Some(SubCommandsArgs::EspTool(_)))
        }
        #[cfg(not(feature = "esp_tool"))]
        {
            false
        }
    };

    if args.subcommand.is_some() && !is_esp_tool {
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
        debug!("Parsed args and initialized CombinedLogger");
    }

    debug!("Parsed args and initialized CombinedLogger");
    let global_args = args.parse_global_config()?;
    match &args.subcommand {
        None => lib::tui::example::run_example().await,
        Some(subcommand) => match subcommand {
            #[cfg(feature = "sys_node")]
            SubCommandsArgs::SystemNode(args) => {
                SystemNode::new(
                    global_args,
                    args.overlay_subcommand_args(SystemNodeConfig::from_yaml(args.config_path.clone())?)?,
                )
                .run()
                .await?
            }
            #[cfg(feature = "orchestrator")]
            SubCommandsArgs::Orchestrator(args) => Orchestrator::new(global_args, args.parse()?).run().await?,
            #[cfg(feature = "visualiser")]
            SubCommandsArgs::Visualiser(args) => Visualiser::new(global_args, args.parse()?).run().await?,
            #[cfg(feature = "esp_tool")]
            SubCommandsArgs::EspTool(args) => EspTool::new(global_args, args.parse()?).run().await?,
        },
    }
    Ok(())
}
