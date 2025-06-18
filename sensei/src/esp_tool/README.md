# ESP Tool

The ESP Tool is a robust CLI component of Sensei for ESP32 CSI (Channel State Information) monitoring and Wi-Fi frame transmission.

## Overview

The ESP Tool provides a terminal user interface (TUI) for:
- Real-time monitoring of CSI data from ESP32 devices
- Configuring ESP32 device parameters (channel, bandwidth, CSI type)
- Transmitting custom Wi-Fi frames for testing and analysis
- Interactive configuration of spam/burst transmission settings

## Setup

### Hardware Requirements
- ESP32 device with CSI firmware flashed
- USB/Serial connection to the ESP32
- Compatible ESP32 firmware that supports CSI data output

### Software Requirements
- Serial port access (typically `/dev/ttyUSB0` on Linux or `/dev/cu.usbserial-*` on macOS)
- Proper permissions to access the serial device

## Usage

### Basic Usage
```bash
cargo run sensei esp_tool --serial-port /dev/ttyUSB0
```

### Key Features

#### 1. CSI Monitoring Mode (Default)
- Automatically connects to the ESP32 device
- Displays real-time CSI data in a table format
- Shows connection status, device configuration, and logs

#### 2. Spam/Transmission Mode
- Switch to spam mode with 'M' key
- Configure custom Wi-Fi frames with source/destination MAC addresses
- Set burst parameters (repetitions, pause duration)
- Trigger single bursts with 'S' or toggle continuous transmission with 'T'

### TUI Controls

#### Main Panel (CSI Monitoring)
- `M` - Switch to Spam mode
- `E` - Edit spam configuration
- `S` - Trigger single burst (in spam mode)
- `T` - Toggle continuous spam transmission
- `C` - Change WiFi channel
- `B` - Toggle bandwidth (20MHz ↔ 40MHz)
- `L` - Toggle CSI type (Legacy ↔ HT-LTF)
- `.` - Clear logs
- `,` - Clear CSI data
- `Q` - Quit application

#### Spam Configuration Panel
- `Tab/Shift+Tab` - Navigate between fields
- `Arrow keys` - Move cursor within fields
- `Enter` - Apply configuration changes
- `Escape` - Exit spam config mode
- `Backspace` - Delete character at cursor

## How It Works

### Architecture

The ESP Tool follows an actor-based architecture with the following components:

1. **ESP Actor Task** (`esp_source_task`)
   - Manages serial communication with the ESP32
   - Handles device configuration updates
   - Processes incoming CSI data frames
   - Applies runtime configuration changes

2. **TUI State Management** (`TuiState`)
   - Maintains application state (connection status, logs, CSI data)
   - Handles user input and keyboard events
   - Manages configuration changes and synchronization

3. **Data Pipeline**
   - CSI data flows from ESP32 → Serial → ESP32Source → ESP32Adapter → TUI
   - Commands flow from TUI → Channel → ESP Actor → ESP32 device

### Communication Protocol

The tool communicates with ESP32 devices using a custom protocol:
- **CSI Data Frames**: Structured data containing channel state information
- **Control Commands**: Configuration updates sent to the device
- **Status Messages**: Connection and operational state information

### Configuration Management

The tool maintains two configuration states:
- **Saved Configuration**: Currently applied to the ESP32 device
- **Unsaved Configuration**: User modifications pending application

Changes are applied atomically when the user confirms them, ensuring the ESP32 device state remains consistent.

### Error Handling

Robust error handling includes:
- Serial connection failures with automatic retry
- Malformed data frame detection and recovery
- Configuration validation before applying to device
- Graceful degradation when device becomes unavailable

## Troubleshooting

### Common Issues

1. **"Failed to initialize ESP32Source"**
   - Check serial port permissions
   - Verify the correct port path
   - Ensure no other applications are using the port

2. **"DISCONNECTED (Start Fail)"**
   - ESP32 may not be properly flashed with CSI firmware
   - Check USB connection

3. **No CSI Data Received**
   - Ensure ESP32 is in the correct mode
   - Check if there's Wi-Fi activity to capture
   - Verify CSI type and channel configuration
   - ESP32 may not be properly flashed with CSI firmware

## Development

### Key Files
- `mod.rs` - Main ESP tool logic and actor task
- `state.rs` - TUI state management and event handling
- `tui.rs` - User interface rendering
- `spam_settings.rs` - Wi-Fi frame transmission configuration

### Adding New Features
1. Update `EspUpdate` enum for new events
2. Add keyboard handling in `TuiState::handle_keyboard_event`
3. Implement event handling in `TuiState::handle_update`
4. Update the UI in `tui.rs` if visual changes are needed
