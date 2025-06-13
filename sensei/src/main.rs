mod cli;
mod esp_tool;
mod orchestrator;
mod registry;
mod services;
mod system_node;
mod visualiser;

use std::fs::File;

use cli::*;
use esp_tool::EspTool;
use log::*;
use services::{FromYaml, Run, SystemNodeConfig};
use simplelog::{ColorChoice, CombinedLogger, LevelFilter, TermLogger, TerminalMode, WriteLogger};

use crate::orchestrator::*;
use crate::system_node::*;
use crate::visualiser::*;

#[tokio::main(flavor = "multi_thread", worker_threads = 1)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Args = argh::from_env();

    if args.subcommand.is_some() && !matches!(&args.subcommand, Some(SubCommandsArgs::EspTool(_))) {
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
    } else {
        // For EspTest, logging will be handled by its TuiLogger.
        // You might want a minimal print or log here indicating EspTest is starting,
        // but TuiLogger in esp_tool.rs will print its own startup messages.
        // println!("Starting ESP Test Tool..."); // Simple console feedback before TUI takes over
    }

    debug!("Parsed args and initialized CombinedLogger");
    let global_args = args.parse_global_config()?;
    match &args.subcommand {
        None => lib::tui::example::run_example().await,
        Some(subcommand) => match subcommand {
            SubCommandsArgs::SystemNode(args) => {
                SystemNode::new(
                    global_args,
                    args.overlay_subcommand_args(SystemNodeConfig::from_yaml(args.config_path.clone())?)?,
                )
                .run()
                .await?
            }
            SubCommandsArgs::Orchestrator(args) => Orchestrator::new(global_args, args.parse()?).run().await?,
            SubCommandsArgs::Visualiser(args) => Visualiser::new(global_args, args.parse()?).run().await?,
            SubCommandsArgs::EspTool(args) => EspTool::new(global_args, args.parse()?).run().await?,
        },
    }
    Ok(())
}
