use std::{fs, path::PathBuf, future};
use clap::Parser;
use log::{error, info};
use env_logger;
use serde_yaml;
use lib::handler::device_handler::{DeviceHandler, DeviceHandlerConfig}; 
use lib::FromConfig;

/// Command-line args for the `config` binary
#[derive(Parser, Debug)]
#[command(name = "config", version, about = "Initialize and start a DeviceHandler from a YAML config")]
struct Args {
    /// Path to the YAML configuration file
    #[arg(value_name = "CONFIG_FILE", value_parser = clap::value_parser!(PathBuf))]
    config_path: PathBuf,
}

#[tokio::main]
async fn main() {
    env_logger::init();

    let args = Args::parse();
    if let Err(err) = run(args).await {
        error!("Application error: {}", err);
        std::process::exit(1);
    }
}

async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    // 1. Read the YAML file into a String
    let yaml = fs::read_to_string(&args.config_path)?;
    info!("Loaded config file: {:?}", &args.config_path);

    // 2. Deserialize into your DeviceHandlerConfig
    let cfg: DeviceHandlerConfig = serde_yaml::from_str(&yaml)?;
    info!("Parsed config: {:?}", cfg);

    // 3. Build & start the handler (note the `.await?`)
    let _handler = DeviceHandler::from_config(cfg).await?;
    info!("DeviceHandler spawned successfully.");

    // Since DeviceHandler::from_config() already calls `.start()` internally
    // (and spawns the Tokio task), we don’t need to do anything else here.

    // Optionally, you can block forever so the process doesn’t exit immediately:
    futures::future::pending::<()>().await;
    Ok(())
}