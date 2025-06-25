# Sensei

**Sensei** is a distributed CSI (Channel State Information) data collection and analysis framework built in Rust. It enables real-time collection, processing, and visualization of Wi-Fi CSI data from various sources including ESP32 devices, Intel Wi-Fi cards, and network interfaces.

## Architecture Overview

Sensei consists of several modular components that work together to form a distributed CSI monitoring network:

- **System Node**: CSI data producers and consumers in the network
- **Orchestrator**: Network coordinator that manages experiments and commands
- **Visualiser**: Real-time data visualization with TUI and GUI interfaces  
- **ESP Tool**: Specialized tool for ESP32 CSI device management

## Quick Start

### Basic Usage

Run Sensei with a specific component:

```bash
cargo run -- [COMPONENT] [OPTIONS]
```

### Examples

```bash
# Start a system node with default configuration
cargo run -- node

# Start a system node with custom address and config
cargo run -- node --addr 192.168.1.100 --port 8080 --config-path my_config.yaml

# Run orchestrator with experiment configuration
cargo run -- orchestrator --experiment-config experiments/my_experiment.yaml

# Start visualiser with TUI interface
cargo run -- visualiser --target 127.0.0.1:8080 --ui-type tui

# Launch ESP32 tool for device management
cargo run -- esp-tool --serial-port /dev/ttyUSB0
```

## Components

### System Node (`node`)

System Nodes are the core data processing units that:
- **Collect CSI data** from various sources (ESP32, Intel Wi-Fi cards, CSV files, TCP streams)
- **Adapt and transform** raw data using configurable adapters
- **Route data** to multiple sinks (files, TCP endpoints, other nodes)
- **Execute experiments** locally or receive commands from orchestrators
- **Manage device lifecycles** with hot-pluggable device handlers

**Key Features:**
- Multi-source data ingestion pipeline
- Real-time data adaptation and filtering
- Distributed data routing and forwarding
- Device configuration management
- Registry integration for discovery

**Configuration:**
```bash
--addr <ADDRESS>          # Server bind address (default: 127.0.0.1)
--port <PORT>             # Server port (default: 6969)
--config-path <PATH>      # Device configuration file path (default: resources/testing_configs/minimal.yaml)
```

### Orchestrator (`orchestrator`)

The Orchestrator provides centralized experiment management and network coordination:
- **Experiment execution** with sequential stages and parallel command blocks
- **Network-wide command dispatch** to system nodes
- **Connection management** between nodes
- **Status monitoring** and health checks
- **Automated experiment workflows**

**Key Features:**
- YAML-based experiment definitions
- Recursive and conditional command execution
- Real-time network topology management
- Distributed experiment coordination
- Command-line and programmatic interfaces

**Configuration:**
```bash
--experiment-config <PATH>  # Experiment YAML configuration file (default: resources/example_configs/orchestrator/experiment_config.yaml)
--tui <BOOL>               # Enable terminal user interface (default: true)
```

### Visualiser (`visualiser`)

Real-time CSI data visualization with multiple interface options:
- **TUI Interface**: Terminal-based charts and graphs using Ratatui
- **GUI Interface**: Web-based visualizations with Charming/Plotters
- **Real-time plotting** of amplitude, phase, and PDP data
- **Multi-device monitoring** with configurable data sources
- **Interactive controls** for graph manipulation

**Key Features:**
- Multiple visualization types (amplitude, phase, power delay profile)
- Configurable time windows and axis bounds
- Multi-core, multi-stream, multi-subcarrier support
- Real-time FFT processing
- Interactive graph management

**Configuration:**
```bash
--target <ADDRESS>    # Target node address for data subscription (default: 127.0.0.1:6969)
--ui-type <TYPE>      # Interface type: "tui" or "gui" (default: tui)
--height <PIXELS>     # Window height for GUI mode (default: 600)
--width <PIXELS>      # Window width for GUI mode (default: 800)
```

### ESP Tool (`esp-tool`)

Specialized tool for ESP32 CSI device management:
- **Device configuration** and parameter tuning
- **Real-time monitoring** with TUI interface
- **Serial communication** management
- **CSI data collection** and validation
- **Device state management**

**Key Features:**
- Interactive ESP32 configuration
- Real-time CSI data preview
- Serial port management
- Device health monitoring
- Configuration persistence

**Configuration:**
```bash
--serial-port <PORT>    # Serial port path (default: /dev/ttyUSB0)
```

## Data Sources & Adapters

Sensei supports multiple CSI data sources through a pluggable adapter system:

### Supported Sources
- **ESP32**: Wi-Fi CSI from ESP32 devices via serial interface
- **Intel Wi-Fi**: CSI from Intel Wi-Fi cards using netlink interface  
- **CSV Files**: Historical data from CSV files
- **TCP Streams**: Network-based CSI data streams
- **Custom Sources**: Extensible source plugin system

### Data Adapters
- **ESP32 Adapter**: Processes ESP32-specific CSI frame formats
- **Intel Wi-Fi Adapter**: Handles Intel Wi-Fi CSI data structures
- **CSV Adapter**: Parses CSV-formatted CSI data
- **TCP Adapter**: Network protocol adaptation
- **Custom Adapters**: Plugin-based adapter extensions

## Configuration

### System Node Configuration

Example `node_config.yaml`:
```yaml
addr: "127.0.0.1:8000"
host_id: 1
registries:
  - "127.0.0.1:9090"
registry_polling_rate_s: 30
device_configs:
  - device_id: 0
    stype: ESP32
    source: !Esp32
      port_name: "/dev/cu.usbmodem101"
      # baud_rate: 3000000
      # csi_buffer_size: 100
      # ack_timeout_ms: 2000
    controller: !Esp32
      mode: Listening
      # device_config:
      #   channel: 1
      #   mode: Receive
      #   bandwidth: Twenty
      #   secondary_channel: None
      #   csi_type: HighThroughputLTF
      #   manual_scale: 0
      # synchronize_time: true
    adapter: !Esp32
      scale_csi: false
    output_to:
      - "stdout"
sinks:
  - id: "stdout"
    config: !File
      file: "/dev/stdout"
```

### Experiment Configuration

Example `experiment.yaml`:
```yaml
- metadata:
    name: "CSI Collection Experiment"
    experiment_host: !Orchestrator
    output_path: "results.csv"
  stages:
    - name: "setup"
      command_blocks:
        - commands:
            - !Connect
              target_addr: "127.0.0.1:8080"
            - !Subscribe  
              target_addr: "127.0.0.1:8080"
              device_id: 0
          delays:
            init_delay: 1000
            is_recurring: !NotRecurring
    - name: "collection"
      command_blocks:
        - commands:
            - !Delay
              delay: 60000  # Collect for 1 minute
          delays:
            is_recurring: !NotRecurring
    - name: "cleanup"
      command_blocks:
        - commands:
            - !Unsubscribe
              target_addr: "127.0.0.1:8080" 
              device_id: 0
            - !Disconnect
              target_addr: "127.0.0.1:8080"
          delays:
            is_recurring: !NotRecurring
```


## Logging and Debugging

Sensei provides comprehensive logging capabilities:

```bash
# Set log level (OFF, ERROR, WARN, INFO, DEBUG, TRACE)
cargo run -- --level DEBUG node

# Log output locations:
# - Terminal: Configurable level output with colors
# - File: sensei.log (ERROR level and above)
```

## Advanced Usage

### Network Topology

Sensei supports complex network topologies:

```
┌─────────────┐    ┌─────────────┐    ┌─────────────┐
│ ESP32 Node  │────│ System Node │────│ Visualiser  │
│ (Producer)  │    │ (Relay)     │    │ (Consumer)  │
└─────────────┘    └─────────────┘    └─────────────┘
       │                   │                   │
       │            ┌─────────────┐            │
       └────────────│ Orchestrator│────────────┘
                    │ (Control)   │
                    └─────────────┘
```

### Multi-Device Setup

Run multiple system nodes for distributed data collection:

```bash
# Node 1: ESP32 CSI collector
cargo run -- node --addr 192.168.1.10 --port 8080 --config-path esp32_config.yaml

# Node 2: Intel Wi-Fi collector  
cargo run -- node --addr 192.168.1.11 --port 8080 --config-path iwl_config.yaml

# Node 3: Data aggregator
cargo run -- node --addr 192.168.1.12 --port 8080 --config-path aggregator_config.yaml

# Orchestrator: Experiment coordinator
cargo run -- orchestrator --experiment-config multi_node_experiment.yaml

# Visualiser: Real-time monitoring
cargo run -- visualiser --target 192.168.1.12:8080
```

### Custom Data Processing Pipeline

Sensei's modular architecture allows custom data processing:

1. **Source**: Raw data ingestion (ESP32, Intel Wi-Fi, CSV, TCP)
2. **Controller**: Device parameter management  
3. **Adapter**: Data format transformation
4. **Sink**: Output routing (file, TCP, visualization)

## CSI Data Format

Sensei uses a standardized CSI data structure:

```rust
pub struct CsiData {
    pub timestamp: f64,              // Reception timestamp
    pub sequence_number: u16,        // Frame sequence number  
    pub rssi: Vec<u16>,             // RSSI per antenna/core
    pub csi: Vec<Vec<Vec<Complex>>>, // 3D CSI matrix: cores × streams × subcarriers
}
```

**Dimensions:**
- **Cores**: Number of antennas/spatial chains
- **Streams**: Number of spatial streams
- **Subcarriers**: Number of OFDM subcarriers (depends on bandwidth)

## Project Structure

```
sensei/
├── Cargo.toml              # Workspace configuration
├── README.md               # This file
├── flake.nix              # Nix development environment
├── lib/                   # Core library
│   ├── src/
│   │   ├── adapters/      # Data format adapters
│   │   ├── csi_types.rs   # CSI data structures
│   │   ├── handler/       # Device management
│   │   ├── network/       # TCP networking and RPC
│   │   ├── sinks/         # Data output modules
│   │   ├── sources/       # Data input modules
│   │   └── tui/           # Terminal UI components
│   └── Cargo.toml
├── sensei/                # Main application
│   ├── src/
│   │   ├── cli.rs         # Command-line interface
│   │   ├── main.rs        # Application entry point
│   │   ├── esp_tool/      # ESP32 management
│   │   ├── orchestrator/  # Experiment coordination
│   │   ├── registry/      # Device registry
│   │   ├── system_node/   # Node implementation
│   │   └── visualiser/    # Data visualization
│   └── Cargo.toml
├── resources/             # Configuration examples
│   ├── example_configs/   # Sample configurations
│   ├── test_data/         # Test datasets
│   └── testing_configs/   # Development configs
└── scripts/               # Development utilities
    ├── check.sh           # Code quality checks
    └── generate_data.py   # Test data generation
```

# Building and Development

Sensei uses Nix flakes for reproducible development environments and builds.

## Installing Nix

### Install Nix on Linux/macOS

```bash
curl -L https://nixos.org/nix/install | sh
```

### Verify Installation

```bash
nix --version
```

### Enable Experimental Features

```bash
mkdir -p $HOME/.config/nix/
echo "experimental-features = nix-command flakes" > "$HOME/.config/nix/nix.conf"
```

## Development Environment

### Enter Development Shell

```bash
# Start development environment with all dependencies
nix develop

# Within the shell, use cargo directly for faster iteration
cargo build
cargo run -- node
cargo test
```

### Build and Run

```bash
# Reproducible build
nix build

# Build and run immediately  
nix run -- node --help

# Run specific component
nix run -- visualiser --target 127.0.0.1:8080
```

## Development Workflow

### Code Quality Checks

Run comprehensive checks before committing:

```bash
# Enter development environment
nix develop

# Run all quality checks (formatting, linting, tests)
./scripts/check.sh
```

### Testing

```bash
# Run all tests
cargo test

# Run tests with logging
RUST_LOG=debug cargo test

# Run specific test module
cargo test --package lib --lib csi_types::tests
```
