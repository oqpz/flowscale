//! Clean-room `config.ini` parser and writer for `[rendering]`, `[capture]`, `[lsfg]`, and `[fps_counter]`.

use serde::{Deserialize, Serialize};
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

pub const MAX_FRAMETIME_BUFFER_SIZE: usize = 1024;

/// `[rendering]` section configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RenderingConfig {
    pub flush: bool,
}

impl Default for RenderingConfig {
    fn default() -> Self {
        Self { flush: true }
    }
}

/// `[capture]` section configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureConfig {
    pub frametime_buffer_size: usize,
    pub frametime_buffer_reset_multiplier: f64,
    pub queue_draining_momentum: f64,
    pub dedup: bool,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            frametime_buffer_size: 15,
            frametime_buffer_reset_multiplier: 6.0,
            queue_draining_momentum: 0.01,
            dedup: false,
        }
    }
}

/// `[lsfg]` section configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LsfgConfig {
    /// Per-profile processing choice, populated by EngineRuntime::apply_game_profile.
    #[serde(skip)]
    pub cpu_vulkan: bool,
    pub ui_detection_rate: u32,
    pub ui_detection_threshold: f64,
    pub ui_detection_debug: bool,
    pub real_timestamp_tolerance: f64,
    pub base_framerate_threshold: f64,
    pub iteration_delta: Option<f64>,
    pub flow_scale: Option<f64>,
    pub multiplier: Option<f64>,
}

impl Default for LsfgConfig {
    fn default() -> Self {
        Self {
            cpu_vulkan: false,
            ui_detection_rate: 1,
            ui_detection_threshold: 0.5,
            ui_detection_debug: false,
            real_timestamp_tolerance: 0.05,
            base_framerate_threshold: 10.0,
            iteration_delta: None,
            flow_scale: None,
            multiplier: None,
        }
    }
}

/// `[fps_counter]` section configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FpsCounterConfig {
    pub scale: f32,
    pub color: u32,
    pub font: String,
    pub offset_x: i32,
    pub offset_y: i32,
    pub show_captured: bool,
}

impl Default for FpsCounterConfig {
    fn default() -> Self {
        Self {
            scale: 1.0,
            color: 0x00D3_D3D3,
            font: "Consolas".to_owned(),
            offset_x: 0,
            offset_y: 0,
            show_captured: true,
        }
    }
}

/// Complete `config.ini` configuration.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EngineConfig {
    pub rendering: RenderingConfig,
    pub capture: CaptureConfig,
    pub lsfg: LsfgConfig,
    pub fps_counter: FpsCounterConfig,
}

impl EngineConfig {
    /// Loads configuration from a file, falling back to defaults for missing keys.
    pub fn load_from_file(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let text = fs::read_to_string(path)?;
        Ok(Self::parse(&text))
    }

    /// Saves configuration to a file in INI format.
    pub fn save_to_file(&self, path: impl AsRef<Path>) -> std::io::Result<()> {
        crate::persistence::write_atomic(path.as_ref(), self.to_ini_string().as_bytes())
    }

    /// Parses an INI string into [`EngineConfig`].
    pub fn parse(input: &str) -> Self {
        let mut cfg = Self::default();
        let mut section = String::new();

        for raw_line in input.lines() {
            let line = raw_line.split(';').next().unwrap_or("").trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                section = line[1..line.len() - 1].trim().to_ascii_lowercase();
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let key = k.trim().to_ascii_lowercase();
            let val = v.trim();

            match section.as_str() {
                "rendering" => {
                    if key.as_str() == "flush" {
                        if let Some(b) = parse_bool(val) {
                            cfg.rendering.flush = b;
                        }
                    }
                }
                "capture" => match key.as_str() {
                    "frametime_buffer_size" => {
                        if let Ok(n) = val.parse::<usize>() {
                            cfg.capture.frametime_buffer_size =
                                n.clamp(1, MAX_FRAMETIME_BUFFER_SIZE);
                        }
                    }
                    "frametime_buffer_reset_multiplier" => {
                        if let Ok(f) = val.parse::<f64>() {
                            if f.is_finite() {
                                cfg.capture.frametime_buffer_reset_multiplier = f.max(1.1);
                            }
                        }
                    }
                    "queue_draining_momentum" => {
                        if let Ok(f) = val.parse::<f64>() {
                            if f.is_finite() {
                                cfg.capture.queue_draining_momentum = f.clamp(0.0, 1.0);
                            }
                        }
                    }
                    "dedup" => {
                        if let Some(b) = parse_bool(val) {
                            cfg.capture.dedup = b;
                        }
                    }
                    _ => {}
                },
                "lsfg" => match key.as_str() {
                    "ui_detection_rate" => {
                        if let Ok(n) = val.parse::<u32>() {
                            cfg.lsfg.ui_detection_rate = n;
                        }
                    }
                    "ui_detection_threshold" => {
                        if let Ok(f) = val.parse::<f64>() {
                            if f.is_finite() {
                                cfg.lsfg.ui_detection_threshold = f.clamp(0.0, 1.0);
                            }
                        }
                    }
                    "ui_detection_debug" => {
                        if let Some(b) = parse_bool(val) {
                            cfg.lsfg.ui_detection_debug = b;
                        }
                    }
                    "real_timestamp_tolerance" => {
                        if let Ok(f) = val.parse::<f64>() {
                            if f.is_finite() {
                                cfg.lsfg.real_timestamp_tolerance = f.max(0.0);
                            }
                        }
                    }
                    "base_framerate_threshold" => {
                        if let Ok(f) = val.parse::<f64>() {
                            if f.is_finite() {
                                cfg.lsfg.base_framerate_threshold = f.max(0.0);
                            }
                        }
                    }
                    "iteration_delta" => {
                        if let Ok(f) = val.parse::<f64>() {
                            if f.is_finite() {
                                cfg.lsfg.iteration_delta = Some(f);
                            }
                        }
                    }
                    "flow_scale" => {
                        if let Ok(f) = val.parse::<f64>() {
                            if f.is_finite() && f > 0.0 {
                                cfg.lsfg.flow_scale = Some(f);
                            }
                        }
                    }
                    "multiplier" => {
                        if let Ok(f) = val.parse::<f64>() {
                            if f.is_finite() && f >= 1.0 {
                                cfg.lsfg.multiplier = Some(f);
                            }
                        }
                    }
                    _ => {}
                },
                "fps_counter" => match key.as_str() {
                    "scale" => {
                        if let Ok(f) = val.parse::<f32>() {
                            if f.is_finite() {
                                cfg.fps_counter.scale = f.clamp(0.1, 5.0);
                            }
                        }
                    }
                    "color" => {
                        if let Some(c) = parse_hex_color(val) {
                            cfg.fps_counter.color = c;
                        }
                    }
                    "font" => {
                        if !val.is_empty() {
                            cfg.fps_counter.font = val.to_owned();
                        }
                    }
                    "offset_x" => {
                        if let Ok(n) = val.parse::<i32>() {
                            cfg.fps_counter.offset_x = n;
                        }
                    }
                    "offset_y" => {
                        if let Ok(n) = val.parse::<i32>() {
                            cfg.fps_counter.offset_y = n;
                        }
                    }
                    "show_captured" => {
                        if let Some(b) = parse_bool(val) {
                            cfg.fps_counter.show_captured = b;
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        }

        cfg
    }

    pub fn parse_checked(input: &str) -> Result<Self, String> {
        let mut recognized_section = false;
        let mut has_content = false;
        for (index, raw_line) in input.lines().enumerate() {
            let line = raw_line.split(';').next().unwrap_or("").trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            has_content = true;
            if line.starts_with('[') {
                if !line.ends_with(']') {
                    return Err(format!(
                        "malformed config.ini section on line {}",
                        index + 1
                    ));
                }
                let section = line[1..line.len() - 1].trim().to_ascii_lowercase();
                recognized_section |= matches!(
                    section.as_str(),
                    "rendering" | "capture" | "lsfg" | "fps_counter"
                );
            } else if !line.contains('=') {
                return Err(format!("malformed config.ini entry on line {}", index + 1));
            }
        }
        if has_content && !recognized_section {
            return Err("config.ini has no recognized FlowScale section".to_owned());
        }
        Ok(Self::parse(input))
    }

    /// Serializes [`EngineConfig`] into canonical `config.ini` text.
    pub fn to_ini_string(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "[rendering]");
        let _ = writeln!(out, "flush = {}", i32::from(self.rendering.flush));
        let _ = writeln!(out);
        let _ = writeln!(out, "[capture]");
        let _ = writeln!(
            out,
            "frametime_buffer_size = {}",
            self.capture.frametime_buffer_size
        );
        let _ = writeln!(
            out,
            "frametime_buffer_reset_multiplier = {}",
            self.capture.frametime_buffer_reset_multiplier
        );
        let _ = writeln!(
            out,
            "queue_draining_momentum = {}",
            self.capture.queue_draining_momentum
        );
        if self.capture.dedup {
            let _ = writeln!(out, "dedup = 1");
        }
        let _ = writeln!(out);
        let _ = writeln!(out, "[lsfg]");
        let _ = writeln!(out, "ui_detection_rate = {}", self.lsfg.ui_detection_rate);
        let _ = writeln!(
            out,
            "ui_detection_threshold = {}",
            self.lsfg.ui_detection_threshold
        );
        let _ = writeln!(
            out,
            "real_timestamp_tolerance = {}",
            self.lsfg.real_timestamp_tolerance
        );
        let _ = writeln!(
            out,
            "base_framerate_threshold = {}",
            self.lsfg.base_framerate_threshold
        );
        if self.lsfg.ui_detection_debug {
            let _ = writeln!(out, "ui_detection_debug = 1");
        }
        if let Some(v) = self.lsfg.iteration_delta {
            let _ = writeln!(out, "iteration_delta = {v}");
        }
        if let Some(v) = self.lsfg.flow_scale {
            let _ = writeln!(out, "flow_scale = {v}");
        }
        if let Some(v) = self.lsfg.multiplier {
            let _ = writeln!(out, "multiplier = {v}");
        }
        let _ = writeln!(out);
        let _ = writeln!(out, "[fps_counter]");
        let _ = writeln!(out, "scale = {}", self.fps_counter.scale);
        let _ = writeln!(
            out,
            "color = 0x{:06X}",
            self.fps_counter.color & 0x00FF_FFFF
        );
        let _ = writeln!(out, "font = {}", self.fps_counter.font);
        let _ = writeln!(out, "offset_x = {}", self.fps_counter.offset_x);
        let _ = writeln!(out, "offset_y = {}", self.fps_counter.offset_y);
        let _ = writeln!(
            out,
            "show_captured = {}",
            i32::from(self.fps_counter.show_captured)
        );
        out
    }
}

fn parse_bool(val: &str) -> Option<bool> {
    match val.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn parse_hex_color(val: &str) -> Option<u32> {
    let trimmed = val
        .trim()
        .strip_prefix("0x")
        .or_else(|| val.trim().strip_prefix("0X"))
        .or_else(|| val.trim().strip_prefix('#'))
        .unwrap_or(val.trim());
    u32::from_str_radix(trimmed, 16).ok()
}
