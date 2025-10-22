use anyhow::Result;
use axum::{
    extract::Json,
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::{get, post},
    Router,
};
use clap::Parser;
use colorgrad::Color;
use crossterm::cursor::Show;
use crossterm::event::{poll, read, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ddp_rs::connection::DDPConnection;
use ddp_rs::protocol::{PixelConfig, ID};
use notify::{Config, Event as NotifyEvent, RecommendedWatcher, RecursiveMode, Watcher};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Terminal;
use serde::{Deserialize, Serialize};
use std::io::{self, Write};
use std::net::UdpSocket;
use std::path::PathBuf;
use std::process::{Command as StdCommand, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Real-time bandwidth visualization on WLED LED strips via DDP protocol",
    long_about = "Monitors network interface bandwidth and visualizes it in real-time on WLED LED strips.\n\
                  Upload traffic is displayed on LEDs 0-599, download traffic on LEDs 600-1199.\n\
                  Supports both linear and logarithmic scaling, custom color gradients, and remote gateway monitoring."
)]
struct Args {
    /// Maximum bandwidth in Gbps
    #[arg(short, long)]
    max: Option<f64>,

    /// LED colors (for both TX and RX unless overridden)
    #[arg(short, long)]
    color: Option<String>,

    /// TX LED colors
    #[arg(long)]
    tx_color: Option<String>,

    /// RX LED colors
    #[arg(long)]
    rx_color: Option<String>,

    /// Remote SSH host
    #[arg(short = 'H', long)]
    host: Option<String>,

    /// WLED device address
    #[arg(short, long)]
    wled_ip: Option<String>,

    /// Network interface to monitor
    #[arg(short = 'i', long = "int")]
    interface: Option<String>,

    /// Total number of LEDs
    #[arg(short = 'L', long)]
    leds: Option<usize>,

    /// LED fill direction mode
    #[arg(short = 'd', long)]
    direction: Option<String>,

    /// Swap TX and RX half assignments
    #[arg(short = 's', long)]
    swap: Option<bool>,

    /// Test mode
    #[arg(short = 't', long)]
    test: Option<String>,

    /// Quiet mode
    #[arg(short = 'q', long)]
    quiet: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct BandwidthConfig {
    max_gbps: f64,
    color: String,
    tx_color: String,
    rx_color: String,
    direction: String,
    swap: bool,
    rx_split_percent: f64,
    strobe_on_max: bool,
    strobe_rate_hz: f64,
    strobe_duration_ms: f64,
    strobe_color: String,
    animation_speed: f64,
    scale_animation_speed: bool,
    tx_animation_direction: String,
    rx_animation_direction: String,
    interpolation_time_ms: f64,
    wled_ip: String,
    interface: String,
    total_leds: usize,
    use_gradient: bool,
    interpolation: String,
    fps: f64,
    httpd_enabled: bool,
    httpd_ip: String,
    httpd_port: u16,
    test_tx: bool,
    test_rx: bool,
    test_tx_percent: f64,
    test_rx_percent: f64,
}

impl Default for BandwidthConfig {
    fn default() -> Self {
        BandwidthConfig {
            max_gbps: 10.0,
            color: "0099FF".to_string(),
            tx_color: "".to_string(),
            rx_color: "".to_string(),
            direction: "mirrored".to_string(),
            swap: false,
            rx_split_percent: 50.0,
            strobe_on_max: false,
            strobe_rate_hz: 3.0,
            strobe_duration_ms: 166.0,
            strobe_color: "000000".to_string(),
            animation_speed: 1.0,
            scale_animation_speed: false,
            tx_animation_direction: "right".to_string(),
            rx_animation_direction: "left".to_string(),
            interpolation_time_ms: 1000.0,
            wled_ip: "led.local".to_string(),
            interface: "en0".to_string(),
            total_leds: 1200,
            use_gradient: true,
            interpolation: "linear".to_string(),
            fps: 60.0,
            httpd_enabled: true,
            httpd_ip: "localhost".to_string(),
            httpd_port: 8080,
            test_tx: false,
            test_rx: false,
            test_tx_percent: 100.0,
            test_rx_percent: 100.0,
        }
    }
}

impl BandwidthConfig {

    fn load_or_default() -> Self {
        Self::load().unwrap_or_else(|_| Self::default())
    }

    fn merge_with_args(&mut self, args: &Args) -> bool {
        // Track if any args were actually provided
        let mut args_provided = false;

        // Only override config values if explicitly specified on command line
        if let Some(ref color) = args.color {
            self.color = color.clone();
            args_provided = true;
            // If -c is specified but --tx_color and --rx_color are not, clear them
            if args.tx_color.is_none() {
                self.tx_color = "".to_string();
            }
            if args.rx_color.is_none() {
                self.rx_color = "".to_string();
            }
        }

        // Individual TX/RX colors only set if explicitly specified
        if let Some(ref tx_color) = args.tx_color {
            self.tx_color = tx_color.clone();
            args_provided = true;
        }

        if let Some(ref rx_color) = args.rx_color {
            self.rx_color = rx_color.clone();
            args_provided = true;
        }

        if let Some(max) = args.max {
            self.max_gbps = max;
            args_provided = true;
        }

        if let Some(ref direction) = args.direction {
            self.direction = direction.clone();
            args_provided = true;
        }

        if let Some(ref wled_ip) = args.wled_ip {
            self.wled_ip = wled_ip.clone();
            args_provided = true;
        }

        if let Some(ref interface) = args.interface {
            self.interface = interface.clone();
            args_provided = true;
        }

        if let Some(leds) = args.leds {
            self.total_leds = leds;
            args_provided = true;
        }

        if let Some(swap) = args.swap {
            self.swap = swap;
            args_provided = true;
        }

        args_provided
    }

    fn config_path() -> Result<PathBuf> {
        let home = std::env::var("HOME")?;
        let config_dir = PathBuf::from(home).join(".config").join("bandwidth_meter");
        std::fs::create_dir_all(&config_dir)?;
        Ok(config_dir.join("config.conf"))
    }

    fn load() -> Result<Self> {
        let path = Self::config_path()?;
        let contents = std::fs::read_to_string(path)?;
        Ok(toml::from_str(&contents)?)
    }

    fn save(&self) -> Result<()> {
        let path = Self::config_path()?;

        // Build TOML with comments manually for better documentation
        let contents = format!(
            r#"# Bandwidth Meter Configuration File
# Edit this file while the program is running to change settings in real-time
# Note: Changes to wled_ip and interface require restart

# Maximum bandwidth in Gbps for visualization scaling
max_gbps = {}

# Default LED color (hex, applies to both TX and RX if not overridden)
# Can be single color: "FF0000" or gradient: "FF0000,00FF00,0000FF"
color = "{}"

# TX (upload) LED colors (hex, overrides 'color' setting)
# Can be single color: "FF0000" or gradient: "FF0000,00FF00,0000FF"
tx_color = "{}"

# RX (download) LED colors (hex, overrides 'color' setting)
# Can be single color: "0000FF" or gradient: "0000FF,00FFFF,00FF00"
rx_color = "{}"

# LED fill direction mode
# Options: "mirrored", "opposing", "left", "right"
direction = "{}"

# Swap TX and RX half assignments
# Options: true, false
swap = {}

# RX/TX LED split percentage
# Percentage of total LEDs allocated to RX (0-100), TX gets the remainder
# Example: 50.0 = 50/50 split, 70.0 = 70/30 split (RX/TX)
rx_split_percent = {}

# Strobe entire RX or TX segment when bandwidth exceeds max
# When enabled, the entire segment will flash on/off when at max utilization
strobe_on_max = {}

# Strobe rate in Hz (flashes per second)
# Controls how fast the strobe flashes when at max bandwidth
strobe_rate_hz = {}

# Strobe duration in milliseconds
# How long the strobe color is displayed (cannot exceed half the strobe cycle time)
# Example: 3 Hz = 333ms cycle, so max duration is 166ms
strobe_duration_ms = {}

# Strobe color in hex (color to display during strobe "off" phase)
# Default is "000000" (black/off). Can be any hex color like "FF0000" for red
strobe_color = "{}"

# Animation speed in LEDs per frame (0.0 = disabled, 1.0 = 60 LEDs/sec)
# Controls how fast gradients travel along the strip
animation_speed = {}

# Scale animation speed based on bandwidth utilization
# When enabled, speed scales from 0.0 (no traffic) to animation_speed (max bandwidth)
# Options: true, false
scale_animation_speed = {}

# TX (upload) animation direction
# Options: "left", "right"
tx_animation_direction = "{}"

# RX (download) animation direction
# Options: "left", "right"
rx_animation_direction = "{}"

# Bandwidth interpolation time in milliseconds
# Smoothly transitions between bandwidth readings over this time period
# Higher values = smoother but more laggy, lower values = more responsive but jittery
interpolation_time_ms = {}

# WLED device IP address or hostname (requires restart to change)
wled_ip = "{}"

# Network interface to monitor (requires restart to change)
# Can be single interface "eth0" or combined with comma "eth0,eth1"
interface = "{}"

# Total number of LEDs in the strip (can be changed while running)
# TX uses first half (0-N/2), RX uses second half (N/2-N)
total_leds = {}

# Use gradient blending between colors
# Options: true (smooth gradients), false (hard color segments)
use_gradient = {}

# Gradient interpolation mode (only applies when use_gradient = true)
# Options: "linear" (sharp), "basis" (smooth B-spline), "catmullrom" (smooth Catmull-Rom)
interpolation = "{}"

# Rendering frame rate (can be changed while running)
# Try different values like 30, 60, 120, 144 to reduce stuttering
fps = {}

# HTTP server configuration
# Enable or disable the built-in web configuration interface
httpd_enabled = {}

# IP address for the HTTP server to listen on
# Use "0.0.0.0" to listen on all interfaces, or "127.0.0.1" for localhost only
httpd_ip = "{}"

# Port for the HTTP server to listen on
httpd_port = {}

# Test Mode - Simulate TX (upload) bandwidth at maximum utilization
# Options: true, false
test_tx = {}

# Test Mode - Simulate RX (download) bandwidth at maximum utilization
# Options: true, false
test_rx = {}

# Test Mode - TX bandwidth utilization percentage (0-100)
# Controls how much of max bandwidth to simulate for TX when test_tx is enabled
test_tx_percent = {}

# Test Mode - RX bandwidth utilization percentage (0-100)
# Controls how much of max bandwidth to simulate for RX when test_rx is enabled
test_rx_percent = {}
"#,
            self.max_gbps,
            self.color,
            self.tx_color,
            self.rx_color,
            self.direction,
            self.swap,
            self.rx_split_percent,
            self.strobe_on_max,
            self.strobe_rate_hz,
            self.strobe_duration_ms,
            self.strobe_color,
            self.animation_speed,
            self.scale_animation_speed,
            self.tx_animation_direction,
            self.rx_animation_direction,
            self.interpolation_time_ms,
            self.wled_ip,
            self.interface,
            self.total_leds,
            self.use_gradient,
            self.interpolation,
            self.fps,
            self.httpd_enabled,
            self.httpd_ip,
            self.httpd_port,
            self.test_tx,
            self.test_rx,
            self.test_tx_percent,
            self.test_rx_percent,
        );

        std::fs::write(path, contents)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum InterpolationMode {
    Linear,
    Basis,
    CatmullRom,
}

#[derive(Debug, Clone, Copy)]
enum DirectionMode {
    Mirrored,
    Opposing,
    Left,
    Right,
}

// Shared state between main thread and render thread
#[derive(Clone)]
struct SharedRenderState {
    current_rx_kbps: f64,
    current_tx_kbps: f64,
    start_rx_kbps: f64,
    start_tx_kbps: f64,
    last_bandwidth_update: Option<Instant>,
    animation_speed: f64,
    scale_animation_speed: bool,
    tx_animation_direction: String,
    rx_animation_direction: String,
    interpolation_time_ms: f64,
    max_bandwidth_kbps: f64,

    // Color configuration (as strings, renderer will rebuild gradients when changed)
    tx_color: String,
    rx_color: String,
    use_gradient: bool,
    interpolation_mode: InterpolationMode,

    // Rendering configuration
    direction: DirectionMode,
    swap: bool,
    fps: f64,
    total_leds: usize,
    rx_split_percent: f64,
    strobe_on_max: bool,
    strobe_rate_hz: f64,
    strobe_duration_ms: f64,
    strobe_color: String,

    // Generation counter to detect changes
    generation: u64,
}

#[derive(Clone, Copy, Debug)]
struct Rgb {
    r: u8,
    g: u8,
    b: u8,
}

impl Rgb {
    fn from_hex(hex: &str) -> Result<Self> {
        let hex = hex.trim_start_matches('#');
        if hex.len() != 6 {
            anyhow::bail!("Invalid hex color: {}", hex);
        }
        Ok(Rgb {
            r: u8::from_str_radix(&hex[0..2], 16)?,
            g: u8::from_str_radix(&hex[2..4], 16)?,
            b: u8::from_str_radix(&hex[4..6], 16)?,
        })
    }
}

// Helper function to build gradient from color string
fn build_gradient_from_color(
    color_str: &str,
    use_gradient: bool,
    interpolation_mode: InterpolationMode,
) -> Result<(Option<colorgrad::Gradient>, Vec<Rgb>, Rgb)> {
    let hex_colors: Vec<&str> = color_str.split(',').map(|s| s.trim()).collect();

    // Parse all colors into RGB
    let mut rgb_colors = Vec::new();
    for hex in hex_colors.iter() {
        rgb_colors.push(Rgb::from_hex(hex)?);
    }

    // Build gradient only if we have multiple colors and use_gradient is enabled
    let gradient = if rgb_colors.len() >= 2 && use_gradient {
        let mut colorgrad_colors = Vec::new();
        let mut domain = Vec::new();

        // Create gradient with color plateaus and smooth transitions
        // Each color occupies ~80% of its segment as pure color, ~20% for smooth fade
        let n = rgb_colors.len();
        let segment_size = 1.0 / n as f64;
        let transition_size = segment_size * 0.1; // 10% on each side for transition

        for (i, rgb) in rgb_colors.iter().enumerate() {
            let color = Color::from_rgba8(rgb.r, rgb.g, rgb.b, 255);
            let segment_start = i as f64 * segment_size;
            let segment_end = (i as f64 + 1.0) * segment_size;

            // Start of plateau (after transition from previous color)
            let plateau_start = segment_start + transition_size;
            // End of plateau (before transition to next color)
            let plateau_end = segment_end - transition_size;

            if i == 0 {
                // First color: add at position 0.0
                colorgrad_colors.push(color.clone());
                domain.push(0.0);
            } else {
                // Transition from previous color
                colorgrad_colors.push(color.clone());
                domain.push(plateau_start);
            }

            // End of this color's plateau
            if i < n - 1 {
                colorgrad_colors.push(color);
                domain.push(plateau_end);
            }
        }

        // Add first color at end to make it cyclic
        if let Some(first_rgb) = rgb_colors.first() {
            let first_color = Color::from_rgba8(first_rgb.r, first_rgb.g, first_rgb.b, 255);
            colorgrad_colors.push(first_color.clone());
            domain.push(1.0 - transition_size);
            colorgrad_colors.push(first_color);
            domain.push(1.0);
        }

        let cg_interpolation = match interpolation_mode {
            InterpolationMode::Basis => colorgrad::Interpolation::Basis,
            InterpolationMode::CatmullRom => colorgrad::Interpolation::CatmullRom,
            _ => colorgrad::Interpolation::Linear,
        };

        let gradient = colorgrad::CustomGradient::new()
            .colors(&colorgrad_colors)
            .domain(&domain)
            .interpolation(cg_interpolation)
            .build()?;

        Some(gradient)
    } else {
        None
    };

    let solid_color = if !rgb_colors.is_empty() {
        rgb_colors[0]
    } else {
        // Default fallback
        Rgb::from_hex("0099FF")?
    };

    Ok((gradient, rgb_colors, solid_color))
}

// Dedicated renderer that runs in its own thread at configurable FPS
struct Renderer {
    ddp_conn: DDPConnection,
    shared_state: Arc<Mutex<SharedRenderState>>,
    shutdown: Arc<AtomicBool>,

    // Owned by renderer thread
    tx_animation_offset: f64,
    rx_animation_offset: f64,

    // Built from shared state
    tx_gradient: Option<colorgrad::Gradient>,
    rx_gradient: Option<colorgrad::Gradient>,
    tx_colors: Vec<Rgb>,
    rx_colors: Vec<Rgb>,
    tx_solid_color: Rgb,
    rx_solid_color: Rgb,

    // Cache to detect when gradients need rebuilding
    last_generation: u64,
}

impl Renderer {
    fn new(
        ddp_conn: DDPConnection,
        shared_state: Arc<Mutex<SharedRenderState>>,
        shutdown: Arc<AtomicBool>,
    ) -> Result<Self> {
        // Lock shared state to get initial colors
        let state = shared_state.lock().unwrap();
        let (tx_gradient, tx_colors, tx_solid_color) =
            build_gradient_from_color(&state.tx_color, state.use_gradient, state.interpolation_mode)?;
        let (rx_gradient, rx_colors, rx_solid_color) =
            build_gradient_from_color(&state.rx_color, state.use_gradient, state.interpolation_mode)?;
        let last_generation = state.generation;
        drop(state);

        Ok(Renderer {
            ddp_conn,
            shared_state,
            shutdown,
            tx_animation_offset: 0.0,
            rx_animation_offset: 0.0,
            tx_gradient,
            rx_gradient,
            tx_colors,
            rx_colors,
            tx_solid_color,
            rx_solid_color,
            last_generation,
        })
    }

    fn rebuild_gradients_if_needed(&mut self) -> Result<()> {
        let state = self.shared_state.lock().unwrap();

        // Check if generation changed (config updated)
        if state.generation != self.last_generation {
            let (tx_gradient, tx_colors, tx_solid_color) =
                build_gradient_from_color(&state.tx_color, state.use_gradient, state.interpolation_mode)?;
            let (rx_gradient, rx_colors, rx_solid_color) =
                build_gradient_from_color(&state.rx_color, state.use_gradient, state.interpolation_mode)?;

            self.tx_gradient = tx_gradient;
            self.tx_colors = tx_colors;
            self.tx_solid_color = tx_solid_color;
            self.rx_gradient = rx_gradient;
            self.rx_colors = rx_colors;
            self.rx_solid_color = rx_solid_color;
            self.last_generation = state.generation;
        }

        Ok(())
    }

    fn calculate_leds(&self, bandwidth_kbps: f64, max_bandwidth_kbps: f64, leds_per_direction: usize) -> usize {
        let percentage = bandwidth_kbps / max_bandwidth_kbps;
        let leds = (percentage * leds_per_direction as f64) as usize;
        leds.min(leds_per_direction)
    }

    fn calculate_effective_speed(&self, rx_kbps: f64, tx_kbps: f64, state: &SharedRenderState) -> (f64, f64) {
        if state.scale_animation_speed {
            // Use the currently displayed (interpolated) bandwidth values, not the target values
            // This ensures animation continues smoothly during the interpolation period
            let tx_utilization = (tx_kbps / state.max_bandwidth_kbps).clamp(0.0, 1.0);
            let rx_utilization = (rx_kbps / state.max_bandwidth_kbps).clamp(0.0, 1.0);

            // Quantize to nice fractions to avoid aliasing/stuttering
            // Use FPS for quantization to avoid stuttering at different frame rates
            let tx_quantized = (tx_utilization * state.fps).round() / state.fps;
            let rx_quantized = (rx_utilization * state.fps).round() / state.fps;

            let tx_speed = state.animation_speed * tx_quantized;
            let rx_speed = state.animation_speed * rx_quantized;

            (tx_speed, rx_speed)
        } else {
            (state.animation_speed, state.animation_speed)
        }
    }

    fn calculate_led_positions(&self, tx_leds: usize, rx_leds: usize, direction: DirectionMode, swap: bool, total_leds: usize, leds_per_direction: usize) -> (Vec<usize>, Vec<usize>) {
        let half = leds_per_direction;

        let (first_half_leds, second_half_leds) = if swap {
            (tx_leds, rx_leds)
        } else {
            (rx_leds, tx_leds)
        };

        let (first_half_pos, second_half_pos) = match direction {
            DirectionMode::Mirrored => {
                let first: Vec<usize> = (0..first_half_leds).map(|i| half - 1 - i).collect();
                let second: Vec<usize> = (0..second_half_leds).map(|i| half + i).collect();
                (first, second)
            }
            DirectionMode::Opposing => {
                let first: Vec<usize> = (0..first_half_leds).collect();
                let second: Vec<usize> = (0..second_half_leds)
                    .map(|i| total_leds - 1 - i)
                    .collect();
                (first, second)
            }
            DirectionMode::Left => {
                let first: Vec<usize> = (0..first_half_leds).map(|i| half - 1 - i).collect();
                let second: Vec<usize> = (0..second_half_leds)
                    .map(|i| total_leds - 1 - i)
                    .collect();
                (first, second)
            }
            DirectionMode::Right => {
                let first: Vec<usize> = (0..first_half_leds).collect();
                let second: Vec<usize> = (0..second_half_leds).map(|i| half + i).collect();
                (first, second)
            }
        };

        if swap {
            (first_half_pos, second_half_pos)
        } else {
            (second_half_pos, first_half_pos)
        }
    }

    fn render_frame(&mut self, delta_seconds: f64) -> Result<()> {
        // Rebuild gradients if config changed (very quick check)
        self.rebuild_gradients_if_needed()?;

        // Lock shared state only long enough to read current values
        let state = self.shared_state.lock().unwrap();

        // Interpolate bandwidth values for smooth transitions
        let (rx_kbps, tx_kbps) = if let Some(last_update) = state.last_bandwidth_update {
            let elapsed_ms = last_update.elapsed().as_secs_f64() * 1000.0;
            let interpolation_time = state.interpolation_time_ms;
            let t = (elapsed_ms / interpolation_time).min(1.0); // Interpolation factor (0.0 to 1.0)

            // Smoothly transition from start to current over interpolation_time_ms
            let interpolated_rx = state.start_rx_kbps + (state.current_rx_kbps - state.start_rx_kbps) * t;
            let interpolated_tx = state.start_tx_kbps + (state.current_tx_kbps - state.start_tx_kbps) * t;

            (interpolated_rx, interpolated_tx)
        } else {
            // No update yet, use current values
            (state.current_rx_kbps, state.current_tx_kbps)
        };

        let max_bandwidth_kbps = state.max_bandwidth_kbps;
        let direction = state.direction;
        let swap = state.swap;
        let use_gradient = state.use_gradient;
        let (tx_effective_speed, rx_effective_speed) = self.calculate_effective_speed(rx_kbps, tx_kbps, &state);
        let fps = state.fps;
        let tx_animation_direction = state.tx_animation_direction.clone();
        let rx_animation_direction = state.rx_animation_direction.clone();
        let total_leds = state.total_leds;
        let rx_split_percent = state.rx_split_percent.clamp(0.0, 100.0);
        let strobe_on_max = state.strobe_on_max;
        let strobe_rate_hz = state.strobe_rate_hz;
        let strobe_duration_ms = state.strobe_duration_ms;
        let strobe_color_str = state.strobe_color.clone();
        drop(state); // Release lock immediately

        // Parse strobe color
        let strobe_color = Rgb::from_hex(&strobe_color_str).unwrap_or(Rgb { r: 0, g: 0, b: 0 });

        // Calculate LED split based on rx_split_percent
        let rx_leds_available = ((total_leds as f64 * rx_split_percent) / 100.0) as usize;
        let tx_leds_available = total_leds - rx_leds_available;
        let leds_per_direction = total_leds / 2; // Keep for backward compatibility with position calculations

        // Calculate LED counts using the configurable split
        let rx_leds = self.calculate_leds(rx_kbps, max_bandwidth_kbps, rx_leds_available);
        let tx_leds = self.calculate_leds(tx_kbps, max_bandwidth_kbps, tx_leds_available);

        // Determine if we're in strobe mode for each segment
        let mut rx_strobe_active = false;
        let mut tx_strobe_active = false;

        if strobe_on_max && strobe_rate_hz > 0.0 {
            let now = SystemTime::now();
            let elapsed_millis = now.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_millis();

            // Calculate full cycle time in milliseconds
            let cycle_ms = (1000.0 / strobe_rate_hz) as u128;
            // Clamp strobe duration to not exceed the full cycle
            let clamped_duration = (strobe_duration_ms as u128).min(cycle_ms);

            // Determine position within the current cycle
            let position_in_cycle = elapsed_millis % cycle_ms;
            // Strobe is active during the last 'duration' milliseconds of each cycle
            let strobe_phase_active = position_in_cycle >= (cycle_ms - clamped_duration);

            // Activate strobe if at max and in strobe phase
            if rx_leds >= rx_leds_available && strobe_phase_active {
                rx_strobe_active = true;
            }

            if tx_leds >= tx_leds_available && strobe_phase_active {
                tx_strobe_active = true;
            }
        }

        // Update animation offsets independently for TX and RX
        if tx_effective_speed > 0.0 {
            let leds_per_second = tx_effective_speed * fps;
            let offset_delta = (leds_per_second * delta_seconds) / leds_per_direction as f64;
            self.tx_animation_offset = (self.tx_animation_offset + offset_delta) % 1.0;
        }

        if rx_effective_speed > 0.0 {
            let leds_per_second = rx_effective_speed * fps;
            let offset_delta = (leds_per_second * delta_seconds) / leds_per_direction as f64;
            self.rx_animation_offset = (self.rx_animation_offset + offset_delta) % 1.0;
        }

        // Prepare frame
        let frame_size = total_leds * 3;
        let mut frame = vec![0u8; frame_size];

        let (tx_positions, rx_positions) = self.calculate_led_positions(tx_leds, rx_leds, direction, swap, total_leds, leds_per_direction);

        // Render TX positions
        if tx_strobe_active {
            // Strobe mode: fill all TX LEDs with strobe color
            for &led_pos in tx_positions.iter() {
                let offset = led_pos * 3;
                frame[offset] = strobe_color.r;
                frame[offset + 1] = strobe_color.g;
                frame[offset + 2] = strobe_color.b;
            }
        } else if !use_gradient && self.tx_colors.len() >= 2 && !tx_positions.is_empty() {
            let num_leds = tx_positions.len() as f64;
            let pattern_offset = if tx_animation_direction == "right" {
                -self.tx_animation_offset * num_leds
            } else {
                self.tx_animation_offset * num_leds
            };
            let segment_size = num_leds / self.tx_colors.len() as f64;

            for (i, &led_pos) in tx_positions.iter().enumerate() {
                let pattern_pos = ((i as f64 + pattern_offset) % num_leds + num_leds) % num_leds;
                let segment_idx = (pattern_pos / segment_size).floor() as usize % self.tx_colors.len();
                let color = &self.tx_colors[segment_idx];

                let offset = led_pos * 3;
                frame[offset] = color.r;
                frame[offset + 1] = color.g;
                frame[offset + 2] = color.b;
            }
        } else if let Some(ref tx_gradient) = self.tx_gradient {
            for &led_pos in tx_positions.iter() {
                // Map LED position to gradient position (0.0-1.0 across the full TX half)
                let pos_ratio = (led_pos % leds_per_direction) as f64 / leds_per_direction as f64;
                let animated_pos = if tx_animation_direction == "right" {
                    (1.0 + pos_ratio - self.tx_animation_offset) % 1.0
                } else {
                    (pos_ratio + self.tx_animation_offset) % 1.0
                };

                let rgba = tx_gradient.at(animated_pos).to_rgba8();
                let offset = led_pos * 3;
                frame[offset] = rgba[0];
                frame[offset + 1] = rgba[1];
                frame[offset + 2] = rgba[2];
            }
        } else {
            for &led_pos in &tx_positions {
                let offset = led_pos * 3;
                frame[offset] = self.tx_solid_color.r;
                frame[offset + 1] = self.tx_solid_color.g;
                frame[offset + 2] = self.tx_solid_color.b;
            }
        }

        // Render RX positions
        if rx_strobe_active {
            // Strobe mode: fill all RX LEDs with strobe color
            for &led_pos in rx_positions.iter() {
                let offset = led_pos * 3;
                frame[offset] = strobe_color.r;
                frame[offset + 1] = strobe_color.g;
                frame[offset + 2] = strobe_color.b;
            }
        } else if !use_gradient && self.rx_colors.len() >= 2 && !rx_positions.is_empty() {
            let num_leds = rx_positions.len() as f64;
            let pattern_offset = if rx_animation_direction == "right" {
                -self.rx_animation_offset * num_leds
            } else {
                self.rx_animation_offset * num_leds
            };
            let segment_size = num_leds / self.rx_colors.len() as f64;

            for (i, &led_pos) in rx_positions.iter().enumerate() {
                let pattern_pos = ((i as f64 + pattern_offset) % num_leds + num_leds) % num_leds;
                let segment_idx = (pattern_pos / segment_size).floor() as usize % self.rx_colors.len();
                let color = &self.rx_colors[segment_idx];

                let offset = led_pos * 3;
                frame[offset] = color.r;
                frame[offset + 1] = color.g;
                frame[offset + 2] = color.b;
            }
        } else if let Some(ref rx_gradient) = self.rx_gradient {
            for &led_pos in rx_positions.iter() {
                // Map LED position to gradient position (0.0-1.0 across the full RX half)
                let pos_ratio = (led_pos % leds_per_direction) as f64 / leds_per_direction as f64;
                let animated_pos = if rx_animation_direction == "right" {
                    (1.0 + pos_ratio - self.rx_animation_offset) % 1.0
                } else {
                    (pos_ratio + self.rx_animation_offset) % 1.0
                };

                let rgba = rx_gradient.at(animated_pos).to_rgba8();
                let offset = led_pos * 3;
                frame[offset] = rgba[0];
                frame[offset + 1] = rgba[1];
                frame[offset + 2] = rgba[2];
            }
        } else {
            for &led_pos in &rx_positions {
                let offset = led_pos * 3;
                frame[offset] = self.rx_solid_color.r;
                frame[offset + 1] = self.rx_solid_color.g;
                frame[offset + 2] = self.rx_solid_color.b;
            }
        }

        // Write to DDP connection
        self.ddp_conn.write_offset(&frame, 0)?;

        Ok(())
    }

    // Main render loop that runs at configurable FPS
    fn run(mut self) {
        let mut last_frame = Instant::now();

        loop {
            // Check for shutdown signal
            if self.shutdown.load(Ordering::Relaxed) {
                break;
            }

            // Read FPS from shared state
            let fps = {
                let state = self.shared_state.lock().unwrap();
                state.fps
            };

            // Calculate frame duration based on FPS
            let frame_duration_micros = (1_000_000.0 / fps) as u64;
            let frame_duration = Duration::from_micros(frame_duration_micros);

            let now = Instant::now();
            let elapsed = now.duration_since(last_frame);

            if elapsed >= frame_duration {
                let delta_seconds = elapsed.as_secs_f64();
                last_frame = now;

                // Render frame - this is the only thing happening in this thread
                let _ = self.render_frame(delta_seconds);
            }

            // Tiny sleep to avoid spinning CPU at 100%
            thread::sleep(Duration::from_micros(100));
        }
    }
}

// Detect OS type (Darwin/Linux) via uname
async fn detect_os(host: Option<&String>) -> Result<String> {
    let output = if let Some(host) = host {
        Command::new("ssh")
            .arg(host)
            .arg("uname")
            .stdin(Stdio::inherit())
            .stderr(Stdio::inherit())
            .output()
            .await?
    } else {
        Command::new("uname")
            .output()
            .await?
    };

    let os_name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(os_name)
}

// Spawn bandwidth monitoring command based on OS
async fn spawn_bandwidth_monitor(args: &Args, config: &BandwidthConfig) -> Result<tokio::process::Child> {
    if args.host.is_some() {
        // For remote hosts, use a single SSH connection that auto-detects OS and runs appropriate command
        spawn_remote_monitor(args.host.as_ref().unwrap(), &config.interface).await
    } else {
        // Local monitoring - detect OS
        let os = detect_os(None).await?;

        let child = if os == "Darwin" {
            // macOS: use netstat
            spawn_netstat_monitor(None, &config.interface).await?
        } else {
            // Linux: use /proc/net/dev
            spawn_procnet_monitor(None, &config.interface).await?
        };

        Ok(child)
    }
}

// Remote monitoring with OS auto-detection in a single SSH session
async fn spawn_remote_monitor(host: &String, interface: &str) -> Result<tokio::process::Child> {
    // Parse comma-separated interfaces for egrep pattern (Linux)
    let interfaces: Vec<&str> = interface.split(',').map(|s| s.trim()).collect();
    let egrep_pattern = interfaces.join("|");

    // Create a script that detects OS and runs appropriate monitoring command
    // This all runs in ONE SSH session, so only ONE password prompt
    let script = format!(
        r#"
OS=$(uname)
if [ "$OS" = "Darwin" ]; then
    # macOS
    netstat -w 1 -I {}
else
    # Linux
    while true; do cat /proc/net/dev | egrep '({})'; sleep 1; done
fi
"#,
        interface, egrep_pattern
    );

    let child = Command::new("ssh")
        .arg(host)
        .arg(&script)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;

    Ok(child)
}

// macOS: netstat -w 1 -I <interfaces>
async fn spawn_netstat_monitor(host: Option<&String>, interface: &str) -> Result<tokio::process::Child> {
    let netstat_cmd = format!("netstat -w 1 -I {}", interface);

    let child = if let Some(host) = host {
        // SSH without pseudo-terminal - allows password prompt via stdin/stderr
        Command::new("ssh")
            .arg(host)
            .arg(&netstat_cmd)
            .stdin(Stdio::inherit())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?
    } else {
        Command::new("sh")
            .arg("-c")
            .arg(&netstat_cmd)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?
    };

    Ok(child)
}

// Linux: poll /proc/net/dev and stream raw data
async fn spawn_procnet_monitor(host: Option<&String>, interface: &str) -> Result<tokio::process::Child> {
    // Parse comma-separated interfaces for egrep pattern
    let interfaces: Vec<&str> = interface.split(',').map(|s| s.trim()).collect();
    let egrep_pattern = interfaces.join("|");

    // Simple script: just output raw /proc/net/dev lines every second
    // All calculation will be done in Rust
    let script = format!(
        "while true; do cat /proc/net/dev | egrep '({})'; sleep 1; done",
        egrep_pattern
    );

    let child = if let Some(host) = host {
        // SSH without pseudo-terminal - allows password prompt via stdin/stderr
        Command::new("ssh")
            .arg(host)
            .arg(&script)
            .stdin(Stdio::inherit())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?
    } else {
        Command::new("sh")
            .arg("-c")
            .arg(&script)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?
    };

    Ok(child)
}

fn get_timestamp() -> String {
    let now = SystemTime::now();
    let duration = now.duration_since(SystemTime::UNIX_EPOCH).unwrap();
    let secs = duration.as_secs();
    let millis = duration.subsec_millis();

    // Format as HH:MM:SS.mmm
    let hours = (secs / 3600) % 24;
    let minutes = (secs / 60) % 60;
    let seconds = secs % 60;

    format!("{:02}:{:02}:{:02}.{:03}", hours, minutes, seconds, millis)
}

// State for tracking bandwidth calculation per interface
struct InterfaceState {
    prev_rx_bytes: u64,
    prev_tx_bytes: u64,
    prev_time: Instant,
}

struct BandwidthTracker {
    interfaces: std::collections::HashMap<String, InterfaceState>,
}

impl BandwidthTracker {
    fn new() -> Self {
        BandwidthTracker {
            interfaces: std::collections::HashMap::new(),
        }
    }

    // Parse /proc/net/dev line and accumulate bandwidth
    // Returns Some when all interfaces have been processed (after collecting all lines)
    fn update_from_procnet_line(&mut self, line: &str) -> Option<(f64, f64)> {
        // Format: "  eth9: 12345 ... (16 fields total)"
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() != 2 {
            return None;
        }

        let iface = parts[0].trim();
        let fields: Vec<&str> = parts[1].trim().split_whitespace().collect();

        // /proc/net/dev format:
        // RX: bytes packets errs drop fifo frame compressed multicast
        // TX: bytes packets errs drop fifo colls carrier compressed
        if fields.len() < 16 {
            return None;
        }

        let rx_bytes = fields[0].parse::<u64>().ok()?;
        let tx_bytes = fields[8].parse::<u64>().ok()?;

        let now = Instant::now();

        if let Some(state) = self.interfaces.get(iface) {
            let time_delta = now.duration_since(state.prev_time).as_secs_f64();
            if time_delta > 0.0 {
                let rx_delta = rx_bytes.saturating_sub(state.prev_rx_bytes) as f64;
                let tx_delta = tx_bytes.saturating_sub(state.prev_tx_bytes) as f64;

                // Calculate kbps: (bytes * 8) / (time_seconds * 1000)
                let rx_kbps = (rx_delta * 8.0) / (time_delta * 1000.0);
                let tx_kbps = (tx_delta * 8.0) / (time_delta * 1000.0);

                self.interfaces.insert(
                    iface.to_string(),
                    InterfaceState {
                        prev_rx_bytes: rx_bytes,
                        prev_tx_bytes: tx_bytes,
                        prev_time: now,
                    },
                );

                // Return the bandwidth for this interface
                return Some((rx_kbps, tx_kbps));
            }
        }

        // First reading - just store values
        self.interfaces.insert(
            iface.to_string(),
            InterfaceState {
                prev_rx_bytes: rx_bytes,
                prev_tx_bytes: tx_bytes,
                prev_time: now,
            },
        );

        None
    }
}

fn parse_bandwidth_line(line: &str, tracker: &mut Option<BandwidthTracker>) -> Option<(f64, f64)> {
    let parts: Vec<&str> = line.trim().split_whitespace().collect();

    // macOS netstat format: 7 columns (packets errs bytes packets errs bytes colls)
    // Column 2 = input bytes/sec, Column 5 = output bytes/sec
    if parts.len() == 7 {
        let rx_bytes_per_sec = parts[2].parse::<f64>().ok()?;
        let tx_bytes_per_sec = parts[5].parse::<f64>().ok()?;

        // Convert bytes/sec to kbps
        let rx_kbps = (rx_bytes_per_sec * 8.0) / 1000.0;
        let tx_kbps = (tx_bytes_per_sec * 8.0) / 1000.0;

        Some((rx_kbps, tx_kbps))
    }
    // Linux /proc/net/dev format: interface: rx_bytes ... (has colon)
    else if line.contains(':') {
        if let Some(t) = tracker {
            t.update_from_procnet_line(line)
        } else {
            None
        }
    } else {
        None
    }
}

fn parse_led_numbers(test_str: &str) -> Result<Vec<usize>> {
    let mut leds = Vec::new();

    for part in test_str.split(',') {
        let part = part.trim();
        if part.contains('-') {
            let range_parts: Vec<&str> = part.split('-').collect();
            if range_parts.len() == 2 {
                let start = range_parts[0].parse::<usize>()?;
                let end = range_parts[1].parse::<usize>()?;
                for i in start..=end {
                    leds.push(i);
                }
            }
        } else {
            leds.push(part.parse::<usize>()?);
        }
    }

    Ok(leds)
}

async fn test_mode(args: &Args) -> Result<()> {
    let test_str = args.test.as_ref().unwrap();
    let led_numbers = parse_led_numbers(test_str)?;

    let default_wled = "led.local".to_string();
    let wled_ip = args.wled_ip.as_ref().unwrap_or(&default_wled);

    println!("Test mode: blinking LEDs {:?}", led_numbers);
    println!("Connecting to WLED at {}:4048", wled_ip);

    let dest_addr = format!("{}:4048", wled_ip);
    let socket = UdpSocket::bind("0.0.0.0:4048")?;
    let mut ddp_conn =
        DDPConnection::try_new(&dest_addr, PixelConfig::default(), ID::Default, socket)?;

    println!("Connected! Starting blink loop...");

    let test_color = Rgb::from_hex("FF0000")?;
    let mut iteration = 0;

    loop {
        iteration += 1;
        println!("\n=== Iteration {} ===", iteration);

        println!("Turning ON LEDs {:?}", led_numbers);
        let max_led = led_numbers.iter().max().copied().unwrap_or(0);
        let frame_size = (max_led + 1) * 3;
        let mut frame = vec![0u8; frame_size];

        for &led_num in &led_numbers {
            let offset = led_num * 3;
            frame[offset] = test_color.r;
            frame[offset + 1] = test_color.g;
            frame[offset + 2] = test_color.b;
        }

        ddp_conn.write_offset(&frame, 0)?;
        println!("Sent {} bytes starting at offset 0", frame.len());

        tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;

        println!("Turning OFF LEDs");
        let frame = vec![0u8; frame_size];
        ddp_conn.write_offset(&frame, 0)?;
        println!("Sent {} bytes of black", frame.len());

        tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
    }
}
// HTTP Configuration Server Module

const WEB_UI_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Bandwidth Meter Configuration</title>
    <style>
        * { margin: 0; padding: 0; box-sizing: border-box; }
        body {
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Oxygen, Ubuntu, Cantarell, sans-serif;
            background: #1a1a1a;
            color: #e0e0e0;
            padding: 20px;
            line-height: 1.6;
        }
        .container {
            max-width: 900px;
            margin: 0 auto;
        }
        h1 {
            color: #00aaff;
            margin-bottom: 30px;
            font-size: 2em;
        }
        .config-grid {
            display: grid;
            gap: 15px;
        }
        .config-item {
            margin-bottom: 12px;
        }
        .config-item label {
            display: block;
            color: #b0b0b0;
            margin-bottom: 8px;
            font-size: 0.9em;
            text-transform: uppercase;
            letter-spacing: 0.5px;
        }
        .input-group {
            display: flex;
            gap: 10px;
            align-items: center;
        }
        input[type="text"], input[type="number"], select, textarea {
            flex: 1;
            background: #1a1a1a;
            border: 1px solid #505050;
            color: #e0e0e0;
            padding: 10px 12px;
            border-radius: 4px;
            font-size: 1em;
        }
        input.invalid {
            border: 2px solid #ff4444;
            background: #3a1a1a;
        }
        button:disabled {
            background: #555555;
            cursor: not-allowed;
            opacity: 0.5;
        }
        input[type="checkbox"] {
            width: 20px;
            height: 20px;
            cursor: pointer;
        }
        input[type="range"] {
            flex: 1;
            cursor: pointer;
        }
        .range-value {
            min-width: 80px;
            text-align: center;
            color: #00aaff;
            font-weight: 600;
        }
        button {
            background: #00aaff;
            color: white;
            border: none;
            padding: 10px 20px;
            border-radius: 4px;
            cursor: pointer;
            font-size: 0.9em;
            font-weight: 600;
            transition: background 0.2s;
        }
        button:hover {
            background: #0088cc;
        }
        button:active {
            background: #006699;
        }
        .message {
            position: fixed;
            top: 20px;
            right: 20px;
            padding: 15px 20px;
            border-radius: 4px;
            font-weight: 500;
            opacity: 0;
            transition: opacity 0.3s;
            z-index: 1000;
        }
        .message.show {
            opacity: 1;
        }
        .message.success {
            background: #2d5016;
            border: 1px solid #4a8028;
            color: #a3d977;
        }
        .message.error {
            background: #5a1a1a;
            border: 1px solid #902020;
            color: #ff9090;
        }
        .help-text {
            font-size: 0.85em;
            color: #808080;
            margin-top: 4px;
        }
        .section {
            background: #2a2a2a;
            border: 1px solid #404040;
            border-radius: 8px;
            padding: 20px;
            margin-bottom: 20px;
        }
        .section-header {
            color: #00aaff;
            font-size: 1.3em;
            font-weight: 600;
            margin-bottom: 20px;
            padding-bottom: 10px;
            border-bottom: 2px solid #404040;
        }
        .testing-grid {
            display: flex;
            justify-content: center;
            gap: 40px;
            align-items: center;
        }
        .testing-item {
            display: flex;
            align-items: center;
            gap: 10px;
        }
        .testing-item label {
            margin: 0;
            cursor: pointer;
        }
    </style>
</head>
<body>
    <div class="container">
        <h1>Bandwidth Meter Configuration</h1>
        <div id="config-container"></div>
    </div>
    <div id="message" class="message"></div>

    <script>
        const fieldSections = [
            {
                title: 'Testing',
                isTesting: true,
                help: 'Enable to simulate bandwidth utilization for testing purposes',
                fields: [
                    { name: 'test_tx', label: 'TX (Upload)', type: 'checkbox' },
                    { name: 'test_tx_percent', label: 'TX Utilization', type: 'range', min: '0', max: '100', step: '1' },
                    { name: 'test_rx', label: 'RX (Download)', type: 'checkbox' },
                    { name: 'test_rx_percent', label: 'RX Utilization', type: 'range', min: '0', max: '100', step: '1' },
                ]
            },
            {
                title: 'Networking',
                fields: [
                    { name: 'interface', label: 'Network Interface', type: 'text', help: 'Interface to monitor, e.g. en0. Multiple interfaces can be comma-separated: en0,en1 (requires restart)' },
                    { name: 'max_gbps', label: 'Max Bandwidth (Gbps)', type: 'number', step: '0.1', help: 'Maximum bandwidth in Gbps for visualization scaling' },
                    { name: 'wled_ip', label: 'WLED IP Address', type: 'text', help: 'WLED device IP or hostname (requires restart)' },
                    { name: 'httpd_ip', label: 'HTTP Server IP', type: 'text', help: 'IP address to listen on (0.0.0.0 for all)' },
                    { name: 'httpd_port', label: 'HTTP Server Port', type: 'number', step: '1', help: 'Port for HTTP server' },
                ]
            },
            {
                title: 'LED Layout',
                fields: [
                    { name: 'direction', label: 'Fill Direction', type: 'select', options: ['mirrored', 'opposing', 'left', 'right'], help: 'How LEDs fill across the strip' },
                    { name: 'swap', label: 'Swap TX/RX Halves', type: 'checkbox', help: 'Swap which half shows TX vs RX' },
                    { name: 'total_leds', label: 'Total LEDs', type: 'number', step: '1', help: 'Total number of LEDs in strip' },
                    { name: 'rx_split_percent', label: 'RX/TX LED Split', type: 'range', min: '0', max: '100', step: '1', help: 'Percentage of LEDs allocated to RX. TX gets the remainder. (50 = 50/50, 70 = 70/30)' },
                ]
            },
            {
                title: 'Color Settings',
                fields: [
                    { name: 'color', label: 'Default Color', type: 'textarea', help: 'Hex color or gradient (e.g., FF0000,00FF00,0000FF)' },
                    { name: 'tx_color', label: 'TX (Upload) Color', type: 'textarea', help: 'Overrides default color for TX. Leave empty to use default.' },
                    { name: 'rx_color', label: 'RX (Download) Color', type: 'textarea', help: 'Overrides default color for RX. Leave empty to use default.' },
                    { name: 'use_gradient', label: 'Use Gradient Blending', type: 'checkbox', help: 'Smooth gradients vs hard color segments' },
                    { name: 'interpolation', label: 'Gradient Interpolation', type: 'select', options: ['linear', 'basis', 'catmullrom'], help: 'Gradient interpolation algorithm' },
                ]
            },
            {
                title: 'Animation Settings',
                fields: [
                    { name: 'animation_speed', label: 'Animation Speed', type: 'number', step: '0.1', help: 'Speed of gradient animation (0 = disabled)' },
                    { name: 'scale_animation_speed', label: 'Scale Animation with Bandwidth', type: 'checkbox', help: 'Animation speed scales with bandwidth utilization' },
                    { name: 'tx_animation_direction', label: 'TX Animation Direction', type: 'radio', options: ['left', 'right'], help: 'Direction TX (upload) animation moves' },
                    { name: 'rx_animation_direction', label: 'RX Animation Direction', type: 'radio', options: ['left', 'right'], help: 'Direction RX (download) animation moves' },
                    { name: 'interpolation_time_ms', label: 'Interpolation Time (ms)', type: 'number', step: '10', help: 'Time in milliseconds to smoothly transition between bandwidth readings' },
                    { name: 'strobe_on_max', label: 'Strobe at Max Bandwidth', type: 'checkbox', help: 'Flash entire RX or TX segment when bandwidth exceeds maximum' },
                    { name: 'strobe_rate_hz', label: 'Strobe Rate (Hz)', type: 'number', step: '0.1', help: 'Strobe frequency in Hz (flashes per second). Default: 3.0 Hz' },
                    { name: 'strobe_duration_ms', label: 'Strobe Duration (ms)', type: 'number', step: '1', help: 'Duration of strobe effect in milliseconds. Cannot exceed cycle time (e.g., 3 Hz = 333ms max)' },
                    { name: 'strobe_color', label: 'Strobe Color (Hex)', type: 'text', help: 'Hex color to display during strobe "off" phase. Default: 000000 (black/off)' },
                    { name: 'fps', label: 'Frame Rate (FPS)', type: 'number', step: '1', help: 'Rendering frame rate. Try 30, 60, 120, or 144' },
                ]
            },
        ];

        let config = {};
        let pollingInterval = null;

        async function loadConfig() {
            try {
                const res = await fetch('/api/config');
                const newConfig = await res.json();

                // Check if config actually changed
                const configChanged = JSON.stringify(newConfig) !== JSON.stringify(config);

                config = newConfig;

                // Only re-render if config changed or this is the initial load
                if (configChanged || pollingInterval === null) {
                    renderConfig();

                    // Show notification if config was updated externally (not on initial load)
                    if (configChanged && pollingInterval !== null) {
                        showMessage('Config reloaded from file', 'success');
                    }
                }
            } catch (e) {
                showMessage('Failed to load configuration', 'error');
            }
        }

        function renderConfig() {
            const container = document.getElementById('config-container');
            container.innerHTML = fieldSections.map(section => {
                // Special handling for Testing section
                if (section.isTesting) {
                    const testingHTML = `
                        <div style="display: grid; grid-template-columns: 1fr 1fr; gap: 30px;">
                            <div style="text-align: center;">
                                <div style="margin-bottom: 15px;">
                                    <label style="display: inline-flex; align-items: center; gap: 8px; cursor: pointer; font-size: 1em;">
                                        <input type="checkbox" id="test_tx" onchange="saveField('test_tx', 'checkbox')" ${config.test_tx ? 'checked' : ''}>
                                        <span>TX (Upload)</span>
                                    </label>
                                </div>
                                <div>
                                    <label style="display: block; font-size: 0.85em; color: #808080; margin-bottom: 8px;">Utilization: <span id="test_tx_percent_value">${(config.test_tx_percent || 100).toFixed(0)}%</span></label>
                                    <input type="range" id="test_tx_percent" value="${config.test_tx_percent || 100}"
                                           min="0" max="100" step="1" style="width: 100%;"
                                           oninput="updateTestRangeValue('test_tx_percent')"
                                           onchange="saveField('test_tx_percent', 'range')">
                                </div>
                            </div>
                            <div style="text-align: center;">
                                <div style="margin-bottom: 15px;">
                                    <label style="display: inline-flex; align-items: center; gap: 8px; cursor: pointer; font-size: 1em;">
                                        <input type="checkbox" id="test_rx" onchange="saveField('test_rx', 'checkbox')" ${config.test_rx ? 'checked' : ''}>
                                        <span>RX (Download)</span>
                                    </label>
                                </div>
                                <div>
                                    <label style="display: block; font-size: 0.85em; color: #808080; margin-bottom: 8px;">Utilization: <span id="test_rx_percent_value">${(config.test_rx_percent || 100).toFixed(0)}%</span></label>
                                    <input type="range" id="test_rx_percent" value="${config.test_rx_percent || 100}"
                                           min="0" max="100" step="1" style="width: 100%;"
                                           oninput="updateTestRangeValue('test_rx_percent')"
                                           onchange="saveField('test_rx_percent', 'range')">
                                </div>
                            </div>
                        </div>
                    `;

                    return `
                        <div class="section">
                            <div class="section-header">${section.title}</div>
                            ${testingHTML}
                            ${section.help ? `<div class="help-text" style="text-align: center; margin-top: 12px;">${section.help}</div>` : ''}
                        </div>
                    `;
                }

                // Regular sections
                const fieldsHTML = section.fields.map(field => {
                    const value = config[field.name];
                    let inputHTML = '';
                    let saveButton = '';

                    if (field.type === 'checkbox') {
                        // Checkboxes auto-save on change, no Save button needed
                        inputHTML = `<input type="checkbox" id="${field.name}" onchange="saveField('${field.name}', '${field.type}')" ${value ? 'checked' : ''}>`;
                    } else if (field.type === 'range') {
                        // Range sliders show current value and auto-save on change
                        const rxValue = value !== undefined && value !== null ? value : 50;
                        const txSplit = (100 - rxValue).toFixed(0);
                        inputHTML = `
                            <input type="range" id="${field.name}" value="${rxValue}"
                                   min="${field.min || 0}" max="${field.max || 100}" step="${field.step || 1}"
                                   oninput="updateRangeValue('${field.name}')"
                                   onchange="saveField('${field.name}', '${field.type}')">
                            <div class="range-value" id="${field.name}_value">RX ${rxValue.toFixed(0)}% / TX ${txSplit}%</div>
                        `;
                    } else if (field.type === 'radio') {
                        // Radio buttons auto-save on change, no Save button needed
                        inputHTML = field.options.map(opt => `
                            <label style="display: inline-flex; align-items: center; gap: 5px; margin-right: 20px; cursor: pointer;">
                                <input type="radio" name="${field.name}" value="${opt}"
                                       onchange="saveField('${field.name}', '${field.type}')"
                                       ${value === opt ? 'checked' : ''}>
                                <span>${opt}</span>
                            </label>
                        `).join('');
                    } else if (field.type === 'select') {
                        inputHTML = `<select id="${field.name}">${field.options.map(opt =>
                            `<option value="${opt}" ${value === opt ? 'selected' : ''}>${opt}</option>`
                        ).join('')}</select>`;
                        saveButton = `<button onclick="saveField('${field.name}', '${field.type}')">Save</button>`;
                    } else if (field.type === 'textarea') {
                        inputHTML = `<textarea id="${field.name}" rows="2" style="resize: vertical; font-family: monospace; overflow: hidden;" oninput="autoResizeTextarea(this)">${value || ''}</textarea>`;
                        saveButton = `<button onclick="saveField('${field.name}', '${field.type}')">Save</button>`;
                    } else {
                        // Special handling for strobe_duration_ms validation
                        if (field.name === 'strobe_duration_ms') {
                            inputHTML = `<input type="${field.type}" id="${field.name}" value="${value || ''}" ${field.step ? `step="${field.step}"` : ''} oninput="validateStrobeDuration()">`;
                            saveButton = `<button id="save_${field.name}" onclick="saveField('${field.name}', '${field.type}')">Save</button>`;
                        } else {
                            inputHTML = `<input type="${field.type}" id="${field.name}" value="${value || ''}" ${field.step ? `step="${field.step}"` : ''}>`;
                            saveButton = `<button onclick="saveField('${field.name}', '${field.type}')">Save</button>`;
                        }
                    }

                    // Dynamic help text for strobe_duration_ms
                    let helpText = '';
                    if (field.help) {
                        if (field.name === 'strobe_duration_ms') {
                            const maxDuration = config.strobe_rate_hz > 0 ? (1000.0 / config.strobe_rate_hz).toFixed(1) : '1000.0';
                            helpText = `<div class="help-text" id="help_${field.name}">${field.help} Current max: ${maxDuration}ms</div>`;
                        } else {
                            helpText = `<div class="help-text">${field.help}</div>`;
                        }
                    }

                    return `
                        <div class="config-item">
                            <label for="${field.name}">${field.label}</label>
                            <div class="input-group">
                                ${inputHTML}
                                ${saveButton}
                            </div>
                            ${helpText}
                        </div>
                    `;
                }).join('');

                return `
                    <div class="section">
                        <div class="section-header">${section.title}</div>
                        <div class="config-grid">
                            ${fieldsHTML}
                        </div>
                    </div>
                `;
            }).join('');

            // After rendering, validate strobe duration and auto-size textareas
            setTimeout(() => {
                validateStrobeDuration();
                document.querySelectorAll('textarea').forEach(ta => autoResizeTextarea(ta));
            }, 0);
        }

        function autoResizeTextarea(textarea) {
            // Reset height to auto to get the correct scrollHeight
            textarea.style.height = 'auto';
            // Set height to scrollHeight to fit content
            textarea.style.height = textarea.scrollHeight + 'px';
        }

        function updateRangeValue(fieldName) {
            const input = document.getElementById(fieldName);
            const display = document.getElementById(fieldName + '_value');
            const rxValue = parseFloat(input.value);
            const txValue = 100 - rxValue;
            display.textContent = `RX ${rxValue.toFixed(0)}% / TX ${txValue.toFixed(0)}%`;
        }

        function updateTestRangeValue(fieldName) {
            const input = document.getElementById(fieldName);
            const display = document.getElementById(fieldName + '_value');
            const value = parseFloat(input.value);
            display.textContent = `${value.toFixed(0)}%`;
        }

        function validateStrobeDuration() {
            const durationInput = document.getElementById('strobe_duration_ms');
            const saveButton = document.getElementById('save_strobe_duration_ms');
            const helpText = document.getElementById('help_strobe_duration_ms');

            if (!durationInput || !saveButton) return;

            const duration = parseFloat(durationInput.value);
            const strobeRateHz = config.strobe_rate_hz || 3.0;
            const maxDuration = strobeRateHz > 0 ? (1000.0 / strobeRateHz) : 1000.0;

            if (duration > maxDuration || duration < 0 || isNaN(duration)) {
                // Invalid - highlight red and disable save
                durationInput.classList.add('invalid');
                saveButton.disabled = true;
                if (helpText) {
                    helpText.innerHTML = `Duration of strobe effect in milliseconds. Cannot exceed cycle time (e.g., 3 Hz = 333ms max) <span style="color: #ff4444; font-weight: bold;">Current max: ${maxDuration.toFixed(1)}ms - Value exceeds maximum!</span>`;
                }
            } else {
                // Valid - remove red and enable save
                durationInput.classList.remove('invalid');
                saveButton.disabled = false;
                if (helpText) {
                    helpText.innerHTML = `Duration of strobe effect in milliseconds. Cannot exceed cycle time (e.g., 3 Hz = 333ms max) Current max: ${maxDuration.toFixed(1)}ms`;
                }
            }
        }

        async function saveField(fieldName, fieldType) {
            let value;

            if (fieldType === 'checkbox') {
                const input = document.getElementById(fieldName);
                value = input.checked;
            } else if (fieldType === 'radio') {
                const selectedRadio = document.querySelector(`input[name="${fieldName}"]:checked`);
                value = selectedRadio ? selectedRadio.value : null;
            } else if (fieldType === 'range') {
                const input = document.getElementById(fieldName);
                value = parseFloat(input.value);
            } else if (fieldType === 'number') {
                const input = document.getElementById(fieldName);
                value = parseFloat(input.value);
            } else {
                const input = document.getElementById(fieldName);
                value = input.value;
            }

            try {
                const res = await fetch('/api/config', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ field: fieldName, value })
                });

                if (res.ok) {
                    showMessage(`${fieldName} updated successfully`, 'success');
                    config[fieldName] = value;

                    // If strobe_rate_hz changed, revalidate strobe_duration_ms
                    if (fieldName === 'strobe_rate_hz') {
                        validateStrobeDuration();
                    }
                } else {
                    showMessage(`Failed to update ${fieldName}`, 'error');
                }
            } catch (e) {
                showMessage(`Error updating ${fieldName}`, 'error');
            }
        }

        function showMessage(text, type) {
            const msg = document.getElementById('message');
            msg.textContent = text;
            msg.className = `message ${type} show`;
            setTimeout(() => msg.className = 'message', 3000);
        }

        // Initial load
        loadConfig();

        // Start polling for config changes every 2 seconds
        pollingInterval = setInterval(loadConfig, 2000);
    </script>
</body>
</html>
"#;

#[derive(Deserialize)]
struct UpdateField {
    field: String,
    value: serde_json::Value,
}

async fn serve_index() -> impl IntoResponse {
    Html(WEB_UI_HTML)
}

async fn get_config() -> impl IntoResponse {
    match BandwidthConfig::load() {
        Ok(config) => (StatusCode::OK, Json(config)).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn update_config(Json(payload): Json<UpdateField>) -> impl IntoResponse {
    let mut config = match BandwidthConfig::load() {
        Ok(c) => c,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let result = match payload.field.as_str() {
        "max_gbps" => payload.value.as_f64().map(|v| { config.max_gbps = v; }).ok_or("Invalid value"),
        "color" => payload.value.as_str().map(|v| { config.color = v.to_string(); }).ok_or("Invalid value"),
        "tx_color" => payload.value.as_str().map(|v| { config.tx_color = v.to_string(); }).ok_or("Invalid value"),
        "rx_color" => payload.value.as_str().map(|v| { config.rx_color = v.to_string(); }).ok_or("Invalid value"),
        "direction" => payload.value.as_str().map(|v| { config.direction = v.to_string(); }).ok_or("Invalid value"),
        "swap" => payload.value.as_bool().map(|v| { config.swap = v; }).ok_or("Invalid value"),
        "rx_split_percent" => payload.value.as_f64().map(|v| { config.rx_split_percent = v.clamp(0.0, 100.0); }).ok_or("Invalid value"),
        "strobe_on_max" => payload.value.as_bool().map(|v| { config.strobe_on_max = v; }).ok_or("Invalid value"),
        "strobe_rate_hz" => payload.value.as_f64().map(|v| {
            config.strobe_rate_hz = v;
            // Validate strobe_duration_ms doesn't exceed the cycle time
            if config.strobe_rate_hz > 0.0 {
                let max_duration = 1000.0 / config.strobe_rate_hz;
                config.strobe_duration_ms = config.strobe_duration_ms.min(max_duration);
            }
        }).ok_or("Invalid value"),
        "strobe_duration_ms" => payload.value.as_f64().map(|v| {
            // Clamp to valid range and ensure it doesn't exceed cycle time
            let max_duration = if config.strobe_rate_hz > 0.0 {
                1000.0 / config.strobe_rate_hz
            } else {
                1000.0
            };
            config.strobe_duration_ms = v.max(0.0).min(max_duration);
        }).ok_or("Invalid value"),
        "strobe_color" => payload.value.as_str().map(|v| { config.strobe_color = v.to_string(); }).ok_or("Invalid value"),
        "animation_speed" => payload.value.as_f64().map(|v| { config.animation_speed = v; }).ok_or("Invalid value"),
        "scale_animation_speed" => payload.value.as_bool().map(|v| { config.scale_animation_speed = v; }).ok_or("Invalid value"),
        "tx_animation_direction" => payload.value.as_str().map(|v| { config.tx_animation_direction = v.to_string(); }).ok_or("Invalid value"),
        "rx_animation_direction" => payload.value.as_str().map(|v| { config.rx_animation_direction = v.to_string(); }).ok_or("Invalid value"),
        "interpolation_time_ms" => payload.value.as_f64().map(|v| { config.interpolation_time_ms = v; }).ok_or("Invalid value"),
        "wled_ip" => payload.value.as_str().map(|v| { config.wled_ip = v.to_string(); }).ok_or("Invalid value"),
        "interface" => payload.value.as_str().map(|v| { config.interface = v.to_string(); }).ok_or("Invalid value"),
        "total_leds" => payload.value.as_u64().map(|v| { config.total_leds = v as usize; }).ok_or("Invalid value"),
        "use_gradient" => payload.value.as_bool().map(|v| { config.use_gradient = v; }).ok_or("Invalid value"),
        "interpolation" => payload.value.as_str().map(|v| { config.interpolation = v.to_string(); }).ok_or("Invalid value"),
        "fps" => payload.value.as_f64().map(|v| { config.fps = v; }).ok_or("Invalid value"),
        "httpd_enabled" => payload.value.as_bool().map(|v| { config.httpd_enabled = v; }).ok_or("Invalid value"),
        "httpd_ip" => payload.value.as_str().map(|v| { config.httpd_ip = v.to_string(); }).ok_or("Invalid value"),
        "httpd_port" => payload.value.as_u64().map(|v| { config.httpd_port = v as u16; }).ok_or("Invalid value"),
        "test_tx" => payload.value.as_bool().map(|v| { config.test_tx = v; }).ok_or("Invalid value"),
        "test_rx" => payload.value.as_bool().map(|v| { config.test_rx = v; }).ok_or("Invalid value"),
        "test_tx_percent" => payload.value.as_f64().map(|v| { config.test_tx_percent = v.clamp(0.0, 100.0); }).ok_or("Invalid value"),
        "test_rx_percent" => payload.value.as_f64().map(|v| { config.test_rx_percent = v.clamp(0.0, 100.0); }).ok_or("Invalid value"),
        _ => Err("Unknown field"),
    };

    if let Err(e) = result {
        return (StatusCode::BAD_REQUEST, e.to_string()).into_response();
    }

    match config.save() {
        Ok(_) => (StatusCode::OK, "Configuration updated").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn run_http_server(ip: String, port: u16) -> Result<()> {
    let app = Router::new()
        .route("/", get(serve_index))
        .route("/api/config", get(get_config))
        .route("/api/config", post(update_config));

    let addr = format!("{}:{}", ip, port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    
    axum::serve(listener, app).await?;
    Ok(())
}

// Get available network interfaces from the system
fn get_network_interfaces() -> Result<Vec<String>> {
    #[cfg(target_os = "macos")]
    {
        // On macOS, use ifconfig to list interfaces
        let output = StdCommand::new("ifconfig")
            .arg("-l")
            .output()?;

        let output_str = String::from_utf8_lossy(&output.stdout);
        let mut interfaces: Vec<String> = output_str
            .split_whitespace()
            .map(|s| s.to_string())
            .filter(|s| !s.starts_with("lo") && !s.starts_with("gif") && !s.starts_with("stf"))
            .collect();

        interfaces.sort();
        return Ok(interfaces);
    }

    #[cfg(target_os = "linux")]
    {
        // On Linux, read from /sys/class/net
        let mut interfaces = Vec::new();
        if let Ok(entries) = std::fs::read_dir("/sys/class/net") {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if !name.starts_with("lo") {
                    interfaces.push(name);
                }
            }
        }

        interfaces.sort();
        return Ok(interfaces);
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Ok(Vec::new())
    }
}

// Run interactive first-time setup
fn run_first_time_setup() -> Result<BandwidthConfig> {
    println!("\n=== WLED Bandwidth Meter - First Time Setup ===\n");

    // 1. Query and display network interfaces
    println!("Detecting network interfaces...\n");
    let interfaces = get_network_interfaces()?;

    if interfaces.is_empty() {
        eprintln!("Error: No network interfaces found!");
        std::process::exit(1);
    }

    println!("Available network interfaces:");
    for (i, iface) in interfaces.iter().enumerate() {
        println!("  {}. {}", i + 1, iface);
    }

    // Prompt for interface selection
    let interface = loop {
        print!("\nSelect interface (1-{}): ", interfaces.len());
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        if let Ok(choice) = input.trim().parse::<usize>() {
            if choice > 0 && choice <= interfaces.len() {
                break interfaces[choice - 1].clone();
            }
        }
        println!("Invalid selection. Please enter a number between 1 and {}", interfaces.len());
    };

    println!("Selected: {}\n", interface);

    // 2. Prompt for WLED IP
    print!("Enter WLED IP address or hostname (e.g., led.local or 192.168.1.100): ");
    io::stdout().flush()?;
    let mut wled_ip = String::new();
    io::stdin().read_line(&mut wled_ip)?;
    let wled_ip = wled_ip.trim().to_string();

    if wled_ip.is_empty() {
        eprintln!("Error: WLED IP address is required!");
        std::process::exit(1);
    }

    println!();

    // 3. Prompt for total LEDs
    let total_leds = loop {
        print!("Enter total number of LEDs in your strip: ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        if let Ok(leds) = input.trim().parse::<usize>() {
            if leds > 0 {
                break leds;
            }
        }
        println!("Invalid input. Please enter a positive number.");
    };

    println!();

    // 4. Prompt for max interface speed
    let max_gbps = loop {
        print!("Enter maximum interface speed in Gbps (e.g., 1.0, 2.5, 10.0): ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        if let Ok(speed) = input.trim().parse::<f64>() {
            if speed > 0.0 {
                break speed;
            }
        }
        println!("Invalid input. Please enter a positive number.");
    };

    println!("\n=== Configuration Summary ===");
    println!("Interface: {}", interface);
    println!("WLED IP: {}", wled_ip);
    println!("Total LEDs: {}", total_leds);
    println!("Max Speed: {} Gbps", max_gbps);
    println!("\nAll other settings will use default values.");
    println!("You can modify these later via the config file or web interface at http://localhost:8080\n");

    // Create config with provided values and defaults
    let mut config = BandwidthConfig::default();
    config.interface = interface;
    config.wled_ip = wled_ip;
    config.total_leds = total_leds;
    config.max_gbps = max_gbps;

    // Save the config
    config.save()?;
    println!("Configuration saved to: {}\n", BandwidthConfig::config_path()?.display());
    println!("Starting bandwidth meter...\n");

    // Give user a moment to read the summary
    thread::sleep(Duration::from_secs(2));

    Ok(config)
}


fn main() -> Result<()> {
    let args = Args::parse();

    if args.test.is_some() {
        // Test mode needs tokio runtime
        let rt = tokio::runtime::Runtime::new()?;
        return rt.block_on(test_mode(&args));
    }

    // Check for first-run scenario BEFORE setting up terminal
    // First-run: no config file exists AND no command-line args provided
    let config_path = BandwidthConfig::config_path()?;
    let config_file_exists = config_path.exists();

    // Check if any meaningful args were provided (excluding program name)
    let has_args = std::env::args().len() > 1;

    if !config_file_exists && !has_args {
        // First run - run interactive setup
        let _config = run_first_time_setup()?;
        // Config has been saved by run_first_time_setup, continue to normal startup
    }

    // Create tokio runtime for bandwidth reading task only - keep it alive for entire session
    let _rt = tokio::runtime::Runtime::new()?;

    // Load existing config or create default, then merge with command line args
    // Note: config_file_exists was already checked above for first-run detection
    let mut config = BandwidthConfig::load_or_default();
    let args_provided = config.merge_with_args(&args);

    // Only save if command-line args were provided OR if config file doesn't exist
    // This prevents overwriting existing config values on every launch
    // After first-run setup, config_file_exists will be true, so this only saves if args provided
    if args_provided || !config_file_exists {
        config.save()?;
    }

    // IMPORTANT: Establish SSH connection BEFORE setting up TUI
    // This allows SSH to prompt for password using normal stdin/stdout
    let quiet = args.quiet;

    println!("Connecting to bandwidth monitor...");
    if args.host.is_some() {
        println!("Please enter your SSH password when prompted...\n");
    }

    let child_result = _rt.block_on(spawn_bandwidth_monitor(&args, &config));
    let mut child = match child_result {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: Failed to start bandwidth monitor: {}", e);
            return Err(e);
        }
    };

    // For remote connections, wait for first line of output to ensure connection succeeded
    if args.host.is_some() {
        println!("Waiting for connection to establish...");

        let wait_result = _rt.block_on(async {
            if let Some(stdout) = child.stdout.take() {
                let mut reader = BufReader::new(stdout);
                let mut first_line = String::new();

                match reader.read_line(&mut first_line).await {
                    Ok(0) => {
                        Err(anyhow::anyhow!("SSH connection failed or closed immediately"))
                    }
                    Ok(_) => {
                        println!("Connection established!");
                        // Put stdout back for later use
                        child.stdout = Some(reader.into_inner());
                        Ok(())
                    }
                    Err(e) => {
                        Err(anyhow::anyhow!("Error reading from SSH: {}", e))
                    }
                }
            } else {
                Err(anyhow::anyhow!("No stdout available"))
            }
        });

        if let Err(e) = wait_result {
            eprintln!("Error: {}", e);
            eprintln!("Please check your SSH credentials and try again");
            return Err(e);
        }
    }

    println!("Connected successfully!\n");

    // Clear the terminal to remove password prompt residue
    print!("\x1B[2J\x1B[1;1H");
    io::stdout().flush()?;

    // NOW setup terminal - after SSH connection is established
    enable_raw_mode()?;
    let mut stdout_handle = io::stdout();
    stdout_handle.execute(EnterAlternateScreen)?;
    stdout_handle.flush()?;
    let backend = CrosstermBackend::new(stdout_handle);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;
    terminal.hide_cursor()?;

    // Setup panic handler to ensure terminal cleanup
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = disable_raw_mode();
        let _ = io::stdout().execute(LeaveAlternateScreen);
        let _ = io::stdout().execute(Show);
        original_hook(panic_info);
    }));

    // Create shared state for renderer
    let tx_color = if config.tx_color.is_empty() {
        config.color.clone()
    } else {
        config.tx_color.clone()
    };
    let rx_color = if config.rx_color.is_empty() {
        config.color.clone()
    } else {
        config.rx_color.clone()
    };

    let interpolation_mode = match config.interpolation.to_lowercase().as_str() {
        "basis" => InterpolationMode::Basis,
        "catmullrom" | "catmull-rom" => InterpolationMode::CatmullRom,
        _ => InterpolationMode::Linear,
    };

    let direction = match config.direction.to_lowercase().as_str() {
        "mirrored" => DirectionMode::Mirrored,
        "opposing" => DirectionMode::Opposing,
        "left" => DirectionMode::Left,
        "right" => DirectionMode::Right,
        _ => DirectionMode::Mirrored,
    };

    // Create shutdown flag for clean termination
    let shutdown = Arc::new(AtomicBool::new(false));

    let shared_state = Arc::new(Mutex::new(SharedRenderState {
        current_rx_kbps: 0.0,
        current_tx_kbps: 0.0,
        start_rx_kbps: 0.0,
        start_tx_kbps: 0.0,
        last_bandwidth_update: None,
        animation_speed: config.animation_speed,
        scale_animation_speed: config.scale_animation_speed,
        tx_animation_direction: config.tx_animation_direction.clone(),
        rx_animation_direction: config.rx_animation_direction.clone(),
        interpolation_time_ms: config.interpolation_time_ms,
        max_bandwidth_kbps: config.max_gbps * 1000.0 * 1000.0,
        tx_color,
        rx_color,
        use_gradient: config.use_gradient,
        interpolation_mode,
        direction,
        swap: config.swap,
        fps: config.fps,
        total_leds: config.total_leds,
        rx_split_percent: config.rx_split_percent,
        strobe_on_max: config.strobe_on_max,
        strobe_rate_hz: config.strobe_rate_hz,
        strobe_duration_ms: config.strobe_duration_ms,
        strobe_color: config.strobe_color.clone(),
        generation: 0,
    }));

    // Create DDP connection for renderer
    let dest_addr = format!("{}:4048", config.wled_ip);
    let socket = match UdpSocket::bind("0.0.0.0:4048") {
        Ok(s) => s,
        Err(e) => {
            terminal.show_cursor()?;
            disable_raw_mode()?;
            terminal.backend_mut().execute(LeaveAlternateScreen)?;
            return Err(e.into());
        }
    };

    let ddp_conn = match DDPConnection::try_new(&dest_addr, PixelConfig::default(), ID::Default, socket) {
        Ok(conn) => conn,
        Err(e) => {
            terminal.show_cursor()?;
            disable_raw_mode()?;
            terminal.backend_mut().execute(LeaveAlternateScreen)?;
            return Err(e.into());
        }
    };

    // Create renderer
    let renderer = match Renderer::new(ddp_conn, shared_state.clone(), shutdown.clone()) {
        Ok(r) => r,
        Err(e) => {
            terminal.show_cursor()?;
            disable_raw_mode()?;
            terminal.backend_mut().execute(LeaveAlternateScreen)?;
            return Err(e);
        }
    };

    // Spawn dedicated render thread - runs at 60 FPS independently
    thread::spawn(move || {
        renderer.run();
    });

    let (bandwidth_tx, bandwidth_rx) = mpsc::channel::<String>();
    let (config_tx, config_rx) = mpsc::channel::<BandwidthConfig>();

    // Message log stored locally
    let mut messages: Vec<String> = Vec::new();

    let leds_per_direction = config.total_leds / 2;

    // Helper function to calculate LEDs (same logic as renderer)
    let calculate_leds = |bandwidth_kbps: f64, max_bandwidth_kbps: f64| -> usize {
        let percentage = bandwidth_kbps / max_bandwidth_kbps;
        let leds = (percentage * leds_per_direction as f64) as usize;
        leds.min(leds_per_direction)
    };

    // Add initial message
    if !quiet {
        messages.push(format!(
            "[{}] Bandwidth meter started. Max: {} Gbps",
            get_timestamp(),
            config.max_gbps
        ));
        messages.push(format!(
            "[{}] Interface: {}, LEDs: {}, WLED: {}",
            get_timestamp(),
            config.interface, config.total_leds, config.wled_ip
        ));
        messages.push(format!("[{}] Config file: {}", get_timestamp(), config_path.display()));
        messages.push(format!("[{}] Edit config file to change settings while running", get_timestamp()));
        messages.push(format!("[{}] Debug log: /tmp/bandwidth_debug.log", get_timestamp()));
    }

    // Spawn bandwidth reader in separate tokio task
    let stdout = child.stdout.take().expect("Failed to capture stdout");
    _rt.spawn(async move {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();

        // Always create debug log file
        let mut debug_log = std::fs::File::create("/tmp/bandwidth_debug.log").ok();

        while let Ok(Some(line)) = lines.next_line().await {
            // Debug: write raw line with timestamp to file when received from SSH
            if let Some(ref mut log) = debug_log {
                use std::io::Write;
                let _ = writeln!(log, "[{}] SSH OUTPUT: {}", get_timestamp(), line);
                let _ = log.flush(); // Flush immediately so tail -f works
            }

            if bandwidth_tx.send(line).is_err() {
                break; // Main thread dropped receiver, time to exit
            }
        }
    });

    // Spawn config file watcher thread
    let config_path_clone = config_path.clone();
    std::thread::spawn(move || -> Result<()> {
        let (tx, rx) = mpsc::channel();
        let mut watcher = match RecommendedWatcher::new(tx, Config::default()) {
            Ok(w) => w,
            Err(_) => return Ok(()),
        };

        if watcher
            .watch(&config_path_clone, RecursiveMode::NonRecursive)
            .is_err()
        {
            return Ok(());
        }

        loop {
            match rx.recv() {
                Ok(Ok(NotifyEvent { kind, .. })) => {
                    // Only respond to modify events
                    if matches!(kind, notify::EventKind::Modify(_)) {
                        if let Ok(new_config) = BandwidthConfig::load() {
                            let _ = config_tx.send(new_config);
                        }
                    }
                }
                _ => {}
            }
        }
    });

    // Start HTTP server if enabled
    if config.httpd_enabled {
        let httpd_ip = config.httpd_ip.clone();
        let httpd_port = config.httpd_port;
        
        _rt.spawn(async move {
            if let Err(e) = run_http_server(httpd_ip.clone(), httpd_port).await {
                eprintln!("HTTP server error: {}", e);
            }
        });
    }

    // Force initial render
    {
        let status_line = if config.httpd_enabled {
            format!(
                "Edit {} to change settings | Web UI: http://{}:{} | Press Ctrl+C to quit",
                config_path.display(),
                config.httpd_ip,
                config.httpd_port
            )
        } else {
            format!(
                "Edit {} to change settings | Press Ctrl+C to quit",
                config_path.display()
            )
        };

        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(1), Constraint::Length(3)].as_ref())
                .split(f.size());

            let messages_text: Vec<Line> = messages
                .iter()
                .rev()
                .take(chunks[0].height as usize)
                .rev()
                .map(|m| Line::from(m.as_str()))
                .collect();

            let messages_widget = Paragraph::new(messages_text).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Bandwidth Monitor"),
            );
            f.render_widget(messages_widget, chunks[0]);

            let status = Paragraph::new(status_line)
                .block(Block::default().borders(Borders::ALL).title("Status"));
            f.render_widget(status, chunks[1]);
        })?;
    }

    let mut needs_render = true;

    // Initialize bandwidth tracker for Linux /proc/net/dev parsing
    let mut bandwidth_tracker: Option<BandwidthTracker> = Some(BandwidthTracker::new());

    // Initialize test mode bandwidth values if enabled
    if config.test_tx || config.test_rx {
        let mut state = shared_state.lock().unwrap();
        if config.test_rx {
            let test_rx_kbps = config.max_gbps * 1000.0 * 1000.0 * (config.test_rx_percent / 100.0);
            state.current_rx_kbps = test_rx_kbps;
            state.start_rx_kbps = test_rx_kbps;
            state.last_bandwidth_update = Some(Instant::now());
        }
        if config.test_tx {
            let test_tx_kbps = config.max_gbps * 1000.0 * 1000.0 * (config.test_tx_percent / 100.0);
            state.current_tx_kbps = test_tx_kbps;
            state.start_tx_kbps = test_tx_kbps;
            state.last_bandwidth_update = Some(Instant::now());
        }
    }

    // Simple main loop - just handle bandwidth and config updates
    // Rendering happens in dedicated thread at configurable FPS
    loop {
        // Check for Ctrl+C key press (using crossterm events since we're in raw mode)
        // Use 50ms timeout to make Ctrl+C more responsive
        if poll(std::time::Duration::from_millis(50))? {
            if let Event::Key(KeyEvent {
                code: KeyCode::Char('c'),
                modifiers: KeyModifiers::CONTROL,
                ..
            }) = read()?
            {
                // Signal render thread to shut down
                shutdown.store(true, Ordering::Relaxed);

                // Give render thread a moment to exit cleanly
                thread::sleep(Duration::from_millis(100));

                // Clean up terminal
                terminal.show_cursor()?;
                disable_raw_mode()?;
                terminal.backend_mut().execute(LeaveAlternateScreen)?;
                break;
            }
        }

        // Check bandwidth updates - update shared state
        match bandwidth_rx.try_recv() {
            Ok(line) => {
                if let Some((rx_kbps, tx_kbps)) = parse_bandwidth_line(&line, &mut bandwidth_tracker) {
                    // Override with test values if test mode is enabled for each direction
                    let rx_kbps = if config.test_rx {
                        config.max_gbps * 1000.0 * 1000.0 * (config.test_rx_percent / 100.0)
                    } else {
                        rx_kbps
                    };

                    let tx_kbps = if config.test_tx {
                        config.max_gbps * 1000.0 * 1000.0 * (config.test_tx_percent / 100.0)
                    } else {
                        tx_kbps
                    };

                    // Update shared state (non-blocking for renderer)
                    {
                        let mut state = shared_state.lock().unwrap();
                        // Store current values as the starting point for interpolation
                        state.start_rx_kbps = state.current_rx_kbps;
                        state.start_tx_kbps = state.current_tx_kbps;
                        // Update to new target values
                        state.current_rx_kbps = rx_kbps;
                        state.current_tx_kbps = tx_kbps;
                        // Record the time when this update happened
                        state.last_bandwidth_update = Some(Instant::now());
                    }

                    // Generate messages for UI
                    let rx_leds = calculate_leds(rx_kbps, config.max_gbps * 1000.0 * 1000.0);
                    let tx_leds = calculate_leds(tx_kbps, config.max_gbps * 1000.0 * 1000.0);

                    // Always show both RX and TX on every update
                    if !quiet {
                        messages.push(format!(
                            "[{}] RX: {} LEDs ({:.1} Mbps) | TX: {} LEDs ({:.1} Mbps)",
                            get_timestamp(),
                            rx_leds,
                            rx_kbps / 1000.0,
                            tx_leds,
                            tx_kbps / 1000.0
                        ));
                        needs_render = true;
                    }

                    // Keep message buffer reasonable
                    if messages.len() > 1000 {
                        messages.remove(0);
                    }
                }
            }
            Err(_) => {
                // No new bandwidth data
            }
        }

        // Check config file updates
        match config_rx.try_recv() {
            Ok(new_config) => {
                // Update shared state with new config
                {
                    let mut state = shared_state.lock().unwrap();

                    // Handle color updates
                    let color_changed = new_config.color != config.color;
                    let tx_color_changed = new_config.tx_color != config.tx_color;
                    let rx_color_changed = new_config.rx_color != config.rx_color;

                    if tx_color_changed || (color_changed && new_config.tx_color.is_empty()) {
                        let tx_color_to_use = if new_config.tx_color.is_empty() {
                            new_config.color.clone()
                        } else {
                            new_config.tx_color.clone()
                        };
                        state.tx_color = tx_color_to_use.clone();
                        state.generation += 1;
                        if !quiet {
                            if new_config.tx_color.is_empty() {
                                messages.push(format!(
                                    "[{}] TX color updated to: {} (from main color)",
                                    get_timestamp(),
                                    tx_color_to_use
                                ));
                            } else {
                                messages.push(format!("[{}] TX color updated to: {}", get_timestamp(), new_config.tx_color));
                            }
                        }
                    }

                    if rx_color_changed || (color_changed && new_config.rx_color.is_empty()) {
                        let rx_color_to_use = if new_config.rx_color.is_empty() {
                            new_config.color.clone()
                        } else {
                            new_config.rx_color.clone()
                        };
                        state.rx_color = rx_color_to_use.clone();
                        state.generation += 1;
                        if !quiet {
                            if new_config.rx_color.is_empty() {
                                messages.push(format!(
                                    "[{}] RX color updated to: {} (from main color)",
                                    get_timestamp(),
                                    rx_color_to_use
                                ));
                            } else {
                                messages.push(format!("[{}] RX color updated to: {}", get_timestamp(), new_config.rx_color));
                            }
                        }
                    }

                    // Update max bandwidth
                    if new_config.max_gbps != config.max_gbps {
                        state.max_bandwidth_kbps = new_config.max_gbps * 1000.0 * 1000.0;
                        if !quiet {
                            messages.push(format!(
                                "[{}] Max bandwidth updated to: {} Gbps",
                                get_timestamp(),
                                new_config.max_gbps
                            ));
                        }
                    }

                    // Update direction
                    if new_config.direction != config.direction {
                        let direction = match new_config.direction.to_lowercase().as_str() {
                            "mirrored" => DirectionMode::Mirrored,
                            "opposing" => DirectionMode::Opposing,
                            "left" => DirectionMode::Left,
                            "right" => DirectionMode::Right,
                            _ => DirectionMode::Mirrored,
                        };
                        state.direction = direction;
                        state.generation += 1;
                        if !quiet {
                            messages.push(format!("[{}] Direction updated to: {}", get_timestamp(), new_config.direction));
                        }
                    }

                    // Update swap
                    if new_config.swap != config.swap {
                        state.swap = new_config.swap;
                        state.generation += 1;
                        if !quiet {
                            messages.push(format!(
                                "[{}] Swap: {}",
                                get_timestamp(),
                                if new_config.swap { "enabled" } else { "disabled" }
                            ));
                        }
                    }

                    // Update RX/TX split percentage
                    if new_config.rx_split_percent != config.rx_split_percent {
                        state.rx_split_percent = new_config.rx_split_percent;
                        if !quiet {
                            let tx_split = 100.0 - new_config.rx_split_percent;
                            messages.push(format!(
                                "[{}] LED split updated to: RX {:.0}% / TX {:.0}%",
                                get_timestamp(),
                                new_config.rx_split_percent,
                                tx_split
                            ));
                        }
                    }

                    // Update strobe on max
                    if new_config.strobe_on_max != config.strobe_on_max {
                        state.strobe_on_max = new_config.strobe_on_max;
                        if !quiet {
                            messages.push(format!(
                                "[{}] Strobe on max: {}",
                                get_timestamp(),
                                if new_config.strobe_on_max { "enabled" } else { "disabled" }
                            ));
                        }
                    }

                    // Update strobe rate
                    if new_config.strobe_rate_hz != config.strobe_rate_hz {
                        state.strobe_rate_hz = new_config.strobe_rate_hz;
                        // Also validate strobe_duration_ms doesn't exceed new cycle time
                        if new_config.strobe_rate_hz > 0.0 {
                            let max_duration = 1000.0 / new_config.strobe_rate_hz;
                            if state.strobe_duration_ms > max_duration {
                                state.strobe_duration_ms = max_duration;
                            }
                        }
                        if !quiet {
                            messages.push(format!(
                                "[{}] Strobe rate updated to: {:.1} Hz",
                                get_timestamp(),
                                new_config.strobe_rate_hz
                            ));
                        }
                    }

                    // Update strobe duration
                    if new_config.strobe_duration_ms != config.strobe_duration_ms {
                        state.strobe_duration_ms = new_config.strobe_duration_ms;
                        if !quiet {
                            messages.push(format!(
                                "[{}] Strobe duration updated to: {:.0} ms",
                                get_timestamp(),
                                new_config.strobe_duration_ms
                            ));
                        }
                    }

                    // Update strobe color
                    if new_config.strobe_color != config.strobe_color {
                        state.strobe_color = new_config.strobe_color.clone();
                        if !quiet {
                            messages.push(format!(
                                "[{}] Strobe color updated to: {}",
                                get_timestamp(),
                                new_config.strobe_color
                            ));
                        }
                    }

                    // Update animation speed
                    if new_config.animation_speed != config.animation_speed {
                        state.animation_speed = new_config.animation_speed;
                        if !quiet && new_config.animation_speed > 0.0 {
                            messages.push(format!(
                                "[{}] Animation speed: {:.3}",
                                get_timestamp(),
                                new_config.animation_speed
                            ));
                        }
                    }

                    // Update animation speed scaling
                    if new_config.scale_animation_speed != config.scale_animation_speed {
                        state.scale_animation_speed = new_config.scale_animation_speed;
                        if !quiet {
                            messages.push(format!(
                                "[{}] Animation speed scaling: {}",
                                get_timestamp(),
                                if new_config.scale_animation_speed {
                                    "enabled (scales with bandwidth)"
                                } else {
                                    "disabled (constant speed)"
                                }
                            ));
                        }
                    }

                    // Update TX animation direction
                    if new_config.tx_animation_direction != config.tx_animation_direction {
                        state.tx_animation_direction = new_config.tx_animation_direction.clone();
                        if !quiet {
                            messages.push(format!(
                                "[{}] TX animation direction: {}",
                                get_timestamp(),
                                new_config.tx_animation_direction
                            ));
                        }
                    }

                    // Update RX animation direction
                    if new_config.rx_animation_direction != config.rx_animation_direction {
                        state.rx_animation_direction = new_config.rx_animation_direction.clone();
                        if !quiet {
                            messages.push(format!(
                                "[{}] RX animation direction: {}",
                                get_timestamp(),
                                new_config.rx_animation_direction
                            ));
                        }
                    }

                    // Update interpolation time
                    if new_config.interpolation_time_ms != config.interpolation_time_ms {
                        state.interpolation_time_ms = new_config.interpolation_time_ms;
                        if !quiet {
                            messages.push(format!(
                                "[{}] Interpolation time: {} ms",
                                get_timestamp(),
                                new_config.interpolation_time_ms
                            ));
                        }
                    }

                    // Update interpolation
                    if new_config.interpolation != config.interpolation {
                        let interpolation_mode = match new_config.interpolation.to_lowercase().as_str() {
                            "basis" => InterpolationMode::Basis,
                            "catmullrom" | "catmull-rom" => InterpolationMode::CatmullRom,
                            _ => InterpolationMode::Linear,
                        };
                        state.interpolation_mode = interpolation_mode;
                        state.generation += 1;
                        if !quiet {
                            messages.push(format!(
                                "[{}] Interpolation updated to: {}",
                                get_timestamp(),
                                new_config.interpolation
                            ));
                        }
                    }

                    // Update gradient mode
                    if new_config.use_gradient != config.use_gradient {
                        state.use_gradient = new_config.use_gradient;
                        state.generation += 1;
                        if !quiet {
                            messages.push(format!(
                                "[{}] Gradient mode: {}",
                                get_timestamp(),
                                if new_config.use_gradient {
                                    "enabled (smooth gradients)"
                                } else {
                                    "disabled (hard segments)"
                                }
                            ));
                        }
                    }

                    // Update FPS
                    if new_config.fps != config.fps {
                        state.fps = new_config.fps;
                        if !quiet {
                            messages.push(format!("[{}] FPS updated to: {}", get_timestamp(), new_config.fps));
                        }
                    }

                    // Update total_leds
                    if new_config.total_leds != config.total_leds {
                        state.total_leds = new_config.total_leds;
                        if !quiet {
                            messages.push(format!("[{}] Total LEDs updated to: {}", get_timestamp(), new_config.total_leds));
                        }
                    }
                }

                // Note: Changes to wled_ip and interface require restart
                if new_config.wled_ip != config.wled_ip
                    || new_config.interface != config.interface
                {
                    if !quiet {
                        messages.push(format!("[{}] Note: wled_ip or interface changed - restart required", get_timestamp()));
                    }
                }

                // Update test mode - immediately update bandwidth values and tracking vars
                if new_config.test_tx != config.test_tx
                    || new_config.test_rx != config.test_rx
                    || new_config.test_tx_percent != config.test_tx_percent
                    || new_config.test_rx_percent != config.test_rx_percent {

                    // Calculate test bandwidth values
                    let test_rx_kbps = if new_config.test_rx {
                        new_config.max_gbps * 1000.0 * 1000.0 * (new_config.test_rx_percent / 100.0)
                    } else {
                        0.0
                    };

                    let test_tx_kbps = if new_config.test_tx {
                        new_config.max_gbps * 1000.0 * 1000.0 * (new_config.test_tx_percent / 100.0)
                    } else {
                        0.0
                    };

                    // Update shared state only if test mode is enabled
                    let mut state = shared_state.lock().unwrap();

                    if new_config.test_rx {
                        state.start_rx_kbps = state.current_rx_kbps;
                        state.current_rx_kbps = test_rx_kbps;
                        state.last_bandwidth_update = Some(Instant::now());
                    }

                    if new_config.test_tx {
                        state.start_tx_kbps = state.current_tx_kbps;
                        state.current_tx_kbps = test_tx_kbps;
                        state.last_bandwidth_update = Some(Instant::now());
                    }

                    drop(state);

                    if !quiet {
                        if new_config.test_tx != config.test_tx {
                            messages.push(format!(
                                "[{}] Test TX: {}",
                                get_timestamp(),
                                if new_config.test_tx { "enabled" } else { "disabled" }
                            ));
                        }
                        if new_config.test_rx != config.test_rx {
                            messages.push(format!(
                                "[{}] Test RX: {}",
                                get_timestamp(),
                                if new_config.test_rx { "enabled" } else { "disabled" }
                            ));
                        }
                        if new_config.test_tx_percent != config.test_tx_percent && new_config.test_tx {
                            messages.push(format!(
                                "[{}] Test TX utilization: {:.0}%",
                                get_timestamp(),
                                new_config.test_tx_percent
                            ));
                        }
                        if new_config.test_rx_percent != config.test_rx_percent && new_config.test_rx {
                            messages.push(format!(
                                "[{}] Test RX utilization: {:.0}%",
                                get_timestamp(),
                                new_config.test_rx_percent
                            ));
                        }
                    }
                }

                // Update config for future comparisons
                config = new_config;

                needs_render = true;
            }
            Err(_) => {
                // No config update
            }
        }

        // Render only when something changed
        if needs_render {
            let status_text = if config.httpd_enabled {
                format!(
                    "Edit {} to change settings | Web UI: http://{}:{} | Press Ctrl+C to quit",
                    config_path.display(),
                    config.httpd_ip,
                    config.httpd_port
                )
            } else {
                format!(
                    "Edit {} to change settings | Press Ctrl+C to quit",
                    config_path.display()
                )
            };

            terminal.draw(|f| {
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Min(1), Constraint::Length(3)].as_ref())
                    .split(f.size());

                // Messages area
                let messages_text: Vec<Line> = messages
                    .iter()
                    .rev()
                    .take(chunks[0].height as usize)
                    .rev()
                    .map(|m| Line::from(m.as_str()))
                    .collect();

                let messages_widget = Paragraph::new(messages_text).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("Bandwidth Monitor"),
                );
                f.render_widget(messages_widget, chunks[0]);

                // Status/Input area
                let status = Paragraph::new(status_text.clone())
                    .block(Block::default().borders(Borders::ALL).title("Status"));
                f.render_widget(status, chunks[1]);
            })?;

            needs_render = false;
        }

        // Small sleep to avoid busy-waiting CPU at 100%
        // Renderer runs in separate thread, so main loop can sleep longer
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    Ok(())
}
