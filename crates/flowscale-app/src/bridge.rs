//! Pure data mapping, hardware probing, `config.ini`/`settings.json` persistence, and IPC schema.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use flowscale_abi::{
    Anime4kSubtype, CaptureApi, ErrorCode, FrameGenerationSize, FrameGenerationType, FsrSubtype,
    Ls1Subtype, Lsfg2Mode, Lsfg3Mode, ScalingFitMode, ScalingMode, ScalingType, StatusCode,
    SyncMode,
};
use flowscale_config::{
    ini::MAX_FRAMETIME_BUFFER_SIZE, normalize_scale_factor, AppSettings, EngineConfig, GameProfile,
    HotkeyConfig, ThemeMode,
};
use flowscale_core::{is_admin, EngineTelemetry};
use flowscale_overlay::hotkey::{key_name_to_vk, vk_to_key_name};
use flowscale_overlay::{
    enumerate_adapters, enumerate_displays, get_dwm_refresh_rate, windows_build_number,
};
use serde::{Deserialize, Serialize};
use windows::core::{Interface, PCWSTR};
use windows::Win32::Graphics::Dxgi::Common::DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020;
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, IDXGIOutput6};
use windows::Win32::Graphics::Gdi::{EnumDisplaySettingsW, DEVMODEW, ENUM_CURRENT_SETTINGS};

/// Default empty payload helper so `{ "type": "...", "payload": {} }` and omitted payloads both parse.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmptyPayload {}

/// Hotkey representation exchanged over WebView2 IPC.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiHotkeyBinding {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub key: String,
    pub display: String,
}

/// Global settings object matching `GlobalSettings` in `IPC_CONTRACT.md`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiGlobalSettings {
    pub hotkey: UiHotkeyBinding,
    pub theme: String,
    pub minimize_to_tray: bool,
    pub close_to_tray: bool,
    pub start_minimized: bool,
    pub start_as_admin: bool,
    pub start_at_startup: bool,
    pub windows_gpu_preference: i32,
    pub selected_profile_id: String,
}

/// Profile object matching `ScalingProfile` in `IPC_CONTRACT.md`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiScalingProfile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_revision: Option<String>,
    pub id: String,
    pub title: String,
    pub is_default: bool,
    pub process_pattern: String,
    pub auto_scale: bool,
    pub auto_scale_delay_sec: i32,

    pub scaling_mode: String,
    pub fit_mode: String,
    pub scale_factor: f32,
    pub resize_before_scaling: bool,
    pub windowed_mode: bool,
    pub scaling_type: String,
    pub scaling_subtype: String,
    pub sharpness: i32,
    pub vrs: bool,

    pub frame_gen_engine: String,
    #[serde(default)]
    pub cpu_vulkan: bool,
    pub lsfg3_mode: String,
    pub lsfg2_mode: String,
    pub lsfg_multiplier: f32,
    pub lsfg_target_fps: f32,
    pub flow_scale: i32,
    pub lsfg_performance: String,

    pub clip_cursor: bool,
    pub adjust_cursor_speed: bool,
    pub hide_cursor: bool,
    pub scale_cursor: bool,

    pub sync_mode: String,
    pub max_frame_latency: i32,
    pub gsync_support: bool,
    pub hdr_support: bool,
    pub draw_fps: bool,

    pub capture_api: String,
    pub queue_target: i32,
    pub crop_enabled: bool,
    pub crop_left: i32,
    pub crop_top: i32,
    pub crop_right: i32,
    pub crop_bottom: i32,

    pub preferred_gpu: String,
    pub output_display: String,
    pub multi_display_mode: bool,
}

/// Rendering INI subset exposed to the Settings UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiRenderingConfig {
    pub flush: bool,
}

/// Capture INI subset exposed to the Settings UI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiCaptureConfig {
    pub frametime_buffer_size: usize,
    pub frametime_buffer_reset_multiplier: f64,
    pub queue_draining_momentum: f64,
    pub dedup: bool,
}

/// Frame generation INI subset exposed to the Settings UI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiLsfgConfig {
    pub ui_detection_rate: f64,
    pub ui_detection_threshold: f64,
    pub real_timestamp_tolerance: f64,
    pub base_framerate_threshold: f64,
}

/// FPS counter INI subset exposed to the Settings UI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiFpsCounterConfig {
    pub scale: f32,
    pub color: String,
    pub font: String,
    pub offset_x: i32,
    pub offset_y: i32,
    pub show_captured: bool,
}

/// Complete `config.ini` object matching `EngineConfig` in `IPC_CONTRACT.md`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiEngineConfig {
    pub rendering: UiRenderingConfig,
    pub capture: UiCaptureConfig,
    pub lsfg: UiLsfgConfig,
    pub fps_counter: UiFpsCounterConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiGpuAdapter {
    pub id: String,
    pub name: String,
    pub dedicated_vram_mb: u64,
    pub is_default: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiDisplayOutput {
    pub id: String,
    pub name: String,
    pub width: i32,
    pub height: i32,
    pub refresh_rate: f32,
    pub hdr_capable: bool,
    pub is_primary: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiHardwareInfo {
    pub adapters: Vec<UiGpuAdapter>,
    pub displays: Vec<UiDisplayOutput>,
    pub dwm_refresh_rate: f32,
    pub windows_build: String,
    pub is_admin: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiTelemetryState {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub countdown_seconds: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub foreground_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale_factor: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub captured_fps: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presented_fps: Option<f32>,
    pub processing_backend: String,
    pub cpu_ms: f32,
    pub processing_status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    /// Full engine counters for the Session log panel; only sent while scaling runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine: Option<Box<EngineTelemetry>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdPayload {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivatePayload {
    #[serde(default)]
    pub hwnd: Option<isize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuPrefPayload {
    pub preference: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalUrlPayload {
    pub url: String,
}

/// Incoming commands sent by `apps/flowscale-ui` via `window.chrome.webview.postMessage`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum UiCommand {
    GetInitialState(#[serde(default)] EmptyPayload),
    SaveSettings(UiGlobalSettings),
    SelectProfile(IdPayload),
    UpsertProfile(Box<UiScalingProfile>),
    DeleteProfile(IdPayload),
    DuplicateProfile(IdPayload),
    ActivateScaling(#[serde(default)] ActivatePayload),
    StopScaling(#[serde(default)] EmptyPayload),
    CancelCountdown(#[serde(default)] EmptyPayload),
    SetHotkey(UiHotkeyBinding),
    PickExecutable(#[serde(default)] EmptyPayload),
    ImportLosslessXml(#[serde(default)] EmptyPayload),
    Configs(crate::configs::ConfigAction),
    SaveEngineConfig(UiEngineConfig),
    ResetEngineConfig(#[serde(default)] EmptyPayload),
    SetWindowsGpuPreference(GpuPrefPayload),
    RelaunchAsAdmin(#[serde(default)] EmptyPayload),
    OpenExternalUrl(ExternalUrlPayload),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateSyncPayload {
    pub settings: UiGlobalSettings,
    pub profiles: Vec<UiScalingProfile>,
    pub telemetry: UiTelemetryState,
    pub hardware: UiHardwareInfo,
    pub engine_config: UiEngineConfig,
    pub is_admin: bool,
    pub windows_build: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToastPayload {
    pub kind: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CountdownPayload {
    pub seconds: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PickedPathPayload {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileUpsertedPayload {
    pub id: String,
    pub revision: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<UiScalingProfile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Events pushed from the native host to `apps/flowscale-ui` via `PostWebMessageAsJson`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigsResult {
    pub entries: Option<Vec<crate::configs::ConfigSummary>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum HostEvent {
    StateSync(Box<StateSyncPayload>),
    TelemetryTick(UiTelemetryState),
    CountdownTick(CountdownPayload),
    PickedExecutable(PickedPathPayload),
    ConfigsResult(ConfigsResult),
    ProfileUpserted(Box<ProfileUpsertedPayload>),
    Toast(ToastPayload),
}

pub fn format_hotkey_display(ctrl: bool, alt: bool, shift: bool, key: &str) -> String {
    let mut parts = Vec::with_capacity(4);
    if ctrl {
        parts.push("Ctrl");
    }
    if alt {
        parts.push("Alt");
    }
    if shift {
        parts.push("Shift");
    }
    let upper = key.trim().to_ascii_uppercase();
    if upper.is_empty() {
        parts.push("S");
    } else {
        let mut out = parts.join(" + ");
        if !out.is_empty() {
            out.push_str(" + ");
        }
        out.push_str(&upper);
        return out;
    }
    parts.join(" + ")
}

pub fn hotkey_to_ui(h: &HotkeyConfig) -> UiHotkeyBinding {
    let key = if h.key.trim().is_empty() {
        vk_to_key_name(h.vk)
    } else {
        h.key.trim().to_owned()
    };
    let display = format_hotkey_display(h.ctrl, h.alt, h.shift, &key);
    UiHotkeyBinding {
        ctrl: h.ctrl,
        alt: h.alt,
        shift: h.shift,
        key,
        display,
    }
}

pub fn ui_to_hotkey(ui: &UiHotkeyBinding) -> Result<HotkeyConfig, String> {
    let key = ui.key.trim().to_owned();
    let vk =
        key_name_to_vk(&key).ok_or_else(|| format!("Unsupported hotkey key name: {:?}", ui.key))?;
    Ok(HotkeyConfig {
        key: if key.is_empty() {
            vk_to_key_name(vk)
        } else {
            key
        },
        vk,
        ctrl: ui.ctrl,
        alt: ui.alt,
        shift: ui.shift,
        win: false,
    })
}

pub fn theme_to_ui(mode: ThemeMode) -> &'static str {
    match mode {
        ThemeMode::System => "system",
        ThemeMode::Light => "light",
        ThemeMode::Dark => "dark",
    }
}

pub fn ui_to_theme(s: &str) -> ThemeMode {
    match s.trim().to_ascii_lowercase().as_str() {
        "light" => ThemeMode::Light,
        "dark" => ThemeMode::Dark,
        _ => ThemeMode::System,
    }
}

pub fn settings_to_ui(settings: &AppSettings, actual_startup: bool) -> UiGlobalSettings {
    UiGlobalSettings {
        hotkey: hotkey_to_ui(&settings.hotkey),
        theme: theme_to_ui(settings.theme).to_owned(),
        minimize_to_tray: settings.minimize_to_tray,
        close_to_tray: settings.close_to_tray,
        start_minimized: settings.start_minimized,
        start_as_admin: settings.start_as_admin,
        start_at_startup: actual_startup,
        windows_gpu_preference: settings.gpu_preference.clamp(0, 2),
        selected_profile_id: settings.selected_profile_id.clone(),
    }
}

pub fn apply_ui_settings(settings: &mut AppSettings, ui: &UiGlobalSettings) {
    if let Ok(mut hk) = ui_to_hotkey(&ui.hotkey) {
        if hk.ctrl == settings.hotkey.ctrl
            && hk.alt == settings.hotkey.alt
            && hk.shift == settings.hotkey.shift
            && hk.vk == settings.hotkey.vk
        {
            hk.win = settings.hotkey.win;
        }
        settings.hotkey = hk;
    }
    settings.theme = ui_to_theme(&ui.theme);
    settings.minimize_to_tray = ui.minimize_to_tray;
    settings.close_to_tray = ui.close_to_tray;
    settings.start_minimized = ui.start_minimized;
    settings.start_as_admin = ui.start_as_admin;
    settings.start_at_windows_startup = ui.start_at_startup;
    let pref = ui.windows_gpu_preference.clamp(0, 2);
    if settings.gpu_preference != pref {
        settings.gpu_preference = pref;
        settings.gpu_preference_change_count += 1;
    }
    if settings
        .profiles
        .iter()
        .any(|p| p.id == ui.selected_profile_id)
    {
        settings.selected_profile_id = ui.selected_profile_id.clone();
    }
    settings.ensure_default_profile();
}

fn parse_device_index<F>(value: &str, prefix: &str, lookup_name: F) -> i32
where
    F: FnOnce(&str) -> Option<i32>,
{
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("auto") {
        return -1;
    }
    trimmed
        .strip_prefix(prefix)
        .and_then(|s| s.parse::<i32>().ok())
        .or_else(|| trimmed.parse::<i32>().ok())
        .filter(|&v| v >= 0)
        .or_else(|| lookup_name(trimmed))
        .unwrap_or(-1)
}

fn format_device_id(id: i32, prefix: &str) -> String {
    if id < 0 {
        "Auto".to_owned()
    } else {
        format!("{prefix}{id}")
    }
}

pub fn profile_to_ui(p: &GameProfile) -> UiScalingProfile {
    let scaling_type = match p.scaling_type {
        ScalingType::Off => "Off",
        ScalingType::Ls1 => "LS1",
        ScalingType::Fsr => "FSR",
        ScalingType::Nis => "NIS",
        ScalingType::Sgsr => "SGSR",
        ScalingType::BicubicCas => "BicubicCAS",
        ScalingType::Anime4k => "Anime4K",
        ScalingType::Xbr => "XBR",
        ScalingType::SharpBilinear => "SharpBilinear",
        ScalingType::Integer => "Integer",
        ScalingType::Nearest => "Nearest",
    };

    let scaling_subtype = match p.scaling_type {
        ScalingType::Fsr => match p.fsr_type {
            FsrSubtype::Original => "Original",
            FsrSubtype::Optimized => "Optimized",
        },
        ScalingType::Ls1 => match p.ls1_type {
            Ls1Subtype::Balanced => "Balanced",
            Ls1Subtype::Performance => "Performance",
        },
        ScalingType::Anime4k => match p.anime4k_type {
            Anime4kSubtype::S => "S",
            Anime4kSubtype::M => "M",
            Anime4kSubtype::L => "L",
            Anime4kSubtype::Vl => "VL",
            Anime4kSubtype::Ul => "UL",
        },
        _ => "None",
    };

    let sharpness = if p.scaling_type == ScalingType::Ls1 {
        p.ls1_sharpness.clamp(0, 4)
    } else {
        p.sharpness.clamp(0, 10)
    };

    UiScalingProfile {
        client_revision: None,
        id: p.id.clone(),
        title: p.title.clone(),
        is_default: p.id == "default",
        process_pattern: p.path.clone(),
        auto_scale: p.auto_scale,
        auto_scale_delay_sec: p.auto_scale_delay.clamp(0, 60),
        scaling_mode: match p.scaling_mode {
            ScalingMode::Auto => "Auto",
            ScalingMode::Custom => "Custom",
        }
        .to_owned(),
        fit_mode: match p.scaling_fit_mode {
            ScalingFitMode::AspectRatio => "AspectRatio",
            ScalingFitMode::Fullscreen => "Fullscreen",
        }
        .to_owned(),
        scale_factor: normalize_scale_factor(p.scaling_type, p.scale_factor),
        resize_before_scaling: p.resize_before_scaling,
        windowed_mode: p.windowed_mode,
        scaling_type: scaling_type.to_owned(),
        scaling_subtype: scaling_subtype.to_owned(),
        sharpness,
        vrs: p.vrs,
        frame_gen_engine: match p.frame_generation {
            FrameGenerationType::Off => "Off",
            FrameGenerationType::Lsfg3 => "LSFG3",
            FrameGenerationType::Lsfg2 => "LSFG2",
            FrameGenerationType::Lsfg1 => "LSFG1",
        }
        .to_owned(),
        cpu_vulkan: p.cpu_vulkan,
        lsfg3_mode: match p.lsfg3_mode {
            Lsfg3Mode::Fixed => "Fixed",
            Lsfg3Mode::Adaptive => "Adaptive",
        }
        .to_owned(),
        lsfg2_mode: match p.lsfg2_mode {
            Lsfg2Mode::X2 => "X2",
            Lsfg2Mode::X3 => "X3",
            Lsfg2Mode::X4 => "X4",
        }
        .to_owned(),
        lsfg_multiplier: p.lsfg3_multiplier.clamp(1.0, 20.0),
        lsfg_target_fps: p.lsfg3_target.clamp(10.0, 1000.0),
        flow_scale: p.lsfg_flow_scale.clamp(25, 100),
        lsfg_performance: match p.lsfg_size {
            FrameGenerationSize::Balanced => "Balanced",
            FrameGenerationSize::Performance => "Performance",
        }
        .to_owned(),
        clip_cursor: p.clip_cursor,
        adjust_cursor_speed: p.adjust_cursor_speed,
        hide_cursor: p.hide_cursor,
        scale_cursor: p.scale_cursor,
        sync_mode: match p.sync_mode {
            SyncMode::Off => "Off",
            SyncMode::Default => "Default",
            SyncMode::Vsync1 => "Vsync1",
            SyncMode::Vsync2 => "Vsync2",
            SyncMode::Vsync3 => "Vsync3",
            SyncMode::Vsync4 => "Vsync4",
        }
        .to_owned(),
        max_frame_latency: p.max_frame_latency.clamp(1, 15),
        gsync_support: p.gsync_support,
        hdr_support: p.hdr_support,
        draw_fps: p.draw_fps,
        capture_api: match p.capture_api {
            CaptureApi::Dxgi => "DXGI",
            CaptureApi::Wgc => "WGC",
            CaptureApi::Gdi => "GDI",
        }
        .to_owned(),
        queue_target: p.queue_target.clamp(0, 2),
        crop_enabled: p.crop_input,
        crop_left: p.crop_left.max(0),
        crop_top: p.crop_top.max(0),
        crop_right: p.crop_right.max(0),
        crop_bottom: p.crop_bottom.max(0),
        preferred_gpu: format_device_id(p.preferred_gpu_id, "gpu-"),
        output_display: format_device_id(p.output_display_id, "disp-"),
        multi_display_mode: p.multi_display_mode,
    }
}

pub fn ui_to_game_profile(ui: &UiScalingProfile, existing: Option<&GameProfile>) -> GameProfile {
    let is_default = ui.is_default || ui.id == "default";
    let scaling_type = match ui.scaling_type.as_str() {
        "LS1" => ScalingType::Ls1,
        "FSR" => ScalingType::Fsr,
        "NIS" => ScalingType::Nis,
        "SGSR" => ScalingType::Sgsr,
        "BicubicCAS" => ScalingType::BicubicCas,
        "Anime4K" => ScalingType::Anime4k,
        "XBR" => ScalingType::Xbr,
        "SharpBilinear" => ScalingType::SharpBilinear,
        "Integer" => ScalingType::Integer,
        "Nearest" => ScalingType::Nearest,
        _ => ScalingType::Off,
    };

    let mut base = existing.cloned().unwrap_or_default();
    base.id = if is_default {
        "default".to_owned()
    } else if ui.id.trim().is_empty() {
        slugify_profile_id(&ui.title)
    } else {
        ui.id.clone()
    };
    base.title = if is_default {
        "Default".to_owned()
    } else if ui.title.trim().is_empty() {
        "Untitled profile".to_owned()
    } else {
        ui.title.clone()
    };
    base.path = if is_default {
        String::new()
    } else {
        ui.process_pattern.clone()
    };
    base.auto_scale = !is_default && ui.auto_scale;
    base.auto_scale_delay = ui.auto_scale_delay_sec.clamp(0, 60);
    base.scaling_mode = if ui.scaling_mode == "Custom" {
        ScalingMode::Custom
    } else {
        ScalingMode::Auto
    };
    base.scaling_fit_mode = if ui.fit_mode == "Fullscreen" {
        ScalingFitMode::Fullscreen
    } else {
        ScalingFitMode::AspectRatio
    };
    base.scaling_type = scaling_type;
    match scaling_type {
        ScalingType::Fsr => {
            base.fsr_type = if ui.scaling_subtype == "Optimized" {
                FsrSubtype::Optimized
            } else {
                FsrSubtype::Original
            };
        }
        ScalingType::Ls1 => {
            base.ls1_type = if ui.scaling_subtype == "Performance" {
                Ls1Subtype::Performance
            } else {
                Ls1Subtype::Balanced
            };
        }
        ScalingType::Anime4k => {
            base.anime4k_type = match ui.scaling_subtype.as_str() {
                "M" => Anime4kSubtype::M,
                "L" => Anime4kSubtype::L,
                "VL" => Anime4kSubtype::Vl,
                "UL" => Anime4kSubtype::Ul,
                _ => Anime4kSubtype::S,
            };
        }
        _ => {}
    }
    base.scale_factor = normalize_scale_factor(scaling_type, ui.scale_factor);
    base.resize_before_scaling = ui.resize_before_scaling;
    base.windowed_mode = ui.windowed_mode;
    if scaling_type == ScalingType::Ls1 {
        base.ls1_sharpness = ui.sharpness.clamp(0, 4);
    } else {
        base.sharpness = ui.sharpness.clamp(0, 10);
    }
    base.vrs = ui.vrs;

    base.cpu_vulkan = ui.cpu_vulkan;
    base.frame_generation = match ui.frame_gen_engine.as_str() {
        "LSFG3" => FrameGenerationType::Lsfg3,
        "LSFG2" => FrameGenerationType::Lsfg2,
        "LSFG1" => FrameGenerationType::Lsfg1,
        _ => FrameGenerationType::Off,
    };
    base.lsfg3_mode = if ui.lsfg3_mode == "Adaptive" {
        Lsfg3Mode::Adaptive
    } else {
        Lsfg3Mode::Fixed
    };
    base.lsfg2_mode = match ui.lsfg2_mode.as_str() {
        "X3" => Lsfg2Mode::X3,
        "X4" => Lsfg2Mode::X4,
        _ => Lsfg2Mode::X2,
    };
    base.lsfg3_multiplier = ui.lsfg_multiplier.clamp(1.0, 20.0);
    base.lsfg3_target = ui.lsfg_target_fps.clamp(10.0, 1000.0);
    base.lsfg_flow_scale = ui.flow_scale.clamp(25, 100);
    base.lsfg_size = if ui.lsfg_performance == "Performance" {
        FrameGenerationSize::Performance
    } else {
        FrameGenerationSize::Balanced
    };

    base.clip_cursor = ui.clip_cursor;
    base.adjust_cursor_speed = ui.adjust_cursor_speed;
    base.hide_cursor = ui.hide_cursor;
    base.scale_cursor = ui.scale_cursor;
    base.sync_mode = match ui.sync_mode.as_str() {
        "Off" => SyncMode::Off,
        "Vsync1" => SyncMode::Vsync1,
        "Vsync2" => SyncMode::Vsync2,
        "Vsync3" => SyncMode::Vsync3,
        "Vsync4" => SyncMode::Vsync4,
        _ => SyncMode::Default,
    };
    base.max_frame_latency = ui.max_frame_latency.clamp(1, 15);
    base.gsync_support = ui.gsync_support;
    base.hdr_support = ui.hdr_support;
    base.draw_fps = ui.draw_fps;
    base.capture_api = match ui.capture_api.as_str() {
        "WGC" => CaptureApi::Wgc,
        "GDI" => CaptureApi::Gdi,
        _ => CaptureApi::Dxgi,
    };
    base.queue_target = ui.queue_target.clamp(0, 2);
    base.crop_input = ui.crop_enabled;
    base.crop_left = ui.crop_left.max(0);
    base.crop_top = ui.crop_top.max(0);
    base.crop_right = ui.crop_right.max(0);
    base.crop_bottom = ui.crop_bottom.max(0);
    base.preferred_gpu_id = parse_device_index(&ui.preferred_gpu, "gpu-", |name| {
        enumerate_adapters(false)
            .ok()?
            .into_iter()
            .find(|a| a.description.eq_ignore_ascii_case(name))
            .map(|a| a.index as i32)
    });
    base.output_display_id = parse_device_index(&ui.output_display, "disp-", |name| {
        enumerate_displays()
            .ok()?
            .into_iter()
            .find(|d| d.device_name.eq_ignore_ascii_case(name))
            .map(|d| d.index as i32)
    });
    base.multi_display_mode = ui.multi_display_mode;
    base
}

pub fn validate_ui_profile(ui: &UiScalingProfile) -> Result<(), String> {
    let allowed = |value: &str, choices: &[&str], field: &str| {
        if choices.contains(&value) {
            Ok(())
        } else {
            Err(format!("Unsupported {field}: {value:?}"))
        }
    };
    allowed(&ui.scaling_mode, &["Auto", "Custom"], "scaling mode")?;
    allowed(&ui.fit_mode, &["AspectRatio", "Fullscreen"], "fit mode")?;
    allowed(
        &ui.scaling_type,
        &[
            "Off",
            "FSR",
            "NIS",
            "SGSR",
            "BicubicCAS",
            "Anime4K",
            "XBR",
            "SharpBilinear",
            "Integer",
            "Nearest",
            "LS1",
        ],
        "scaling type",
    )?;
    allowed(
        &ui.frame_gen_engine,
        &["Off", "LSFG1", "LSFG2", "LSFG3"],
        "frame generation engine",
    )?;
    allowed(&ui.lsfg3_mode, &["Fixed", "Adaptive"], "FlowGen 3 mode")?;
    allowed(&ui.lsfg2_mode, &["X2", "X3", "X4"], "FlowGen 2 mode")?;
    allowed(
        &ui.lsfg_performance,
        &["Balanced", "Performance"],
        "flow mode",
    )?;
    allowed(
        &ui.sync_mode,
        &["Default", "Off", "Vsync1", "Vsync2", "Vsync3", "Vsync4"],
        "sync mode",
    )?;
    allowed(&ui.capture_api, &["DXGI", "WGC", "GDI"], "capture API")?;
    match ui.scaling_type.as_str() {
        "FSR" => allowed(
            &ui.scaling_subtype,
            &["Original", "Optimized"],
            "FSR subtype",
        ),
        "LS1" => allowed(
            &ui.scaling_subtype,
            &["Balanced", "Performance"],
            "FlowSharp subtype",
        ),
        "Anime4K" => allowed(
            &ui.scaling_subtype,
            &["S", "M", "L", "VL", "UL"],
            "Anime4K subtype",
        ),
        _ => Ok(()),
    }
}

pub fn validate_ui_theme(theme: &str) -> Result<(), String> {
    if ["system", "light", "dark"].contains(&theme) {
        Ok(())
    } else {
        Err(format!("Unsupported theme: {theme:?}"))
    }
}

pub fn slugify_profile_id(title: &str) -> String {
    let raw: String = title
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = raw.trim_matches('-');
    if trimmed.is_empty() {
        "profile".to_owned()
    } else {
        trimmed.to_owned()
    }
}

pub fn parse_hex_rgb(input: &str) -> Option<u32> {
    let stripped = input
        .trim()
        .strip_prefix('#')
        .or_else(|| input.trim().strip_prefix("0x"))
        .or_else(|| input.trim().strip_prefix("0X"))
        .unwrap_or(input.trim());
    if stripped.len() > 8 || stripped.is_empty() {
        return None;
    }
    u32::from_str_radix(stripped, 16)
        .ok()
        .map(|v| v & 0x00FF_FFFF)
}

pub fn format_hex_rgb(color: u32) -> String {
    format!("#{:06X}", color & 0x00FF_FFFF)
}

pub fn engine_config_to_ui(cfg: &EngineConfig) -> UiEngineConfig {
    UiEngineConfig {
        rendering: UiRenderingConfig {
            flush: cfg.rendering.flush,
        },
        capture: UiCaptureConfig {
            frametime_buffer_size: cfg.capture.frametime_buffer_size,
            frametime_buffer_reset_multiplier: cfg.capture.frametime_buffer_reset_multiplier,
            queue_draining_momentum: cfg.capture.queue_draining_momentum,
            dedup: cfg.capture.dedup,
        },
        lsfg: UiLsfgConfig {
            ui_detection_rate: f64::from(cfg.lsfg.ui_detection_rate),
            ui_detection_threshold: cfg.lsfg.ui_detection_threshold,
            real_timestamp_tolerance: cfg.lsfg.real_timestamp_tolerance,
            base_framerate_threshold: cfg.lsfg.base_framerate_threshold,
        },
        fps_counter: UiFpsCounterConfig {
            scale: cfg.fps_counter.scale,
            color: format_hex_rgb(cfg.fps_counter.color),
            font: cfg.fps_counter.font.clone(),
            offset_x: cfg.fps_counter.offset_x,
            offset_y: cfg.fps_counter.offset_y,
            show_captured: cfg.fps_counter.show_captured,
        },
    }
}

pub fn ui_to_engine_config(ui: &UiEngineConfig, prev: &EngineConfig) -> EngineConfig {
    let mut cfg = prev.clone();
    cfg.rendering.flush = ui.rendering.flush;
    cfg.capture.frametime_buffer_size = ui
        .capture
        .frametime_buffer_size
        .clamp(1, MAX_FRAMETIME_BUFFER_SIZE);
    cfg.capture.frametime_buffer_reset_multiplier =
        ui.capture.frametime_buffer_reset_multiplier.max(1.1);
    cfg.capture.queue_draining_momentum = ui.capture.queue_draining_momentum.clamp(0.0, 1.0);
    cfg.capture.dedup = ui.capture.dedup;
    cfg.lsfg.ui_detection_rate = ui.lsfg.ui_detection_rate.round().clamp(1.0, 60.0) as u32;
    cfg.lsfg.ui_detection_threshold = ui.lsfg.ui_detection_threshold.clamp(0.0, 1.0);
    cfg.lsfg.real_timestamp_tolerance = ui.lsfg.real_timestamp_tolerance.max(0.0);
    cfg.lsfg.base_framerate_threshold = ui.lsfg.base_framerate_threshold.max(0.0);
    cfg.fps_counter.scale = ui.fps_counter.scale.clamp(0.25, 4.0);
    if let Some(rgb) = parse_hex_rgb(&ui.fps_counter.color) {
        cfg.fps_counter.color = rgb;
    }
    if !ui.fps_counter.font.trim().is_empty() {
        cfg.fps_counter.font = ui.fps_counter.font.trim().to_owned();
    }
    cfg.fps_counter.offset_x = ui.fps_counter.offset_x;
    cfg.fps_counter.offset_y = ui.fps_counter.offset_y;
    cfg.fps_counter.show_captured = ui.fps_counter.show_captured;
    cfg
}

pub fn error_code_message(code: i32) -> Option<&'static str> {
    match code {
        x if x == ErrorCode::NoCode as i32 => None,
        x if x == ErrorCode::ResizeFailed as i32 => {
            Some("Failed to resize target window for scaling.")
        }
        x if x == ErrorCode::LargeOffset as i32 => {
            Some("Crop offsets exceed the captured window area.")
        }
        x if x == ErrorCode::CaptureFailed as i32 => {
            Some("Frame capture stopped or failed (try switching between DXGI and WGC).")
        }
        _ => Some("Engine reported an unexpected error."),
    }
}

pub fn telemetry_to_ui(
    t: &EngineTelemetry,
    countdown_seconds: Option<u32>,
    error_override: Option<&str>,
) -> UiTelemetryState {
    let cpu_error = t.processing_backend.starts_with("CPU")
        && t.error_code != ErrorCode::NoCode as i32
        && !t.last_output_drop_reason.is_empty();
    let err_msg = cpu_error.then(|| t.last_output_drop_reason.clone()).or_else(|| error_override
        .map(str::to_owned)
        .or_else(|| error_code_message(t.error_code).map(str::to_owned)));

    let status = if countdown_seconds.is_some() {
        "countdown"
    } else if err_msg.is_some() {
        "error"
    } else {
        match t.status {
            x if x == StatusCode::Active as i32 => "active",
            x if x == StatusCode::TempDisabled as i32 => "paused",
            _ => "idle",
        }
    };

    let has_dims = t.input_width > 0 && t.output_width > 0;
    let is_running = matches!(status, "active" | "paused");

    UiTelemetryState {
        status: status.to_owned(),
        countdown_seconds,
        target_title: (!t.target_title.is_empty()).then(|| t.target_title.clone()),
        foreground_title: (!t.foreground_title.is_empty()).then(|| t.foreground_title.clone()),
        input_width: has_dims.then_some(t.input_width),
        input_height: has_dims.then_some(t.input_height),
        output_width: has_dims.then_some(t.output_width),
        output_height: has_dims.then_some(t.output_height),
        scale_factor: is_running.then_some(t.scale_factor),
        captured_fps: is_running.then_some(t.captured_fps),
        presented_fps: is_running.then_some(t.presented_fps),
        processing_backend: t.processing_backend.clone(),
        cpu_ms: t.cpu_ms,
        processing_status: if t.processing_backend.starts_with("CPU") { t.last_output_drop_reason.clone() } else { String::new() },
        error_message: err_msg,
        engine: is_running.then(|| Box::new(t.clone())),
    }
}

/// Resolves the canonical `config.ini` path: local `./config.ini` if it exists,
/// otherwise `%LocalAppData%\FlowScale\config.ini`.
pub fn config_ini_path() -> PathBuf {
    let local = Path::new("config.ini");
    if local.exists() {
        return local.to_path_buf();
    }
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("FlowScale").join("config.ini")
}

pub fn load_engine_config() -> EngineConfig {
    load_engine_config_checked().0
}

pub fn load_engine_config_checked() -> (EngineConfig, Option<String>) {
    load_engine_config_checked_at(&config_ini_path())
}

pub fn load_engine_config_checked_at(path: &Path) -> (EngineConfig, Option<String>) {
    match std::fs::read_to_string(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            (EngineConfig::default(), None)
        }
        Err(error) => (
            EngineConfig::default(),
            Some(format!("failed to read config.ini: {error}")),
        ),
        Ok(text) => match EngineConfig::parse_checked(&text) {
            Ok(config) => (config, None),
            Err(error) => (EngineConfig::default(), Some(error)),
        },
    }
}

/// Collects live DXGI adapter VRAM, monitor refresh/HDR capabilities, DWM timing, and Windows build.
pub fn collect_hardware_info() -> UiHardwareInfo {
    let dwm_hz = get_dwm_refresh_rate();
    let mut adapters = Vec::new();
    let mut displays = Vec::new();

    if let Ok(factory) = unsafe { CreateDXGIFactory1::<IDXGIFactory1>() } {
        let mut a_idx = 0u32;
        while let Ok(adapter) = unsafe { factory.EnumAdapters1(a_idx) } {
            if let Ok(desc) = unsafe { adapter.GetDesc1() } {
                let is_sw = (desc.Flags & 2) != 0; // DXGI_ADAPTER_FLAG_SOFTWARE
                if !is_sw {
                    let len = desc
                        .Description
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(desc.Description.len());
                    let name = String::from_utf16_lossy(&desc.Description[..len]);
                    let vram_mb = (desc.DedicatedVideoMemory as u64) / (1024 * 1024);
                    let id_idx = adapters.len();
                    adapters.push(UiGpuAdapter {
                        id: format!("gpu-{id_idx}"),
                        name,
                        dedicated_vram_mb: vram_mb,
                        is_default: id_idx == 0,
                    });
                }
            }

            let mut o_idx = 0u32;
            while let Ok(output) = unsafe { adapter.EnumOutputs(o_idx) } {
                if let Ok(desc) = unsafe { output.GetDesc() } {
                    let len = desc
                        .DeviceName
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(desc.DeviceName.len());
                    let dev_name = String::from_utf16_lossy(&desc.DeviceName[..len]);
                    let r = desc.DesktopCoordinates;
                    let width = (r.right - r.left).max(0);
                    let height = (r.bottom - r.top).max(0);
                    let refresh_rate = query_display_refresh_hz(&dev_name).unwrap_or(dwm_hz);
                    let hdr_capable = output
                        .cast::<IDXGIOutput6>()
                        .ok()
                        .and_then(|o6| unsafe { o6.GetDesc1() }.ok())
                        .is_some_and(|d6| {
                            d6.ColorSpace == DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020
                        });
                    let d_idx = displays.len();
                    displays.push(UiDisplayOutput {
                        id: format!("disp-{d_idx}"),
                        name: dev_name,
                        width,
                        height,
                        refresh_rate,
                        hdr_capable,
                        is_primary: r.left == 0 && r.top == 0,
                    });
                }
                o_idx += 1;
            }
            a_idx += 1;
        }
    } else {
        for a in enumerate_adapters(false).unwrap_or_default() {
            adapters.push(UiGpuAdapter {
                id: format!("gpu-{}", a.index),
                name: a.description,
                dedicated_vram_mb: 0,
                is_default: a.index == 0,
            });
        }
    }

    UiHardwareInfo {
        adapters,
        displays,
        dwm_refresh_rate: dwm_hz,
        windows_build: query_windows_build_string(),
        is_admin: is_admin(),
    }
}

fn query_display_refresh_hz(device_name: &str) -> Option<f32> {
    let wide: Vec<u16> = device_name
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut dm = DEVMODEW {
        dmSize: std::mem::size_of::<DEVMODEW>() as u16,
        ..Default::default()
    };
    let ok = unsafe {
        EnumDisplaySettingsW(PCWSTR(wide.as_ptr()), ENUM_CURRENT_SETTINGS, &mut dm).as_bool()
    };
    (ok && dm.dmDisplayFrequency > 1).then_some(dm.dmDisplayFrequency as f32)
}

fn query_windows_build_string() -> String {
    match windows_build_number() {
        Some(b) if b >= 22000 => format!("Windows 11 (Build {b})"),
        Some(b) => format!("Windows 10 (Build {b})"),
        None => "Windows".to_owned(),
    }
}

const AUTO_SCALE_RETRY_COOLDOWN: Duration = Duration::from_secs(2);
const AUTO_SCALE_MAX_FAILURES: u32 = 3;

/// Foreground auto-scale countdown state machine.
#[derive(Debug, Default)]
pub struct AutoScaleTracker {
    pending_hwnd: isize,
    pending_profile_id: String,
    started_at: Option<Instant>,
    last_emitted_sec: Option<u32>,
    suppressed_hwnd: isize,
    failed_hwnd: isize,
    failure_count: u32,
    retry_after: Option<Instant>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AutoScaleAction {
    None,
    Countdown(u32),
    Trigger(isize, Box<GameProfile>),
}

impl AutoScaleTracker {
    pub fn reset(&mut self) {
        self.pending_hwnd = 0;
        self.pending_profile_id.clear();
        self.started_at = None;
        self.last_emitted_sec = None;
    }

    fn clear_failures(&mut self) {
        self.failed_hwnd = 0;
        self.failure_count = 0;
        self.retry_after = None;
    }

    pub fn suppress_hwnd(&mut self, hwnd: isize) {
        self.suppressed_hwnd = hwnd;
        self.clear_failures();
        self.reset();
    }

    /// Records a failed auto-scale activation for `hwnd`, applying a bounded retry cooldown
    /// and suppressing `hwnd` only after repeated failures until focus changes.
    pub fn record_failure(&mut self, hwnd: isize, now: Instant) {
        if hwnd == 0 {
            self.reset();
            return;
        }
        if self.failed_hwnd == hwnd {
            self.failure_count = self.failure_count.saturating_add(1);
        } else {
            self.failed_hwnd = hwnd;
            self.failure_count = 1;
        }
        if self.failure_count >= AUTO_SCALE_MAX_FAILURES {
            self.suppress_hwnd(hwnd);
        } else {
            self.reset();
            self.retry_after = Some(now + AUTO_SCALE_RETRY_COOLDOWN);
        }
    }

    pub fn active_countdown(&self) -> Option<u32> {
        self.last_emitted_sec
    }

    #[allow(clippy::too_many_arguments)]
    pub fn tick(
        &mut self,
        now: Instant,
        fg_hwnd: isize,
        exe_path: &str,
        window_title: &str,
        is_engine_active: bool,
        main_hwnd: isize,
        settings: &AppSettings,
    ) -> AutoScaleAction {
        if is_engine_active {
            if self.pending_hwnd != 0 {
                self.reset();
            }
            return AutoScaleAction::None;
        }

        if fg_hwnd == 0 || fg_hwnd == main_hwnd {
            if fg_hwnd != self.suppressed_hwnd {
                self.suppressed_hwnd = 0;
            }
            if fg_hwnd != self.failed_hwnd {
                self.clear_failures();
            }
            self.reset();
            return AutoScaleAction::None;
        }

        if fg_hwnd == self.suppressed_hwnd {
            self.reset();
            return AutoScaleAction::None;
        }
        self.suppressed_hwnd = 0;

        if fg_hwnd != self.failed_hwnd {
            self.clear_failures();
        } else if let Some(until) = self.retry_after {
            if now < until {
                self.reset();
                return AutoScaleAction::None;
            }
            self.retry_after = None;
        }

        let Some(matched) = settings.find_matching_profile(exe_path, window_title) else {
            self.reset();
            return AutoScaleAction::None;
        };
        if !matched.auto_scale {
            self.reset();
            return AutoScaleAction::None;
        }

        let delay = Duration::from_secs(matched.auto_scale_delay.clamp(0, 60) as u64);
        if self.pending_hwnd != fg_hwnd || self.pending_profile_id != matched.id {
            self.pending_hwnd = fg_hwnd;
            self.pending_profile_id = matched.id.clone();
            self.started_at = Some(now);
            self.last_emitted_sec = None;
        }

        let elapsed = self
            .started_at
            .map_or(Duration::ZERO, |t| now.saturating_duration_since(t));
        if elapsed >= delay {
            let profile = matched.clone();
            self.reset();
            return AutoScaleAction::Trigger(fg_hwnd, Box::new(profile));
        }

        let rem = delay.saturating_sub(elapsed).as_secs_f32().ceil() as u32;
        if self.last_emitted_sec != Some(rem) {
            self.last_emitted_sec = Some(rem);
            return AutoScaleAction::Countdown(rem);
        }

        AutoScaleAction::None
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn cpu_runtime_failures_keep_the_actionable_message() {
        let telemetry = EngineTelemetry {
            processing_backend: "CPU Vulkan".into(),
            error_code: ErrorCode::CaptureFailed as i32,
            last_output_drop_reason: "CPU Vulkan runtime is missing. Install the complete package.".into(),
            ..EngineTelemetry::default()
        };
        let ui = telemetry_to_ui(&telemetry, None, Some("Activation failed"));
        assert_eq!(ui.error_message.as_deref(), Some("CPU Vulkan runtime is missing. Install the complete package."));
    }

    #[test]
    fn session_log_counters_are_sent_only_while_scaling_runs() {
        let active = EngineTelemetry {
            status: StatusCode::Active as i32,
            gpu_ms_max: 4.2,
            dropped_outputs: 3,
            ..EngineTelemetry::default()
        };
        let engine = telemetry_to_ui(&active, None, None).engine.expect("active run carries counters");
        assert_eq!((engine.gpu_ms_max, engine.dropped_outputs), (4.2, 3));
        let json = serde_json::to_string(&telemetry_to_ui(&EngineTelemetry::default(), None, None)).unwrap();
        assert!(!json.contains("\"engine\""));
    }
    use super::*;

    #[test]
    fn parses_all_ui_commands_from_contract() {
        let samples = [
            r#"{"type":"get_initial_state","payload":{}}"#,
            r#"{"type":"select_profile","payload":{"id":"default"}}"#,
            r#"{"type":"delete_profile","payload":{"id":"game-1"}}"#,
            r#"{"type":"duplicate_profile","payload":{"id":"default"}}"#,
            r#"{"type":"activate_scaling","payload":{}}"#,
            r#"{"type":"activate_scaling","payload":{"hwnd":12345}}"#,
            r#"{"type":"stop_scaling","payload":{}}"#,
            r#"{"type":"cancel_countdown","payload":{}}"#,
            r#"{"type":"set_hotkey","payload":{"ctrl":true,"alt":true,"shift":false,"key":"F10","display":"Ctrl + Alt + F10"}}"#,
            r#"{"type":"pick_executable","payload":{}}"#,
            r#"{"type":"import_lossless_xml","payload":{}}"#,
            r#"{"type":"reset_engine_config","payload":{}}"#,
            r#"{"type":"set_windows_gpu_preference","payload":{"preference":2}}"#,
            r#"{"type":"relaunch_as_admin","payload":{}}"#,
            r#"{"type":"open_external_url","payload":{"url":"https://github.com"}}"#,
        ];
        for raw in samples {
            let cmd: UiCommand = serde_json::from_str(raw).expect(raw);
            let _ = cmd;
        }
    }

    #[test]
    fn boxed_profile_command_keeps_flat_ipc_payload() {
        let command = UiCommand::UpsertProfile(Box::new(profile_to_ui(&GameProfile::default())));
        let value = serde_json::to_value(command).unwrap();
        assert_eq!(value["type"], "upsert_profile");
        assert_eq!(value["payload"]["id"], "default");
        assert!(value["payload"]["profile"].is_null());
    }

    #[test]
    fn profile_roundtrip_enforces_scale_and_sharpness_bounds() {
        let gp = GameProfile {
            id: "xbr-game".to_owned(),
            title: "Retro".to_owned(),
            scaling_type: ScalingType::Xbr,
            scale_factor: 2.34,
            preferred_gpu_id: -1,
            output_display_id: 1,
            ..Default::default()
        };

        let mut ui = profile_to_ui(&gp);
        assert_eq!(ui.scale_factor, 2.2);
        assert_eq!(ui.preferred_gpu, "Auto");
        assert_eq!(ui.output_display, "disp-1");

        let toast_json = serde_json::to_string(&HostEvent::Toast(ToastPayload {
            kind: "success".to_owned(),
            message: "Ready".to_owned(),
        }))
        .unwrap();
        assert!(toast_json.contains(r#""type":"toast""#));
        assert!(toast_json.contains(r#""payload":{"kind":"success""#));

        ui.scaling_type = "LS1".to_owned();
        ui.sharpness = 9;
        ui.preferred_gpu = "gpu-0".to_owned();
        let converted = ui_to_game_profile(&ui, Some(&gp));
        assert_eq!(converted.ls1_sharpness, 4);
        assert_eq!(converted.preferred_gpu_id, 0);
        assert_eq!(converted.output_display_id, 1);

        ui.scaling_type = "Integer".to_owned();
        ui.scale_factor = 3.7;
        let int_conv = ui_to_game_profile(&ui, None);
        assert_eq!(int_conv.scale_factor, 4.0);
    }

    #[test]
    fn hex_rgb_roundtrip_and_ini_conversion() {
        assert_eq!(parse_hex_rgb("#00BF63"), Some(0x0000_BF63));
        assert_eq!(parse_hex_rgb("0xD3D3D3"), Some(0x00D3_D3D3));
        assert_eq!(format_hex_rgb(0x0000_BF63), "#00BF63");

        let mut cfg = EngineConfig::default();
        cfg.fps_counter.color = 0x0000_BF63;
        let ui = engine_config_to_ui(&cfg);
        assert_eq!(ui.fps_counter.color, "#00BF63");
        let back = ui_to_engine_config(&ui, &cfg);
        assert_eq!(back.fps_counter.color, 0x0000_BF63);
    }

    #[test]
    fn bridge_clamps_untrusted_frametime_buffer_size() {
        let mut ui = engine_config_to_ui(&EngineConfig::default());
        ui.capture.frametime_buffer_size = usize::MAX;
        assert_eq!(
            ui_to_engine_config(&ui, &EngineConfig::default())
                .capture
                .frametime_buffer_size,
            1024
        );
    }

    #[test]
    fn profile_enum_validation_rejects_unknown_values() {
        let mut ui = profile_to_ui(&GameProfile::default());
        ui.frame_gen_engine = "future-engine".to_owned();
        assert!(validate_ui_profile(&ui)
            .unwrap_err()
            .contains("frame generation engine"));
        assert!(validate_ui_theme("neon").is_err());
    }

    #[test]
    fn corrupt_ini_load_is_reported_without_changing_the_file() {
        let path =
            std::env::temp_dir().join(format!("flowscale-corrupt-ini-{}", std::process::id()));
        std::fs::write(&path, "not a config file").unwrap();
        let (fallback, error) = load_engine_config_checked_at(&path);
        assert!(error.is_some());
        assert_eq!(fallback, EngineConfig::default());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "not a config file");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn auto_scale_tracker_counts_down_and_resets_on_focus_change() {
        let mut settings = AppSettings::default();
        let game = GameProfile {
            id: "elden".to_owned(),
            path: "eldenring.exe".to_owned(),
            auto_scale: true,
            auto_scale_delay: 3,
            ..Default::default()
        };
        settings.profiles.push(game);

        let mut tracker = AutoScaleTracker::default();
        let t0 = Instant::now();

        assert_eq!(
            tracker.tick(
                t0,
                1001,
                r"C:\Games\eldenring.exe",
                "ELDEN RING",
                false,
                999,
                &settings
            ),
            AutoScaleAction::Countdown(3)
        );
        assert_eq!(
            tracker.tick(
                t0 + Duration::from_millis(1200),
                1001,
                r"C:\Games\eldenring.exe",
                "ELDEN RING",
                false,
                999,
                &settings
            ),
            AutoScaleAction::Countdown(2)
        );

        // Switching to desktop cancels countdown.
        assert_eq!(
            tracker.tick(
                t0 + Duration::from_millis(1500),
                2002,
                r"C:\Windows\explorer.exe",
                "Desktop",
                false,
                999,
                &settings
            ),
            AutoScaleAction::None
        );

        // Returning to game starts fresh and triggers after full 3s delay.
        let t1 = t0 + Duration::from_secs(2);
        assert_eq!(
            tracker.tick(
                t1,
                1001,
                r"C:\Games\eldenring.exe",
                "ELDEN RING",
                false,
                999,
                &settings
            ),
            AutoScaleAction::Countdown(3)
        );
        match tracker.tick(
            t1 + Duration::from_secs(3),
            1001,
            r"C:\Games\eldenring.exe",
            "ELDEN RING",
            false,
            999,
            &settings,
        ) {
            AutoScaleAction::Trigger(hwnd, p) => {
                assert_eq!(hwnd, 1001);
                assert_eq!(p.id, "elden");
            }
            other => panic!("expected Trigger, got {other:?}"),
        }
        assert_eq!(tracker.active_countdown(), None);
    }

    #[test]
    fn auto_scale_countdown_persists_between_ticks_and_settings_preserve_win_hotkey() {
        let mut settings = AppSettings::default();
        let game = GameProfile {
            id: "elden".to_owned(),
            path: "eldenring.exe".to_owned(),
            auto_scale: true,
            auto_scale_delay: 3,
            ..Default::default()
        };
        settings.profiles.push(game);
        settings.hotkey.win = true;

        let mut tracker = AutoScaleTracker::default();
        let t0 = Instant::now();
        assert_eq!(
            tracker.tick(
                t0,
                1001,
                r"C:\Games\eldenring.exe",
                "ELDEN RING",
                false,
                999,
                &settings
            ),
            AutoScaleAction::Countdown(3)
        );
        // Intermediate 200ms tick still inside second 3 emits None, but active_countdown stays 3.
        assert_eq!(
            tracker.tick(
                t0 + Duration::from_millis(200),
                1001,
                r"C:\Games\eldenring.exe",
                "ELDEN RING",
                false,
                999,
                &settings
            ),
            AutoScaleAction::None
        );
        assert_eq!(tracker.active_countdown(), Some(3));

        // Saving unrelated UI settings must preserve an imported `win` hotkey flag.
        let mut ui = settings_to_ui(&settings, false);
        ui.minimize_to_tray = true;
        apply_ui_settings(&mut settings, &ui);
        assert!(settings.hotkey.win);
        assert!(settings.minimize_to_tray);
    }

    #[test]
    fn auto_scale_retries_transient_failure_then_suppresses_after_max_attempts() {
        let mut settings = AppSettings::default();
        let game = GameProfile {
            id: "elden".to_owned(),
            path: "eldenring.exe".to_owned(),
            auto_scale: true,
            auto_scale_delay: 0,
            ..Default::default()
        };
        settings.profiles.push(game);

        let mut tracker = AutoScaleTracker::default();
        let t0 = Instant::now();

        // Attempt 1 triggers and fails transiently: cooldown blocks immediate spam, then retries.
        assert!(matches!(
            tracker.tick(
                t0,
                1001,
                r"C:\Games\eldenring.exe",
                "ELDEN RING",
                false,
                999,
                &settings
            ),
            AutoScaleAction::Trigger(1001, _)
        ));
        tracker.record_failure(1001, t0);
        assert_eq!(
            tracker.tick(
                t0 + Duration::from_secs(1),
                1001,
                r"C:\Games\eldenring.exe",
                "ELDEN RING",
                false,
                999,
                &settings
            ),
            AutoScaleAction::None
        );

        // Attempt 2 retries after cooldown and fails.
        let t2 = t0 + Duration::from_secs(2);
        assert!(matches!(
            tracker.tick(
                t2,
                1001,
                r"C:\Games\eldenring.exe",
                "ELDEN RING",
                false,
                999,
                &settings
            ),
            AutoScaleAction::Trigger(1001, _)
        ));
        tracker.record_failure(1001, t2);

        // Attempt 3 retries after cooldown and fails -> hits max attempts, suppressing HWND.
        let t4 = t2 + Duration::from_secs(2);
        assert!(matches!(
            tracker.tick(
                t4,
                1001,
                r"C:\Games\eldenring.exe",
                "ELDEN RING",
                false,
                999,
                &settings
            ),
            AutoScaleAction::Trigger(1001, _)
        ));
        tracker.record_failure(1001, t4);

        // Suppressed while focus stays on 1001 even after cooldown.
        assert_eq!(
            tracker.tick(
                t4 + Duration::from_secs(10),
                1001,
                r"C:\Games\eldenring.exe",
                "ELDEN RING",
                false,
                999,
                &settings
            ),
            AutoScaleAction::None
        );

        // Focus change clears suppression so returning to 1001 can trigger again.
        assert_eq!(
            tracker.tick(
                t4 + Duration::from_secs(11),
                2002,
                r"C:\Windows\explorer.exe",
                "Desktop",
                false,
                999,
                &settings
            ),
            AutoScaleAction::None
        );
        assert!(matches!(
            tracker.tick(
                t4 + Duration::from_secs(12),
                1001,
                r"C:\Games\eldenring.exe",
                "ELDEN RING",
                false,
                999,
                &settings
            ),
            AutoScaleAction::Trigger(1001, _)
        ));

        // Explicit suppression on success prevents re-triggering until focus changes.
        tracker.suppress_hwnd(1001);
        assert_eq!(
            tracker.tick(
                t4 + Duration::from_secs(13),
                1001,
                r"C:\Games\eldenring.exe",
                "ELDEN RING",
                false,
                999,
                &settings
            ),
            AutoScaleAction::None
        );
    }
}
