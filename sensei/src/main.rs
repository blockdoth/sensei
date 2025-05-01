mod orchestrator;
mod registry;
mod system_node;

fn main() -> Result<(), Box<dyn std::error::Error>> {
  Ok(system_node::run()?)
}
