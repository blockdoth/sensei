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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;
    use std::fs as std_fs;
    use simplelog::LevelFilter; // Import LevelFilter directly

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
addr: "127.0.0.1:9090"
host_id: 1
device_configs: []
"#;
        let config_path = create_dummy_config_file(temp_dir.path(), "sys_config.yaml", config_content);

        let args = Args {
            subcommand: Some(SubCommandsArgs::SystemNode(SystemNodeSubcommandArgs {
                config_path: config_path.clone(),
                addr: String::from("127.0.0.1"), // Default addr for test case
                port: 9090, // Default port for test case
            })),
            level: LevelFilter::Error,
        };

        let global_args = args.parse_global_config().unwrap();

        match &args.subcommand {
            Some(SubCommandsArgs::SystemNode(sn_args)) => {
                let sn_config_from_file = SystemNodeConfig::from_yaml(sn_args.config_path.clone()).unwrap();
                let final_config = sn_args.overlay_subcommand_args(sn_config_from_file).unwrap();
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
                experiment_config: exp_path.clone(),
                tui: false, // Default tui setting for test
            })),
            level: LevelFilter::Error,
        };
        let global_args = args.parse_global_config().unwrap();

        match &args.subcommand {
            Some(SubCommandsArgs::Orchestrator(orch_args)) => {
                let orch_config = orch_args.parse().unwrap();
                let _orchestrator = Orchestrator::new(global_args, orch_config);
                // Cannot assert on private field _orchestrator.experiment_config directly.
            }
            _ => panic!("Unexpected subcommand type"),
        }
    }
}
