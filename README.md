# Bandwidth Meter for WLED

A real-time network bandwidth visualization tool that displays upload and download traffic on WLED LED strips using the DDP protocol. Written in Rust for high performance and low latency.

## Features

- **Real-time Visualization**: Monitor network bandwidth with smooth, interpolated LED animations
- **Multi-Platform**: Supports both macOS (via `netstat`) and Linux (via `/proc/net/dev`)
- **Remote Monitoring**: Monitor bandwidth on remote hosts via SSH
- **Dual-Direction Display**: Separate visualization for TX (upload) and RX (download) traffic
- **Flexible LED Layouts**: Multiple fill direction modes (mirrored, opposing, left, right) with configurable split ratios
- **Customizable Colors**: Support for solid colors or multi-color gradients with smooth transitions
- **Gradient Animation**: Animated color patterns that move along the LED strip with independent TX/RX animation speeds
- **Strobe Effect**: Configurable strobe alerts when bandwidth exceeds maximum capacity
- **Web Interface**: Built-in HTTP server for easy configuration via web browser
- **Live Configuration**: Change settings in real-time without restarting the application
- **Test Mode**: Simulate bandwidth at variable utilization levels or test individual LEDs

## Table of Contents

- [Installation](#installation)
- [Quick Start](#quick-start)
- [Configuration](#configuration)
  - [Networking Settings](#networking-settings)
  - [LED Layout](#led-layout)
  - [Color Settings](#color-settings)
  - [Animation Settings](#animation-settings)
  - [Advanced Settings](#advanced-settings)
- [Usage Examples](#usage-examples)
- [How It Works](#how-it-works)
- [Web Interface](#web-interface)
- [Troubleshooting](#troubleshooting)
- [Performance Tuning](#performance-tuning)

## Installation

### Prerequisites

- **WLED device** on your network
- **Rust 1.70 or later** (installation instructions below)
- **For remote monitoring**: SSH access to the target host

### Step 1: Install Rust

If you don't have Rust installed, install it using [rustup](https://rustup.rs/):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

After installation, restart your terminal and verify:
```bash
rustc --version
cargo --version
```

### Step 2: Clone the Repository

```bash
git clone https://github.com/dlnetworks/wled-bandwidth-meter.git
cd wled-bandwidth-meter
```

### Step 3: Build the Project

```bash
cargo build --release
```

This compiles an optimized binary. The build process may take a few minutes on the first run as it downloads and compiles dependencies.

The compiled binary will be located at `target/release/bandwidth_meter`.

### Step 4: Install (Optional)

**Option A: Install to Cargo bin directory** (recommended)

This installs the binary to `~/.cargo/bin/` which should be in your PATH:

```bash
cargo install --path .
```

After this, you can run `bandwidth_meter` from anywhere.

**Option B: Manual installation**

Copy the binary to a location in your PATH:

```bash
# System-wide installation
sudo cp target/release/bandwidth_meter /usr/local/bin/

# Or user-only installation
mkdir -p ~/bin
cp target/release/bandwidth_meter ~/bin/
# Add ~/bin to PATH in ~/.bashrc or ~/.zshrc if not already present
```

**Option C: Run directly from build directory**

```bash
./target/release/bandwidth_meter --help
```

## Quick Start

### First Run - Interactive Setup

When you run the bandwidth meter for the first time without any arguments, it will launch an interactive setup wizard:

```bash
bandwidth_meter
```

The setup wizard will:

1. **Detect network interfaces** - Scans your system and displays a numbered list of available interfaces
2. **Prompt for interface selection** - Choose your interface by number (e.g., 1 for en0, 2 for eth0)
3. **Request WLED IP** - Enter your WLED device IP address or hostname (e.g., `led.local` or `192.168.1.100`)
4. **Request total LEDs** - Enter the number of LEDs in your strip (e.g., `600`, `1200`)
5. **Request max bandwidth** - Enter your maximum interface speed in Gbps (e.g., `1.0`, `2.5`, `10.0`)

Example first-run session:

```
=== WLED Bandwidth Meter - First Time Setup ===

Detecting network interfaces...

Available network interfaces:
  1. en0
  2. en1
  3. eth0

Select interface (1-3): 1
Selected: en0

Enter WLED IP address or hostname (e.g., led.local or 192.168.1.100): led.local

Enter total number of LEDs in your strip: 600

Enter maximum interface speed in Gbps (e.g., 1.0, 2.5, 10.0): 10.0

=== Configuration Summary ===
Interface: en0
WLED IP: led.local
Total LEDs: 600
Max Speed: 10.0 Gbps

All other settings will use default values.
You can modify these later via the config file or web interface at http://localhost:8080

Configuration saved to: /Users/username/.config/bandwidth_meter/config.conf

Starting bandwidth meter...
```

After the initial setup, the configuration is saved and you can run `bandwidth_meter` without arguments to use your saved settings.

### Command-Line Usage

1. **Basic local monitoring** (macOS/Linux):
   ```bash
   bandwidth_meter -i en0 -w led.local
   ```

2. **Remote host monitoring** via SSH:
   ```bash
   bandwidth_meter -H root@192.168.1.1 -i eth0 -w led.local
   ```

3. **With custom colors and bandwidth limit**:
   ```bash
   bandwidth_meter -i en0 -w led.local -m 10.0 --tx_color FF0000 --rx_color 0000FF
   ```

4. **Open web interface**:
   ```bash
   bandwidth_meter -i en0 -w led.local
   # Navigate to http://localhost:8080 in your browser
   ```

## Configuration

Configuration is stored in `~/.config/bandwidth_meter/config.conf` and can be edited while the program is running. Most settings take effect immediately without requiring a restart.

### Networking Settings

#### `interface`
**Type:** String
**Default:** `"en0"`
**Requires Restart:** Yes

The network interface to monitor. Can monitor multiple interfaces by comma-separating them:
```toml
interface = "eth0"          # Single interface
interface = "eth0,eth1"     # Multiple interfaces (bandwidth is summed)
```

Common interface names:
- macOS: `en0`, `en1` (WiFi/Ethernet)
- Linux: `eth0`, `eth1`, `wlan0`, `eno1`

#### `max_gbps`
**Type:** Float
**Default:** `10.0`
**Requires Restart:** No

Maximum bandwidth in Gbps for visualization scaling. This determines what 100% LED illumination represents.

Examples:
```toml
max_gbps = 1.0      # 1 Gigabit connection
max_gbps = 2.5      # 2.5 Gigabit connection
max_gbps = 10.0     # 10 Gigabit connection
```

#### `wled_ip`
**Type:** String
**Default:** `"led.local"`
**Requires Restart:** Yes

IP address or hostname of your WLED device.

Examples:
```toml
wled_ip = "led.local"       # mDNS hostname
wled_ip = "192.168.1.100"   # Static IP
```

#### `httpd_enabled`
**Type:** Boolean
**Default:** `true`
**Requires Restart:** Yes

Enable or disable the built-in web configuration interface.

#### `httpd_ip`
**Type:** String
**Default:** `"localhost"`
**Requires Restart:** Yes

IP address for the HTTP server to listen on.
- `"localhost"` or `"127.0.0.1"` - Localhost only (default, more secure)
- `"0.0.0.0"` - Listen on all interfaces (use when accessing from other devices)

#### `httpd_port`
**Type:** Integer
**Default:** `8080`
**Requires Restart:** Yes

Port number for the HTTP server.

### LED Layout

#### `total_leds`
**Type:** Integer
**Default:** `1200`
**Requires Restart:** No

Total number of LEDs in your strip. The strip is divided in half:
- First half: One direction (TX or RX depending on `swap`)
- Second half: Other direction (RX or TX)

#### `direction`
**Type:** String (enum)
**Default:** `"mirrored"`
**Requires Restart:** No
**Options:** `"mirrored"`, `"opposing"`, `"left"`, `"right"`

Controls how LEDs fill across the strip:

- **`mirrored`**: Both halves fill from the center outward
  ```
  RX: ←←←←←← | TX: →→→→→→
       center
  ```

- **`opposing`**: Both halves fill from opposite ends inward
  ```
  RX: →→→→→→ | TX: ←←←←←←
       center
  ```

- **`left`**: Both halves fill from right to left
  ```
  RX: ←←←←←← | TX: ←←←←←←
  ```

- **`right`**: Both halves fill from left to right
  ```
  RX: →→→→→→ | TX: →→→→→→
  ```

#### `swap`
**Type:** Boolean
**Default:** `false`
**Requires Restart:** No

Swap which half of the LED strip shows TX vs RX.
- `false`: First half = RX, Second half = TX
- `true`: First half = TX, Second half = RX

#### `rx_split_percent`
**Type:** Float (0.0-100.0)
**Default:** `50.0`
**Requires Restart:** No

Percentage of LEDs allocated to RX (download) traffic. The remaining percentage is allocated to TX (upload) traffic.

Examples:
```toml
rx_split_percent = 50.0   # Equal split: 50% RX, 50% TX
rx_split_percent = 70.0   # Asymmetric: 70% RX, 30% TX
rx_split_percent = 30.0   # Asymmetric: 30% RX, 70% TX
```

Use this to emphasize download or upload traffic based on your monitoring needs.

### Color Settings

#### `color`
**Type:** String (hex color or gradient)
**Default:** `"0099FF"`
**Requires Restart:** No

Default LED color used for both TX and RX unless overridden by `tx_color` or `rx_color`.

Examples:
```toml
color = "FF0000"                    # Solid red
color = "FF0000,00FF00,0000FF"      # Red → Green → Blue gradient
```

#### `tx_color`
**Type:** String (hex color or gradient)
**Default:** `""` (uses `color`)
**Requires Restart:** No

Color specifically for TX (upload) traffic. Leave empty to use the default `color`.

Examples:
```toml
tx_color = "FF0000"                 # Red for uploads
tx_color = "FF0000,FF8800,FFFF00"   # Red → Orange → Yellow gradient
tx_color = ""                       # Use default color
```

#### `rx_color`
**Type:** String (hex color or gradient)
**Default:** `""` (uses `color`)
**Requires Restart:** No

Color specifically for RX (download) traffic. Leave empty to use the default `color`.

Examples:
```toml
rx_color = "0000FF"                 # Blue for downloads
rx_color = "0000FF,00FFFF,00FF00"   # Blue → Cyan → Green gradient
rx_color = ""                       # Use default color
```

#### `use_gradient`
**Type:** Boolean
**Default:** `true`
**Requires Restart:** No

Enable smooth gradient blending between colors.
- `true`: Smooth transitions between colors (recommended)
- `false`: Hard color segments (each LED is a solid color)

#### `interpolation`
**Type:** String (enum)
**Default:** `"linear"`
**Requires Restart:** No
**Options:** `"linear"`, `"basis"`, `"catmullrom"`

Gradient interpolation algorithm (only applies when `use_gradient = true`):
- **`linear`**: Sharp, direct transitions between colors
- **`basis`**: Smooth B-spline interpolation (very smooth)
- **`catmullrom`**: Catmull-Rom spline interpolation (smooth, passes through color points)

### Animation Settings

#### `animation_speed`
**Type:** Float
**Default:** `1.0`
**Requires Restart:** No

Speed of gradient animation in relative units.
- `0.0` - Animation disabled (static gradient)
- `0.5` - Half speed
- `1.0` - Normal speed (60 LEDs per second at 60 FPS)
- `2.0` - Double speed

The actual LED movement per second = `animation_speed × fps`.

#### `scale_animation_speed`
**Type:** Boolean
**Default:** `false`
**Requires Restart:** No

Scale animation speed based on bandwidth utilization.
- `false`: Constant animation speed
- `true`: Animation speed scales from 0 (no traffic) to `animation_speed` (max bandwidth)

When enabled, TX and RX animation speeds scale independently based on their respective bandwidth utilization. For example, if RX is at 50% and TX is at 80%, RX animation runs at 50% speed while TX animation runs at 80% speed.

#### `tx_animation_direction`
**Type:** String (enum)
**Default:** `"right"`
**Requires Restart:** No
**Options:** `"left"`, `"right"`

Direction the TX (upload) gradient animation moves:
- `"left"`: Animation moves left (gradient flows left)
- `"right"`: Animation moves right (gradient flows right)

#### `rx_animation_direction`
**Type:** String (enum)
**Default:** `"left"`
**Requires Restart:** No
**Options:** `"left"`, `"right"`

Direction the RX (download) gradient animation moves:
- `"left"`: Animation moves left (gradient flows left)
- `"right"`: Animation moves right (gradient flows right)

#### `interpolation_time_ms`
**Type:** Float
**Default:** `1000.0`
**Requires Restart:** No

Time in milliseconds to smoothly transition between bandwidth readings.

This creates a "smoothing" effect where sudden bandwidth changes are gradually animated:
- `100.0` - Very responsive, minimal smoothing (may look jittery)
- `500.0` - Moderate smoothing
- `1000.0` - Smooth, gradual transitions (default)
- `2000.0` - Very smooth, slower to react

#### `fps`
**Type:** Float
**Default:** `60.0`
**Requires Restart:** No

Rendering frame rate (frames per second).

Common values:
- `30.0` - Lower CPU usage, acceptable smoothness
- `60.0` - Good balance of smoothness and performance
- `120.0` - Very smooth, higher CPU usage
- `144.0` - Maximum smoothness for high-refresh displays

Higher FPS provides smoother animations but increases CPU usage.

### Strobe Effect Settings

#### `strobe_on_max`
**Type:** Boolean
**Default:** `false`
**Requires Restart:** No

Enable strobe effect when bandwidth exceeds maximum capacity. When enabled, the entire TX or RX segment strobes with the configured strobe color when that direction reaches 100% utilization.

#### `strobe_rate_hz`
**Type:** Float
**Default:** `3.0`
**Requires Restart:** No

Strobe frequency in Hertz (cycles per second).

Examples:
```toml
strobe_rate_hz = 1.0    # Slow: 1 strobe per second
strobe_rate_hz = 3.0    # Medium: 3 strobes per second (default)
strobe_rate_hz = 10.0   # Fast: 10 strobes per second
```

#### `strobe_duration_ms`
**Type:** Float
**Default:** `166.0`
**Requires Restart:** No

Duration of each strobe flash in milliseconds. Cannot exceed the cycle time (1000 / strobe_rate_hz).

Examples:
```toml
# At 3 Hz (333ms cycle):
strobe_duration_ms = 100.0   # Short flash
strobe_duration_ms = 166.0   # Medium flash (default, ~50% duty cycle)
strobe_duration_ms = 250.0   # Long flash
```

The web interface automatically validates this value and prevents setting durations longer than the cycle time.

#### `strobe_color`
**Type:** String (hex color)
**Default:** `"000000"` (black/off)
**Requires Restart:** No

Color to display during strobe flash. Default is black (all LEDs off), creating a flashing effect. Can be set to any hex color:

Examples:
```toml
strobe_color = "000000"   # Flash off (default)
strobe_color = "FFFFFF"   # Flash white
strobe_color = "FF0000"   # Flash red (alert color)
```

### Advanced Settings

#### `test_tx`
**Type:** Boolean
**Default:** `false`
**Requires Restart:** No

Enable TX (upload) bandwidth simulation for testing purposes. When enabled, simulates bandwidth at the percentage specified by `test_tx_percent`.

#### `test_tx_percent`
**Type:** Float (0.0-100.0)
**Default:** `100.0`
**Requires Restart:** No

Percentage of maximum bandwidth to simulate for TX when `test_tx` is enabled.

Examples:
```toml
test_tx_percent = 25.0    # Simulate 25% TX utilization
test_tx_percent = 50.0    # Simulate 50% TX utilization
test_tx_percent = 100.0   # Simulate maximum TX utilization
```

#### `test_rx`
**Type:** Boolean
**Default:** `false`
**Requires Restart:** No

Enable RX (download) bandwidth simulation for testing purposes. When enabled, simulates bandwidth at the percentage specified by `test_rx_percent`.

#### `test_rx_percent`
**Type:** Float (0.0-100.0)
**Default:** `100.0`
**Requires Restart:** No

Percentage of maximum bandwidth to simulate for RX when `test_rx` is enabled.

Examples:
```toml
test_rx_percent = 25.0    # Simulate 25% RX utilization
test_rx_percent = 50.0    # Simulate 50% RX utilization
test_rx_percent = 100.0   # Simulate maximum RX utilization
```

## Usage Examples

### Example 1: Basic Home Network Monitoring

Monitor your home network with default settings:

```bash
bandwidth_meter -i en0 -w led.local -m 1.0
```

### Example 2: Data Center with 10G Connection

Monitor a 10 Gigabit connection with custom colors:

```bash
bandwidth_meter -i eth0 -w 192.168.1.100 -m 10.0 \
  --tx_color "FF0000,FF8800" \
  --rx_color "0000FF,00FFFF"
```

Configuration file:
```toml
max_gbps = 10.0
interface = "eth0"
wled_ip = "192.168.1.100"
tx_color = "FF0000,FF8800"
rx_color = "0000FF,00FFFF"
animation_speed = 1.5
use_gradient = true
interpolation = "catmullrom"
```

### Example 3: Remote Gateway Monitoring

Monitor a remote router/gateway via SSH:

```bash
bandwidth_meter -H root@192.168.1.1 -i eth9 -w led.local -m 5.0
```

This connects to the remote host and monitors its network interface.

### Example 4: Quiet Mode for Background Operation

Run without the TUI for background/daemon operation:

```bash
bandwidth_meter -q -i en0 -w led.local
```

### Example 5: Multiple Interfaces (Bonded Connection)

Monitor multiple interfaces and sum their bandwidth:

```toml
interface = "eth0,eth1"
max_gbps = 20.0
```

### Example 6: Advanced Animation Setup

Create an animated rainbow gradient that scales with bandwidth:

```toml
color = "FF0000,FF8800,FFFF00,00FF00,0000FF,8800FF"
use_gradient = true
interpolation = "catmullrom"
animation_speed = 2.0
scale_animation_speed = true
tx_animation_direction = "right"
rx_animation_direction = "left"
interpolation_time_ms = 750.0
fps = 120.0
```

## How It Works

### Architecture

1. **Bandwidth Monitoring Thread** (Tokio async):
   - Spawns `netstat` (macOS) or reads `/proc/net/dev` (Linux)
   - For remote hosts, connects via SSH and runs monitoring command
   - Parses output and calculates bandwidth in kbps
   - Sends bandwidth updates to main thread

2. **Main Thread**:
   - Runs the TUI (Terminal User Interface)
   - Receives bandwidth updates and config changes
   - Updates shared state for the renderer
   - Displays status messages and logs

3. **Render Thread** (dedicated):
   - Runs at configurable FPS (default 60 FPS)
   - Reads current bandwidth from shared state
   - Performs smooth interpolation over `interpolation_time_ms`
   - Calculates LED positions based on `direction` mode
   - Applies colors (solid or gradient with animation)
   - Sends pixel data to WLED via DDP protocol

4. **Config Watcher Thread**:
   - Monitors `~/.config/bandwidth_meter/config.conf` for changes
   - Automatically reloads configuration when file is modified
   - Most settings apply immediately without restart

5. **HTTP Server Thread** (if enabled):
   - Provides web interface on configured IP:port
   - Allows live configuration changes
   - Auto-reloads when config file changes externally

### Data Flow

```
Network Interface → Bandwidth Monitor → Main Thread → Shared State
                                           ↓
                                      Render Thread → WLED (DDP)
                                           ↑
Config File → File Watcher → Main Thread →
Web UI → HTTP Server →
```

### Bandwidth Calculation

**macOS (`netstat -w 1 -I <interface>`):**
- Output provides bytes/second directly
- Converted to kbps: `(bytes/sec × 8) / 1000`

**Linux (`/proc/net/dev`):**
- Polls cumulative byte counters every second
- Calculates delta: `bytes_current - bytes_previous`
- Calculates time delta for precision
- Bandwidth: `(bytes_delta × 8) / (time_delta_seconds × 1000)`

**Remote (SSH):**
- Same as above, but commands run over SSH connection
- Uses `-tt` flag to disable buffering for consistent timing

### LED Mapping

The LED strip is divided into two halves:
- **First Half**: LEDs 0 to (total_leds/2 - 1)
- **Second Half**: LEDs (total_leds/2) to (total_leds - 1)

Depending on the `direction` setting, bandwidth percentage determines how many LEDs light up in each half. The `swap` setting determines which half represents TX vs RX.

### Smooth Interpolation

Bandwidth updates arrive once per second. To avoid jarring jumps, the renderer smoothly interpolates between old and new values over `interpolation_time_ms`:

```
displayed_value = old_value + (new_value - old_value) × t
where t = elapsed_time / interpolation_time_ms  (0.0 to 1.0)
```

This creates smooth, gradual transitions even with discrete bandwidth updates.

### Gradient Animation

When gradients are enabled, colors cycle along the strip. The animation offset advances each frame based on:

```
offset_delta = (animation_speed × fps × delta_time) / leds_per_direction
```

When `scale_animation_speed` is enabled, animation speed scales with the currently displayed bandwidth level, ensuring animation continues smoothly even during interpolation periods.

## Web Interface

Access the web interface at `http://localhost:8080` (or your configured IP/port).

### Features

- **Live Preview**: See current configuration
- **Easy Editing**: Click to edit any setting with auto-sizing textareas for color gradients
- **Instant Updates**: Changes saved immediately, most settings apply in real-time
- **Auto-Reload**: Detects external config file changes
- **Organized Sections**: Settings grouped by category (Testing, Network, LED Layout, Colors, Animation, Strobe, Advanced)
- **Test Mode**: Enable/disable TX and RX test mode with checkboxes and percentage sliders for variable utilization testing
- **Input Validation**: Real-time validation for settings like strobe duration with visual feedback

### Security Considerations

By default, the web interface listens on `localhost:8080` (local access only). To allow access from other devices on your network:

1. **Allow network access**:
   ```toml
   httpd_ip = "0.0.0.0"
   ```

2. **Disable HTTP server** (if not needed):
   ```toml
   httpd_enabled = false
   ```

3. **Firewall**: Use your OS firewall to restrict access if using `0.0.0.0`

## Troubleshooting

### No LEDs lighting up

1. **Check WLED connectivity**:
   ```bash
   ping led.local  # or your WLED IP
   ```

2. **Verify interface name**:
   ```bash
   # macOS
   ifconfig

   # Linux
   ip addr
   ```

3. **Check bandwidth is detected**:
   - Generate traffic (download/upload something)
   - Watch the TUI for bandwidth messages

4. **Verify WLED is receiving DDP**:
   - Check WLED web interface
   - Look for "DDP" indicator or realtime updates

### Remote monitoring not working

1. **Test SSH connection**:
   ```bash
   ssh root@192.168.1.1 "cat /proc/net/dev"
   ```

2. **Check interface exists on remote host**:
   ```bash
   ssh root@192.168.1.1 "ip addr"
   ```

3. **Verify permissions**:
   - May need root access for network stats
   - Try with `sudo` if needed

### Bandwidth values seem wrong

1. **Check `max_gbps` setting**:
   - Should match your connection speed
   - Common values: 1.0, 2.5, 5.0, 10.0

2. **Review debug log**:
   ```bash
   tail -f /tmp/bandwidth_debug.log
   ```

3. **Verify correct interface**:
   - Some systems have multiple interfaces
   - Make sure you're monitoring the active one

### Animation stuttering or choppy

1. **Lower FPS**:
   ```toml
   fps = 30.0
   ```

2. **Reduce animation speed**:
   ```toml
   animation_speed = 0.5
   ```

3. **Disable animation speed scaling**:
   ```toml
   scale_animation_speed = false
   ```

4. **Check CPU usage**:
   - High CPU may cause frame drops
   - Try lower FPS or simpler gradients

### Config changes not taking effect

- **Requires Restart**: `wled_ip`, `interface`, `httpd_*`
- **Immediate Effect**: Colors, animation, FPS, bandwidth limits, `total_leds`

Restart the application if changing settings that require it.

## Performance Tuning

### Low-End Systems

Optimize for lower CPU usage:

```toml
fps = 30.0                      # Lower frame rate
animation_speed = 0.0           # Disable animation
interpolation_time_ms = 500.0   # Faster transitions
use_gradient = false            # Hard color segments
```

### High-Performance Systems

Maximize smoothness:

```toml
fps = 144.0                     # High frame rate
animation_speed = 2.0           # Fast animation
interpolation_time_ms = 1500.0  # Very smooth transitions
use_gradient = true
interpolation = "catmullrom"    # Smoothest interpolation
```

### Network Considerations

- **Local WLED**: Minimal latency, can use high FPS
- **Remote WLED**: May want lower FPS to reduce network traffic
- **WiFi WLED**: Lower FPS (30-60) recommended for reliability

## Test Mode

### LED Position Testing

Test specific LED positions to verify wiring and layout:

```bash
bandwidth_meter -t "0-10,50,100,590-599"
```

This blinks the specified LEDs red on/off every second. Use this to:
- Verify LED strip connectivity
- Determine correct LED numbering
- Test WLED configuration

### Bandwidth Simulation Testing

Test the visualization without actual network traffic at variable utilization levels:

```toml
test_tx = true           # Enable TX simulation
test_tx_percent = 75.0   # Simulate 75% TX utilization
test_rx = true           # Enable RX simulation
test_rx_percent = 50.0   # Simulate 50% RX utilization
```

Or via web interface: Enable the TX/RX test checkboxes in the "Testing" section and adjust the percentage sliders to control simulated bandwidth levels. This allows you to:
- Test how different bandwidth levels look on your LED strip
- Verify animation speed scaling at various utilization levels
- Test strobe effect by setting percentages to 100%
- Test interpolation by changing percentages in real-time

## Command-Line Arguments

```
Usage: bandwidth_meter [OPTIONS]

Options:
  -m, --max <MAX>
          Maximum bandwidth in Gbps

  -c, --color <COLOR>
          LED colors (for both TX and RX unless overridden)

      --tx_color <TX_COLOR>
          TX LED colors

      --rx_color <RX_COLOR>
          RX LED colors

  -H, --host <HOST>
          Remote SSH host

  -w, --wled_ip <WLED_IP>
          WLED device address

  -i, --int <INTERFACE>
          Network interface to monitor

  -L, --leds <LEDS>
          Total number of LEDs

  -d, --direction <DIRECTION>
          LED fill direction mode

  -s, --swap <SWAP>
          Swap TX and RX half assignments

  -t, --test <TEST>
          Test mode (LED positions, e.g., "0-10,50,100")

  -q, --quiet
          Quiet mode (no TUI output)

  -h, --help
          Print help

  -V, --version
          Print version
```

**Note**: Command-line arguments override config file settings and save the new values to the config file on launch.

## File Locations

- **Config**: `~/.config/bandwidth_meter/config.conf`
- **Debug Log**: `/tmp/bandwidth_debug.log`

## License

[Your license here]

## Contributing

[Your contributing guidelines here]

## Support

For issues, questions, or feature requests, please [file an issue](https://github.com/dlnetworks/wled-bandwidth-meter/issues).
