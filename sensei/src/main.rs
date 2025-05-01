mod orchestrator;
mod registry;
mod system_node;

use argh::FromArgs;

/// A simple app to perform collection from configured sources
#[derive(FromArgs, PartialEq, Debug)]
struct Args {
    /// server address (default: 127.0.0.1)
    #[argh(option)]
    addr: Option<String>,

    /// server port (default: 8080)
    #[argh(option)]
    port: Option<u16>,

    #[argh(subcommand)]
    subcommands: Option<SubCommandsEnum>,
}

#[derive(FromArgs, PartialEq, Debug)]
#[argh(subcommand)]
enum SubCommandsEnum {
    One(SystemNodeCommand),
    Two(RegistryCommand),
    Three(OrchestratorCommand),
}

#[derive(FromArgs, PartialEq, Debug)]
/// Runs the system_node code
#[argh(subcommand, name = "system_node")]
struct SystemNodeCommand {
    // Args
}

#[derive(FromArgs, PartialEq, Debug)]
/// Runs the registry code
#[argh(subcommand, name = "registery")]
struct RegistryCommand {}

#[derive(FromArgs, PartialEq, Debug)]
/// Runs the orchestrator code
#[argh(subcommand, name = "orchestrator")]
struct OrchestratorCommand {}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Args = argh::from_env();
    let addr = args.addr.unwrap_or_else(|| "127.0.0.1".to_string());
    let port = args.port.unwrap_or(1234);

    match &args.subcommands {
        Some(SubCommandsEnum::One(system_node)) => {
            println!("Running system_node code");
            system_node::run(addr, port);
        }
        Some(SubCommandsEnum::Two(registry)) => {
            println!("Running registry subcommand");
            registry::run();
        }
        Some(SubCommandsEnum::Three(orchestrator)) => {
            println!("Running orchestrator subcommand");
            orchestrator::run(addr, port);
        }
        None => {
            println!("No subcommand was provided.");
        }
    }
    Ok(())
}
