# Sensei

Sensei can be run using:
```bash
cargo run --bin  -- [variant]
```

An example of this would be:
```bash
cargo run --bin sensei system_node
```

The variants of Sensei consist of:
- system_node
- registry
- orchestrator
- visualiser
- esp_tool

### System Node
System nodes should run on any device that should be included in the network that should broadcast CSI data.
Nodes are passive components of the system, meaning they can not act without outside commands from any of the other 3 variants.

### Orchestrator
The orchestrator can run on any machine, and is a way to interface with the network.
It lets you send any command to any listener in the system.
It has a number of commands, such as:
```bash
- connect [target_addr]
- disconnect [target_addr]
- subscribe [target_addr]
- unsubscribe [target_addr]
```

### Registry

The registry should run on a stronger machine (not a router) that will be on permanently.

### Visualiser
The visualiser connects on startup to a target node, the "data aggregator."
This node should be the last node in the system, where all the data ends up at.
The visualiser has a number of commands to manipulate and graph the data, such as:
```bash
- add/remove [graph_type] [data_source_addr] [device_id] [core] [stream] [subcarrier]
- interval [graph_index] [interval_length_ms]
- clear
```
Use esc to end the visualiser

### ESP Tool
The ESP Tool provides a terminal interface for ESP32 CSI monitoring and Wi-Fi frame transmission.
It connects directly to ESP32 devices via serial port for real-time CSI data collection and device configuration.

```bash
cargo run --bin sensei esp_tool --serial-port /dev/ttyUSB0
```

For detailed setup instructions and usage guide, see [ESP Tool Documentation](sensei/src/esp_tool/README.md).


# Building and development
Sensei uses nix flakes for setting up a declarative development environment and achieving reproducible builds.   

## Installing Nix

### Install Nix on Linux/macOS

Run the following command in your terminal to install Nix:

```bash
curl -L https://nixos.org/nix/install | sh
```

### Verify the Installation

After the installation completes, verify Nix is correctly installed by running:
```bash
nix --version
```

### Enable required nix features
This project uses the experimental, but widely adopted flakes and nix-command features. To avoid having to manually enable these features with flags each time you can run the following.

```
mkdir $HOME/.config/nix/
echo "experimental-features = nix-command flakes" > "$HOME/.config/nix/nix.conf"
```

## Using Nix
The 3 most important nix features for this project are:

### `nix build` 
Reproducibly builds the project and produces an executable.  
### `nix run` 
Reproducibly builds and immediately runs the project. 
### `nix develop`
Reproducibly creates a development environment containing all tools and dependencies specified in the flake.

## CI/CD pipeline
The pipeline is based on the nix flake. Since the flake is fully reproducible you can be completely sure that if the `nix build` / `nix run` works locally it will work in the pipeline and on other people's machines.

However at the linting and formatting stage is not directly integrated into the flake. Therefore you need to run the following command to run all code checks.
```nix
nix develop --command "./scripts/check.sh"
```
IMPORTANT: Run this script before every push and fix any issues it brings up. Otherwise the CI will likely fail.

Using the `nix` cli directly guarantees the project will compile in any environment. However during active development it will be a lot faster to enter the dev shell environment using `nix develop` and use `cargo` directly for compiling. This is mainly because nix does not cache compiled crates and instead reevaluates from scratch.

