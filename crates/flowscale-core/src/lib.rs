//! Clean-room `flowscale-core` engine runtime and C ABI (`cdylib` + `rlib`).

#![allow(clippy::too_many_arguments)]

use std::io::{BufWriter, Write};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

mod cpu_vulkan;

use flowscale_abi::{
    CaptureApi, ErrorCode, FrameGenerationType, Lsfg3Mode, ScalingMode, ScalingProfile,
    StatusCallback, StatusCode, StatusEvent, SyncMode,
};
use flowscale_capture::{
    CaptureBackend, CaptureError, CaptureRegion, CapturedFrame, DxgiDuplicationCapture,
    FrameRingQueue, GdiCapture, WgcCapture,
};
use flowscale_config::pacer::{present_deadline, FrametimeProfiler, OutputTimeline, ScheduledOutput};
use flowscale_config::{
    EngineConfig, FrametimeTracker, GameProfile, HybridPacer, InterpolationMode,
    InterpolationScheduler, QueueDrainingController,
};
use flowscale_gpu::{
    compute_dst_rect, search_radius_for_fg_size, ComputePipeline, D3d11Context, FpsOverlay,
    FrameLatencyWait, GpuError, GpuTimingSample, OverlaySwapChain, PresentOutcome,
};
use flowscale_overlay::{
    enumerate_adapters, enumerate_displays, extract_cursor_rgba, get_adapter_names_joined,
    get_display_names_joined, get_dwm_refresh_rate, get_foreground_window,
    get_refresh_rate_for_rect, get_window_title_and_exe, is_windows_build_at_least,
    resize_window_for_scale, set_cursor_clip, AdapterInfo, CursorSnapshot, CursorSpeedGuard,
    DisplayInfo, OverlayWindow, WindowWatcher,
};
use serde::{Deserialize, Serialize};
use windows::core::{w, Interface, BSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, POINT, RECT};
use windows::Win32::Graphics::Direct3D11::{ID3D11ShaderResourceView, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R16G16B16A16_FLOAT};
use windows::Win32::Graphics::Dxgi::IDXGIOutput1;
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_WRITE,
    REG_OPTION_NON_VOLATILE, REG_SZ,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::UI::HiDpi::{
    SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetClientRect, GetCursorInfo, GetClipCursor, GetSystemMetrics, PeekMessageW, SetWindowPos,
    TranslateMessage, CURSORINFO, CURSOR_SHOWING, MSG, PM_REMOVE, SM_CXVIRTUALSCREEN,
    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE,
    SWP_NOZORDER,
};

/// Live snapshot of engine status, target window state, dimensions, and FPS counters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EngineTelemetry {
    pub status: i32,
    pub error_code: i32,
    pub target_hwnd: isize,
    pub target_title: String,
    pub foreground_title: String,
    pub input_width: u32,
    pub input_height: u32,
    pub output_width: u32,
    pub output_height: u32,
    pub scale_factor: f32,
    pub captured_fps: f32,
    pub presented_fps: f32,
    #[serde(default)]
    pub presented_1pct_low_fps: f32,
    #[serde(default)]
    pub cap_ms: f32,
    #[serde(default)]
    pub gpu_ms: f32,
    #[serde(default)]
    pub gpu_ms_min: f32,
    #[serde(default)]
    pub gpu_ms_max: f32,
    #[serde(default)]
    pub gpu_timing_generation: u64,
    #[serde(default)]
    pub gpu_stage_ms: [Option<f32>;3],
    #[serde(default)]
    pub present_ms: f32,
    pub queue_depth: usize,
    pub active_profile_id: String,
    #[serde(default)]
    pub requested_capture_api: String,
    #[serde(default)]
    pub actual_capture_api: String,
    #[serde(default)]
    pub capture_fallback_reason: String,
    #[serde(default)]
    pub dxgi_accumulated_frames: u64,
    #[serde(default)]
    pub output_id: u64,
    #[serde(default)]
    pub source_id: i64,
    #[serde(default)]
    pub output_target_sec: f64,
    #[serde(default)]
    pub output_source_sec: f64,
    #[serde(default)]
    pub output_alpha: f32,
    #[serde(default)]
    pub expired_output_slots: u64,
    #[serde(default)]
    pub latency_wait_timeouts: u64,
    #[serde(default)]
    pub latency_wait_ms: f32,
    #[serde(default)]
    pub submitted_outputs: u64,
    #[serde(default)]
    pub dropped_outputs: u64,
    #[serde(default)]
    pub last_output_drop_reason: String,
    #[serde(default)]
    pub occluded_outputs: u64,
    #[serde(default)]
    pub processing_backend: String,
    #[serde(default)]
    pub cpu_ms: f32,
}

impl Default for EngineTelemetry {
    fn default() -> Self {
        Self {
            status: StatusCode::Disabled as i32,
            error_code: ErrorCode::NoCode as i32,
            target_hwnd: 0,
            target_title: String::new(),
            foreground_title: String::new(),
            input_width: 0,
            input_height: 0,
            output_width: 0,
            output_height: 0,
            scale_factor: 1.0,
            captured_fps: 0.0,
            presented_fps: 0.0,
            presented_1pct_low_fps: 0.0,
            cap_ms: 0.0,
            gpu_ms: 0.0, gpu_ms_min: 0.0, gpu_ms_max: 0.0, gpu_timing_generation: 0, gpu_stage_ms: [None;3],
            present_ms: 0.0,
            queue_depth: 0,
            active_profile_id: "default".to_owned(),
            requested_capture_api: String::new(), actual_capture_api: String::new(), capture_fallback_reason: String::new(),
            dxgi_accumulated_frames: 0, output_id: 0, source_id: 0, output_target_sec: 0.0, output_source_sec: 0.0,
            output_alpha: 1.0, expired_output_slots: 0, latency_wait_timeouts: 0, latency_wait_ms: 0.0,
            submitted_outputs: 0, occluded_outputs: 0, dropped_outputs: 0, last_output_drop_reason: String::new(),
            processing_backend: "D3D11 GPU".into(), cpu_ms: 0.0,
        }
    }
}

/// Enumerates hardware DXGI adapters for UI selection.
pub fn get_adapters_info() -> Vec<AdapterInfo> {
    enumerate_adapters(false).unwrap_or_default()
}

/// Enumerates DXGI display outputs for UI selection.
pub fn get_displays_info() -> Vec<DisplayInfo> {
    enumerate_displays().unwrap_or_default()
}

/// Returns `true` when the current process runs with an elevated administrator token.
pub fn is_admin() -> bool {
    let mut token = HANDLE::default();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.is_err() {
        return false;
    }
    let mut elevation = TOKEN_ELEVATION::default();
    let mut ret_len = 0u32;
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            Some((&mut elevation as *mut TOKEN_ELEVATION).cast()),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut ret_len,
        )
    }
    .is_ok();
    unsafe {
        let _ = CloseHandle(token);
    }
    ok && elevation.TokenIsElevated != 0
}

/// Writes Windows per-app DirectX GPU preference (`0 = System`, `1 = Power saving`, `2 = High perf`).
pub fn set_windows_gpu_preference(gpu_preference: i32) -> bool {
    let Ok(exe_path) = std::env::current_exe() else {
        return false;
    };
    let mut exe_w: Vec<u16> = exe_path.to_string_lossy().encode_utf16().collect();
    exe_w.push(0);

    let val_str = format!("GpuPreference={};", gpu_preference.clamp(0, 2));
    let mut val_w: Vec<u16> = val_str.encode_utf16().collect();
    val_w.push(0);

    let mut hkey = HKEY::default();
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\DirectX\UserGpuPreferences"),
            None,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut hkey,
            None,
        )
    };
    if status.is_err() {
        return false;
    }
    let bytes: &[u8] =
        unsafe { std::slice::from_raw_parts(val_w.as_ptr().cast::<u8>(), val_w.len() * 2) };
    let set_res = unsafe {
        RegSetValueExW(
            hkey,
            windows::core::PCWSTR(exe_w.as_ptr()),
            None,
            REG_SZ,
            Some(bytes),
        )
    };
    unsafe {
        let _ = RegCloseKey(hkey);
    }
    set_res.is_ok()
}

/// Clamps `rect` to fit inside `screen`, preserving size when smaller than `screen`.
pub fn clamp_rect_to_screen(rect: RECT, screen: RECT) -> RECT {
    let max_w = (screen.right - screen.left).max(1);
    let max_h = (screen.bottom - screen.top).max(1);
    let w = (rect.right - rect.left).clamp(1, max_w);
    let h = (rect.bottom - rect.top).clamp(1, max_h);
    let left = rect.left.clamp(screen.left, screen.right - w);
    let top = rect.top.clamp(screen.top, screen.bottom - h);
    RECT {
        left,
        top,
        right: left + w,
        bottom: top + h,
    }
}

#[derive(Clone)]
struct SessionReporter {
    callback: Option<StatusCallback>,
    telemetry: Arc<Mutex<EngineTelemetry>>,
}

impl SessionReporter {
    fn emit_state(
        &self,
        status: StatusCode,
        hwnd: isize,
        input_width: i32,
        input_height: i32,
        output_width: i32,
        output_height: i32,
        resized: bool,
        scale_factor: f32,
        error_code: ErrorCode,
    ) {
        {
            let mut t = self.telemetry.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            t.status = status as i32;
            t.error_code = error_code as i32;
            t.target_hwnd = hwnd;
            t.input_width = input_width.max(0) as u32;
            t.input_height = input_height.max(0) as u32;
            t.output_width = output_width.max(0) as u32;
            t.output_height = output_height.max(0) as u32;
            t.scale_factor = scale_factor;
            if status == StatusCode::Disabled {
                t.captured_fps = 0.0;
                t.presented_fps = 0.0;
                t.queue_depth = 0;
            }
        }
        let ev = StatusEvent {
            status,
            hwnd,
            input_width,
            input_height,
            output_width,
            output_height,
            resized,
            scale_factor,
            error_code,
        };
        unsafe {
            ev.emit(self.callback);
        }
    }
}

struct ActiveSession {
    stop_flag: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    hwnd: isize,
}

/// Safe Rust engine runtime coordinating capture, GPU compute, overlay, and pacing.
pub struct EngineRuntime {
    callback: Option<StatusCallback>,
    profile: ScalingProfile,
    config: EngineConfig,
    live_settings: Arc<Mutex<(u64, ScalingProfile, EngineConfig)>>,
    telemetry: Arc<Mutex<EngineTelemetry>>,
    active_profile_id: String,
    session: Option<ActiveSession>,
    initialized: bool,
}

impl EngineRuntime {
    pub fn new() -> Self {
        let config = Self::load_default_config();
        let profile = ScalingProfile::default();
        let live_settings = Arc::new(Mutex::new((0u64, profile.clone(), config.clone())));
        Self {
            callback: None,
            profile,
            config,
            live_settings,
            telemetry: Arc::new(Mutex::new(EngineTelemetry::default())),
            active_profile_id: "default".to_owned(),

            session: None,
            initialized: false,
        }
    }

    fn load_default_config() -> EngineConfig {
        if let Ok(s) = std::fs::read_to_string("config.ini") {
            return EngineConfig::parse(&s);
        }
        if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
            let path = std::path::PathBuf::from(local_app_data)
                .join("FlowScale")
                .join("config.ini");
            if let Ok(s) = std::fs::read_to_string(path) {
                return EngineConfig::parse(&s);
            }
        }
        EngineConfig::default()
    }

    pub fn init(&mut self, callback: Option<StatusCallback>) -> bool {
        self.callback = callback;
        self.initialized = true;
        true
    }

    pub fn uninit(&mut self) {
        self.stop_session();
        self.initialized = false;
        self.callback = None;
    }

    pub fn apply_settings(&mut self, profile: ScalingProfile) {
        self.profile = profile.clone();
        {
            let mut live = self.live_settings.lock().unwrap();
            live.0 = live.0.wrapping_add(1);
            live.1 = profile;
        }
        let mut t = self.telemetry.lock().unwrap();
        t.scale_factor = self.profile.scale_factor;
    }

    /// Replaces the active runtime `config.ini` settings for active and subsequent sessions.
    pub fn apply_engine_config(&mut self, config: EngineConfig) {
        let mut config = config;
        config.lsfg.cpu_vulkan = self.config.lsfg.cpu_vulkan;
        self.config = config.clone();
        let mut live = self.live_settings.lock().unwrap();
        live.0 = live.0.wrapping_add(1);
        live.2 = config;
    }

    /// Returns a thread-safe snapshot of current engine telemetry and counters.
    pub fn telemetry(&self) -> EngineTelemetry {
        self.telemetry.lock().unwrap().clone()
    }

    /// Updates the active profile identifier reported in telemetry.
    pub fn set_active_profile_id(&mut self, id: impl Into<String>) {
        let id = id.into();
        self.active_profile_id = id.clone();
        self.telemetry.lock().unwrap().active_profile_id = id;
    }

    /// Applies a full UI [`GameProfile`] and updates active profile telemetry.
    pub fn apply_game_profile(&mut self, game: &GameProfile) {
        self.set_active_profile_id(game.id.clone());
        self.config.lsfg.cpu_vulkan = game.cpu_vulkan;
        self.apply_engine_config(self.config.clone());
        self.apply_settings(game.to_scaling_profile());
    }

    pub fn profile(&self) -> &ScalingProfile {
        &self.profile
    }

    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    pub fn is_active(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|s| !s.finished.load(Ordering::Acquire))
    }

    /// Stops any active scaling session, joins its worker thread, and restores window bounds if resized.
    pub fn stop_session(&mut self) {
        if let Some(mut sess) = self.session.take() {
            let was_running = !sess.finished.load(Ordering::Acquire);
            sess.stop_flag.store(true, Ordering::SeqCst);
            if let Some(handle) = sess.handle.take() {
                if handle.join().is_err() {
                    self.emit_status(StatusCode::Disabled,sess.hwnd,0,0,0,0,false,1.0,ErrorCode::CaptureFailed);
                }
            }
            if was_running && self.telemetry().status != StatusCode::Disabled as i32 {
                self.emit_status(
                    StatusCode::Disabled,
                    sess.hwnd,
                    0,
                    0,
                    0,
                    0,
                    false,
                    1.0,
                    ErrorCode::NoCode,
                );
            }
        }
    }

    /// Activates or toggles off a live scaling session for `hwnd_raw` (0 = foreground window).
    pub fn activate(&mut self, hwnd_raw: isize) -> bool {
        if !self.initialized {
            return false;
        }

        // Toggle off if already running; otherwise reap any finished session whose target closed.
        if self.is_active() {
            self.stop_session();
            return false;
        }
        if self.session.is_some() {
            self.stop_session();
        }
        let _dpi = PhysicalWindowCoordinates::enter();

        let target_raw = if hwnd_raw == 0 {
            get_foreground_window().0 as isize
        } else {
            hwnd_raw
        };
        if target_raw == 0 {
            self.emit_status(
                StatusCode::Disabled,
                0,
                0,
                0,
                0,
                0,
                false,
                self.profile.scale_factor,
                ErrorCode::CaptureFailed,
            );
            return false;
        }

        let hwnd = HWND(target_raw as *mut std::ffi::c_void);
        let mut restored_rect = None;
        let mut resized = false;

        if self.profile.resize_before_scale && self.profile.scaling_mode == ScalingMode::Custom {
            match resize_window_for_scale(hwnd, self.profile.scale_factor) {
                Ok((prev_rect, _new_rect)) => {
                    restored_rect = Some(prev_rect);
                    resized = true;
                }
                Err(_) => {
                    self.emit_status(
                        StatusCode::Disabled,
                        target_raw,
                        0,
                        0,
                        0,
                        0,
                        false,
                        self.profile.scale_factor,
                        ErrorCode::ResizeFailed,
                    );
                    return false;
                }
            }
        }

        let mut client = RECT::default();
        if unsafe { GetClientRect(hwnd, &mut client) }.is_err() {
            if let Some(prev) = restored_rect {
                restore_window_bounds(hwnd, prev);
            }
            self.emit_status(
                StatusCode::Disabled,
                target_raw,
                0,
                0,
                0,
                0,
                false,
                self.profile.scale_factor,
                ErrorCode::CaptureFailed,
            );
            return false;
        }

        let region = match CaptureRegion::from_rect_and_offsets(
            (client.left, client.top, client.right, client.bottom),
            self.profile.capture_offset_left,
            self.profile.capture_offset_top,
            self.profile.capture_offset_right,
            self.profile.capture_offset_bottom,
        ) {
            Ok(r) => r,
            Err(_) => {
                if let Some(prev) = restored_rect {
                    restore_window_bounds(hwnd, prev);
                }
                self.emit_status(
                    StatusCode::Disabled,
                    target_raw,
                    client.right - client.left,
                    client.bottom - client.top,
                    0,
                    0,
                    false,
                    self.profile.scale_factor,
                    ErrorCode::LargeOffset,
                );
                return false;
            }
        };

        let output_rect = resolve_output_rect(hwnd, &self.profile).unwrap_or(RECT {
            left: 0,
            top: 0,
            right: ((region.width as f32) * self.profile.scale_factor).round() as i32,
            bottom: ((region.height as f32) * self.profile.scale_factor).round() as i32,
        });
        let out_w = (output_rect.right - output_rect.left).max(1) as u32;
        let out_h = (output_rect.bottom - output_rect.top).max(1) as u32;

        let (target_title, _) = get_window_title_and_exe(hwnd);
        let (fg_title, _) = get_window_title_and_exe(get_foreground_window());
        {
            let mut t = self.telemetry.lock().unwrap();
            t.target_title = target_title;
            t.foreground_title = fg_title;
            t.active_profile_id = self.active_profile_id.clone();
        }

        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_clone = Arc::clone(&stop_flag);
        let finished = Arc::new(AtomicBool::new(false));
        let finished_clone = Arc::clone(&finished);
        let profile = self.profile.clone();
        let config = self.config.clone();
        let live_settings = Arc::clone(&self.live_settings);
        let reporter = SessionReporter {
            callback: self.callback,
            telemetry: Arc::clone(&self.telemetry),
        };
        let (init_tx, init_rx) = mpsc::channel::<Result<(), String>>();

        let handle = thread::Builder::new()
            .name("flowscale-render".into())
            .spawn(move || {
                run_scaling_session_with_live(
                    target_raw,
                    region,
                    output_rect,
                    profile,
                    config,
                    live_settings,
                    stop_clone,
                    finished_clone,
                    reporter,
                    restored_rect,
                    Some(init_tx),
                );
            })
            .ok();

        let init_ok = handle.is_some()
            && matches!(init_rx.recv_timeout(Duration::from_secs(3)), Ok(Ok(())));
        if !init_ok {
            stop_flag.store(true, Ordering::SeqCst);
            if let Some(handle) = handle {
                let _ = handle.join();
            }
            if let Some(prev) = restored_rect {
                restore_window_bounds(hwnd, prev);
            }
            self.emit_status(
                StatusCode::Disabled,
                target_raw,
                region.width as i32,
                region.height as i32,
                out_w as i32,
                out_h as i32,
                false,
                self.profile.scale_factor,
                ErrorCode::CaptureFailed,
            );
            return false;
        }

        self.emit_status(
            StatusCode::Active,
            target_raw,
            region.width as i32,
            region.height as i32,
            out_w as i32,
            out_h as i32,
            resized,
            self.profile.scale_factor,
            ErrorCode::NoCode,
        );

        self.session = Some(ActiveSession {
            stop_flag,
            finished,
            handle,
            hwnd: target_raw,
        });

        if self
            .session
            .as_ref()
            .is_some_and(|session| session.handle.is_none())
        {
            if let Some(rect) = restored_rect {
                restore_window_bounds(hwnd, rect);
            }
            self.stop_session();
            self.emit_status(
                StatusCode::Disabled,
                target_raw,
                0,
                0,
                0,
                0,
                false,
                self.profile.scale_factor,
                ErrorCode::CaptureFailed,
            );
            return false;
        }
        true
    }

    /// Runs a self-contained GPU compute + pacing benchmark without needing a target game window.
    pub fn run_gpu_benchmark(&self, frames: u32) -> Result<(String, f64), String> {
        let ctx = D3d11Context::new(self.profile.gpu_id).map_err(|e| e.to_string())?;
        let pipeline = ComputePipeline::new(&ctx, 1280, 720).map_err(|e| e.to_string())?;
        let (_in_tex, in_srv, _in_uav) = ctx
            .create_rw_texture(1280, 720, DXGI_FORMAT_B8G8R8A8_UNORM)
            .map_err(|e| e.to_string())?;
        let (_out_tex, _out_srv, out_uav) = ctx
            .create_rw_texture(1920, 1080, DXGI_FORMAT_B8G8R8A8_UNORM)
            .map_err(|e| e.to_string())?;

        let pacer = HybridPacer::new().map_err(|e| e.to_string())?;
        let mut tracker = FrametimeTracker::new(
            self.config.capture.frametime_buffer_size,
            self.config.capture.frametime_buffer_reset_multiplier,
        );
        let mut drain = QueueDrainingController::new(
            self.config.capture.queue_draining_momentum,
            self.profile.queue_target.max(0) as usize,
        );
        let mut sched = InterpolationScheduler::new(
            InterpolationMode::Fixed {
                multiplier: f64::from(self.profile.frame_generation_multiplier.max(1.0)),
            },
            self.config.lsfg.base_framerate_threshold,
            self.config.lsfg.real_timestamp_tolerance,
        );

        let start = pacer.now_sec();
        let mut last_present = Some(start);
        let mut last = start;
        for _ in 0..frames.max(1) {
            pipeline
                .dispatch_upscale(
                    &ctx,
                    self.profile.scaling_type,
                    &in_srv,
                    &out_uav,
                    1280,
                    720,
                    1920,
                    1080,
                    self.profile.sharpness,
                )
                .map_err(|e| e.to_string())?;
            unsafe {
                ctx.context.Flush();
            }
            wait_for_present(
                &pacer,
                &mut last_present,
                1.0 / 60.0,
                SyncMode::Off,
                false,
                60.0,
            );
            let now = pacer.now_sec();
            let dt = (now - last).max(1e-5);
            last = now;
            tracker.record_delta(dt);
            let _ = drain.update(1);
            let _ = sched.plan_interval(dt);
        }
        let elapsed = (pacer.now_sec() - start).max(1e-6);
        Ok((ctx.caps.adapter_name, elapsed))
    }

    /// Creates a temporary overlay + swapchain + FPS overlay to validate presentation end-to-end.
    pub fn smoke_test_overlay_present(&self) -> Result<(), String> {
        let ctx = D3d11Context::new(self.profile.gpu_id).map_err(|e| e.to_string())?;
        let rect = RECT {
            left: 0,
            top: 0,
            right: 320,
            bottom: 240,
        };
        let win = OverlayWindow::create(rect, false, false).map_err(|e| e.to_string())?;
        let sc = OverlaySwapChain::new(
            &ctx,
            win.hwnd(),
            320,
            240,
            self.profile.max_frame_latency.max(1) as u32,
            self.profile.hdr_support,
        )
        .map_err(|e| e.to_string())?;
        let fps = FpsOverlay::new(18.0).map_err(|e| e.to_string())?;
        let bb = sc.back_buffer().map_err(|e| e.to_string())?;
        fps.draw_for_format(&bb, 60, 120, 8.0, 8.0, sc.format)
            .map_err(|e| e.to_string())?;
        sc.present(self.profile.sync_mode)
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn emit_status(
        &self,
        status: StatusCode,
        hwnd: isize,
        input_width: i32,
        input_height: i32,
        output_width: i32,
        output_height: i32,
        resized: bool,
        scale_factor: f32,
        error_code: ErrorCode,
    ) {
        SessionReporter {
            callback: self.callback,
            telemetry: Arc::clone(&self.telemetry),
        }
        .emit_state(
            status,
            hwnd,
            input_width,
            input_height,
            output_width,
            output_height,
            resized,
            scale_factor,
            error_code,
        );
    }
}

impl Default for EngineRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for EngineRuntime {
    fn drop(&mut self) {
        self.stop_session();
    }
}

fn resolve_dxgi_output(
    adapter: &windows::Win32::Graphics::Dxgi::IDXGIAdapter1,
    hwnd: HWND,
) -> Option<IDXGIOutput1> {
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    if monitor.is_invalid() {
        return None;
    }
    for index in 0.. {
        let output = unsafe { adapter.EnumOutputs(index) }.ok()?;
        if unsafe { output.GetDesc() }.is_ok_and(|desc| desc.Monitor == monitor) {
            return output.cast().ok();
        }
    }
    None
}

fn resolve_monitor_rect(hwnd: HWND) -> Option<RECT> {
    let hmon = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    if hmon.is_invalid() {
        return None;
    }
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetMonitorInfoW(hmon, &mut info) }.as_bool() {
        Some(info.rcMonitor)
    } else {
        None
    }
}

fn virtual_screen_rect() -> RECT {
    let left = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
    let top = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
    let w = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) }.max(1);
    let h = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) }.max(1);
    RECT {
        left,
        top,
        right: left + w,
        bottom: top + h,
    }
}

fn resolve_output_rect(hwnd: HWND, profile: &ScalingProfile) -> Option<RECT> {
    let monitor_rect = resolve_monitor_rect(hwnd);
    let selected_display = if profile.display_id >= 0 {
        enumerate_displays()
            .ok()
            .and_then(|ds| ds.into_iter().nth(profile.display_id as usize))
            .map(|d| RECT {
                left: d.left,
                top: d.top,
                right: d.right,
                bottom: d.bottom,
            })
    } else {
        None
    };
    let base = selected_display.or(monitor_rect)?;

    let bounds = if profile.multi_display_mode {
        virtual_screen_rect()
    } else {
        selected_display.or(monitor_rect).unwrap_or(base)
    };
    Some(clamp_rect_to_screen(base, bounds))
}

fn window_client_screen_rect(hwnd: HWND) -> Option<RECT> {
    let mut client = RECT::default();
    unsafe { GetClientRect(hwnd, &mut client).ok()? };
    let mut tl = POINT {
        x: client.left,
        y: client.top,
    };
    if unsafe { !ClientToScreen(hwnd, &mut tl).as_bool() } {
        return None;
    }
    let w = (client.right - client.left).max(1);
    let h = (client.bottom - client.top).max(1);
    Some(RECT {
        left: tl.x,
        top: tl.y,
        right: tl.x + w,
        bottom: tl.y + h,
    })
}

fn rect_size(rect: RECT) -> (u32, u32) {
    (
        (rect.right - rect.left).max(1) as u32,
        (rect.bottom - rect.top).max(1) as u32,
    )
}

fn dxgi_crop(client: RECT, desktop: RECT, region: CaptureRegion) -> Option<CaptureRegion> {
    let x = client.left + region.left;
    let y = client.top + region.top;
    let left = x.max(desktop.left);
    let top = y.max(desktop.top);
    let width = (x + region.width as i32).min(desktop.right) - left;
    let height = (y + region.height as i32).min(desktop.bottom) - top;
    (width > 0 && height > 0).then_some(CaptureRegion {
        left: left - desktop.left,
        top: top - desktop.top,
        width: width.max(0) as u32,
        height: height.max(0) as u32,
    })
}

fn try_start_wgc(
    ctx: &D3d11Context,
    hwnd: HWND,
    region: CaptureRegion,
    cap_capacity: usize,
    hdr: bool,
) -> Option<Box<dyn CaptureBackend>> {
    let mut wgc = if hdr { WgcCapture::new_hdr(ctx.device.clone(), hwnd, region, true, cap_capacity) } else { WgcCapture::new(ctx.device.clone(), hwnd, region, true, cap_capacity) }.ok()?;
    wgc.start().map_err(|error| eprintln!("WGC start failed: {error}")).ok()?;
    eprintln!("WGC effective session properties: {:?}", wgc.wgc_session_status());
    Some(Box::new(wgc))
}

fn try_start_dxgi(
    ctx: &D3d11Context,
    hwnd: HWND,
    client: RECT,
    region: CaptureRegion,
    dedup: bool,
    cap_capacity: usize,
) -> Option<Box<dyn CaptureBackend>> {
    let output1 = resolve_dxgi_output(&ctx.adapter, hwnd)?;
    let desktop = unsafe { output1.GetDesc() }.ok()?.DesktopCoordinates;
    let dxgi_region = dxgi_crop(client, desktop, region)?;
    let mut dxgi = DxgiDuplicationCapture::new(
        ctx.device.clone(),
        ctx.context.clone(),
        output1,
        dxgi_region,
        cap_capacity,
    )
    .ok()?
    .with_dedup(dedup);
    dxgi.start().map_err(|error| eprintln!("DXGI start failed: {error}")).ok()?;
    Some(Box::new(dxgi))
}

fn try_start_gdi(
    ctx: &D3d11Context,
    hwnd: HWND,
    region: CaptureRegion,
    cap_capacity: usize,
) -> Option<Box<dyn CaptureBackend>> {
    let mut gdi = GdiCapture::new(
        ctx.device.clone(),
        ctx.context.clone(),
        hwnd,
        region,
        cap_capacity,
    )
    .ok()?;
    gdi.start().ok()?;
    Some(Box::new(gdi))
}

fn create_capture_backend(
    ctx: &D3d11Context,
    hwnd: HWND,
    client: RECT,
    region: CaptureRegion,
    profile: &ScalingProfile,
    config: &EngineConfig,
    allow_dxgi: bool,
) -> Option<Box<dyn CaptureBackend>> {
    // One frame for the next acquire, plus the requested number of buffered frames.
    let cap_capacity = profile.queue_target.clamp(0, 2) as usize + 1;
    let wgc = || try_start_wgc(ctx, hwnd, region, cap_capacity, profile.hdr_support);
    let gdi = || try_start_gdi(ctx, hwnd, region, cap_capacity);
    if !allow_dxgi || profile.hdr_support {
        return wgc().or_else(gdi);
    }
    let dxgi = || {
        try_start_dxgi(
            ctx,
            hwnd,
            client,
            region,
            config.capture.dedup,
            cap_capacity,
        )
    };
    match profile.capture_api {
        CaptureApi::Gdi => gdi().or_else(wgc).or_else(dxgi),
        CaptureApi::Wgc => wgc().or_else(dxgi).or_else(gdi),
        CaptureApi::Dxgi => dxgi().or_else(wgc).or_else(gdi),
    }
}

fn native_pointer_may_duplicate(cursor_embedded: bool) -> bool {
    cursor_embedded && cursor_showing()
}

/// Whether the system cursor is visible; errors count as visible (the conservative answer).
fn cursor_showing() -> bool {
    let mut cursor = CURSORINFO {
        cbSize: std::mem::size_of::<CURSORINFO>() as u32,
        ..Default::default()
    };
    unsafe { GetCursorInfo(&mut cursor) }.is_err() || cursor.flags.0 & CURSOR_SHOWING.0 != 0
}

fn source_has_embedded_cursor(current: Option<bool>, held: Option<bool>) -> bool {
    current.or(held).unwrap_or(false)
}

fn cursor_clip_restore_target(saved: Option<(RECT,RECT)>, current: Option<RECT>) -> Option<RECT> {
    let (prior,applied)=saved?;
    (current==Some(applied)).then_some(prior)
}

struct CursorClipGuard {
    saved: Option<(RECT, RECT)>,
}

impl CursorClipGuard {
    fn new() -> Self { Self { saved: None } }
    fn apply(&mut self, rect: RECT) {
        let mut current = RECT::default();
        if unsafe { GetClipCursor(&mut current) }.is_err() { return; }
        let prior = cursor_clip_restore_target(self.saved,Some(current)).unwrap_or(current);
        if set_cursor_clip(Some(rect)).is_ok() { self.saved = Some((prior, rect)); }
    }
    fn release(&mut self) {
        if let Some((prior, applied)) = self.saved.take() {
            let mut current = RECT::default();
            let current=unsafe { GetClipCursor(&mut current) }.ok().map(|()|current);
            if let Some(target)=cursor_clip_restore_target(Some((prior,applied)),current) { let _=set_cursor_clip(Some(target)); }
        }
    }
}
impl Drop for CursorClipGuard { fn drop(&mut self) { self.release(); } }

struct SessionFinishedGuard<'a> {
    finished: Arc<AtomicBool>,
    reporter: &'a SessionReporter,
    hwnd: isize,
    restored_rect: Option<RECT>,
}
impl Drop for SessionFinishedGuard<'_> {
    fn drop(&mut self) {
        if let Some(rect) = self.restored_rect { restore_window_bounds(HWND(self.hwnd as *mut _), rect); }
        if thread::panicking() {
            self.reporter.emit_state(StatusCode::Disabled, self.hwnd, 0, 0, 0, 0, false, 1.0, ErrorCode::CaptureFailed);
        }
        self.finished.store(true, Ordering::Release);
    }
}

fn invalidate_render_resources<R,H>(resources: &mut Option<R>, history: &mut Option<H>) -> Option<H> {
    *resources = None;
    history.take()
}

struct PhysicalWindowCoordinates(DPI_AWARENESS_CONTEXT);

impl PhysicalWindowCoordinates {
    fn enter() -> Self {
        Self(unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) })
    }
}

impl Drop for PhysicalWindowCoordinates {
    fn drop(&mut self) {
        if !self.0 .0.is_null() {
            unsafe { SetThreadDpiAwarenessContext(self.0) };
        }
    }
}

fn restore_window_bounds(hwnd: HWND, rect: RECT) {
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            None,
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top,
            SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
        );
    }
}

fn fresh_capture(frame: Option<&CapturedFrame>) -> Option<(&CapturedFrame, &ID3D11Texture2D)> {
    let frame = frame.filter(|frame| !frame.is_duplicate)?;
    Some((frame, frame.texture.as_ref()?))
}

fn capture_is_current(captured_at: f64, resume_at: Option<f64>) -> bool {
    resume_at.is_none_or(|resume_at| captured_at >= resume_at)
}

fn cpu_pacer_target(
    last_present: Option<f64>,
    now: f64,
    interval: f64,
    sync_mode: SyncMode,
    gsync_support: bool,
    refresh_hz: f32,
) -> Option<(f64, f64)> {
    let sync_divisor = match sync_mode {
        // Present(0, 0) submits to DXGI; displayed cadence depends on DWM and is measured separately.
        SyncMode::Off | SyncMode::Default => 0,
        SyncMode::Vsync1 => 1,
        SyncMode::Vsync2 => 2,
        SyncMode::Vsync3 => 3,
        SyncMode::Vsync4 => 4,
    };
    if sync_divisor == 0 {
        let target = present_deadline(last_present, now, interval);
        return Some((target, target));
    }
    let refresh_dt = 1.0 / f64::from(refresh_hz.max(24.0));
    let vsync_dt = (sync_divisor as f64) * refresh_dt;
    // Never stack a zero-headroom QPC sleep before blocking Present(sync_divisor, 0) (which misses
    // 144 Hz VBlanks at 70-72 FPS). Pre-sleep with half-refresh headroom only when slower than VSync.
    let margin = if gsync_support { 1.15 } else { 1.05 };
    if interval > vsync_dt * margin {
        let raw_target = present_deadline(last_present, now, interval.max(refresh_dt));
        Some((raw_target - refresh_dt * 0.45, raw_target))
    } else {
        None
    }
}

fn wait_for_present(
    pacer: &HybridPacer,
    last_present: &mut Option<f64>,
    interval: f64,
    sync_mode: SyncMode,
    gsync_support: bool,
    refresh_hz: f32,
) -> Option<f64> {
    let now = pacer.now_sec();
    if let Some((sleep_until, recorded)) = cpu_pacer_target(
        *last_present,
        now,
        interval,
        sync_mode,
        gsync_support,
        refresh_hz,
    ) {
        pacer.wait_until_sec(sleep_until);
        *last_present = Some(recorded);
        Some(recorded)
    } else {
        *last_present = Some(now);
        None
    }
}

const DXGI_RETRY_DELAY: Duration = Duration::from_secs(2);

/// Whether a session that fell back from a requested DXGI capture after a runtime error should retry it.
fn dxgi_retry_due(
    requested: CaptureApi,
    active: CaptureApi,
    allow_dxgi: bool,
    retry_at: Option<Instant>,
    now: Instant,
    pointer_blocked: bool,
) -> bool {
    requested == CaptureApi::Dxgi
        && active != CaptureApi::Dxgi
        && allow_dxgi
        && !pointer_blocked
        && retry_at.is_some_and(|at| now >= at)
}

fn should_skip_fg_for_gap(
    superseded_count: u32,
    prev_captured_at: Option<f64>,
    captured_at: f64,
    tracked_dt: f64,
) -> bool {
    if superseded_count == 0 {
        return false;
    }
    let Some(prev) = prev_captured_at else {
        return false;
    };
    tracked_dt <= 0.0 || (captured_at - prev) >= tracked_dt * 1.6
}

#[cfg(test)]
fn finalize_last_present(
    last_present: &mut Option<f64>,
    paced_recorded: Option<f64>,
    present_end: f64,
    interval: f64,
    pace_present: bool,
) {
    if !pace_present
        || paced_recorded.is_none_or(|recorded| present_end - recorded >= interval * 0.25)
    {
        *last_present = Some(present_end);
    }
}

struct FgWorkEstimate {
    motion_ms: f32,
    interpolation_ms: f32,
    upscale_ms: f32,
}

impl FgWorkEstimate {
    fn observe(&mut self, sample: GpuTimingSample) {
        if let Some(ms) = sample.motion_ms {
            self.motion_ms = ms;
        }
        if let Some(ms) = sample.interpolation_ms {
            self.interpolation_ms = ms;
        }
        if let Some(ms) = sample.upscale_ms {
            self.upscale_ms = ms;
        }
    }

    fn seconds_for(&self, interpolated: bool, needs_motion: bool) -> f64 {
        let interpolation = if interpolated { self.interpolation_ms } else { 0.0 };
        let motion = if needs_motion { self.motion_ms } else { 0.0 };
        f64::from(self.upscale_ms + interpolation + motion) / 1000.0
    }
}

// Appended per-session trace. The frametime CSV schema remains available to existing tools.
struct OutputTrace(BufWriter<std::fs::File>);

impl OutputTrace {
    fn from_env() -> Option<Self> {
        let path = std::env::var_os("FLOWSCALE_OUTPUT_TRACE")?;
        let file = std::fs::OpenOptions::new().create(true).append(true).open(path).ok()?;
        Some(Self(BufWriter::new(file)))
    }

    fn event(&mut self, event: &str, now: f64, fields: std::fmt::Arguments<'_>) {
        let _ = writeln!(self.0, "{now:.9} {event} {fields}");
    }
}

fn prepare_output(sc: &OverlaySwapChain, pacer: &HybridPacer, timeline: &mut OutputTimeline, reporter: &SessionReporter,
    source_id: i64, source_time: f64, alpha: f32, interval: f64, work_hint_sec: f64,
    trace: Option<&mut OutputTrace>) -> Result<Option<ScheduledOutput>, GpuError> {
    let ready = pacer.now_sec() + work_hint_sec.max(0.0);
    let previous_expired = timeline.expired_slots;
    let Some(ticket) = timeline.schedule(ready, interval, source_id, source_time, alpha) else {
        if let Some(trace) = trace {
            trace.event("schedule_rejected", pacer.now_sec(), format_args!("source={source_id} alpha={alpha:.6}"));
        }
        return Ok(None);
    };
    {
        let mut t = reporter.telemetry.lock().unwrap();
        t.output_id = ticket.output_id; t.source_id = ticket.source_id;
        t.output_target_sec = ticket.deadline; t.output_source_sec = ticket.source_time; t.output_alpha = ticket.alpha;
        t.expired_output_slots = timeline.expired_slots;
        if timeline.expired_slots > previous_expired { t.last_output_drop_reason = "ExpiredDeadline".into(); }
    }
    let wait_start = pacer.now_sec();
    let waited = sc.wait_for_frame_latency(((ticket.deadline - wait_start).max(0.0) * 1000.0).ceil().clamp(1.0, 16.0) as u32);
    let wait_end = pacer.now_sec();
    reporter.telemetry.lock().unwrap().latency_wait_ms = ((wait_end - wait_start) * 1000.0) as f32;
    if let Some(trace) = trace {
        trace.event("wait", wait_end, format_args!(
            "output={} source={} captured={:.9} alpha={:.6} interval={interval:.9} deadline={:.9} ready={ready:.9} start={wait_start:.9} result={waited:?} expired={}",
            ticket.output_id, source_id, source_time, alpha, ticket.deadline,
            timeline.expired_slots - previous_expired,
        ));
    }
    match waited {
        FrameLatencyWait::Signaled => {},
        FrameLatencyWait::Timeout => { let mut t=reporter.telemetry.lock().unwrap(); t.latency_wait_timeouts += 1; t.dropped_outputs += 1; t.last_output_drop_reason="LatencyBackpressure".into(); return Ok(None); },
        FrameLatencyWait::Failed(code) => return Err(GpuError::Dxgi(format!("frame latency wait failed: Win32 {code}"))),
    }

    Ok(Some(ticket))
}

fn draw_and_present(
    ctx: &D3d11Context,
    sc: &OverlaySwapChain,
    pipeline: &ComputePipeline,
    fps_overlay: Option<&FpsOverlay>,
    src_srv: &ID3D11ShaderResourceView,
    in_w: u32,
    in_h: u32,
    out_w: u32,
    out_h: u32,
    dst_rect: [u32; 4],
    client: RECT,
    region: CaptureRegion,
    _refresh_hz: f32,
    profile: &ScalingProfile,
    config: &EngineConfig,
    cap_fps: u32,
    pres_fps: u32,
    pacer: &HybridPacer,
    ticket: ScheduledOutput,
    reporter: &SessionReporter,
    sdr_white_scale: f32,
    trace: Option<&mut OutputTrace>,
) -> Result<Option<(f64, f32)>, GpuError> {
    let render_start = trace.as_ref().map(|_| pacer.now_sec());
    {
        let bb_uav = sc.back_buffer_uav(ctx)?;
        pipeline.dispatch_upscale_ex(
            ctx,
            profile.scaling_type,
            src_srv,
            &bb_uav,
            in_w,
            in_h,
            out_w,
            out_h,
            dst_rect,
            profile.sharpness,
            profile.scaling_subtype.max(0) as u32,
            profile.vrs,
        )?;
    }

    // Queue cursor/FPS overlay before Flush() and HybridPacer sleep so no post-sleep GPU/D2D
    // submission jitter occurs between waking and Present().
    let cursor_snap: Option<(CursorSnapshot, f32, f32, f32)> =
        if profile.scale_cursor && !profile.hide_cursor {
            extract_cursor_rgba().and_then(|snap| {
                let cx_src = snap.screen_x - client.left - region.left;
                let cy_src = snap.screen_y - client.top - region.top;
                if snap.visible
                    && cx_src >= 0
                    && cy_src >= 0
                    && cx_src < region.width as i32
                    && cy_src < region.height as i32
                {
                    let sx = dst_rect[2] as f32 / in_w.max(1) as f32;
                    let sy = dst_rect[3] as f32 / in_h.max(1) as f32;
                    Some((
                        snap,
                        dst_rect[0] as f32 + (cx_src as f32) * sx,
                        dst_rect[1] as f32 + (cy_src as f32) * sy,
                        sx.min(sy).max(0.1),
                    ))
                } else {
                    None
                }
            })
        } else {
            None
        };

    if profile.draw_fps || cursor_snap.is_some() {
        if let (Some(overlay_2d), Ok(bb)) = (fps_overlay, sc.back_buffer()) {
            let fps_arg = profile.draw_fps.then_some((
                cap_fps,
                pres_fps,
                config.fps_counter.offset_x as f32,
                config.fps_counter.offset_y as f32,
            ));
            let cursor_arg = cursor_snap
                .as_ref()
                .map(|(snap, cx, cy, s)| (snap, *cx, *cy, *s));
            let _ = overlay_2d.draw_overlays_with_white(&bb, sc.format, fps_arg, cursor_arg, sdr_white_scale);
        }
    }

    let flush_start = trace.as_ref().map(|_| pacer.now_sec());
    if config.rendering.flush && ticket.deadline > pacer.now_sec() { unsafe { ctx.context.Flush() }; }
    let pace_start = trace.as_ref().map(|_| pacer.now_sec());
    pacer.wait_until_sec(ticket.deadline);

    let present_start = pacer.now_sec();
    let outcome = sc.present_outcome(profile.sync_mode).map_err(|error| match error {
        GpuError::DeviceLost(_) => unsafe { ctx.device.GetDeviceRemovedReason() }
            .err()
            .map_or(error, |reason| GpuError::DeviceLost(reason.to_string())),
        _ => error,
    });
    let present_end = pacer.now_sec();
    if let Some(trace) = trace {
        let result = match &outcome {
            Ok(PresentOutcome::Presented) => "submitted",
            Ok(PresentOutcome::Occluded) => "occluded",
            Err(_) => "error",
        };
        trace.event("present", present_end, format_args!(
            "output={} source={} render_start={:.9} flush_start={:.9} pace_start={:.9} start={present_start:.9} result={result}",
            ticket.output_id, ticket.source_id, render_start.unwrap_or_default(),
            flush_start.unwrap_or_default(), pace_start.unwrap_or_default(),
        ));
    }
    let outcome = outcome?;
    {
        let mut t = reporter.telemetry.lock().unwrap();
        t.submitted_outputs += 1;
        if outcome == PresentOutcome::Occluded { t.occluded_outputs += 1; return Ok(None); }
    }
    Ok(Some((
        present_end,
        ((present_end - present_start).max(0.0) * 1_000.0) as f32,
    )))
}

fn run_scaling_session_with_live(
    hwnd_raw: isize,
    region: CaptureRegion,
    output_rect: RECT,
    mut profile: ScalingProfile,
    mut config: EngineConfig,
    live_settings: Arc<Mutex<(u64, ScalingProfile, EngineConfig)>>,
    stop_flag: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
    reporter: SessionReporter,
    restored_rect: Option<RECT>,
    mut init_tx: Option<mpsc::Sender<Result<(), String>>>,
) {
    let _finished_guard = SessionFinishedGuard { finished, reporter: &reporter, hwnd: hwnd_raw, restored_rect };
    let _dpi = PhysicalWindowCoordinates::enter();
    let result = loop {
        let result = render_loop(
            hwnd_raw,
            region,
            output_rect,
            profile,
            config,
            &live_settings,
            &stop_flag,
            &reporter,
            restored_rect.is_some(),
            init_tx.take(),
        );
        match result {
            Ok(SessionExit::Restart) if !stop_flag.load(Ordering::Acquire) => {
                let settings = live_settings.lock().unwrap();
                profile = settings.1.clone();
                config = settings.2.clone();
            }
            other => break other,
        }
    };
    if let Err(error) = result {
        eprintln!("FlowScale session stopped: {error}");
        let scale_factor = reporter
            .telemetry
            .lock()
            .map(|t| t.scale_factor)
            .unwrap_or(1.0);
        reporter.emit_state(
            StatusCode::Disabled,
            hwnd_raw,
            0,
            0,
            0,
            0,
            false,
            scale_factor,
            ErrorCode::CaptureFailed,
        );
    }
}

fn build_interp_scheduler(
    profile: &ScalingProfile,
    config: &EngineConfig,
    refresh_hz: f32,
) -> (InterpolationMode, InterpolationScheduler) {
    let fixed_multiplier = config
        .lsfg
        .multiplier
        .unwrap_or_else(|| f64::from(profile.effective_lsfg_multiplier(refresh_hz)))
        .max(1.0);
    let interp_mode = if profile.frame_generation_type == FrameGenerationType::Lsfg3
        && profile.frame_generation_mode == Lsfg3Mode::Adaptive as i32
    {
        InterpolationMode::Adaptive {
            target_fps: f64::from(profile.frame_generation_target.max(10.0)),
        }
    } else {
        InterpolationMode::Fixed {
            multiplier: fixed_multiplier,
        }
    };
    let sched = InterpolationScheduler::new(
        interp_mode,
        config.lsfg.base_framerate_threshold,
        config.lsfg.real_timestamp_tolerance,
    );
    (interp_mode, sched)
}

fn pump_window_messages() {
    unsafe {
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

struct CapturedSlot {
    frame: CapturedFrame,
    cursor_embedded: bool,
}

enum CapturePoll {
    Frame(CapturedSlot),
    Empty,
    Error(CaptureError),
}

struct CaptureMailboxState {
    latest: Option<CapturedSlot>,
    recycle_bin: Vec<ID3D11Texture2D>,
    discontinuity: bool,
    queue_depth: usize,
    cursor_embedded: bool,
    last_error: Option<CaptureError>,
    superseded_count: u32,
    accumulated_frames: u64,
}

struct CaptureMailbox {
    state: Mutex<CaptureMailboxState>,
    has_latest: AtomicBool,
    frame_cv: Condvar,
    control_cv: Condvar,
}

impl CaptureMailbox {
    fn new(cursor_embedded: bool) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(CaptureMailboxState {
                latest: None,
                recycle_bin: Vec::new(),
                discontinuity: false,
                queue_depth: 0,
                cursor_embedded,
                last_error: None,
                superseded_count: 0,
                accumulated_frames: 0,
            }),
            has_latest: AtomicBool::new(false),
            frame_cv: Condvar::new(),
            control_cv: Condvar::new(),
        })
    }

    fn take_poll(&self, guard: &mut CaptureMailboxState) -> (CapturePoll, bool, usize, bool, u32) {
        let disc = std::mem::take(&mut guard.discontinuity);
        let qlen = guard.queue_depth;
        let cursor_emb = guard.cursor_embedded;
        let superseded = std::mem::take(&mut guard.superseded_count);
        if let Some(err) = guard.last_error.take() {
            return (CapturePoll::Error(err), disc, qlen, cursor_emb, superseded);
        }
        if let Some(slot) = guard.latest.take() {
            self.has_latest.store(false, Ordering::Release);
            return (CapturePoll::Frame(slot), disc, qlen, cursor_emb, superseded);
        }
        (CapturePoll::Empty, disc, qlen, cursor_emb, superseded)
    }
}

struct CaptureProducer {
    api: CaptureApi,
    recycler: Option<Arc<FrameRingQueue>>,
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    timeout_ms: Arc<AtomicU32>,
    mailbox: Arc<CaptureMailbox>,
    handle: Option<JoinHandle<Box<dyn CaptureBackend>>>,
}

impl CaptureProducer {
    fn spawn(mut capture: Box<dyn CaptureBackend>) -> Self {
        let api = capture.api();
        eprintln!("FlowScale capture backend: {api:?}");
        let recycler = capture.frame_recycler();
        let initial_cursor = capture.cursor_embedded();
        let stop = Arc::new(AtomicBool::new(false));
        let paused = Arc::new(AtomicBool::new(true));
        let timeout_ms = Arc::new(AtomicU32::new(12));
        let mailbox = CaptureMailbox::new(initial_cursor);

        let stop_bg = Arc::clone(&stop);
        let paused_bg = Arc::clone(&paused);
        let timeout_bg = Arc::clone(&timeout_ms);
        let mailbox_bg = Arc::clone(&mailbox);
        let recycler_bg = recycler.clone();

        let handle = thread::Builder::new()
            .name("flowscale-capture-producer".into())
            .spawn(move || {
                while !stop_bg.load(Ordering::Acquire) {
                    {
                        let mut guard = mailbox_bg.state.lock().unwrap();
                        if !guard.recycle_bin.is_empty() {
                            let drained: Vec<_> = guard.recycle_bin.drain(..).collect();
                            drop(guard);
                            for tex in drained {
                                capture.recycle_frame_texture(tex);
                            }
                        }
                    }

                    if paused_bg.load(Ordering::Acquire) {
                        let guard = mailbox_bg.state.lock().unwrap();
                        let _ = mailbox_bg
                            .control_cv
                            .wait_timeout_while(guard, Duration::from_millis(12), |_| {
                                paused_bg.load(Ordering::Acquire)
                                    && !stop_bg.load(Ordering::Acquire)
                            });
                        continue;
                    }

                    let wait_ms = timeout_bg.load(Ordering::Relaxed).clamp(1, 16);
                    match capture.acquire_frame(wait_ms) {
                        Ok(Some(mut frame)) => {
                            let disc = capture.take_discontinuity();
                            let backend_superseded = capture.take_superseded_count();
                            let accumulated_frames = capture.take_accumulated_frames();
                            let cursor_emb = capture.cursor_embedded();
                            let qlen = capture.ready_len();
                            let recycle_tex = {
                                let mut guard = mailbox_bg.state.lock().unwrap();
                                guard.accumulated_frames = guard.accumulated_frames.saturating_add(accumulated_frames);
                            guard.last_error = None;
                                guard.discontinuity |= disc;
                                guard.queue_depth = qlen;
                                guard.cursor_embedded = cursor_emb;
                                guard.superseded_count =
                                    guard.superseded_count.saturating_add(backend_superseded);
                                if !frame.is_duplicate && frame.texture.is_some() {
                                    let superseded = guard.latest.replace(CapturedSlot {
                                        frame,
                                        cursor_embedded: cursor_emb,
                                    });
                                    if superseded.is_some() {
                                        guard.superseded_count =
                                            guard.superseded_count.saturating_add(1);
                                    }
                                    mailbox_bg.has_latest.store(true, Ordering::Release);
                                    mailbox_bg.frame_cv.notify_one();
                                    superseded.and_then(|s| s.frame.texture)
                                } else {
                                    frame.texture.take()
                                }
                            };
                            if let Some(tex) = recycle_tex {
                                if let Some(r) = &recycler_bg {
                                    r.recycle_texture(tex);
                                } else {
                                    capture.recycle_frame_texture(tex);
                                }
                            }
                            if api == CaptureApi::Gdi {
                                thread::sleep(Duration::from_millis(4));
                            }
                        }
                        Ok(None) => {
                            let disc = capture.take_discontinuity();
                            let backend_superseded = capture.take_superseded_count();
                            let accumulated_frames = capture.take_accumulated_frames();
                            let cursor_emb = capture.cursor_embedded();
                            let qlen = capture.ready_len();
                            let mut guard = mailbox_bg.state.lock().unwrap();
                            guard.accumulated_frames = guard.accumulated_frames.saturating_add(accumulated_frames);
                            guard.last_error = None;
                            guard.discontinuity |= disc;
                            guard.queue_depth = qlen;
                            guard.cursor_embedded = cursor_emb;
                            guard.superseded_count =
                                guard.superseded_count.saturating_add(backend_superseded);
                            if api == CaptureApi::Gdi {
                                drop(guard);
                                thread::sleep(Duration::from_millis(4));
                            }
                        }
                        Err(err) => {
                            let transient = matches!(err, CaptureError::DxgiDesktopUnavailable);
                            let mut guard = mailbox_bg.state.lock().unwrap();
                            guard.last_error = Some(err);
                            mailbox_bg.frame_cv.notify_one();
                            if transient {
                                let _ = mailbox_bg.control_cv.wait_timeout_while(
                                    guard,
                                    Duration::from_millis(100),
                                    |_| {
                                        !stop_bg.load(Ordering::Acquire)
                                            && !paused_bg.load(Ordering::Acquire)
                                    },
                                );
                                continue;
                            }
                            break;
                        }
                    }
                }

                let mut guard = mailbox_bg.state.lock().unwrap();
                let latest_tex = guard.latest.take().and_then(|s| s.frame.texture);
                let drained: Vec<_> = guard.recycle_bin.drain(..).collect();
                drop(guard);
                if let Some(tex) = latest_tex {
                    capture.recycle_frame_texture(tex);
                }
                for tex in drained {
                    capture.recycle_frame_texture(tex);
                }
                capture
            })
            .expect("spawn flowscale-capture-producer");

        Self {
            api,
            recycler,
            stop,
            paused,
            timeout_ms,
            mailbox,
            handle: Some(handle),
        }
    }

    fn api(&self) -> CaptureApi {
        self.api
    }

    fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Release);
        self.mailbox.control_cv.notify_all();
    }

    fn set_timeout_ms(&self, ms: u32) {
        self.timeout_ms.store(ms.clamp(1, 16), Ordering::Relaxed);
    }

    fn recycle_texture(&self, texture: ID3D11Texture2D) {
        if let Some(r) = &self.recycler {
            r.recycle_texture(texture);
        } else {
            self.mailbox.state.lock().unwrap().recycle_bin.push(texture);
        }
    }

    fn has_fresh_frame(&self) -> bool {
        self.mailbox.has_latest.load(Ordering::Acquire)
    }

    #[cfg(test)]
    fn wait_for_fresh_or_deadline(&self, target_sec: f64, pacer: &HybridPacer) -> bool {
        if self.has_fresh_frame() {
            return true;
        }
        let remaining = target_sec - pacer.now_sec();
        if remaining <= 0.0 {
            return self.has_fresh_frame();
        }
        let spin_threshold = pacer.spin_threshold_sec();
        if remaining > spin_threshold {
            let coarse_dur = Duration::from_secs_f64(remaining - spin_threshold);
            let mut guard = self.mailbox.state.lock().unwrap();
            if guard.latest.is_none()
                && guard.last_error.is_none()
                && !self.stop.load(Ordering::Acquire)
            {
                let (next_guard, _) = self
                    .mailbox
                    .frame_cv
                    .wait_timeout_while(guard, coarse_dur, |s| {
                        s.latest.is_none()
                            && s.last_error.is_none()
                            && !self.stop.load(Ordering::Acquire)
                    })
                    .unwrap();
                guard = next_guard;
            }
            let has_fresh = guard.latest.is_some();
            let abort = guard.last_error.is_some() || self.stop.load(Ordering::Acquire);
            drop(guard);
            if has_fresh {
                return true;
            }
            if abort {
                return false;
            }
        }
        while pacer.now_sec() < target_sec {
            if self.has_fresh_frame() {
                return true;
            }
            if self.stop.load(Ordering::Acquire) {
                return false;
            }
            std::hint::spin_loop();
        }
        self.has_fresh_frame()
    }

    fn poll_latest(&self) -> (CapturePoll, bool, usize, bool, u32) {
        let mut guard = self.mailbox.state.lock().unwrap();
        self.mailbox.take_poll(&mut guard)
    }

    fn wait_for_latest(&self, max_wait: Duration) -> (CapturePoll, bool, usize, bool, u32) {
        let mut guard = self.mailbox.state.lock().unwrap();
        if guard.latest.is_none() && guard.last_error.is_none() {
            let (next_guard, _) = self
                .mailbox
                .frame_cv
                .wait_timeout_while(guard, max_wait, |s| {
                    s.latest.is_none() && s.last_error.is_none() && !self.stop.load(Ordering::Acquire)
                })
                .unwrap();
            guard = next_guard;
        }
        self.mailbox.take_poll(&mut guard)
    }

    fn poll_or_wait_latest(
        &self,
        wait_if_empty: bool,
        max_wait: Duration,
    ) -> (CapturePoll, bool, usize, bool, u32) {
        let (poll, disc1, qlen1, cursor_emb1, sup1) = self.poll_latest();
        if wait_if_empty && matches!(poll, CapturePoll::Empty) {
            let (poll2, disc2, qlen2, cursor_emb2, sup2) = self.wait_for_latest(max_wait);
            (
                poll2,
                disc1 | disc2,
                qlen2,
                cursor_emb2,
                sup1.saturating_add(sup2),
            )
        } else {
            (poll, disc1, qlen1, cursor_emb1, sup1)
        }
    }

    fn stop(mut self) {
        self.shutdown_inner();
    }

    fn shutdown_inner(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.mailbox.control_cv.notify_all();
        self.mailbox.frame_cv.notify_all();
        if let Some(r) = &self.recycler {
            r.wake_waiter();
        }
        if let Some(handle) = self.handle.take() {
            if let Ok(mut capture) = handle.join() {
                capture.stop();
            }
        }
    }
}

impl Drop for CaptureProducer {
    fn drop(&mut self) {
        self.shutdown_inner();
    }
}

struct FrametimeCsvLogger {
    writer: BufWriter<std::fs::File>,
}

impl FrametimeCsvLogger {
    fn from_env() -> Option<Self> {
        let path = std::env::var_os("FLOWSCALE_FRAMETIME_CSV")?;
        if path.is_empty() {
            return None;
        }
        let file = std::fs::File::create(path).ok()?;
        let mut writer = BufWriter::new(file);
        let _ = writeln!(
            writer,
            "timestamp_sec,present_dt_ms,cap_ms,gpu_ms,present_ms,is_interpolated,alpha,presented_fps,one_pct_low_fps"
        );
        Some(Self { writer })
    }

    fn log(
        &mut self,
        timestamp_sec: f64,
        present_dt_ms: f64,
        cap_ms: f32,
        gpu_ms: f32,
        present_ms: f32,
        is_interpolated: bool,
        alpha: f32,
        presented_fps: f32,
        one_pct_low_fps: f32,
    ) {
        let _ = writeln!(
            self.writer,
            "{timestamp_sec:.6},{present_dt_ms:.3},{cap_ms:.3},{gpu_ms:.3},{present_ms:.3},{},{alpha:.3},{presented_fps:.2},{one_pct_low_fps:.2}",
            u8::from(is_interpolated),
        );
    }
}

#[derive(Debug, PartialEq, Eq)]
enum SessionExit {
    Stopped,
    Restart,
}

fn session_needs_restart(
    profile: &ScalingProfile,
    next: &ScalingProfile,
    config: &EngineConfig,
    next_config: &EngineConfig,
) -> bool {
    config.lsfg.cpu_vulkan != next_config.lsfg.cpu_vulkan
        || profile.gpu_id != next.gpu_id
        || profile.hdr_support != next.hdr_support
        || profile.windowed_mode != next.windowed_mode
        || profile.capture_api != next.capture_api
        || profile.queue_target != next.queue_target
        || profile.capture_offset_left != next.capture_offset_left
        || profile.capture_offset_top != next.capture_offset_top
        || profile.capture_offset_right != next.capture_offset_right
        || profile.capture_offset_bottom != next.capture_offset_bottom
        || config.capture != next_config.capture
}

fn render_loop(
    hwnd_raw: isize,
    mut region: CaptureRegion,
    mut output_rect: RECT,
    mut profile: ScalingProfile,
    mut config: EngineConfig,
    live_settings: &Arc<Mutex<(u64, ScalingProfile, EngineConfig)>>,
    stop_flag: &AtomicBool,
    reporter: &SessionReporter,
    resized: bool,
    mut init_tx: Option<mpsc::Sender<Result<(), String>>>,
) -> Result<SessionExit, String> {
    if config.lsfg.cpu_vulkan {
        return cpu_vulkan::run(hwnd_raw, profile, config, live_settings, stop_flag, reporter, resized, init_tx);
    }
    {
        let mut telemetry = reporter.telemetry.lock().unwrap();
        telemetry.processing_backend = "D3D11 GPU".into();
        telemetry.cpu_ms = 0.0;
        telemetry.last_output_drop_reason.clear();
    }
    let mut notify_init = |res: Result<(), String>| {
        if let Some(tx) = init_tx.take() {
            let _ = tx.send(res);
        }
    };
    let hwnd = HWND(hwnd_raw as *mut std::ffi::c_void);
    let mut client = match window_client_screen_rect(hwnd) {
        Some(c) => c,
        None => {
            notify_init(Err("cannot read target client bounds".into()));
            return Err("cannot read target client bounds".into());
        }
    };
    let requested_region = CaptureRegion::from_rect_and_offsets(
        (0, 0, client.right - client.left, client.bottom - client.top),
        profile.capture_offset_left,
        profile.capture_offset_top,
        profile.capture_offset_right,
        profile.capture_offset_bottom,
    ).map_err(|e| e.to_string())?;
    if region != requested_region {
        region = requested_region;
    }
    output_rect = resolve_output_rect(hwnd, &profile).unwrap_or(output_rect);
    let ctx = match D3d11Context::new(profile.gpu_id) {
        Ok(c) => c,
        Err(e) => {
            notify_init(Err(e.to_string()));
            return Err(e.to_string());
        }
    };
    let (mut out_w, mut out_h) = rect_size(output_rect);
    let overlay = match OverlayWindow::create(output_rect, !profile.windowed_mode, false) {
        Ok(o) => o,
        Err(e) => {
            notify_init(Err(e.to_string()));
            return Err(e.to_string());
        }
    };
    let sc = match OverlaySwapChain::new(
        &ctx,
        overlay.hwnd(),
        out_w,
        out_h,
        profile.max_frame_latency.max(1) as u32,
        profile.hdr_support,
    ) {
        Ok(s) => s,
        Err(e) => {
            notify_init(Err(e.to_string()));
            return Err(e.to_string());
        }
    };
    let resolve_flow_pct = |p: &ScalingProfile, c: &EngineConfig| {
        c.lsfg
            .flow_scale
            .map(|f| {
                if f <= 1.0 {
                    (f * 100.0).round() as i32
                } else {
                    f.round() as i32
                }
            })
            .unwrap_or(p.frame_generation_flow_scale)
            .clamp(25, 100)
    };
    let sdr_white_scale = flowscale_gpu::sdr_white_scale_for_window(overlay.hwnd()).unwrap_or_else(|reason| {
        eprintln!("SDR white-level query unavailable ({reason}); using scRGB reference white 80 nits"); 1.0
    });
    let mut flow_scale_pct = resolve_flow_pct(&profile, &config);
    let mut fps_overlay = FpsOverlay::from_config(&config.fps_counter).ok();
    if profile.hide_cursor { eprintln!("hide_cursor suppresses software/capture pointers; the target's native OS pointer is controlled by that application"); }
    let pacer = match HybridPacer::new() {
        Ok(p) => p,
        Err(e) => {
            notify_init(Err(e.to_string()));
            return Err(e.to_string());
        }
    };
    let allow_dxgi = match overlay.set_exclude_from_capture(true) {
        Ok(()) => true,
        Err(error) => { eprintln!("DXGI disabled: overlay capture exclusion failed: {error}"); false }
    };
    // Runtime DXGI errors (e.g. a loading-screen mode switch) only suspend DXGI until this retry time.
    let mut dxgi_retry_at: Option<Instant> = None;
    // DXGI suspended because the visible pointer would be duplicated: retry only once the game hides it.
    let mut dxgi_pointer_paused = false;
    let initial_capture =
        match create_capture_backend(&ctx, hwnd, client, region, &profile, &config, allow_dxgi) {
            Some(cap) => cap,
            None => {
                notify_init(Err("no capture backend could start".into()));
                return Err("no capture backend could start".into());
            }
        };
    let mut producer = CaptureProducer::spawn(initial_capture);


    let mut refresh_hz = get_refresh_rate_for_rect(
        output_rect.left,
        output_rect.top,
        output_rect.right,
        output_rect.bottom,
    );
    let mut tracker = FrametimeTracker::new(
        config.capture.frametime_buffer_size,
        config.capture.frametime_buffer_reset_multiplier,
    );
    let mut profiler = FrametimeProfiler::new();
    let mut csv_logger = FrametimeCsvLogger::from_env();
    let mut drain = QueueDrainingController::new(
        config.capture.queue_draining_momentum,
        profile.queue_target.clamp(0, 2) as usize,
    );
    let (mut interp_mode, mut sched) = build_interp_scheduler(&profile, &config, refresh_hz);

    let watcher = WindowWatcher::new(hwnd, Some(overlay.hwnd()), overlay.backdrop_hwnd());
    let mut clip_guard = CursorClipGuard::new();
    let mut speed_guard: Option<CursorSpeedGuard> = None;
    let mut search_radius = search_radius_for_fg_size(profile.frame_generation_size);
    let mut previous: Option<(ID3D11ShaderResourceView, ID3D11Texture2D, bool)> = None;
    let mut resources = None;
    let mut input_size = (0, 0);
    let mut fg_work = FgWorkEstimate {
        motion_ms: 1.0,
        interpolation_ms: 0.5,
        upscale_ms: 0.5,
    };
    let mut timeline = {
        let t = reporter.telemetry.lock().unwrap();
        OutputTimeline::continuing_from(t.output_id,t.expired_output_slots)
    };
    let mut source_id = 0i64;
    let mut previous_source_id = 0i64;
    let mut output_trace = OutputTrace::from_env();
    if let Some(trace) = output_trace.as_mut() {
        trace.event("session", pacer.now_sec(), format_args!("backend={:?}", producer.api()));
    }
    let mut cursor_signature = None;
    let mut effective_cursor_scale = None;
    let mut presented_fps = 0.0f32;
    let mut was_paused = false;
    let mut desktop_unavailable = false;
    let mut recovered_frame = None;
    let mut geometry_dirty = true;
    let mut display_changed = false;
    let mut crop_was_invalid = false;
    let mut back_buffer_needs_frame = true;
    let mut resume_at = None;
    // Inspect the latest snapshot even if settings changed while resources were starting.
    let mut seen_settings_gen = None;
    let mut last_fg_hwnd = 0isize;

    notify_init(Ok(()));

    while !stop_flag.load(Ordering::Relaxed) {
        pump_window_messages();
        {
            let guard = live_settings.lock().unwrap();
            if Some(guard.0) != seen_settings_gen {
                seen_settings_gen = Some(guard.0);
                let next_profile = guard.1.clone();
                let next_config = guard.2.clone();
                drop(guard);

                if session_needs_restart(&profile, &next_profile, &config, &next_config) {
                    return Ok(SessionExit::Restart);
                }

                let next_flow_pct = resolve_flow_pct(&next_profile, &next_config);
                if next_flow_pct != flow_scale_pct
                    || next_profile.frame_generation_type != profile.frame_generation_type
                {
                    flow_scale_pct = next_flow_pct;
                    if let Some((_, tex, _)) = invalidate_render_resources(&mut resources, &mut previous) { producer.recycle_texture(tex); }
                    tracker.break_capture_sequence(); sched.reset_phase(); timeline.reset();
                }
                if next_config.fps_counter != config.fps_counter {
                    fps_overlay = FpsOverlay::from_config(&next_config.fps_counter).ok();
                }
                if next_profile.clip_cursor != profile.clip_cursor {
                    geometry_dirty = true;
                    if !next_profile.clip_cursor { clip_guard.release(); }
                }
                if !next_profile.cursor_sensitivity {
                    speed_guard = None;
                }
                if next_profile.display_id != profile.display_id
                    || next_profile.multi_display_mode != profile.multi_display_mode
                {
                    geometry_dirty = true;
                }
                if next_profile.max_frame_latency != profile.max_frame_latency {
                    let _ = sc.set_max_frame_latency(next_profile.max_frame_latency.max(1) as u32);
                }
                search_radius = search_radius_for_fg_size(next_profile.frame_generation_size);
                drain = QueueDrainingController::new(
                    next_config.capture.queue_draining_momentum,
                    next_profile.queue_target.clamp(0, 2) as usize,
                );
                (interp_mode, sched) =
                    build_interp_scheduler(&next_profile, &next_config, refresh_hz);
                profile = next_profile;
                config = next_config;
                back_buffer_needs_frame = true;
            }
        }
        display_changed |= overlay.take_display_change();
        geometry_dirty |= display_changed;

        let state = watcher.snapshot();
        if state.destroyed {
            reporter.emit_state(
                StatusCode::Disabled,
                hwnd_raw,
                0,
                0,
                0,
                0,
                false,
                profile.scale_factor,
                ErrorCode::NoCode,
            );
            break;
        }

        geometry_dirty |= state.location_dirty;
        let retry_dxgi = dxgi_retry_due(
            profile.capture_api,
            producer.api(),
            allow_dxgi,
            dxgi_retry_at,
            Instant::now(),
            dxgi_pointer_paused && cursor_showing(),
        );
        geometry_dirty |= retry_dxgi;
        if !state.focused || state.minimized {
            recovered_frame = None;
            if !was_paused {
                producer.set_paused(true);
                overlay.set_visible(false);
                clip_guard.release();
                speed_guard = None;
                back_buffer_needs_frame = true;
                tracker.break_capture_sequence();
                profiler.break_sequence();
                timeline.reset();
                reporter.emit_state(
                    StatusCode::TempDisabled,
                    hwnd_raw,
                    region.width as i32,
                    region.height as i32,
                    out_w as i32,
                    out_h as i32,
                    resized,
                    profile.scale_factor,
                    ErrorCode::NoCode,
                );
            }
            was_paused = true;
            if state.foreground_hwnd != last_fg_hwnd {
                last_fg_hwnd = state.foreground_hwnd;
                let (title, _) =
                    get_window_title_and_exe(HWND(state.foreground_hwnd as *mut std::ffi::c_void));
                reporter.telemetry.lock().unwrap().foreground_title = title;
            }
            thread::sleep(Duration::from_millis(12));
            continue;
        }

        if !desktop_unavailable && (geometry_dirty || was_paused) {
            let Some(new_client) = window_client_screen_rect(hwnd) else {
                thread::sleep(Duration::from_millis(12));
                continue;
            };
            let new_output = resolve_output_rect(hwnd, &profile).unwrap_or(output_rect);
            let (width, height) = rect_size(new_client);
            let new_region = CaptureRegion::from_rect_and_offsets(
                (0, 0, width as i32, height as i32),
                profile.capture_offset_left,
                profile.capture_offset_top,
                profile.capture_offset_right,
                profile.capture_offset_bottom,
            );
            let Ok(new_region) = new_region else {
                crop_was_invalid = true;
                recovered_frame = None;
                producer.set_paused(true);
                overlay.set_visible(false);
                clip_guard.release();
                speed_guard = None;
                previous = None;
                tracker.break_capture_sequence();
                profiler.break_sequence();
                timeline.reset();
                thread::sleep(Duration::from_millis(12));
                continue;
            };

            let output_changed = new_output != output_rect;
            let restart_capture = crop_was_invalid
                || retry_dxgi
                || new_region != region
                || (producer.api() == CaptureApi::Dxgi
                    && (display_changed || new_client != client || output_changed));
            if restart_capture {
                recovered_frame = None;
                // A geometry restart while DXGI is suspended keeps the fallback and the pending retry.
                let dxgi_ok = allow_dxgi && (retry_dxgi || dxgi_retry_at.is_none());
                if retry_dxgi {
                    dxgi_retry_at = None;
                    dxgi_pointer_paused = false;
                }
                producer.stop();
                let next_cap = create_capture_backend(
                    &ctx, hwnd, new_client, new_region, &profile, &config, dxgi_ok,
                )
                .ok_or("capture backend could not restart for the new target bounds")?;
                producer = CaptureProducer::spawn(next_cap);

                region = new_region;
                previous = None;
                tracker.break_capture_sequence();
                profiler.break_sequence();
                timeline.reset();
                resume_at = None;
                back_buffer_needs_frame = true;
            } else {
                producer.set_paused(false);
            }
            if output_changed || display_changed {
                let (width, height) = rect_size(new_output);
                if (width, height) != (out_w, out_h) {
                    sc.resize(width, height).map_err(|e| e.to_string())?;
                    (out_w, out_h) = (width, height);
                    back_buffer_needs_frame = true;
                }
                overlay.set_bounds(new_output).map_err(|e| e.to_string())?;
                output_rect = new_output;
                refresh_hz = get_refresh_rate_for_rect(
                    output_rect.left,
                    output_rect.top,
                    output_rect.right,
                    output_rect.bottom,
                );
                (interp_mode, sched) = build_interp_scheduler(&profile, &config, refresh_hz);
            }

            if profile.clip_cursor {
                let bounds = if profile.multi_display_mode {
                    virtual_screen_rect()
                } else {
                    output_rect
                };
                clip_guard.apply(clamp_rect_to_screen(new_client, bounds));
            }
            if output_changed {
                speed_guard = None;
            }
            client = new_client;
            geometry_dirty = false;
            display_changed = false;
            crop_was_invalid = false;
            if was_paused && !restart_capture && producer.api() == CaptureApi::Wgc {
                resume_at = Some(pacer.now_sec());
            }
            was_paused = false;
            reporter.emit_state(
                StatusCode::Active,
                hwnd_raw,
                region.width as i32,
                region.height as i32,
                out_w as i32,
                out_h as i32,
                resized,
                profile.scale_factor,
                ErrorCode::NoCode,
            );
        }

        if profile.scale_cursor && !profile.hide_cursor {
            let mut cursor = CURSORINFO { cbSize: std::mem::size_of::<CURSORINFO>() as u32, ..Default::default() };
            if unsafe { GetCursorInfo(&mut cursor) }.is_ok() {
                let next = (cursor.flags.0, cursor.hCursor.0 as usize, cursor.ptScreenPos.x, cursor.ptScreenPos.y);
                back_buffer_needs_frame |= cursor_signature != Some(next);
                cursor_signature = Some(next);
            }
        }
        let capture_wait = tracker.average_frametime_sec() * 1_000.0;
        let timeout_ms = if capture_wait > 0.0 {
            capture_wait.ceil().clamp(4.0, 16.0) as u32
        } else {
            12
        };
        producer.set_timeout_ms(timeout_ms);
        producer.set_paused(false);

        let (poll, disc, queue_depth, mut cursor_embedded, superseded_count) =
            if let Some(frame) = recovered_frame.take() {
                let (_, disc, qlen, cursor_emb, superseded) = producer.poll_latest();
                (
                    CapturePoll::Frame(CapturedSlot {
                        frame,
                        cursor_embedded: cursor_emb,
                    }),
                    disc,
                    qlen,
                    cursor_emb,
                    superseded,
                )
            } else {
                producer.poll_or_wait_latest(!back_buffer_needs_frame, Duration::from_millis(4))
            };

        let frame = match poll {
            CapturePoll::Frame(slot) => {
                cursor_embedded = slot.cursor_embedded;
                if std::mem::take(&mut desktop_unavailable) {
                    geometry_dirty = true;
                    recovered_frame = Some(slot.frame);
                    continue;
                }
                Some(slot.frame)
            }
            CapturePoll::Empty => {
                if std::mem::take(&mut desktop_unavailable) {
                    geometry_dirty = true;
                    continue;
                }
                None
            }
            CapturePoll::Error(CaptureError::DxgiDesktopUnavailable) => {
                if !desktop_unavailable {
                    desktop_unavailable = true;
                    overlay.set_visible(false);
                    clip_guard.release();
                    speed_guard = None;
                    previous = None;
                    tracker.break_capture_sequence();
                    profiler.break_sequence();
                    timeline.reset();
                    reporter.emit_state(
                        StatusCode::TempDisabled,
                        hwnd_raw,
                        region.width as i32,
                        region.height as i32,
                        out_w as i32,
                        out_h as i32,
                        resized,
                        profile.scale_factor,
                        ErrorCode::NoCode,
                    );
                }
                thread::sleep(Duration::from_millis(150));
                continue;
            }
            CapturePoll::Error(error @ CaptureError::DxgiRecoveryFailed(_)) => {
                let current = watcher.snapshot();
                geometry_dirty |= current.location_dirty;
                if current.destroyed || !current.focused || current.minimized {
                    continue;
                }
                dxgi_retry_at = Some(Instant::now() + DXGI_RETRY_DELAY);
                producer.stop();
                let next_cap = create_capture_backend(
                    &ctx, hwnd, client, region, &profile, &config, false,
                )
                .ok_or_else(|| error.to_string())?;
                producer = CaptureProducer::spawn(next_cap);

                previous = None;
                tracker.break_capture_sequence();
                profiler.break_sequence();
                timeline.reset();
                continue;
            }
            CapturePoll::Error(error) => {
                let current = watcher.snapshot();
                if current.destroyed {
                    continue;
                }
                // Runtime capture interruption (e.g. WGC item closed while switching exclusive fullscreen):
                // fall back through remaining backends if target window is still alive.
                reporter.telemetry.lock().unwrap().capture_fallback_reason = error.to_string();
                let failed_api = producer.api();
                producer.stop();
                if failed_api == CaptureApi::Dxgi {
                    dxgi_retry_at = Some(Instant::now() + DXGI_RETRY_DELAY);
                }
                let fallback = if failed_api == CaptureApi::Wgc {
                    (allow_dxgi && dxgi_retry_at.is_none() && !profile.hdr_support).then(|| try_start_dxgi(
                        &ctx,
                        hwnd,
                        client,
                        region,
                        config.capture.dedup,
                        profile.queue_target.clamp(0, 2) as usize + 1,
                    )).flatten()
                    .or_else(|| {
                        try_start_gdi(
                            &ctx,
                            hwnd,
                            region,
                            profile.queue_target.clamp(0, 2) as usize + 1,
                        )
                    })
                } else {
                    try_start_wgc(
                        &ctx,
                        hwnd,
                        region,
                        profile.queue_target.clamp(0, 2) as usize + 1,
                        profile.hdr_support,
                    )
                    .or_else(|| {
                        try_start_gdi(
                            &ctx,
                            hwnd,
                            region,
                            profile.queue_target.clamp(0, 2) as usize + 1,
                        )
                    })
                };
                if let Some(next_cap) = fallback {
                    producer = CaptureProducer::spawn(next_cap);

                    previous = None;
                    tracker.break_capture_sequence();
                    profiler.break_sequence();
                    timeline.reset();
                    continue;
                }
                return Err(error.to_string());
            }
        };
        if frame.is_none() {
            unsafe { ctx.device.GetDeviceRemovedReason() }
                .map_err(|error| GpuError::DeviceLost(error.to_string()).to_string())?;
        }
        if disc {
            previous = None;
            tracker.break_capture_sequence();
            profiler.break_sequence();
            timeline.reset();
        }
        let source_embedded = source_has_embedded_cursor(
            fresh_capture(frame.as_ref()).map(|_| cursor_embedded),
            previous.as_ref().map(|(_, _, embedded)| *embedded),
        );
        if producer.api() == CaptureApi::Dxgi && native_pointer_may_duplicate(source_embedded) {
            if let Some(tex) = frame.and_then(|f| f.texture) {
                producer.recycle_texture(tex);
            }
            dxgi_pointer_paused = true;
            dxgi_retry_at = Some(Instant::now() + DXGI_RETRY_DELAY);
            producer.stop();
            let next_cap =
                create_capture_backend(&ctx, hwnd, client, region, &profile, &config, false)
                    .ok_or(
                    "DXGI may embed the visible pointer and cursor-free capture could not start",
                )?;
            producer = CaptureProducer::spawn(next_cap);

            previous = None;
            tracker.break_capture_sequence();
            profiler.break_sequence();
            timeline.reset();
            continue;
        }
        let fg_requested = profile.frame_generation_type != FrameGenerationType::Off;
        let mut generate = false;
        let mut current: Option<(ID3D11ShaderResourceView, ID3D11Texture2D)> = None;
        if let Some(mut captured_frame) = frame.filter(|f| !f.is_duplicate && f.texture.is_some()) {
            let captured_at = pacer.qpc_seconds(captured_frame.timestamp_qpc);
            if !capture_is_current(captured_at, resume_at)
                || !tracker.is_new_capture_timestamp(captured_at)
            {
                if let Some(tex) = captured_frame.texture.take() {
                    producer.recycle_texture(tex);
                }
                continue;
            }
            resume_at = None;
            source_id = captured_frame.timestamp_qpc;
            let source_format = captured_frame.format().unwrap_or(DXGI_FORMAT_B8G8R8A8_UNORM);
            let mut tex = captured_frame.texture.take().unwrap();
            let internal_format = if profile.hdr_support { DXGI_FORMAT_R16G16B16A16_FLOAT } else { source_format };
            let size = (captured_frame.width, captured_frame.height);
            if size != input_size || resources.is_none() {
                let pipeline =
                    ComputePipeline::new_with_flow_scale(&ctx, size.0, size.1, flow_scale_pct)
                        .map_err(|e| e.to_string())?;
                let interpolation = fg_requested
                    .then(|| ctx.create_rw_texture(size.0, size.1, internal_format))
                    .transpose()
                    .map_err(|e| e.to_string())?;
                resources = Some((pipeline, interpolation));
                input_size = size;
                if let Some((_, old_tex, _)) = previous.take() {
                    producer.recycle_texture(old_tex);
                }
                speed_guard = None;
                tracker.break_capture_sequence();
                profiler.break_sequence();
                timeline.reset();
            }
            let prev_captured_at = tracker.last_timestamp_sec();
            let tracked_dt = tracker.average_frametime_sec();
            let cadence_ok = tracker.record_superseded_timestamp(captured_at, superseded_count);
            let multi_frame_gap = should_skip_fg_for_gap(
                superseded_count,
                prev_captured_at,
                captured_at,
                tracked_dt,
            );
            generate = cadence_ok
                && !multi_frame_gap
                && fg_requested
                && previous
                    .as_ref()
                    .is_some_and(|(_, _, embedded)| *embedded == cursor_embedded);
            if let Some(trace) = output_trace.as_mut() {
                trace.event("capture", pacer.now_sec(), format_args!(
                    "source={source_id} captured={captured_at:.9} previous={} superseded={superseded_count} depth={queue_depth} discontinuity={disc} gap={multi_frame_gap} eligible={generate}",
                    if previous.is_some() { previous_source_id } else { 0 },
                ));
            }
            if profile.hdr_support && source_format != DXGI_FORMAT_R16G16B16A16_FLOAT {
                let input = ctx.create_srv(&tex, source_format).map_err(|e| e.to_string())?;
                let (linear, _, linear_uav) = ctx.create_rw_texture(size.0, size.1, internal_format).map_err(|e| e.to_string())?;
                resources.as_ref().unwrap().0.dispatch_sdr_to_scrgb(&ctx, &input, &linear_uav, size.0, size.1, sdr_white_scale).map_err(|e| e.to_string())?;
                producer.recycle_texture(tex);
                tex = linear;
            }
            let srv = ctx
                .create_srv(&tex, internal_format)
                .map_err(|e| e.to_string())?;
            current = Some((srv, tex));
        }
        let holding = current.is_none();
        let Some(src) = current
            .as_ref()
            .map(|(srv, _)| srv)
            .or_else(|| previous.as_ref().map(|(srv, _, _)| srv))
        else {
            thread::sleep(Duration::from_millis(1));
            continue;
        };
        let Some((pipeline, interpolation)) = resources.as_ref() else { continue; };
        let (in_w, in_h) = input_size;
        let dst = compute_dst_rect(
            in_w,
            in_h,
            out_w,
            out_h,
            profile.scaling_mode,
            profile.scaling_fit_mode,
            profile.scale_factor,
        );
        if holding && !back_buffer_needs_frame {
            continue;
        }
        let cursor_scale = (dst[2] as f32 / in_w.max(1) as f32).min(dst[3] as f32 / in_h.max(1) as f32);
        if effective_cursor_scale != Some(cursor_scale) { speed_guard = None; effective_cursor_scale = Some(cursor_scale); }
        if speed_guard.is_none() && profile.cursor_sensitivity { speed_guard = CursorSpeedGuard::apply(cursor_scale); }

        let capture_dt = tracker.average_frametime_sec();
        let pacing_dt = tracker.pacing_frametime_sec();
        let cap_fps = tracker.estimated_fps() as f32;
        let dt = if pacing_dt > 0.0 {
            pacing_dt
        } else if capture_dt > 0.0 {
            capture_dt
        } else {
            1.0 / 60.0
        };
        let cap_ms = (if capture_dt > 0.0 { capture_dt } else { dt } * 1_000.0) as f32;
        let present =
            |srv: &ID3D11ShaderResourceView,
             fps: f32,
             ticket: ScheduledOutput,
             trace: Option<&mut OutputTrace>| {
                draw_and_present(
                    &ctx,
                    &sc,
                    pipeline,
                    fps_overlay.as_ref(),
                    srv,
                    in_w,
                    in_h,
                    out_w,
                    out_h,
                    dst,
                    client,
                    region,
                    refresh_hz,
                    &profile,
                    &config,
                    cap_fps.round() as u32,
                    fps.round() as u32,
                    &pacer,
                    ticket,
                    reporter,
                    sdr_white_scale,
                    trace,
                )
                .map_err(|e| e.to_string())
            };

        if generate {
            let (_, interp_srv, interp_uav) = interpolation.as_ref().unwrap();
            let (prev, _, _) = previous.as_ref().unwrap();
            let schedule_dt = match interp_mode {
                InterpolationMode::Fixed { .. }
                    if producer.api() == CaptureApi::Wgc && profile.queue_target > 0 =>
                {
                    dt * drain.update(queue_depth).min(1.0)
                }
                InterpolationMode::Fixed { .. } | InterpolationMode::Adaptive { .. } => dt,
            };
            let steps = sched.plan_interval(schedule_dt);
            let mut motion_ready = false;
            if let Some(first) = steps.first() {
                let measured = profiler.average_fps();
                presented_fps = if measured > 0.0 {
                    measured
                } else {
                    (1.0 / first.output_dt_sec) as f32
                };
            }
            let ui_threshold =
                if config.lsfg.ui_detection_rate != 0 && config.lsfg.ui_detection_threshold > 0.0 {
                    config.lsfg.ui_detection_threshold as f32
                } else {
                    0.0
                };
            for step in &steps {
                if stop_flag.load(Ordering::Relaxed) {
                    break;
                }
                pump_window_messages();
                let mid_state = watcher.snapshot();
                if mid_state.destroyed || !mid_state.focused || mid_state.minimized {
                    break;
                }
                let source_time = tracker.last_timestamp_sec().unwrap_or(0.0);
                if producer.has_fresh_frame() && pacer.now_sec()-source_time > dt*1.5 {
                    if let Some(trace) = output_trace.as_mut() {
                        trace.event("drop", pacer.now_sec(), format_args!(
                            "source={source_id} alpha={:.6} reason=SupersededExpiredSource", step.alpha,
                        ));
                    }
                    let mut t=reporter.telemetry.lock().unwrap(); t.dropped_outputs+=1; t.last_output_drop_reason="SupersededExpiredSource".into();
                    continue;
                }
                if let Some(sample) = pipeline.last_gpu_timing_sample() {
                    fg_work.observe(sample);
                }
                let needs_motion = step.is_interpolated && !motion_ready;
                let hint = fg_work.seconds_for(step.is_interpolated, needs_motion);
                let Some(ticket) = prepare_output(&sc,&pacer,&mut timeline,reporter,source_id,source_time,step.alpha,step.output_dt_sec,hint,output_trace.as_mut()).map_err(|e|e.to_string())? else { continue; };
                let fg_start = output_trace.as_ref().map(|_| pacer.now_sec());
                if needs_motion {
                    pipeline.dispatch_motion_est(&ctx,prev,src,in_w,in_h,search_radius).map_err(|e|e.to_string())?;
                    motion_ready=true;
                }
                let interp_start = output_trace.as_ref().map(|_| pacer.now_sec());
                let srv = if step.is_interpolated {
                    pipeline
                        .dispatch_frame_interp_ex(
                            &ctx,
                            prev,
                            src,
                            interp_uav,
                            in_w,
                            in_h,
                            step.alpha,
                            ui_threshold,
                        )
                        .map_err(|e| e.to_string())?;
                    interp_srv
                } else {
                    src
                };
                if let Some(trace) = output_trace.as_mut() {
                    trace.event("fg", pacer.now_sec(), format_args!(
                        "output={} source={} motion={} interpolated={} start={:.9} interp_start={:.9}",
                        ticket.output_id, source_id, needs_motion, step.is_interpolated,
                        fg_start.unwrap_or_default(), interp_start.unwrap_or_default(),
                    ));
                }
                let Some((presented_at, present_ms)) = present(
                    srv,
                    presented_fps,
                    ticket,
                    output_trace.as_mut(),
                )? else { continue; };
                timeline.record_submission(presented_at - f64::from(present_ms) / 1000.0);
                let gpu_ms = pipeline
                    .last_gpu_timings()
                    .map(|(m, i, u)| m + i + u)
                    .unwrap_or(0.0);
                let out_dt_ms = profiler
                    .record_present_timestamp(presented_at, cap_ms, gpu_ms, present_ms)
                    .map_or(0.0, |d| d * 1_000.0);
                let measured = profiler.average_fps();
                if measured > 0.0 {
                    presented_fps = measured;
                }
                if let Some(csv) = csv_logger.as_mut() {
                    csv.log(
                        presented_at,
                        out_dt_ms,
                        cap_ms,
                        gpu_ms,
                        present_ms,
                        step.is_interpolated,
                        step.alpha,
                        presented_fps,
                        profiler.one_percent_low_fps(),
                    );
                }
            }
        } else {
            if !holding {
                sched.reset_phase();
                let measured = profiler.average_fps();
                presented_fps = if measured > 0.0 { measured } else { cap_fps };
            }
            let hint = pipeline.last_gpu_timings().map_or(0.001, |(_,_,u)| f64::from(u)/1000.0);
            let ticket = prepare_output(&sc,&pacer,&mut timeline,reporter,source_id,tracker.last_timestamp_sec().unwrap_or(0.0),1.0,dt,hint,output_trace.as_mut()).map_err(|e|e.to_string())?;
            if let Some((presented_at, present_ms)) = ticket.map(|ticket|present(src,presented_fps,ticket,output_trace.as_mut())).transpose()?.flatten() {
            if fg_requested {
                timeline.record_submission(presented_at - f64::from(present_ms) / 1000.0);
            }
            let gpu_ms = pipeline
                .last_gpu_timings()
                .map(|(m, i, u)| m + i + u)
                .unwrap_or(0.0);
            let out_dt_ms = profiler
                .record_present_timestamp(presented_at, cap_ms, gpu_ms, present_ms)
                .map_or(0.0, |d| d * 1_000.0);
            let measured = profiler.average_fps();
            if measured > 0.0 {
                presented_fps = measured;
            }
            if let Some(csv) = csv_logger.as_mut() {
                csv.log(
                    presented_at,
                    out_dt_ms,
                    cap_ms,
                    gpu_ms,
                    present_ms,
                    false,
                    1.0,
                    presented_fps,
                    profiler.one_percent_low_fps(),
                );
            }
            }
        }
        if back_buffer_needs_frame || previous.is_none() {
            overlay.set_visible(true);
        }
        back_buffer_needs_frame = false;
        if let Some((srv, tex)) = current {
            previous_source_id = source_id;
            if let Some((_, old_tex, _)) = previous.replace((srv, tex, cursor_embedded)) {
                producer.recycle_texture(old_tex);
            }
        }
        if state.foreground_hwnd != last_fg_hwnd {
            last_fg_hwnd = state.foreground_hwnd;
            let (fg_title, _) =
                get_window_title_and_exe(HWND(state.foreground_hwnd as *mut std::ffi::c_void));
            reporter.telemetry.lock().unwrap().foreground_title = fg_title;
        }
        let mut telemetry = reporter.telemetry.lock().unwrap();
        telemetry.input_width = in_w;
        telemetry.input_height = in_h;
        telemetry.captured_fps = cap_fps;
        telemetry.presented_fps = presented_fps;
        telemetry.presented_1pct_low_fps = profiler.one_percent_low_fps();
        telemetry.cap_ms = profiler.cap_ms();
        telemetry.gpu_ms = profiler.gpu_ms();
        (telemetry.gpu_ms_min, telemetry.gpu_ms_max) = profiler.gpu_ms_range();
        if let Some(sample) = pipeline.last_gpu_timing_sample() {
            telemetry.gpu_timing_generation = sample.generation;
            telemetry.gpu_stage_ms = [sample.motion_ms,sample.interpolation_ms,sample.upscale_ms];
        }
        telemetry.present_ms = profiler.present_ms();
        telemetry.queue_depth = queue_depth;
        telemetry.requested_capture_api = format!("{:?}", profile.capture_api);
        telemetry.actual_capture_api = format!("{:?}", producer.api());
        telemetry.dxgi_accumulated_frames += std::mem::take(&mut producer.mailbox.state.lock().unwrap().accumulated_frames);
        if producer.api() == profile.capture_api {
            telemetry.capture_fallback_reason.clear();
        } else if telemetry.capture_fallback_reason.is_empty() {
            telemetry.capture_fallback_reason = if profile.hdr_support { "Requested backend cannot preserve HDR; FP16 WGC preferred, SDR fallback is decoded once".into() }
                else if dxgi_pointer_paused { "DXGI paused while the cursor is visible (menus); resumes when the game hides it".into() }
                else if !allow_dxgi { "DXGI unavailable: capture exclusion or native-pointer policy requires a cursor-free backend".into() }
                else if resolve_dxgi_output(&ctx.adapter, hwnd).and_then(|output| unsafe { output.GetDesc() }.ok()).is_some_and(|desc| desc.Rotation.0 > 1) { "Rotated DXGI output requires upright WGC capture".into() }
                else { "Requested capture backend could not start; using an available fallback".into() };
        }
    }

    if let Some((_, old_tex, _)) = previous.take() {
        producer.recycle_texture(old_tex);
    }
    producer.stop();
    Ok(SessionExit::Stopped)
}

fn global_runtime() -> &'static Arc<Mutex<EngineRuntime>> {
    static INSTANCE: OnceLock<Arc<Mutex<EngineRuntime>>> = OnceLock::new();
    INSTANCE.get_or_init(|| Arc::new(Mutex::new(EngineRuntime::new())))
}

/// # Safety
/// The callback, if provided, must remain callable from the render thread until UnInit returns; it must not reenter the engine.
#[no_mangle]
pub unsafe extern "C" fn Init(callback: Option<StatusCallback>) -> u8 {
    let mut rt = global_runtime().lock().unwrap();
    u8::from(rt.init(callback))
}

/// # Safety
/// Call only after initialization; callbacks must not reenter this function while a session is being joined.
#[no_mangle]
pub unsafe extern "C" fn UnInit() {
    let mut rt = global_runtime().lock().unwrap();
    rt.uninit();
}

/// # Safety
/// The target must be a valid live HWND (or zero for the foreground window); callbacks must not reenter the engine.
#[no_mangle]
pub unsafe extern "C" fn Activate(hwnd: isize) -> u8 {
    let mut rt = global_runtime().lock().unwrap();
    u8::from(rt.activate(hwnd))
}

/// # Safety
/// Any registered callback must remain valid and must not reenter the engine during settings changes.
#[no_mangle]
pub unsafe extern "C" fn ApplySettings(
    scaling_mode: i32,
    scaling_fit_mode: i32,
    scaling_type: i32,
    scaling_subtype: i32,
    scale_factor: f32,
    resize_before_scale: u8,
    sharpness: i32,
    vrs: u8,
    frame_generation_type: i32,
    frame_generation_size: i32,
    frame_generation_mode: i32,
    frame_generation_multiplier: f32,
    frame_generation_target: f32,
    frame_generation_flow_scale: i32,
    clip_cursor: u8,
    cursor_sensitivity: u8,
    hide_cursor: u8,
    scale_cursor: u8,
    sync_mode: i32,
    max_frame_latency: i32,
    gsync_support: u8,
    hdr_support: u8,
    capture_api: i32,
    queue_target: i32,
    draw_fps: u8,
    gpu_id: i32,
    display_id: i32,
    capture_offset_left: i32,
    capture_offset_top: i32,
    capture_offset_right: i32,
    capture_offset_bottom: i32,
    multi_display_mode: u8,
    setup_phase: i32,
) {
    let profile = ScalingProfile::from_abi_args(
        scaling_mode,
        scaling_fit_mode,
        scaling_type,
        scaling_subtype,
        scale_factor,
        resize_before_scale,
        sharpness,
        vrs,
        frame_generation_type,
        frame_generation_size,
        frame_generation_mode,
        frame_generation_multiplier,
        frame_generation_target,
        frame_generation_flow_scale,
        clip_cursor,
        cursor_sensitivity,
        hide_cursor,
        scale_cursor,
        sync_mode,
        max_frame_latency,
        gsync_support,
        hdr_support,
        capture_api,
        queue_target,
        draw_fps,
        gpu_id,
        display_id,
        capture_offset_left,
        capture_offset_top,
        capture_offset_right,
        capture_offset_bottom,
        multi_display_mode,
        setup_phase,
    );
    let mut rt = global_runtime().lock().unwrap();
    rt.apply_settings(profile);
}

/// # Safety
/// The returned HWND is borrowed and can become invalid at any time; validate it before use.
#[no_mangle]
pub unsafe extern "C" fn GetForegroundWindowEx() -> isize {
    get_foreground_window().0 as isize
}

/// # Safety
/// The caller owns the returned BSTR and must release it exactly once with SysFreeString.
#[no_mangle]
pub unsafe extern "C" fn GetAdapterNames() -> *mut u16 {
    let joined = get_adapter_names_joined();
    BSTR::from(joined.as_str()).into_raw() as *mut u16
}

/// # Safety
/// The caller owns the returned BSTR and must release it exactly once with SysFreeString.
#[no_mangle]
pub unsafe extern "C" fn GetDisplayNames() -> *mut u16 {
    let joined = get_display_names_joined();
    BSTR::from(joined.as_str()).into_raw() as *mut u16
}

/// # Safety
/// This function has no additional caller safety requirements.
#[no_mangle]
pub unsafe extern "C" fn GetDwmRefreshRate() -> f32 {
    get_dwm_refresh_rate()
}

/// # Safety
/// This function has no additional caller safety requirements.
#[no_mangle]
pub unsafe extern "C" fn IsWindowsBuildAtLeast(build: u32) -> u8 {
    u8::from(is_windows_build_at_least(build))
}

/// # Safety
/// This function has no additional caller safety requirements.
#[no_mangle]
pub unsafe extern "C" fn SetWindowsSettings(gpu_preference: i32) -> u8 {
    u8::from(set_windows_gpu_preference(gpu_preference))
}

/// # Safety
/// This function has no additional caller safety requirements.
#[no_mangle]
pub unsafe extern "C" fn SetDriverSettings() -> u8 {
    // Clean-room no-op: vendor driver profile overrides are optional.
    1
}

#[cfg(test)]
mod tests {
    #[test]
    fn profile_processing_choice_survives_global_config_updates() {
        let mut runtime = EngineRuntime::new();
        runtime.apply_game_profile(&GameProfile { cpu_vulkan: true, ..GameProfile::default() });
        runtime.apply_engine_config(EngineConfig::default());
        assert!(runtime.config().lsfg.cpu_vulkan);
        runtime.apply_game_profile(&GameProfile::default());
        assert!(!runtime.config().lsfg.cpu_vulkan);
    }
    use super::*;
    use std::sync::atomic::{AtomicI32, Ordering};
    use windows::Win32::Foundation::{SysFreeString, LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassExW, WINDOW_EX_STYLE,
        WNDCLASSEXW, WS_POPUP,
    };

    static LAST_STATUS: AtomicI32 = AtomicI32::new(-1);

    #[test]
    fn stage2_fresh_capture_never_unpaces_an_old_real_output() {
        let mut clock = OutputTimeline::default();
        let a = clock.schedule(1.0, 1.0/144.0, 1, 1.0, 0.5).unwrap();
        let b = clock.schedule(a.deadline + 0.0003, 1.0/144.0, 1, 1.0, 1.0).unwrap();
        assert!(b.deadline - a.deadline >= 1.0/144.0 - 1e-9);
    }

    #[test]
    fn stage2_pipeline_invalidation_discards_incompatible_history_on_empty_polls() {
        for held_id in [None,Some(42)] {
            let mut resources=Some((50,FrameGenerationType::Lsfg3));
            let mut held=held_id;
            assert_eq!(invalidate_render_resources(&mut resources,&mut held),held_id);
            for _ in 0..3 { assert!(resources.as_ref().zip(held.as_ref()).is_none()); }
            resources=Some((75,FrameGenerationType::Off));
            assert!(resources.as_ref().zip(held.as_ref()).is_none());
            held=Some(43);
            assert!(resources.as_ref().zip(held.as_ref()).is_some());
            assert_eq!(invalidate_render_resources(&mut resources,&mut held),Some(43));
        }
    }

    #[test]
    fn stage2_cursor_clip_restores_only_its_own_applied_rectangle() {
        let prior=RECT{left:0,top:0,right:100,bottom:100};
        let applied=RECT{left:10,top:10,right:90,bottom:90};
        assert_eq!(cursor_clip_restore_target(Some((prior,applied)),Some(applied)),Some(prior));
        assert_eq!(cursor_clip_restore_target(Some((prior,applied)),Some(prior)),None);
        assert_eq!(cursor_clip_restore_target(Some((prior,applied)),None),None);
    }

    #[test]
    fn stage2_worker_unwind_publishes_failure_and_finishes() {
        let finished = Arc::new(AtomicBool::new(false));
        let reporter = SessionReporter { callback: None, telemetry: Arc::new(Mutex::new(EngineTelemetry { status: StatusCode::Active as i32, ..Default::default() })) };
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = SessionFinishedGuard { finished: finished.clone(), reporter: &reporter, hwnd: 0, restored_rect: None };
            panic!("injected render worker panic");
        }));
        assert!(finished.load(Ordering::Acquire));
        assert_eq!(reporter.telemetry.lock().unwrap().status, StatusCode::Disabled as i32);
    }

    unsafe extern "C" fn test_cb(
        status: i32,
        _hwnd: isize,
        _in_w: i32,
        _in_h: i32,
        _out_w: i32,
        _out_h: i32,
        _resized: u8,
        _scale_factor: f32,
        _error_code: i32,
    ) {
        LAST_STATUS.store(status, Ordering::SeqCst);
    }

    #[test]
    fn failed_render_session_publishes_error_and_finishes() {
        let telemetry = Arc::new(Mutex::new(EngineTelemetry {
            status: StatusCode::Active as i32,
            presented_fps: 120.0,
            ..Default::default()
        }));
        let finished = Arc::new(AtomicBool::new(false));
        let live_settings = Arc::new(Mutex::new((
            0u64,
            ScalingProfile::default(),
            EngineConfig::default(),
        )));
        run_scaling_session_with_live(
            0,
            CaptureRegion {
                left: 0,
                top: 0,
                width: 64,
                height: 64,
            },
            RECT::default(),
            ScalingProfile::default(),
            EngineConfig::default(),
            live_settings,
            Arc::new(AtomicBool::new(false)),
            Arc::clone(&finished),
            SessionReporter {
                callback: None,
                telemetry: Arc::clone(&telemetry),
            },
            None,
            None,
        );
        assert!(finished.load(Ordering::Acquire));
        let telemetry = telemetry.lock().unwrap();
        assert_eq!(telemetry.status, StatusCode::Disabled as i32);
        assert_eq!(telemetry.error_code, ErrorCode::CaptureFailed as i32);
        assert_eq!(telemetry.presented_fps, 0.0);
    }

    #[test]
    fn pmv2_scope_restores_a_dpi_unaware_host_thread() {
        use windows::Win32::UI::HiDpi::{
            AreDpiAwarenessContextsEqual, GetDpiForWindow, GetThreadDpiAwarenessContext,
            DPI_AWARENESS_CONTEXT_UNAWARE,
        };

        unsafe {
            let original = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_UNAWARE);
            assert!(!original.0.is_null());
            let _restore = PhysicalWindowCoordinates(original);
            {
                let _scope = PhysicalWindowCoordinates::enter();
                assert!(AreDpiAwarenessContextsEqual(
                    GetThreadDpiAwarenessContext(),
                    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
                )
                .as_bool());
                let rect = RECT {
                    left: 120,
                    top: 100,
                    right: 440,
                    bottom: 340,
                };
                let window = OverlayWindow::create(rect, false, false).unwrap();
                let dpi = GetDpiForWindow(window.hwnd());
                assert!(dpi >= 96);
                let client = window_client_screen_rect(window.hwnd()).unwrap();
                assert_eq!(client, rect);
                eprintln!("PMv2 client coordinates verified against a hidden window at {dpi} DPI");
            }
            assert!(AreDpiAwarenessContextsEqual(
                GetThreadDpiAwarenessContext(),
                DPI_AWARENESS_CONTEXT_UNAWARE,
            )
            .as_bool());
        }
    }

    #[test]
    fn pre_resume_wgc_frames_are_rejected_even_when_popped_after_resume() {
        assert!(!capture_is_current(1.990, Some(2.000)));
        assert!(capture_is_current(2.000, Some(2.000)));
        assert!(capture_is_current(2.010, Some(2.000)));
        assert!(capture_is_current(1.990, None));
    }

    #[test]
    fn cursor_fallback_uses_the_frame_being_presented() {
        assert!(source_has_embedded_cursor(None, Some(true)));
        assert!(!source_has_embedded_cursor(Some(false), Some(true)));
        assert!(source_has_embedded_cursor(Some(true), Some(false)));
        assert!(!source_has_embedded_cursor(None, None));
    }

    #[test]
    fn fg_work_estimate_keeps_motion_cost_across_real_outputs() {
        let mut estimate = FgWorkEstimate {
            motion_ms: 1.0,
            interpolation_ms: 0.5,
            upscale_ms: 0.5,
        };
        estimate.observe(GpuTimingSample {
            generation: 1,
            motion_ms: Some(1.5),
            interpolation_ms: Some(0.5),
            upscale_ms: Some(0.5),
        });
        estimate.observe(GpuTimingSample {
            generation: 2,
            motion_ms: None,
            interpolation_ms: None,
            upscale_ms: Some(1.0),
        });
        assert_eq!(estimate.seconds_for(true, true), 0.003);
        assert_eq!(estimate.seconds_for(true, false), 0.0015);
        assert_eq!(estimate.seconds_for(false, false), 0.001);
    }

    #[test]
    fn dxgi_profile_can_capture_on_a_cursor_free_fallback() {
        let Ok(ctx) = D3d11Context::new(0) else {
            eprintln!("Cursor fallback fixture requires a D3D11 adapter");
            return;
        };
        let _dpi = PhysicalWindowCoordinates::enter();
        let window = OverlayWindow::create(
            RECT {
                left: -30_000,
                top: -30_000,
                right: -29_936,
                bottom: -29_936,
            },
            false,
            true,
        )
        .unwrap();
        window.set_exclude_from_capture(false).unwrap();
        let hwnd = window.hwnd();
        // OverlayWindow uses WS_EX_NOREDIRECTIONBITMAP, so bind and present a
        // swapchain frame so DWM has a live surface for WGC fallback capture.
        let swapchain = OverlaySwapChain::new(&ctx, hwnd, 64, 64, 1, false).unwrap();
        let client = window_client_screen_rect(hwnd).unwrap();
        let region = CaptureRegion {
            left: 0,
            top: 0,
            width: 64,
            height: 64,
        };
        let profile = ScalingProfile {
            capture_api: CaptureApi::Dxgi,
            ..Default::default()
        };
        let mut capture = create_capture_backend(
            &ctx,
            hwnd,
            client,
            region,
            &profile,
            &EngineConfig::default(),
            false,
        )
        .expect("cursor-free backend could not start");
        assert!(!matches!(capture.api(), CaptureApi::Dxgi));
        assert!(!capture.cursor_embedded());
        let deadline = std::time::Instant::now()+Duration::from_secs(2);
        let frame = loop {
            swapchain.present(SyncMode::Off).unwrap();
            pump_window_messages();
            if let Some(frame) = capture.acquire_frame(0).unwrap() { break frame; }
            assert!(std::time::Instant::now()<deadline,"fallback produced no frame");
            thread::sleep(Duration::from_millis(2));
        };
        assert_eq!((frame.width, frame.height), (64, 64));
        assert!(frame.texture.is_some());
        eprintln!("DXGI profile produced a frame via {:?}", capture.api());
        capture.stop();
    }

    #[test]
    fn timeout_and_pointer_only_frame_keep_the_source_pair() {
        let Ok(ctx) = D3d11Context::new(0) else {
            assert!(fresh_capture(None).is_none());
            return;
        };
        let (texture, srv, _) = ctx
            .create_rw_texture(16, 16, DXGI_FORMAT_B8G8R8A8_UNORM)
            .unwrap();
        let mut frame = CapturedFrame::metadata_only(16, 16, 100, false);
        frame.texture = Some(texture);
        assert!(fresh_capture(Some(&frame)).is_some());
        let previous = Some(srv);
        frame.is_duplicate = true;
        for candidate in [None, Some(&frame)] {
            let current = fresh_capture(candidate)
                .map(|(_, texture)| ctx.create_srv(texture, DXGI_FORMAT_B8G8R8A8_UNORM).unwrap());
            assert!(current.is_none());
            assert_eq!(current.as_ref().or(previous.as_ref()), previous.as_ref());
        }
    }

    #[test]
    fn dxgi_crop_maps_partial_client_to_the_output() {
        let desktop = RECT {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1080,
        };
        let client = RECT {
            left: -100,
            top: -20,
            right: 300,
            bottom: 280,
        };
        let requested = CaptureRegion {
            left: 10,
            top: 5,
            width: 380,
            height: 290,
        };
        let crop = dxgi_crop(client, desktop, requested).unwrap();
        assert_eq!(
            crop,
            CaptureRegion {
                left: 1830,
                top: 0,
                width: 90,
                height: 275
            }
        );
    }

    #[test]
    fn c_abi_init_uninit_and_queries() {
        unsafe {
            assert_eq!(Init(Some(test_cb)), 1);
            assert_eq!(IsWindowsBuildAtLeast(10240), 1);
            let hz = GetDwmRefreshRate();
            assert!((24.0..=1000.0).contains(&hz));

            let adapters = GetAdapterNames();
            assert!(!adapters.is_null());
            SysFreeString(&BSTR::from_raw(adapters));

            let displays = GetDisplayNames();
            assert!(!displays.is_null());
            SysFreeString(&BSTR::from_raw(displays));

            assert_eq!(SetDriverSettings(), 1);
            UnInit();
        }
    }

    #[test]
    fn present_deadline_absorbs_jitter_without_catching_up_after_a_stall() {
        let interval = 1.0 / 60.0;
        let first = present_deadline(None, 1.0, interval);
        let second = present_deadline(Some(first), 1.0 + interval + 0.002, interval);
        assert!((second - first - 2.0*interval).abs() < 1e-9);
        assert!(present_deadline(Some(second), 1.5, interval) >= 1.5);
    }

    #[test]
    fn runtime_gpu_benchmark_runs() {
        let rt = EngineRuntime::new();
        if let Ok((name, elapsed)) = rt.run_gpu_benchmark(3) {
            assert!(!name.is_empty());
            assert!(elapsed > 0.0);
        }
    }

    #[test]
    fn live_session_activate_toggle_and_uninit() {
        if D3d11Context::new(0).is_err() {
            return;
        }

        unsafe extern "system" fn wndproc(
            hwnd: HWND,
            msg: u32,
            wparam: WPARAM,
            lparam: LPARAM,
        ) -> LRESULT {
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }

        let cls = w!("FlowScaleCoreLiveTest");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            lpszClassName: cls,
            ..Default::default()
        };
        unsafe {
            RegisterClassExW(&wc);
        }
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                cls,
                w!("CoreTest"),
                WS_POPUP,
                0,
                0,
                320,
                240,
                None,
                None,
                None,
                None,
            )
        }
        .expect("CreateWindowExW failed");

        let mut rt = EngineRuntime::new();
        assert!(rt.init(Some(test_cb)));

        let profile = ScalingProfile {
            scaling_type: flowscale_abi::ScalingType::Fsr,
            frame_generation_type: FrameGenerationType::Lsfg2,
            capture_api: CaptureApi::Wgc,
            ..Default::default()
        };
        rt.apply_settings(profile);

        assert!(rt.activate(hwnd.0 as isize));
        assert!(rt.is_active());
        assert_eq!(
            LAST_STATUS.load(Ordering::SeqCst),
            StatusCode::Active as i32
        );

        thread::sleep(Duration::from_millis(80));

        // Second Activate toggles OFF cleanly.
        assert!(!rt.activate(hwnd.0 as isize));
        assert!(!rt.is_active());
        assert_eq!(
            LAST_STATUS.load(Ordering::SeqCst),
            StatusCode::Disabled as i32
        );

        rt.uninit();
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }

    #[test]
    fn telemetry_json_roundtrip_and_game_profile_application() {
        let mut rt = EngineRuntime::new();
        let initial = rt.telemetry();
        assert_eq!(initial.status, StatusCode::Disabled as i32);
        assert_eq!(initial.active_profile_id, "default");

        let gp = GameProfile {
            id: "elden-ring".to_owned(),
            scaling_type: flowscale_abi::ScalingType::Ls1,
            ls1_sharpness: 3,
            scale_factor: 1.5,
            windowed_mode: true,
            ..Default::default()
        };
        rt.apply_game_profile(&gp);

        assert_eq!(rt.profile().scaling_type, flowscale_abi::ScalingType::Ls1);
        assert_eq!(rt.profile().sharpness, 3);
        assert!(rt.profile().windowed_mode);

        let snap = rt.telemetry();
        assert_eq!(snap.active_profile_id, "elden-ring");
        assert!((snap.scale_factor - 1.5).abs() < 1e-6);

        let json = serde_json::to_string(&snap).expect("serialize EngineTelemetry");
        let back: EngineTelemetry =
            serde_json::from_str(&json).expect("deserialize EngineTelemetry");
        assert_eq!(snap, back);
    }

    #[test]
    fn multi_display_clamping_and_enumeration_helpers() {
        let screen = RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        let off_screen = RECT {
            left: -200,
            top: 900,
            right: 600,
            bottom: 1300,
        };
        let clamped = clamp_rect_to_screen(off_screen, screen);
        assert_eq!(clamped.left, 0);
        assert_eq!(clamped.right, 800);
        assert_eq!(clamped.bottom, 1080);
        assert_eq!(clamped.top, 680);

        let displays = get_displays_info();
        let adapters = get_adapters_info();
        assert!(!displays.is_empty() || adapters.is_empty());
        let _admin = is_admin();
    }

    #[test]
    fn live_session_temp_disables_when_unfocused_and_stops_on_destroy() {
        if D3d11Context::new(0).is_err() {
            return;
        }

        unsafe extern "system" fn wndproc(
            hwnd: HWND,
            msg: u32,
            wparam: WPARAM,
            lparam: LPARAM,
        ) -> LRESULT {
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }

        let cls = w!("FlowScaleCoreDestroyTest");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            lpszClassName: cls,
            ..Default::default()
        };
        unsafe {
            RegisterClassExW(&wc);
        }
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                cls,
                w!("DestroyTarget"),
                WS_POPUP,
                0,
                0,
                320,
                240,
                None,
                None,
                None,
                None,
            )
        }
        .expect("CreateWindowExW failed");

        let mut rt = EngineRuntime::new();
        assert!(rt.init(Some(test_cb)));
        let profile = ScalingProfile {
            capture_api: CaptureApi::Gdi,
            windowed_mode: true,
            ..Default::default()
        };
        rt.apply_settings(profile);

        assert!(rt.activate(hwnd.0 as isize));

        // Wait up to 600ms for worker thread to observe unfocused test window -> TempDisabled.
        let mut saw_temp_disabled = false;
        for _ in 0..30 {
            if rt.telemetry().status == StatusCode::TempDisabled as i32 {
                saw_temp_disabled = true;
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(
            saw_temp_disabled,
            "expected unfocused target to transition to TempDisabled"
        );

        // Live crop/queue settings must replace resources even while the target is paused.
        let before_crop = rt.telemetry();
        let mut cropped = rt.profile().clone();
        cropped.capture_offset_left = 16;
        cropped.capture_offset_right = 16;
        cropped.capture_offset_top = 8;
        cropped.capture_offset_bottom = 8;
        cropped.queue_target = 1;
        rt.apply_settings(cropped);
        let mut saw_crop = false;
        for _ in 0..100 {
            let t = rt.telemetry();
            if t.input_width == before_crop.input_width - 32
                && t.input_height == before_crop.input_height - 16
            {
                saw_crop = true;
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(saw_crop, "live crop was not applied to the restarted capture backend: {:?}", rt.telemetry());

        // Destroying the target window must cleanly stop the session and update telemetry to Disabled.
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
        let mut stopped = false;
        for _ in 0..40 {
            if !rt.is_active() && rt.telemetry().status == StatusCode::Disabled as i32 {
                stopped = true;
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(
            stopped,
            "expected destroyed target window to stop active session cleanly"
        );
        rt.uninit();
    }

    #[test]
    fn cpu_pacer_target_avoids_double_wait_under_vsync_and_draws_hdr_fps() {
        // 144 FPS on 144 Hz with VSync 1x: Present handles VBlank, no pre-sleep stacked.
        assert_eq!(
            cpu_pacer_target(Some(1.0), 1.006, 1.0 / 144.0, SyncMode::Vsync1, false, 144.0),
            None
        );
        // 120 FPS on 144 Hz with VSync 1x (> vsync_dt * 1.05): pre-sleeps evenly instead of bursting.
        let paced_120 =
            cpu_pacer_target(Some(1.0), 1.002, 1.0 / 120.0, SyncMode::Vsync1, false, 144.0)
                .expect("120 FPS on 144 Hz VSync should pre-sleep with headroom");
        assert!(paced_120.0 < paced_120.1);
        // SyncMode::Off & SyncMode::Default (mailbox): pace directly to the exact deadline.
        for mode in [SyncMode::Off, SyncMode::Default] {
            let off = cpu_pacer_target(Some(1.0), 1.002, 1.0 / 120.0, mode, false, 144.0)
                .expect("SyncMode::Off/Default should return explicit deadline");
            assert!((off.0 - (1.0 + 1.0 / 120.0)).abs() < 1e-6);
            assert!((off.1 - off.0).abs() < 1e-9);
        }
        // Throttled 60 FPS on 144 Hz with VSync 1x: wakes early with ~0.45 VBlank headroom.
        let throttled = cpu_pacer_target(Some(1.0), 1.002, 1.0 / 60.0, SyncMode::Vsync1, false, 144.0)
            .expect("Throttled VSync should pre-sleep with headroom");
        assert!(throttled.0 < throttled.1);

        // HDR swapchain FPS overlay smoke test (R16G16B16A16_FLOAT + D2D premultiplied alpha).
        if D3d11Context::new(0).is_ok() {
            let mut rt = EngineRuntime::new();
            let profile = ScalingProfile {
                hdr_support: true,
                sync_mode: SyncMode::Off,
                ..Default::default()
            };
            rt.apply_settings(profile);
            rt.smoke_test_overlay_present()
                .expect("HDR FpsOverlay + OverlaySwapChain smoke failed");
        }
    }

    #[test]
    fn fixed_2x_pacer_preserves_real_frames_and_avoids_capture_wait_drift() {
        let mut clock = OutputTimeline::default();
        let mut scheduler = InterpolationScheduler::new(InterpolationMode::Fixed { multiplier: 2.0 }, 10.0, 0.05);
        let mut now = 10.0;
        let mut previous = None;
        for source in 1..30 {
            for step in scheduler.plan_interval(1.0/72.0) {
                let ticket = clock.schedule(now, step.output_dt_sec, source, source as f64/72.0, step.alpha).unwrap();
                if let Some(last) = previous { assert!(ticket.deadline - last >= 1.0/144.0 - 1e-9); }
                now = ticket.deadline + 0.0003;
                previous = Some(ticket.deadline);
            }
        }
    }

    #[test]
    fn fixed_2x_hybrid_pacer_steady_output_intervals() {
        let pacer = HybridPacer::new().expect("HybridPacer::new failed");
        let mut sched = InterpolationScheduler::new(
            InterpolationMode::Fixed { multiplier: 2.0 },
            10.0,
            0.05,
        );
        let source_dt = 1.0 / 72.0;
        let out_dt = 1.0 / 144.0;
        let start = pacer.now_sec();
        let mut last_present = Some(start);
        let mut presented_alphas = Vec::new();

        for _ in 0..4 {
            let steps = sched.plan_interval(source_dt);
            assert_eq!(steps.len(), 2);
            for step in steps {
                wait_for_present(
                    &pacer,
                    &mut last_present,
                    step.output_dt_sec,
                    SyncMode::Off,
                    false,
                    144.0,
                );
                presented_alphas.push((step.is_interpolated, step.alpha));
            }
        }

        let elapsed = pacer.now_sec() - start;
        let expected = 8.0 * out_dt;
        assert_eq!(
            presented_alphas,
            vec![
                (true, 0.5),
                (false, 1.0),
                (true, 0.5),
                (false, 1.0),
                (true, 0.5),
                (false, 1.0),
                (true, 0.5),
                (false, 1.0),
            ]
        );
        assert!(
            (elapsed - expected).abs() < 0.015,
            "8 frames at 144 Hz took {elapsed:.4}s (expected ~{expected:.4}s)"
        );
    }

    #[test]
    fn decoupled_capture_producer_and_one_percent_low_cadence() {
        let Ok(ctx) = D3d11Context::new(0) else {
            return;
        };
        struct MockTimedCapture {
            queue: Arc<FrameRingQueue>,
            tex_a: Option<ID3D11Texture2D>,
            tex_b: Option<ID3D11Texture2D>,
            produced: usize,
            stopped: Arc<AtomicBool>,
        }
        impl CaptureBackend for MockTimedCapture {
            fn api(&self) -> CaptureApi {
                CaptureApi::Wgc
            }
            fn start(&mut self) -> Result<(), CaptureError> {
                Ok(())
            }
            fn acquire_frame(
                &mut self,
                timeout_ms: u32,
            ) -> Result<Option<CapturedFrame>, CaptureError> {
                if self.produced >= 6 {
                    thread::sleep(Duration::from_millis(timeout_ms.min(5) as u64));
                    return Ok(None);
                }
                // Simulate 72 Hz (~14ms) source cadence with 2ms jitter.
                let jitter = if self.produced.is_multiple_of(2) { 12 } else { 15 };
                thread::sleep(Duration::from_millis(jitter));
                self.produced += 1;
                let tex = self
                    .queue
                    .take_free_texture()
                    .or_else(|| self.tex_a.take())
                    .or_else(|| self.tex_b.take());
                let mut frame = CapturedFrame::metadata_only(16, 16, self.produced as i64 * 10_000, false);
                frame.texture = tex;
                Ok(Some(frame))
            }
            fn frame_recycler(&self) -> Option<Arc<FrameRingQueue>> {
                Some(Arc::clone(&self.queue))
            }
            fn recycle_frame_texture(&self, texture: ID3D11Texture2D) {
                self.queue.recycle_texture(texture);
            }
            fn ready_len(&self) -> usize {
                0
            }
            fn stop(&mut self) {
                self.stopped.store(true, Ordering::Release);
            }
        }

        let (tex_a, _, _) = ctx
            .create_rw_texture(16, 16, DXGI_FORMAT_B8G8R8A8_UNORM)
            .unwrap();
        let (tex_b, _, _) = ctx
            .create_rw_texture(16, 16, DXGI_FORMAT_B8G8R8A8_UNORM)
            .unwrap();
        let stopped = Arc::new(AtomicBool::new(false));
        let producer = CaptureProducer::spawn(Box::new(MockTimedCapture {
            queue: Arc::new(FrameRingQueue::new(2).unwrap()),
            tex_a: Some(tex_a),
            tex_b: Some(tex_b),
            produced: 0,
            stopped: Arc::clone(&stopped),
        }));
        producer.set_paused(false);

        let pacer = HybridPacer::new().unwrap();
        let mut profiler = FrametimeProfiler::new();
        let mut received = 0usize;
        let deadline = pacer.now_sec() + 0.5;
        while received < 4 && pacer.now_sec() < deadline {
            let (poll, _, _, _, _) = producer.wait_for_latest(Duration::from_millis(25));
            if let CapturePoll::Frame(mut slot) = poll {
                received += 1;
                let _ = profiler.record_present_timestamp(pacer.now_sec(), 13.88, 0.8, 0.2);
                if let Some(tex) = slot.frame.texture.take() {
                    producer.recycle_texture(tex);
                }
            }
        }
        producer.stop();
        assert!(stopped.load(Ordering::Acquire));
        assert!(received >= 4, "expected >=4 frames, got {received}");
        assert!(profiler.one_percent_low_fps() > 0.0);
    }

    #[test]
    fn vrr_and_vsync_cpu_pacer_and_post_present_deadline_update() {
        let refresh_hz = 144.0f32;
        let refresh_dt = 1.0 / 144.0;
        let dt_120 = 1.0 / 120.0;
        let dt_240 = 1.0 / 240.0;

        // VRR (gsync_support = true) under Vsync1 pre-sleeps with headroom for 60->120 FPS (avoids VRR bursts)
        // and skips CPU sleep near/above refresh (71->142 FPS, 240 FPS) so Present(1) never misses VBlanks.
        let vrr_120 = cpu_pacer_target(Some(1.0), 1.002, dt_120, SyncMode::Vsync1, true, refresh_hz)
            .expect("VRR 60->120 under Vsync1 must pre-sleep with headroom");
        assert!((vrr_120.1 - (1.0 + dt_120)).abs() < 1e-9);
        assert!((vrr_120.0 - (vrr_120.1 - refresh_dt * 0.45)).abs() < 1e-9);

        assert_eq!(
            cpu_pacer_target(Some(1.0), 1.002, 1.0 / 142.0, SyncMode::Vsync1, true, refresh_hz),
            None
        );
        assert_eq!(
            cpu_pacer_target(Some(1.0), 1.002, dt_240, SyncMode::Vsync1, true, refresh_hz),
            None
        );

        // Post-present update: hardware VSync (None) always records present_end.
        let mut last = Some(1.0);
        finalize_last_present(&mut last, None, 1.007, dt_120, true);
        assert_eq!(last, Some(1.007));

        // CPU-paced: small present overhead (< 0.25 * interval) keeps the phase-locked grid deadline.
        let rec = 1.0 + dt_120;
        last = Some(rec);
        finalize_last_present(&mut last, Some(rec), rec + dt_120 * 0.2, dt_120, true);
        assert_eq!(last, Some(rec));

        // CPU-paced: late present block (>= 0.25 * interval) advances to present_end.
        finalize_last_present(&mut last, Some(rec), rec + dt_120 * 0.3, dt_120, true);
        assert_eq!(last, Some(rec + dt_120 * 0.3));

        // Unpaced holding fallback (!pace_present) records present_end.
        finalize_last_present(&mut last, Some(rec), 1.05, dt_120, false);
        assert_eq!(last, Some(1.05));
    }

    #[test]
    fn schedule_dt_never_slows_fixed_pacing_and_presented_fps_is_uniform() {
        let mut drain = QueueDrainingController::new(0.5, 1);
        let dt = 1.0 / 60.0;
        for _ in 0..5 {
            // Even when QueueDrainingController sees depth=0, clamping to <=1.0 prevents slowing Fixed pacing.
            assert_eq!(dt * drain.update(0).min(1.0), dt);
        }
        assert!(dt * drain.update(3).min(1.0) < dt);

        // Fractional Fixed 2.5x at 60 FPS produces uniform output_dt_sec -> constant 150 FPS across 2-step & 3-step intervals.
        let mut sched = InterpolationScheduler::new(
            InterpolationMode::Fixed { multiplier: 2.5 },
            10.0,
            0.05,
        );
        for expected_len in [2usize, 3, 2, 3] {
            let steps = sched.plan_interval(dt);
            assert_eq!(steps.len(), expected_len);
            let presented_fps = (1.0 / steps[0].output_dt_sec) as f32;
            assert!((presented_fps - 150.0).abs() < 1e-3);
        }
    }

    #[test]
    fn capture_producer_recycles_duplicates_and_tracks_superseded_count() {
        use std::collections::VecDeque;
        let Ok(ctx) = D3d11Context::new(0) else {
            return;
        };
        struct ScriptedBackend {
            queue: Arc<FrameRingQueue>,
            pending: Arc<Mutex<VecDeque<CapturedFrame>>>,
        }
        impl CaptureBackend for ScriptedBackend {
            fn api(&self) -> CaptureApi {
                CaptureApi::Dxgi
            }
            fn start(&mut self) -> Result<(), CaptureError> {
                Ok(())
            }
            fn acquire_frame(
                &mut self,
                timeout_ms: u32,
            ) -> Result<Option<CapturedFrame>, CaptureError> {
                if let Some(frame) = self.pending.lock().unwrap().pop_front() {
                    return Ok(Some(frame));
                }
                thread::sleep(Duration::from_millis(timeout_ms.min(2) as u64));
                Ok(None)
            }
            fn frame_recycler(&self) -> Option<Arc<FrameRingQueue>> {
                Some(Arc::clone(&self.queue))
            }
            fn recycle_frame_texture(&self, texture: ID3D11Texture2D) {
                self.queue.recycle_texture(texture);
            }
            fn ready_len(&self) -> usize {
                0
            }
            fn stop(&mut self) {}
        }

        let queue = Arc::new(FrameRingQueue::new(4).unwrap());
        let pending = Arc::new(Mutex::new(VecDeque::new()));
        let (tex_a, _, _) = ctx
            .create_rw_texture(16, 16, DXGI_FORMAT_B8G8R8A8_UNORM)
            .unwrap();
        let (tex_b, _, _) = ctx
            .create_rw_texture(16, 16, DXGI_FORMAT_B8G8R8A8_UNORM)
            .unwrap();
        let (tex_dup, _, _) = ctx
            .create_rw_texture(16, 16, DXGI_FORMAT_B8G8R8A8_UNORM)
            .unwrap();

        {
            let mut q = pending.lock().unwrap();
            let mut f1 = CapturedFrame::metadata_only(16, 16, 100_000, false);
            f1.texture = Some(tex_a);
            let mut f2 = CapturedFrame::metadata_only(16, 16, 200_000, false);
            f2.texture = Some(tex_b);
            q.push_back(f1);
            q.push_back(f2);
        }

        let producer = CaptureProducer::spawn(Box::new(ScriptedBackend {
            queue: Arc::clone(&queue),
            pending: Arc::clone(&pending),
        }));
        producer.set_paused(false);

        // Wait until both f1 and f2 are processed so f1 is superseded and recycled.
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while queue.free_pool_len() < 1 && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(2));
        }
        let (poll, _, _, _, superseded) = producer.poll_latest();
        assert_eq!(superseded, 1);
        assert_eq!(queue.free_pool_len(), 1);
        let CapturePoll::Frame(mut slot) = poll else {
            panic!("expected latest non-duplicate frame");
        };
        assert_eq!(slot.frame.timestamp_qpc, 200_000);
        producer.recycle_texture(slot.frame.texture.take().unwrap());

        // Subsequent poll resets superseded_count to 0, and duplicate textures are recycled instead of dropped.
        {
            let mut dup = CapturedFrame::metadata_only(16, 16, 300_000, true);
            dup.texture = Some(tex_dup);
            pending.lock().unwrap().push_back(dup);
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while queue.free_pool_len() < 3 && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(2));
        }
        let (dup_poll, _, _, _, dup_superseded) = producer.poll_latest();
        assert!(matches!(dup_poll, CapturePoll::Empty));
        assert_eq!(dup_superseded, 0);
        assert_eq!(queue.free_pool_len(), 3);
        producer.stop();

        // Advancing FrametimeTracker on superseded > 0 preserves cadence AND keeps FG enabled without recording 2*dt.
        let mut tracker = FrametimeTracker::new(8, 6.0);
        let dt = 1.0 / 60.0;
        assert!(!tracker.record_capture_timestamp(1.0));
        assert!(tracker.record_capture_timestamp(1.0 + dt));
        assert!(tracker.record_capture_timestamp(1.0 + 2.0 * dt));
        assert!(tracker.record_superseded_timestamp(1.0 + 4.0 * dt, 1));
        assert!((tracker.estimated_fps() - 60.0).abs() < 0.01);
        assert!(tracker.record_capture_timestamp(1.0 + 5.0 * dt));
        assert!((tracker.estimated_fps() - 60.0).abs() < 0.01);
    }

    #[test]
    fn capture_producer_survives_dxgi_desktop_unavailable_and_resumes() {
        use std::sync::atomic::{AtomicBool, AtomicU32};
        let Ok(ctx) = D3d11Context::new(0) else {
            return;
        };
        struct DesktopToggleBackend {
            queue: Arc<FrameRingQueue>,
            unavailable: Arc<AtomicBool>,
            backend_superseded: Arc<AtomicU32>,
            pending: Arc<Mutex<Option<CapturedFrame>>>,
        }
        impl CaptureBackend for DesktopToggleBackend {
            fn api(&self) -> CaptureApi {
                CaptureApi::Dxgi
            }
            fn start(&mut self) -> Result<(), CaptureError> {
                Ok(())
            }
            fn acquire_frame(
                &mut self,
                timeout_ms: u32,
            ) -> Result<Option<CapturedFrame>, CaptureError> {
                if self.unavailable.load(Ordering::Acquire) {
                    return Err(CaptureError::DxgiDesktopUnavailable);
                }
                if let Some(frame) = self.pending.lock().unwrap().take() {
                    return Ok(Some(frame));
                }
                thread::sleep(Duration::from_millis(timeout_ms.min(2) as u64));
                Ok(None)
            }
            fn frame_recycler(&self) -> Option<Arc<FrameRingQueue>> {
                Some(Arc::clone(&self.queue))
            }
            fn recycle_frame_texture(&self, texture: ID3D11Texture2D) {
                self.queue.recycle_texture(texture);
            }
            fn take_superseded_count(&self) -> u32 {
                self.backend_superseded.swap(0, Ordering::AcqRel)
            }
            fn ready_len(&self) -> usize {
                0
            }
            fn stop(&mut self) {}
        }

        let queue = Arc::new(FrameRingQueue::new(2).unwrap());
        let unavailable = Arc::new(AtomicBool::new(true));
        let backend_superseded = Arc::new(AtomicU32::new(0));
        let pending = Arc::new(Mutex::new(None));
        let (tex, _, _) = ctx
            .create_rw_texture(16, 16, DXGI_FORMAT_B8G8R8A8_UNORM)
            .unwrap();

        let producer = CaptureProducer::spawn(Box::new(DesktopToggleBackend {
            queue: Arc::clone(&queue),
            unavailable: Arc::clone(&unavailable),
            backend_superseded: Arc::clone(&backend_superseded),
            pending: Arc::clone(&pending),
        }));
        producer.set_paused(false);

        let (err_poll, _, _, _, _) = producer.wait_for_latest(Duration::from_secs(1));
        assert!(matches!(
            err_poll,
            CapturePoll::Error(CaptureError::DxgiDesktopUnavailable)
        ));

        let mut recovered = CapturedFrame::metadata_only(16, 16, 500_000, false);
        recovered.texture = Some(tex);
        *pending.lock().unwrap() = Some(recovered);
        backend_superseded.store(2, Ordering::Release);
        unavailable.store(false, Ordering::Release);

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let mut resumed_slot = None;
        let mut total_superseded = 0u32;
        while std::time::Instant::now() < deadline {
            let (poll, _, _, _, sup) = producer.wait_for_latest(Duration::from_millis(200));
            total_superseded = total_superseded.saturating_add(sup);
            if let CapturePoll::Frame(slot) = poll {
                resumed_slot = Some(slot);
                break;
            }
        }
        let mut slot = resumed_slot.expect("producer should recover after DxgiDesktopUnavailable");
        assert_eq!(slot.frame.timestamp_qpc, 500_000);
        assert_eq!(total_superseded, 2);
        producer.recycle_texture(slot.frame.texture.take().unwrap());
        producer.stop();
    }

    #[test]
    fn poll_or_wait_latest_preserves_discontinuity_and_superseded_across_empty_poll() {
        let Ok(ctx) = D3d11Context::new(0) else {
            return;
        };
        struct IdleBackend;
        impl CaptureBackend for IdleBackend {
            fn api(&self) -> CaptureApi {
                CaptureApi::Wgc
            }
            fn start(&mut self) -> Result<(), CaptureError> {
                Ok(())
            }
            fn acquire_frame(
                &mut self,
                _timeout_ms: u32,
            ) -> Result<Option<CapturedFrame>, CaptureError> {
                Ok(None)
            }
            fn ready_len(&self) -> usize {
                0
            }
            fn stop(&mut self) {}
        }

        let producer = CaptureProducer::spawn(Box::new(IdleBackend));

        // Case 1: Empty first poll has disc=true, sup=2; wait_for_latest times out with Empty.
        {
            let mut guard = producer.mailbox.state.lock().unwrap();
            guard.discontinuity = true;
            guard.superseded_count = 2;
            guard.latest = None;
        }
        let (poll, disc, _, _, sup) = producer.poll_or_wait_latest(true, Duration::from_millis(5));
        assert!(matches!(poll, CapturePoll::Empty));
        assert!(disc);
        assert_eq!(sup, 2);

        // Case 2: Empty first poll has disc=true, sup=2; wait_for_latest receives Frame with disc=false, sup=3.
        let (tex, _, _) = ctx
            .create_rw_texture(16, 16, DXGI_FORMAT_B8G8R8A8_UNORM)
            .unwrap();
        {
            let mut guard = producer.mailbox.state.lock().unwrap();
            guard.discontinuity = true;
            guard.superseded_count = 2;
            guard.latest = None;
        }
        let mailbox = Arc::clone(&producer.mailbox);
        let poster = thread::spawn(move || {
            thread::sleep(Duration::from_millis(15));
            let mut frame = CapturedFrame::metadata_only(16, 16, 900_000, false);
            frame.texture = Some(tex);
            let mut guard = mailbox.state.lock().unwrap();
            guard.superseded_count = 3;
            guard.latest = Some(CapturedSlot {
                frame,
                cursor_embedded: false,
            });
            mailbox.frame_cv.notify_one();
        });
        let (poll2, disc2, _, _, sup2) =
            producer.poll_or_wait_latest(true, Duration::from_millis(500));
        poster.join().unwrap();
        assert!(matches!(poll2, CapturePoll::Frame(_)));
        assert!(disc2);
        assert_eq!(sup2, 5);
        producer.stop();
    }

    #[test]
    fn step_scheduling_never_drops_real_frame_and_skips_fg_on_multi_frame_gap() {
        // Multi-frame superseded gap (>= 1.6x tracked_dt) or unseeded cadence (tracked_dt <= 0.0)
        // suppresses FG for that discontinuity frame.
        let dt = 1.0 / 70.0;
        assert!(should_skip_fg_for_gap(1, Some(1.0), 1.0 + 2.0 * dt, dt));
        assert!(should_skip_fg_for_gap(1, Some(1.0), 1.0 + dt, 0.0));
        assert!(!should_skip_fg_for_gap(1, None, 1.0 + dt, 0.0));
        assert!(!should_skip_fg_for_gap(1, Some(1.0), 1.0 + 1.1 * dt, dt));
        assert!(!should_skip_fg_for_gap(0, Some(1.0), 1.0 + 2.0 * dt, dt));
        assert!(!should_skip_fg_for_gap(0, Some(1.0), 1.0 + dt, 0.0));
    }

    #[test]
    fn dxgi_retry_due_only_after_backoff_and_only_when_dxgi_requested() {
        let now = Instant::now();
        let due = Some(now - Duration::from_millis(1));
        let pending = Some(now + DXGI_RETRY_DELAY);
        assert!(dxgi_retry_due(CaptureApi::Dxgi, CaptureApi::Gdi, true, due, now, false));
        assert!(dxgi_retry_due(CaptureApi::Dxgi, CaptureApi::Wgc, true, due, now, false));
        assert!(!dxgi_retry_due(CaptureApi::Dxgi, CaptureApi::Gdi, true, pending, now, false));
        assert!(!dxgi_retry_due(CaptureApi::Dxgi, CaptureApi::Gdi, true, None, now, false));
        assert!(!dxgi_retry_due(CaptureApi::Dxgi, CaptureApi::Dxgi, true, due, now, false));
        assert!(!dxgi_retry_due(CaptureApi::Wgc, CaptureApi::Gdi, true, due, now, false));
        // Capture-exclusion failure disables DXGI for the session; it is never retried.
        assert!(!dxgi_retry_due(CaptureApi::Dxgi, CaptureApi::Gdi, false, due, now, false));
    }

    #[test]
    fn dxgi_paused_for_visible_pointer_resumes_only_once_the_cursor_is_hidden() {
        let now = Instant::now();
        let due = Some(now - Duration::from_millis(1));
        // Menu: cursor still visible after the backoff, so stay on the cursor-free fallback.
        assert!(!dxgi_retry_due(CaptureApi::Dxgi, CaptureApi::Wgc, true, due, now, true));
        // Gameplay: the game hid the cursor, so DXGI comes back.
        assert!(dxgi_retry_due(CaptureApi::Dxgi, CaptureApi::Wgc, true, due, now, false));
    }

    #[test]
    fn wait_for_fresh_or_deadline_wakes_early_on_frame_cv() {
        let Ok(ctx) = D3d11Context::new(0) else {
            return;
        };
        struct IdleBackend;
        impl CaptureBackend for IdleBackend {
            fn api(&self) -> CaptureApi {
                CaptureApi::Wgc
            }
            fn start(&mut self) -> Result<(), CaptureError> {
                Ok(())
            }
            fn acquire_frame(
                &mut self,
                _timeout_ms: u32,
            ) -> Result<Option<CapturedFrame>, CaptureError> {
                Ok(None)
            }
            fn ready_len(&self) -> usize {
                0
            }
            fn stop(&mut self) {}
        }

        let producer = CaptureProducer::spawn(Box::new(IdleBackend));
        let pacer = HybridPacer::new().expect("HybridPacer");
        let (tex, _, _) = ctx
            .create_rw_texture(16, 16, DXGI_FORMAT_B8G8R8A8_UNORM)
            .unwrap();
        let mailbox = Arc::clone(&producer.mailbox);
        let poster = thread::spawn(move || {
            thread::sleep(Duration::from_millis(15));
            let mut frame = CapturedFrame::metadata_only(16, 16, 900_001, false);
            frame.texture = Some(tex);
            let mut guard = mailbox.state.lock().unwrap();
            guard.latest = Some(CapturedSlot {
                frame,
                cursor_embedded: false,
            });
            mailbox.has_latest.store(true, Ordering::Release);
            mailbox.frame_cv.notify_one();
        });

        let start = pacer.now_sec();
        let woke_early = producer.wait_for_fresh_or_deadline(start + 0.500, &pacer);
        let elapsed = pacer.now_sec() - start;
        poster.join().unwrap();
        assert!(woke_early);
        assert!(
            elapsed < 0.200,
            "expected early wakeup on frame_cv well before 500ms deadline, got {elapsed:.3}s"
        );
        producer.stop();
    }

    #[test]
    fn committed_output_pacing_is_monotonic_across_late_step0_readiness() {
        let dt = 1.0 / 70.5;
        let out_dt = dt * 0.5;
        let mut last_present = Some(10.0);
        // Simulate Step 0 arriving 2.2 ms past its deadline, followed by Step 1 and next Step 0.
        let nows = [10.0 + out_dt + 0.0022, 10.0 + out_dt + 0.0025, 10.0 + 3.0 * out_dt - 0.0010];
        let mut committed = Vec::new();
        for (idx, &now) in nows.iter().enumerate() {
            let (sleep_until, recorded) = cpu_pacer_target(
                last_present,
                now,
                out_dt,
                if idx % 2 == 0 { SyncMode::Default } else { SyncMode::Off },
                true,
                144.0,
            )
            .expect("Off/Default must use explicit CPU pacing");
            assert!((sleep_until - recorded).abs() < 1e-9);
            let actual_present = sleep_until.max(now);
            committed.push(actual_present);
            last_present = Some(recorded);
        }
        assert!(committed[0] < committed[1] && committed[1] < committed[2]);
        assert!(((committed[2] - committed[1]) - (out_dt - 0.0)).abs() < 1e-9);
    }
}
