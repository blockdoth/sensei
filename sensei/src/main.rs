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
use crate::registry::*;
use crate::system_node::*;
use crate::visualiser::*;

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Args = argh::from_env();

    if args.subcommand.is_some()
        && !matches!(&args.subcommand, Some(SubCommandsArgs::Five(_)))
        && !matches!(&args.subcommand, Some(SubCommandsArgs::Three(_)))
    {
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
    }

    debug!("Parsed args and initialized CombinedLogger");
    let global_args = args.parse_global_config()?;
    match &args.subcommand {
        None => lib::tui::example::run_example().await,
        Some(subcommand) => match subcommand {
            SubCommandsArgs::One(args) => {
                SystemNode::new(
                    global_args,
                    args.overlay_subcommand_args(SystemNodeConfig::from_yaml(args.config_path.clone())?)?,
                )
                .run()
                .await?
            }
            SubCommandsArgs::Two(args) => Registry::new(global_args, args.parse()?).run().await?,
            SubCommandsArgs::Three(args) => Orchestrator::new(global_args, args.parse()?).run().await?,
            SubCommandsArgs::Four(args) => Visualiser::new(global_args, args.parse()?).run().await?,
            SubCommandsArgs::Five(args) => EspTool::new(global_args, args.parse()?).run().await?,
        },
    }
    Ok(())
}
