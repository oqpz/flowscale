//! Application settings, game profile matching, Lossless Scaling XML import, and Windows startup helpers.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use flowscale_abi::{
    Anime4kSubtype, CaptureApi, FrameGenerationSize, FrameGenerationType, FsrSubtype, Ls1Subtype,
    Lsfg2Mode, Lsfg3Mode, ScalingFitMode, ScalingMode, ScalingProfile, ScalingType, SyncMode,
};
use serde::{Deserialize, Serialize};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ,
};

const STARTUP_RUN_SUBKEY: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");

/// UI theme selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

/// Global toggle hotkey configuration (defaults to `Ctrl+Alt+S`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HotkeyConfig {
    pub key: String,
    pub vk: u32,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            key: "S".to_owned(),
            vk: 0x53,
            ctrl: true,
            alt: true,
            shift: false,
            win: false,
        }
    }
}

/// Per-application / default Lossless Scaling profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GameProfile {
    pub id: String,
    pub title: String,
    pub path: String,
    pub auto_scale: bool,
    pub auto_scale_delay: i32,
    pub windowed_mode: bool,
    pub scaling_mode: ScalingMode,
    pub scaling_fit_mode: ScalingFitMode,
    pub scaling_type: ScalingType,
    pub fsr_type: FsrSubtype,
    pub ls1_type: Ls1Subtype,
    pub anime4k_type: Anime4kSubtype,
    pub scale_factor: f32,
    pub resize_before_scaling: bool,
    pub sharpness: i32,
    pub ls1_sharpness: i32,
    pub vrs: bool,
    pub frame_generation: FrameGenerationType,
    pub cpu_vulkan: bool,
    pub lsfg2_mode: Lsfg2Mode,
    pub lsfg3_mode: Lsfg3Mode,
    pub lsfg3_multiplier: f32,
    pub lsfg3_target: f32,
    pub lsfg_flow_scale: i32,
    pub lsfg_size: FrameGenerationSize,
    pub clip_cursor: bool,
    pub adjust_cursor_speed: bool,
    pub hide_cursor: bool,
    pub scale_cursor: bool,
    pub sync_mode: SyncMode,
    pub max_frame_latency: i32,
    pub gsync_support: bool,
    pub hdr_support: bool,
    pub draw_fps: bool,
    pub capture_api: CaptureApi,
    pub queue_target: i32,
    pub preferred_gpu_id: i32,
    pub output_display_id: i32,
    pub crop_input: bool,
    pub crop_left: i32,
    pub crop_top: i32,
    pub crop_right: i32,
    pub crop_bottom: i32,
    pub multi_display_mode: bool,
}

impl Default for GameProfile {
    fn default() -> Self {
        Self {
            id: "default".to_owned(),
            title: "Default".to_owned(),
            path: String::new(),
            auto_scale: false,
            auto_scale_delay: 0,
            windowed_mode: false,
            scaling_mode: ScalingMode::Auto,
            scaling_fit_mode: ScalingFitMode::AspectRatio,
            scaling_type: ScalingType::Off,
            fsr_type: FsrSubtype::Original,
            ls1_type: Ls1Subtype::Balanced,
            anime4k_type: Anime4kSubtype::S,
            scale_factor: 1.5,
            resize_before_scaling: false,
            sharpness: 5,
            ls1_sharpness: 2,
            vrs: false,
            frame_generation: FrameGenerationType::Off,
            cpu_vulkan: false,
            lsfg2_mode: Lsfg2Mode::X2,
            lsfg3_mode: Lsfg3Mode::Fixed,
            lsfg3_multiplier: 2.0,
            lsfg3_target: 60.0,
            lsfg_flow_scale: 100,
            lsfg_size: FrameGenerationSize::Balanced,
            clip_cursor: true,
            adjust_cursor_speed: false,
            hide_cursor: false,
            scale_cursor: false,
            sync_mode: SyncMode::Default,
            max_frame_latency: 3,
            gsync_support: true,
            hdr_support: false,
            draw_fps: true,
            capture_api: CaptureApi::Dxgi,
            queue_target: 1,
            preferred_gpu_id: -1,
            output_display_id: -1,
            crop_input: false,
            crop_left: 0,
            crop_top: 0,
            crop_right: 0,
            crop_bottom: 0,
            multi_display_mode: false,
        }
    }
}

impl GameProfile {
    /// Converts this UI/settings profile into the engine C ABI [`ScalingProfile`].
    pub fn to_scaling_profile(&self) -> ScalingProfile {
        let scaling_subtype = match self.scaling_type {
            ScalingType::Fsr => self.fsr_type as i32,
            ScalingType::Ls1 => self.ls1_type as i32,
            ScalingType::Anime4k => self.anime4k_type as i32,
            _ => 0,
        };
        let frame_generation_mode = match self.frame_generation {
            FrameGenerationType::Lsfg3 => self.lsfg3_mode as i32,
            FrameGenerationType::Lsfg2 => self.lsfg2_mode as i32,
            _ => 0,
        };
        let sharpness = if self.scaling_type == ScalingType::Ls1 {
            self.ls1_sharpness.clamp(0, 4)
        } else {
            self.sharpness.clamp(0, 10)
        };
        ScalingProfile {
            scaling_mode: self.scaling_mode,
            scaling_fit_mode: self.scaling_fit_mode,
            scaling_type: self.scaling_type,
            scaling_subtype,
            scale_factor: normalize_scale_factor(self.scaling_type, self.scale_factor),
            resize_before_scale: self.resize_before_scaling,
            sharpness,
            vrs: self.vrs,
            frame_generation_type: self.frame_generation,
            frame_generation_size: self.lsfg_size,
            frame_generation_mode,
            frame_generation_multiplier: self.lsfg3_multiplier.clamp(1.0, 20.0),
            frame_generation_target: self.lsfg3_target.clamp(10.0, 1000.0),
            frame_generation_flow_scale: self.lsfg_flow_scale.clamp(25, 100),
            clip_cursor: self.clip_cursor,
            cursor_sensitivity: self.adjust_cursor_speed,
            hide_cursor: self.hide_cursor,
            scale_cursor: self.scale_cursor,
            sync_mode: self.sync_mode,
            max_frame_latency: self.max_frame_latency.clamp(1, 15),
            gsync_support: self.gsync_support,
            hdr_support: self.hdr_support,
            capture_api: self.capture_api,
            queue_target: self.queue_target.clamp(0, 2),
            draw_fps: self.draw_fps,
            gpu_id: self.preferred_gpu_id.max(-1),
            display_id: self.output_display_id.max(-1),
            capture_offset_left: if self.crop_input {
                self.crop_left.max(0)
            } else {
                0
            },
            capture_offset_top: if self.crop_input {
                self.crop_top.max(0)
            } else {
                0
            },
            capture_offset_right: if self.crop_input {
                self.crop_right.max(0)
            } else {
                0
            },
            capture_offset_bottom: if self.crop_input {
                self.crop_bottom.max(0)
            } else {
                0
            },
            multi_display_mode: self.multi_display_mode,
            windowed_mode: self.windowed_mode,
            setup_phase: 0,
        }
    }
}

/// Top-level persistent FlowScale application settings (`settings.json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub window_maximized: bool,
    pub window_width: i32,
    pub window_height: i32,
    pub hotkey: HotkeyConfig,
    pub gpu_preference: i32,
    pub gpu_preference_change_count: i32,
    pub setup_phase: i32,
    pub start_as_admin: bool,
    pub start_at_windows_startup: bool,
    pub minimize_to_tray: bool,
    pub close_to_tray: bool,
    pub start_minimized: bool,
    pub theme: ThemeMode,
    pub selected_profile_id: String,
    pub profiles: Vec<GameProfile>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            window_maximized: false,
            window_width: 1100,
            window_height: 720,
            hotkey: HotkeyConfig::default(),
            gpu_preference: 0,
            gpu_preference_change_count: 0,
            setup_phase: 0,
            start_as_admin: false,
            start_at_windows_startup: false,
            minimize_to_tray: false,
            close_to_tray: false,
            start_minimized: false,
            theme: ThemeMode::System,
            selected_profile_id: "default".to_owned(),
            profiles: vec![GameProfile::default()],
        }
    }
}

impl AppSettings {
    /// Returns the default JSON settings path (`%LocalAppData%\FlowScale\settings.json`).
    pub fn default_settings_path() -> PathBuf {
        let base = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("FlowScale").join("settings.json")
    }

    /// Ensures the `"default"` (`Default`) profile exists at index 0 and `selected_profile_id` is valid.
    pub fn ensure_default_profile(&mut self) {
        if let Some(pos) = self.profiles.iter().position(|p| p.id == "default") {
            if pos > 0 {
                let def = self.profiles.remove(pos);
                self.profiles.insert(0, def);
            }
        } else {
            self.profiles.insert(0, GameProfile::default());
        }
        if !self
            .profiles
            .iter()
            .any(|p| p.id == self.selected_profile_id)
        {
            self.selected_profile_id = "default".to_owned();
        }
    }

    /// Loads settings from `path`, ensuring the default profile invariant.
    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self, String> {
        let text = fs::read_to_string(path).map_err(|e| format!("failed to read settings: {e}"))?;
        let mut parsed: Self = serde_json::from_str(&text)
            .map_err(|e| format!("failed to parse settings JSON: {e}"))?;
        parsed.ensure_default_profile();
        Ok(parsed)
    }

    /// Loads settings from `default_settings_path()`, falling back to defaults if missing or unreadable.
    pub fn load_or_default() -> Self {
        Self::load_or_default_checked().0
    }

    /// Loads settings and reports any unreadable/corrupt existing file so callers can protect it.
    pub fn load_or_default_checked() -> (Self, Option<String>) {
        let path = Self::default_settings_path();
        Self::load_or_default_checked_at(&path)
    }

    pub fn load_or_default_checked_at(path: impl AsRef<Path>) -> (Self, Option<String>) {
        let path = path.as_ref();
        match fs::read_to_string(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (Self::default(), None),
            Err(error) => (
                Self::default(),
                Some(format!("failed to read settings: {error}")),
            ),
            Ok(text) => match serde_json::from_str::<Self>(&text) {
                Ok(mut settings) => {
                    settings.ensure_default_profile();
                    (settings, None)
                }
                Err(error) => (
                    Self::default(),
                    Some(format!("failed to parse settings JSON: {error}")),
                ),
            },
        }
    }

    /// Saves settings cleanly to `path`, creating parent directories as needed.
    pub fn save_to_path(&self, path: impl AsRef<Path>) -> Result<(), String> {
        let mut normalized = self.clone();
        normalized.ensure_default_profile();
        let path_ref = path.as_ref();
        let json = serde_json::to_string_pretty(&normalized)
            .map_err(|e| format!("failed to serialize settings JSON: {e}"))?;
        crate::persistence::write_atomic(path_ref, json.as_bytes())
            .map_err(|e| format!("failed to atomically write settings file: {e}"))
    }

    /// Saves settings to `default_settings_path()`.
    pub fn save(&self) -> Result<(), String> {
        self.save_to_path(Self::default_settings_path())
    }

    /// Returns the first non-default profile whose `path` matches `(exe_path, window_title)`.
    pub fn find_matching_profile(
        &self,
        exe_path: &str,
        window_title: &str,
    ) -> Option<&GameProfile> {
        self.profiles
            .iter()
            .filter(|p| p.id != "default" && !p.path.trim().is_empty())
            .find(|p| matches_profile_path(&p.path, exe_path, window_title))
    }
}

/// Normalizes a custom scale factor according to Lossless Scaling rules:
/// clamped to `[1.0, 10.0]`; `Integer` -> `.round()`; `Xbr` -> 1 decimal snapped to even tenth
/// (`-0.1` if odd tenth); all others -> rounded to 3 decimal places.
pub fn normalize_scale_factor(scaling_type: ScalingType, raw: f32) -> f32 {
    let clamped = if raw.is_finite() {
        raw.clamp(1.0, 10.0)
    } else {
        1.0
    };
    match scaling_type {
        ScalingType::Integer => clamped.round().clamp(1.0, 10.0),
        ScalingType::Xbr => {
            let tenths = (clamped * 10.0).round() as i32;
            let even_tenths = tenths - (tenths % 2);
            ((even_tenths as f32) / 10.0).clamp(1.0, 10.0)
        }
        _ => ((clamped * 1000.0).round() / 1000.0).clamp(1.0, 10.0),
    }
}

/// Matches semicolon-delimited glob patterns (`*` and `?`, case-insensitive) against an
/// executable path, its basename, or the window title.
pub fn matches_profile_path(patterns: &str, exe_path: &str, window_title: &str) -> bool {
    let norm_exe = exe_path.replace('/', "\\");
    let exe_name = norm_exe
        .rsplit('\\')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("");

    for raw_pat in patterns.split(';') {
        let pat = raw_pat.trim();
        if pat.is_empty() {
            continue;
        }
        if pat.contains('\\') || pat.contains('/') {
            let norm_pat = pat.replace('/', "\\");
            if !norm_exe.is_empty() && glob_match_ci(&norm_pat, &norm_exe) {
                return true;
            }
        } else if (!exe_name.is_empty() && glob_match_ci(pat, exe_name))
            || (!norm_exe.is_empty() && glob_match_ci(pat, &norm_exe))
            || (!window_title.is_empty() && glob_match_ci(pat, window_title))
        {
            return true;
        }
    }
    false
}

fn glob_match_ci(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut match_ti = 0usize;

    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            pi += 1;
            match_ti = ti;
        } else if let Some(s) = star {
            pi = s + 1;
            match_ti += 1;
            ti = match_ti;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Imports hotkey configuration and game profiles from Lossless Scaling `Settings.xml` text.
pub fn import_lossless_settings_xml(
    xml: &str,
) -> Result<(Option<HotkeyConfig>, Vec<GameProfile>), String> {
    let doc = roxmltree::Document::parse(xml)
        .map_err(|e| format!("failed to parse Lossless Scaling XML: {e}"))?;
    let root = doc.root_element();

    let hotkey = parse_xml_hotkey(root);
    let mut profiles = Vec::new();
    let mut seen_ids = HashSet::new();

    if let Some(gp_node) = find_child_ci(root, &["GameProfiles", "Profiles"]) {
        for (idx, pnode) in gp_node
            .children()
            .filter(|n| n.is_element() && n.tag_name().name().eq_ignore_ascii_case("Profile"))
            .enumerate()
        {
            let profile = parse_xml_profile(pnode, idx, &mut seen_ids);
            profiles.push(profile);
        }
    }

    Ok((hotkey, profiles))
}

/// Imports settings from `%LocalAppData%\Lossless Scaling\Settings.xml`.
pub fn import_lossless_default_xml() -> Result<(Option<HotkeyConfig>, Vec<GameProfile>), String> {
    let local_app_data = std::env::var_os("LOCALAPPDATA")
        .ok_or_else(|| "LOCALAPPDATA environment variable is not set".to_owned())?;
    let xml_path = PathBuf::from(local_app_data)
        .join("Lossless Scaling")
        .join("Settings.xml");
    let xml = fs::read_to_string(&xml_path)
        .map_err(|e| format!("failed to read {}: {e}", xml_path.display()))?;
    import_lossless_settings_xml(&xml)
}

fn find_child_ci<'a, 'input>(
    node: roxmltree::Node<'a, 'input>,
    names: &[&str],
) -> Option<roxmltree::Node<'a, 'input>> {
    node.children().find(|c| {
        c.is_element()
            && names
                .iter()
                .any(|name| c.tag_name().name().eq_ignore_ascii_case(name))
    })
}

fn child_text<'a, 'input>(node: roxmltree::Node<'a, 'input>, names: &[&str]) -> Option<&'a str> {
    find_child_ci(node, names)
        .and_then(|c| c.text())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn parse_xml_bool(val: &str) -> Option<bool> {
    match val.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn parse_xml_hotkey(root: roxmltree::Node<'_, '_>) -> Option<HotkeyConfig> {
    let raw_key = child_text(root, &["Hotkey", "ScaleHotkey", "HotkeyKey"])?;
    if raw_key.eq_ignore_ascii_case("None") {
        return None;
    }

    let mut cfg = HotkeyConfig {
        key: String::new(),
        ctrl: false,
        alt: false,
        shift: false,
        win: false,
        vk: 0,
    };
    if let Some(mods) = child_text(root, &["HotkeyModifiers", "HotkeyModifierKeys"]) {
        let lower = mods.to_ascii_lowercase();
        cfg.ctrl = lower.contains("control") || lower.contains("ctrl");
        cfg.alt = lower.contains("alt") || lower.contains("menu");
        cfg.shift = lower.contains("shift");
        cfg.win = lower.contains("win");
    } else if raw_key.contains('+') {
        let parts: Vec<&str> = raw_key.split('+').map(str::trim).collect();
        let lower = parts[..parts.len().saturating_sub(1)]
            .join(" ")
            .to_ascii_lowercase();
        cfg.ctrl = lower.contains("ctrl") || lower.contains("control");
        cfg.alt = lower.contains("alt");
        cfg.shift = lower.contains("shift");
        cfg.win = lower.contains("win");
    }

    let key_token = raw_key.rsplit('+').next().unwrap_or(raw_key).trim();
    if let Some((canonical, vk)) = parse_xml_key_name(key_token) {
        cfg.key = canonical;
        cfg.vk = vk;
        Some(cfg)
    } else {
        None
    }
}

fn parse_xml_key_name(raw: &str) -> Option<(String, u32)> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let upper = trimmed.to_ascii_uppercase();
    if upper.len() == 1 {
        let b = upper.as_bytes()[0];
        if b.is_ascii_alphanumeric() {
            return Some((upper, u32::from(b)));
        }
    }
    if let Some(rest) = upper.strip_prefix('D') {
        if rest.len() == 1 && rest.as_bytes()[0].is_ascii_digit() {
            return Some((rest.to_owned(), u32::from(rest.as_bytes()[0])));
        }
    }
    if let Some(num_str) = upper.strip_prefix('F') {
        if let Ok(n) = num_str.parse::<u32>() {
            if (1..=24).contains(&n) {
                return Some((upper, 0x6F + n));
            }
        }
    }
    let vk = match upper.as_str() {
        "SPACE" => 0x20,
        "TAB" => 0x09,
        "RETURN" | "ENTER" => 0x0D,
        "ESCAPE" | "ESC" => 0x1B,
        "BACK" | "BACKSPACE" => 0x08,
        "INSERT" => 0x2D,
        "DELETE" => 0x2E,
        "HOME" => 0x24,
        "END" => 0x23,
        "PRIOR" | "PAGEUP" => 0x21,
        "NEXT" | "PAGEDOWN" => 0x22,
        "LEFT" => 0x25,
        "UP" => 0x26,
        "RIGHT" => 0x27,
        "DOWN" => 0x28,
        "PAUSE" => 0x13,
        _ => return None,
    };
    Some((upper, vk))
}

fn parse_xml_profile(
    node: roxmltree::Node<'_, '_>,
    idx: usize,
    seen_ids: &mut HashSet<String>,
) -> GameProfile {
    let mut p = GameProfile::default();

    if let Some(v) = child_text(node, &["Title", "Name"]) {
        p.title = v.to_owned();
    }
    if let Some(v) = child_text(node, &["Path", "ExecutablePath"]) {
        p.path = v.to_owned();
    }
    if let Some(v) = child_text(node, &["AutoScale"]).and_then(parse_xml_bool) {
        p.auto_scale = v;
    }
    if let Some(v) = child_text(node, &["AutoScaleDelay"]).and_then(|s| s.parse().ok()) {
        p.auto_scale_delay = v;
    }
    if let Some(v) = child_text(node, &["WindowedMode"]).and_then(parse_xml_bool) {
        p.windowed_mode = v;
    }
    if let Some(v) = child_text(node, &["ScalingMode"]) {
        p.scaling_mode = match v.to_ascii_lowercase().as_str() {
            "custom" | "1" => ScalingMode::Custom,
            _ => ScalingMode::Auto,
        };
    }
    if let Some(v) = child_text(node, &["ScalingFitMode"]) {
        p.scaling_fit_mode = match v.to_ascii_lowercase().as_str() {
            "fullscreen" | "1" => ScalingFitMode::Fullscreen,
            _ => ScalingFitMode::AspectRatio,
        };
    }
    if let Some(v) = child_text(node, &["ScalingType"]) {
        p.scaling_type = parse_xml_scaling_type(v);
    }
    if let Some(v) = child_text(node, &["FsrType", "FSRType", "FsrSubtype"]) {
        p.fsr_type = match v.to_ascii_lowercase().as_str() {
            "optimized" | "1" => FsrSubtype::Optimized,
            _ => FsrSubtype::Original,
        };
    }
    if let Some(v) = child_text(node, &["Ls1Type", "LS1Type", "Ls1Subtype"]) {
        p.ls1_type = match v.to_ascii_lowercase().as_str() {
            "performance" | "1" => Ls1Subtype::Performance,
            _ => Ls1Subtype::Balanced,
        };
    }
    if let Some(v) = child_text(node, &["Anime4kType", "Anime4KType", "Anime4kSubtype"]) {
        p.anime4k_type = match v.to_ascii_lowercase().as_str() {
            "m" | "1" => Anime4kSubtype::M,
            "l" | "2" => Anime4kSubtype::L,
            "vl" | "3" => Anime4kSubtype::Vl,
            "ul" | "4" => Anime4kSubtype::Ul,
            _ => Anime4kSubtype::S,
        };
    }
    if let Some(v) = child_text(node, &["ScaleFactor"]).and_then(|s| s.parse::<f32>().ok()) {
        p.scale_factor = normalize_scale_factor(p.scaling_type, v);
    }
    if let Some(v) = child_text(node, &["ResizeBeforeScaling"]).and_then(parse_xml_bool) {
        p.resize_before_scaling = v;
    }
    if let Some(v) = child_text(node, &["Sharpness"]).and_then(|s| s.parse::<i32>().ok()) {
        p.sharpness = v.clamp(0, 10);
    }
    if let Some(v) =
        child_text(node, &["LS1Sharpness", "Ls1Sharpness"]).and_then(|s| s.parse::<i32>().ok())
    {
        p.ls1_sharpness = v.clamp(0, 4);
    }
    if let Some(v) = child_text(node, &["VRS", "Vrs"]).and_then(parse_xml_bool) {
        p.vrs = v;
    }
    if let Some(v) = child_text(node, &["FrameGeneration"]) {
        p.frame_generation = parse_xml_fg_type(v);
    }
    if let Some(v) = child_text(node, &["LSFG2Mode", "Lsfg2Mode"]) {
        p.lsfg2_mode = match v.to_ascii_lowercase().as_str() {
            "x3" | "3" | "1" => Lsfg2Mode::X3,
            "x4" | "4" | "2" => Lsfg2Mode::X4,
            _ => Lsfg2Mode::X2,
        };
    }
    if let Some(v) = child_text(node, &["LSFG3Mode", "Lsfg3Mode"]) {
        p.lsfg3_mode = match v.to_ascii_lowercase().as_str() {
            "adaptive" | "1" => Lsfg3Mode::Adaptive,
            _ => Lsfg3Mode::Fixed,
        };
    }
    if let Some(v) = child_text(node, &["LSFG3Multiplier", "Lsfg3Multiplier"])
        .and_then(|s| s.parse::<f32>().ok())
        .filter(|v| v.is_finite())
    {
        p.lsfg3_multiplier = v.clamp(1.0, 20.0);
    }
    if let Some(v) = child_text(node, &["LSFG3Target", "Lsfg3Target"])
        .and_then(|s| s.parse::<f32>().ok())
        .filter(|v| v.is_finite())
    {
        p.lsfg3_target = v.clamp(10.0, 1000.0);
    }
    if let Some(v) =
        child_text(node, &["LSFGFlowScale", "LsfgFlowScale"]).and_then(|s| s.parse::<i32>().ok())
    {
        p.lsfg_flow_scale = v.clamp(25, 100);
    }
    if let Some(v) = child_text(node, &["LSFGSize", "LsfgSize"]) {
        p.lsfg_size = match v.to_ascii_lowercase().as_str() {
            "performance" | "1" => FrameGenerationSize::Performance,
            _ => FrameGenerationSize::Balanced,
        };
    } else if let Some(true) = child_text(node, &["LSFGPerformance"]).and_then(parse_xml_bool) {
        p.lsfg_size = FrameGenerationSize::Performance;
    }
    if let Some(v) = child_text(node, &["ClipCursor"]).and_then(parse_xml_bool) {
        p.clip_cursor = v;
    }
    if let Some(v) = child_text(node, &["AdjustCursorSpeed"]).and_then(parse_xml_bool) {
        p.adjust_cursor_speed = v;
    }
    if let Some(v) = child_text(node, &["HideCursor"]).and_then(parse_xml_bool) {
        p.hide_cursor = v;
    }
    if let Some(v) = child_text(node, &["ScaleCursor"]).and_then(parse_xml_bool) {
        p.scale_cursor = v;
    }
    if let Some(v) = child_text(node, &["SyncMode"]) {
        p.sync_mode = parse_xml_sync_mode(v);
    }
    if let Some(v) = child_text(node, &["MaxFrameLatency"]).and_then(|s| s.parse::<i32>().ok()) {
        p.max_frame_latency = v.clamp(1, 15);
    }
    if let Some(v) = child_text(node, &["GSyncSupport", "GsyncSupport"]).and_then(parse_xml_bool) {
        p.gsync_support = v;
    }
    if let Some(v) = child_text(node, &["HDRSupport", "HdrSupport"]).and_then(parse_xml_bool) {
        p.hdr_support = v;
    }
    if let Some(v) = child_text(node, &["DrawFps", "DrawFPS"]).and_then(parse_xml_bool) {
        p.draw_fps = v;
    }
    if let Some(v) = child_text(node, &["CaptureApi", "CaptureAPI"]) {
        p.capture_api = match v.to_ascii_lowercase().as_str() {
            "wgc" | "1" => CaptureApi::Wgc,
            "gdi" | "2" => CaptureApi::Gdi,
            _ => CaptureApi::Dxgi,
        };
    }
    if let Some(v) = child_text(node, &["QueueTarget"]).and_then(|s| s.parse::<i32>().ok()) {
        p.queue_target = v.clamp(0, 2);
    }
    if let Some(v) = child_text(node, &["PreferredGpuId", "PreferredGPU", "GpuId"])
        .and_then(|s| s.parse::<i32>().ok())
    {
        p.preferred_gpu_id = v.max(-1);
    }
    if let Some(v) = child_text(node, &["OutputDisplayId", "OutputDisplay", "DisplayId"])
        .and_then(|s| s.parse::<i32>().ok())
    {
        p.output_display_id = v.max(-1);
    }
    if let Some(v) = child_text(node, &["CropInput"]).and_then(parse_xml_bool) {
        p.crop_input = v;
    }
    if let Some(v) =
        child_text(node, &["CropLeft", "CaptureOffsetLeft"]).and_then(|s| s.parse().ok())
    {
        p.crop_left = v;
    }
    if let Some(v) = child_text(node, &["CropTop", "CaptureOffsetTop"]).and_then(|s| s.parse().ok())
    {
        p.crop_top = v;
    }
    if let Some(v) =
        child_text(node, &["CropRight", "CaptureOffsetRight"]).and_then(|s| s.parse().ok())
    {
        p.crop_right = v;
    }
    if let Some(v) =
        child_text(node, &["CropBottom", "CaptureOffsetBottom"]).and_then(|s| s.parse().ok())
    {
        p.crop_bottom = v;
    }
    if let Some(v) = child_text(node, &["MultiDisplayMode"]).and_then(parse_xml_bool) {
        p.multi_display_mode = v;
    }

    let base_id = if p.title.eq_ignore_ascii_case("Default") && p.path.is_empty() {
        "default".to_owned()
    } else {
        let slug: String = p
            .title
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect();
        let trimmed = slug.trim_matches('-');
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("default") {
            format!("profile-{idx}")
        } else {
            trimmed.to_owned()
        }
    };
    let mut unique_id = base_id.clone();
    let mut suffix = 2usize;
    while !seen_ids.insert(unique_id.clone()) {
        unique_id = format!("{base_id}-{suffix}");
        suffix += 1;
    }
    p.id = unique_id;
    p
}

fn parse_xml_scaling_type(v: &str) -> ScalingType {
    match v.trim().to_ascii_lowercase().as_str() {
        "ls1" | "1" => ScalingType::Ls1,
        "fsr" | "2" => ScalingType::Fsr,
        "nis" | "3" => ScalingType::Nis,
        "sgsr" | "4" => ScalingType::Sgsr,
        "bcas" | "bicubiccas" | "bicubic cas" | "5" => ScalingType::BicubicCas,
        "anime4k" | "6" => ScalingType::Anime4k,
        "xbr" | "7" => ScalingType::Xbr,
        "sharpbilinear" | "sharp bilinear" | "8" => ScalingType::SharpBilinear,
        "integer" | "9" => ScalingType::Integer,
        "nearest" | "10" => ScalingType::Nearest,
        _ => ScalingType::Off,
    }
}

fn parse_xml_fg_type(v: &str) -> FrameGenerationType {
    let lower = v.trim().to_ascii_lowercase();
    if lower == "1" || lower.contains("lsfg3") || lower.contains("lsfg 3") {
        FrameGenerationType::Lsfg3
    } else if lower == "2" || lower == "4" || lower.contains("lsfg2") || lower.contains("lsfg 2") {
        FrameGenerationType::Lsfg2
    } else if lower == "3" || lower == "5" || lower.contains("lsfg1") || lower.contains("lsfg 1") {
        FrameGenerationType::Lsfg1
    } else {
        FrameGenerationType::Off
    }
}

fn parse_xml_sync_mode(v: &str) -> SyncMode {
    match v.trim().to_ascii_lowercase().as_str() {
        "off" | "0" => SyncMode::Off,
        "vsync" | "vsync1" | "2" => SyncMode::Vsync1,
        "vsync2" | "1/2" | "3" => SyncMode::Vsync2,
        "vsync3" | "1/3" | "4" => SyncMode::Vsync3,
        "vsync4" | "1/4" | "5" => SyncMode::Vsync4,
        _ => SyncMode::Default,
    }
}

/// Returns `true` if `app_name` is configured to launch at Windows startup in `HKCU\...\Run`.
pub fn get_windows_startup(app_name: &str) -> bool {
    if app_name.is_empty() {
        return false;
    }
    let name_w = to_wide_null(app_name);
    let mut hkey = HKEY::default();
    let open = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            STARTUP_RUN_SUBKEY,
            Some(0),
            KEY_READ,
            &mut hkey,
        )
    };
    if open.is_err() {
        return false;
    }
    let mut byte_len = 0u32;
    let query = unsafe {
        RegQueryValueExW(
            hkey,
            PCWSTR(name_w.as_ptr()),
            None,
            None,
            None,
            Some(&mut byte_len),
        )
    };
    unsafe {
        let _ = RegCloseKey(hkey);
    }
    query.is_ok() && byte_len > 2
}

/// Enables or disables Windows startup entry for `app_name` under `HKCU\...\Run`.
pub fn set_windows_startup(app_name: &str, exe_path: &str, enabled: bool) -> Result<(), String> {
    if app_name.is_empty() {
        return Err("app_name cannot be empty".to_owned());
    }
    let name_w = to_wide_null(app_name);
    let mut hkey = HKEY::default();
    let open = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            STARTUP_RUN_SUBKEY,
            None,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut hkey,
            None,
        )
    };
    if open.is_err() {
        return Err(format!("RegCreateKeyExW failed: {:?}", open));
    }

    let res = if enabled {
        let quoted = if exe_path.starts_with('"') {
            exe_path.to_owned()
        } else {
            format!("\"{exe_path}\"")
        };
        let val_w = to_wide_null(&quoted);
        let bytes =
            unsafe { std::slice::from_raw_parts(val_w.as_ptr().cast::<u8>(), val_w.len() * 2) };
        let set_err =
            unsafe { RegSetValueExW(hkey, PCWSTR(name_w.as_ptr()), None, REG_SZ, Some(bytes)) };
        if set_err.is_err() {
            Err(format!("RegSetValueExW failed: {:?}", set_err))
        } else {
            Ok(())
        }
    } else {
        let del_err = unsafe { RegDeleteValueW(hkey, PCWSTR(name_w.as_ptr())) };
        if del_err.is_err() && del_err != ERROR_FILE_NOT_FOUND {
            Err(format!("RegDeleteValueW failed: {:?}", del_err))
        } else {
            Ok(())
        }
    };

    unsafe {
        let _ = RegCloseKey(hkey);
    }
    res
}

fn to_wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
