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
#[cfg(feature = "registry")]
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
use lib::tui::example::run_example;
use log::*;
#[cfg(feature = "sys_node")]
use services::FromYaml;
#[cfg(any(feature = "esp_tool", feature = "sys_node", feature = "orchestrator", feature = "visualiser"))]
use services::Run;
#[cfg(feature = "sys_node")]
use services::SystemNodeConfig;
use simplelog::{ColorChoice, CombinedLogger, LevelFilter, TermLogger, TerminalMode, WriteLogger};
use tokio::runtime::Builder;

#[cfg(feature = "orchestrator")]
use crate::orchestrator::*;
#[cfg(feature = "registry")]
use crate::registry::Registry;
use crate::services::{OrchestratorConfig, VisualiserConfig};
#[cfg(feature = "sys_node")]
use crate::system_node::*;
#[cfg(feature = "visualiser")]
use crate::visualiser::*;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Args = argh::from_env();

    let tui_enabled = {
        #[cfg(all(feature = "esp_tool", feature = "orchestrator"))]
        {
            matches!(
                &args.subcommand,
                Some(SubCommandsArgs::EspTool(_)) | Some(SubCommandsArgs::Orchestrator(_))
            )
        }
        #[cfg(all(feature = "esp_tool", not(feature = "orchestrator")))]
        {
            matches!(&args.subcommand, Some(SubCommandsArgs::EspTool(_)))
        }
        #[cfg(all(not(feature = "esp_tool"), feature = "orchestrator"))]
        {
            matches!(&args.subcommand, Some(SubCommandsArgs::Orchestrator(_)))
        }
        #[cfg(all(not(feature = "esp_tool"), not(feature = "orchestrator")))]
        {
            false
        }
    };

    if args.subcommand.is_some() && !tui_enabled {
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

    let global_config = args.parse_global_config()?;
    // This builders allows us to select the number of worker threads based on
    // either compile flags or CLI arguments, instead of statically setting them in the source.
    let runtime = Builder::new_multi_thread()
        .worker_threads(global_config.num_workers)
        .enable_all()
        .build()?;
    debug!("Created a builder with {} workers", global_config.num_workers);
    runtime.block_on(async {
        match &args.subcommand {
            None => run_example().await,
            Some(subcommand) => match subcommand {
                #[cfg(feature = "sys_node")]
                SubCommandsArgs::SystemNode(args) => {
                    let config = match SystemNodeConfig::from_yaml(args.config_path.clone()) {
                        Ok(config_yaml) => args.merge_with_config(config_yaml),
                        Err(e) => {
                            error!("Unable to parse system node config file: {e}");
                            args.into()
                        }
                    };
                    SystemNode::new(global_config, config).run().await?;
                }

                #[cfg(feature = "orchestrator")]
                SubCommandsArgs::Orchestrator(args) => {
                    let config = match OrchestratorConfig::from_yaml(args.config_path.clone()) {
                        Ok(config_yaml) => args.merge_with_config(config_yaml),
                        Err(e) => {
                            error!("Unable to parse orchestrator node config file: {e}");
                            args.into()
                        }
                    };
                    Orchestrator::new(global_config, config).run().await?;
                }

                #[cfg(feature = "visualiser")]
                SubCommandsArgs::Visualiser(args) => {
                    let config = match VisualiserConfig::from_yaml(args.config_path.clone()) {
                        Ok(config_yaml) => args.merge_with_config(config_yaml),
                        Err(e) => {
                            error!("Unable to parse orchestrator node config file: {e}");
                            args.into()
                        }
                    };
                    Visualiser::new(global_config, config).run().await?;
                }

                #[cfg(feature = "esp_tool")]
                SubCommandsArgs::EspTool(args) => {
                    EspTool::new(global_config, args.into()).run().await?;
                }

                #[cfg(feature = "registry")]
                SubCommandsArgs::Registry(args) => {
                    Registry::new(global_config, args.into()).run().await?;
                }
            },
        }

        Ok::<_, Box<dyn std::error::Error>>(()) // ensure the block_on closure has a Result type
    })
}

#[cfg(test)]
mod tests {
    use std::fs as std_fs;
    use std::path::PathBuf;

    use simplelog::LevelFilter;
    use tempfile::tempdir;

    use super::*; // Import LevelFilter directly

    // Helper to create a dummy config file
    fn create_dummy_config_file(dir: &std::path::Path, file_name: &str, content: &str) -> PathBuf {
        let file_path = dir.join(file_name);
        std_fs::write(&file_path, content).unwrap();
        file_path
    }

    #[tokio::test]
    async fn test_main_system_node_subcommand() {
        let temp_dir = tempdir().unwrap();
        let config_content = r#"
address: "127.0.0.1:9090"
host_id: 1
device_configs: []
"#;
        let config_path = create_dummy_config_file(temp_dir.path(), "sys_config.yaml", config_content);

        let args = Args {
            subcommand: Some(SubCommandsArgs::SystemNode(SystemNodeSubcommandArgs {
                config_path: config_path.clone(),
                address: String::from("127.0.0.1"), // Default addr for test case
                port: 9090,                         // Default port for test case
            })),
            level: LevelFilter::Error,
            num_workers: 4,
        };

        let global_args = args.parse_global_config().unwrap();

        match &args.subcommand {
            Some(SubCommandsArgs::SystemNode(sn_args)) => {
                let sn_config_from_file = SystemNodeConfig::from_yaml(sn_args.config_path.clone()).unwrap();
                let final_config = sn_args.merge_with_config(sn_config_from_file);
                // Instantiate SystemNode to ensure config processing works
                let _system_node = SystemNode::new(global_args, final_config);
                // Cannot assert on private fields like _system_node.config.addr directly.
                // We trust that if new() succeeds, the config was processed correctly.
                // To verify, one would need public accessors or behavioral tests.
            }
            _ => panic!("Unexpected subcommand type"),
        }
    }

    #[tokio::test]
    async fn test_main_orchestrator_subcommand() {
        let temp_dir = tempdir().unwrap();
        let exp_content = "metadata:
  name: test_exp
stages: []";
        let exp_path = create_dummy_config_file(temp_dir.path(), "orch_exp.yaml", exp_content);

        let args = Args {
            subcommand: Some(SubCommandsArgs::Orchestrator(OrchestratorSubcommandArgs {
                experiments_dir: exp_path.clone(),
                config_path: DEFAULT_HOST_CONFIG.parse().unwrap(),
                tui: false, // Default tui setting for test
                polling_interval: 5,
            })),
            level: LevelFilter::Error,
            num_workers: 4,
        };
        let global_args = args.parse_global_config().unwrap();

        match &args.subcommand {
            Some(SubCommandsArgs::Orchestrator(orch_args)) => {
                let orch_config: OrchestratorConfig = orch_args.into();
                let _orchestrator = Orchestrator::new(global_args, orch_config);
                // Cannot assert on private field _orchestrator.experiment_config directly.
            }
            _ => panic!("Unexpected subcommand type"),
        }
    }
}
