#![cfg_attr(not(test), windows_subsystem = "windows")]

mod bridge;
mod configs;

use configs::{ConfigAction, ConfigLibrary, PortableConfig};
mod win_host;

use std::cell::RefCell;
use std::collections::VecDeque;
use std::ffi::c_void;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use bridge::{
    apply_ui_settings, collect_hardware_info, config_ini_path, engine_config_to_ui,
    load_engine_config, load_engine_config_checked, profile_to_ui, settings_to_ui,
    slugify_profile_id, telemetry_to_ui, ui_to_engine_config, ui_to_game_profile, ui_to_hotkey,
    validate_ui_profile, validate_ui_theme, AutoScaleAction, AutoScaleTracker, HostEvent,
    PickedPathPayload, ProfileUpsertedPayload, StateSyncPayload, ToastPayload, UiCommand,
    UiHardwareInfo, UiScalingProfile, UiTelemetryState,
};
use flowscale_config::{
    get_windows_startup, import_lossless_default_xml, import_lossless_settings_xml,
    set_windows_startup, AppSettings, EngineConfig, GameProfile, ThemeMode,
};
use flowscale_core::{set_windows_gpu_preference, EngineRuntime};
use flowscale_overlay::get_foreground_window;
use flowscale_overlay::hotkey::{
    get_window_title_and_exe, ForegroundProcessWatcher, GlobalHotkeyHook, HotkeyBinding,
};
use win_host::{
    enable_dpi_awareness, load_app_icon, open_external_url, pick_file_dialog,
    relaunch_current_exe_as_admin, restore_window_message_id, set_dwm_window_theme,
    should_hide_to_tray, show_error_box, show_tray_context_menu, taskbar_created_message_id,
    to_wide_null, HostWebView, SingleInstanceGuard, TrayIcon, TRAY_CMD_EXIT, TRAY_CMD_SHOW,
    TRAY_CMD_TOGGLE_SCALE, WINDOW_CLASS_NAME,
};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::System::Com::{
    CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_MULTITHREADED,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    ChangeWindowMessageFilterEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    GetMessageW, GetWindowLongPtrW, GetWindowRect, IsIconic, IsWindow, IsWindowVisible, IsZoomed,
    KillTimer, LoadCursorW, PostMessageW, PostQuitMessage, RegisterClassExW, SetForegroundWindow,
    SetTimer, SetWindowLongPtrW, ShowWindow, TranslateMessage, CW_USEDEFAULT, GWLP_USERDATA, HICON,
    ICON_BIG, ICON_SMALL, IDC_ARROW, MSG, MSGFLT_ALLOW, SIZE_MAXIMIZED, SIZE_MINIMIZED, SW_HIDE,
    SW_RESTORE, SW_SHOW, SW_SHOWMAXIMIZED, WM_APP, WM_CLOSE, WM_CONTEXTMENU, WM_DESTROY,
    WM_DISPLAYCHANGE, WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_RBUTTONUP, WM_SETICON, WM_SETTINGCHANGE,
    WM_SIZE, WM_TIMER, WNDCLASSEXW, WS_OVERLAPPEDWINDOW,
};

const APP_STARTUP_NAME: &str = "FlowScale";
const TIMER_ID: usize = 1;
const TIMER_INTERVAL_MS: u32 = 200;
const IPC_BATCH_LIMIT: usize = 32;

const WM_APP_TRAY: u32 = WM_APP + 1;
const WM_APP_HOTKEY: u32 = WM_APP + 2;
const WM_APP_IPC: u32 = WM_APP + 3;
const WM_APP_ENGINE_COMPLETE: u32 = WM_APP + 4;

enum PersistenceJob {
    #[cfg(test)]
    Pause(mpsc::Sender<()>, mpsc::Receiver<()>),
    Settings(
        PathBuf,
        AppSettings,
        Option<(String, String, Box<UiScalingProfile>)>,
    ),
    EngineConfig(PathBuf, EngineConfig),
    CommitSettings(PathBuf, Box<AppSettings>, mpsc::Sender<Result<(), String>>),
    Flush(mpsc::Sender<()>),
    Stop,
}

struct ProfileSaveResult {
    id: String,
    revision: String,
    profile: Result<UiScalingProfile, String>,
}

struct PersistenceWorker {
    sender: mpsc::Sender<PersistenceJob>,
    errors: mpsc::Receiver<String>,
    profile_results: mpsc::Receiver<ProfileSaveResult>,
    thread: Option<JoinHandle<()>>,
}

impl PersistenceWorker {
    fn new() -> Self {
        let (sender, jobs) = mpsc::channel();
        let (error_tx, errors) = mpsc::channel();
        let (profile_tx, profile_results) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("flowscale-persistence".into())
            .spawn(move || {
                while let Ok(job) = jobs.recv() {
                    let result = match job {
                        #[cfg(test)]
                        PersistenceJob::Pause(started, release) => {
                            let _ = started.send(());
                            let _ = release.recv();
                            continue;
                        }
                        PersistenceJob::Settings(path, settings, ack) => {
                            let result = settings.save_to_path(path);
                            if let Some((id, revision, profile)) = ack {
                                let profile = result.map(|()| *profile);
                                let _ = profile_tx.send(ProfileSaveResult {
                                    id,
                                    revision,
                                    profile,
                                });
                                continue;
                            }
                            result
                        }
                        PersistenceJob::EngineConfig(path, config) => config
                            .save_to_file(path)
                            .map_err(|e| format!("failed to write config.ini: {e}")),
                        PersistenceJob::CommitSettings(path, settings, done) => {
                            let _ = done.send(settings.save_to_path(path));
                            continue;
                        }
                        PersistenceJob::Flush(done) => {
                            let _ = done.send(());
                            continue;
                        }
                        PersistenceJob::Stop => break,
                    };
                    if let Err(error) = result {
                        let _ = error_tx.send(error);
                    }
                }
            })
            .expect("failed to start settings persistence worker");
        Self {
            sender,
            errors,
            profile_results,
            thread: Some(worker),
        }
    }

    fn commit_settings(&self, settings: AppSettings) -> Result<(), String> {
        let (done, result) = mpsc::channel();
        self.sender.send(PersistenceJob::CommitSettings(
            AppSettings::default_settings_path(), Box::new(settings), done,
        )).map_err(|_| "Settings persistence worker stopped.".to_owned())?;
        result.recv().map_err(|_| "Settings persistence worker stopped.".to_owned())?
    }

    fn save_settings(&self, settings: AppSettings) -> Result<(), String> {
        self.sender
            .send(PersistenceJob::Settings(
                AppSettings::default_settings_path(),
                settings,
                None,
            ))
            .map_err(|e| format!("settings persistence worker stopped: {e}"))
    }

    fn save_profile(
        &self,
        settings: AppSettings,
        profile: UiScalingProfile,
        revision: String,
    ) -> Result<(), String> {
        self.sender
            .send(PersistenceJob::Settings(
                AppSettings::default_settings_path(),
                settings,
                Some((profile.id.clone(), revision, Box::new(profile))),
            ))
            .map_err(|e| format!("settings persistence worker stopped: {e}"))
    }

    fn save_engine_config(&self, config: EngineConfig) -> Result<(), String> {
        self.sender
            .send(PersistenceJob::EngineConfig(config_ini_path(), config))
            .map_err(|e| format!("settings persistence worker stopped: {e}"))
    }

    fn flush(&self) -> Result<(), String> {
        let (done, flushed) = mpsc::channel();
        self.sender
            .send(PersistenceJob::Flush(done))
            .map_err(|e| format!("settings persistence worker stopped: {e}"))?;
        flushed
            .recv()
            .map_err(|e| format!("settings persistence flush failed: {e}"))
    }

    fn take_errors(&self) -> impl Iterator<Item = String> + '_ {
        self.errors.try_iter()
    }

    fn take_profile_results(&self) -> Vec<ProfileSaveResult> {
        self.profile_results.try_iter().collect()
    }
}

impl Drop for PersistenceWorker {
    fn drop(&mut self) {
        let _ = self.sender.send(PersistenceJob::Stop);
        if let Some(worker) = self.thread.take() {
            let _ = worker.join();
        }
    }
}

fn with_runtime<R>(
    runtime: &Mutex<EngineRuntime>,
    f: impl FnOnce(&EngineRuntime) -> R,
) -> Option<R> {
    runtime.try_lock().ok().map(|runtime| f(&runtime))
}

fn with_runtime_mut<R>(
    runtime: &Mutex<EngineRuntime>,
    f: impl FnOnce(&mut EngineRuntime) -> R,
) -> Option<R> {
    runtime.try_lock().ok().map(|mut runtime| f(&mut runtime))
}

fn defer_stop_if_busy(engine_busy: bool, pending_stop: &mut bool) -> bool {
    if engine_busy {
        *pending_stop = true;
        true
    } else {
        false
    }
}

fn take_ipc_batch(queue: &mut VecDeque<String>) -> Vec<String> {
    (0..IPC_BATCH_LIMIT)
        .filter_map(|_| queue.pop_front())
        .collect()
}

fn malformed_profile_revision(raw: &str) -> Option<(String, String)> {
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    if value.get("type")?.as_str()? != "upsert_profile" {
        return None;
    }
    let payload = value.get("payload")?;
    let id = payload.get("id")?.as_str()?;
    let revision = payload.get("client_revision")?.as_str()?;
    (!id.is_empty() && id.len() <= 128 && !revision.is_empty() && revision.len() <= 64)
        .then(|| (id.to_owned(), revision.to_owned()))
}

struct AppController {
    hwnd: HWND,
    restore_msg: u32,
    taskbar_created_msg: u32,
    settings: AppSettings,
    settings_load_error: Option<String>,
    persistence: PersistenceWorker,
    engine_config: EngineConfig,
    engine_config_load_error: Option<String>,
    hardware: UiHardwareInfo,
    hardware_dirty: bool,
    runtime: Arc<Mutex<EngineRuntime>>,
    engine_busy: bool,
    stop_after_engine_start: bool,
    pending_runtime_profile: Option<GameProfile>,
    pending_runtime_config: Option<EngineConfig>,
    pending_activation: Option<(isize, bool)>,
    shutting_down: Arc<AtomicBool>,
    shutdown_complete: Arc<AtomicBool>,
    fg_watcher: ForegroundProcessWatcher,
    auto_scaler: AutoScaleTracker,
    hotkey_hook: Option<GlobalHotkeyHook>,
    hotkey_error: Option<String>,
    tray: Option<TrayIcon>,
    webview: Option<HostWebView>,
    ipc_queue: Rc<RefCell<VecDeque<String>>>,
    last_telemetry: Option<UiTelemetryState>,
    last_error: Option<String>,
    last_tray_tooltip: String,
    last_external_hwnd: isize,
    single_instance: Option<SingleInstanceGuard>,
    force_exit: bool,
}

impl AppController {
    fn install_hotkey_hook(hwnd: HWND, binding: HotkeyBinding) -> Result<GlobalHotkeyHook, String> {
        let hwnd_raw = hwnd.0 as isize;
        GlobalHotkeyHook::start(binding, move || unsafe {
            let target = HWND(hwnd_raw as *mut c_void);
            let _ = PostMessageW(Some(target), WM_APP_HOTKEY, WPARAM(0), LPARAM(0));
        })
    }

    fn new(
        hwnd: HWND,
        restore_msg: u32,
        taskbar_created_msg: u32,
        single_instance: SingleInstanceGuard,
    ) -> Self {
        let settings_path = AppSettings::default_settings_path();
        let is_first_run = !settings_path.exists();
        let (mut settings, settings_load_error) =
            AppSettings::load_or_default_checked_at(&settings_path);
        settings.start_at_windows_startup = get_windows_startup(APP_STARTUP_NAME);
        if is_first_run {
            if let Some(def) = settings.profiles.iter_mut().find(|p| p.id == "default") {
                def.preferred_gpu_id = -1;
                def.output_display_id = -1;
            }
        }

        let (engine_config, engine_config_load_error) = load_engine_config_checked();
        let hardware = collect_hardware_info();

        let mut runtime = EngineRuntime::new();
        let _ = runtime.init(None);
        runtime.apply_engine_config(engine_config.clone());
        if let Some(selected) = Self::find_selected_profile(&settings) {
            runtime.apply_game_profile(selected);
        }

        let hotkey_binding = HotkeyBinding {
            vk: settings.hotkey.vk,
            ctrl: settings.hotkey.ctrl,
            alt: settings.hotkey.alt,
            shift: settings.hotkey.shift,
            win: settings.hotkey.win,
        };
        let (hotkey_hook, hotkey_error) = match Self::install_hotkey_hook(hwnd, hotkey_binding) {
            Ok(hook) => (Some(hook), None),
            Err(e) => {
                let msg = format!("Global hotkey hook failed: {e}");
                eprintln!("{msg}");
                (None, Some(msg))
            }
        };

        let tray = Some(TrayIcon::new(hwnd, WM_APP_TRAY, "FlowScale - Idle"));

        Self {
            hwnd,
            restore_msg,
            taskbar_created_msg,
            settings,
            settings_load_error,
            persistence: PersistenceWorker::new(),
            engine_config,
            engine_config_load_error,
            hardware,
            hardware_dirty: false,
            runtime: Arc::new(Mutex::new(runtime)),
            engine_busy: false,
            stop_after_engine_start: false,
            pending_runtime_profile: None,
            pending_runtime_config: None,
            pending_activation: None,
            shutting_down: Arc::new(AtomicBool::new(false)),
            shutdown_complete: Arc::new(AtomicBool::new(false)),
            fg_watcher: ForegroundProcessWatcher::new(),
            auto_scaler: AutoScaleTracker::default(),
            hotkey_hook,
            hotkey_error,
            tray,
            webview: None,
            ipc_queue: Rc::new(RefCell::new(VecDeque::new())),
            last_telemetry: None,
            last_error: None,
            last_tray_tooltip: "FlowScale - Idle".to_owned(),
            last_external_hwnd: 0,
            single_instance: Some(single_instance),
            force_exit: false,
        }
    }

    fn tray_active(&self) -> bool {
        self.tray.as_ref().is_some_and(TrayIcon::is_active)
    }

    fn apply_runtime_profile(&mut self, profile: &GameProfile) {
        if with_runtime_mut(&self.runtime, |runtime| runtime.apply_game_profile(profile)).is_none()
        {
            self.pending_runtime_profile = Some(profile.clone());
        }
    }

    fn apply_runtime_config(&mut self, config: EngineConfig) {
        if with_runtime_mut(&self.runtime, |runtime| {
            runtime.apply_engine_config(config.clone())
        })
        .is_none()
        {
            self.pending_runtime_config = Some(config);
        }
    }

    fn runtime_is_active(&self) -> bool {
        with_runtime(&self.runtime, EngineRuntime::is_active).unwrap_or(self.engine_busy)
    }

    fn activate_async(&mut self, hwnd: isize, profile: &GameProfile, auto_scale: bool) {
        if self.engine_busy {
            return;
        }
        self.apply_runtime_config(self.engine_config.clone());
        self.apply_runtime_profile(profile);
        let runtime = Arc::clone(&self.runtime);
        let shutting_down = Arc::clone(&self.shutting_down);
        let target_hwnd = self.hwnd.0 as isize;
        self.engine_busy = true;
        self.pending_activation = Some((hwnd, auto_scale));
        let spawn = thread::Builder::new()
            .name("FlowScale activation".to_owned())
            .spawn(move || {
                let com_initialized = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).is_ok() };
                let result = if !com_initialized {
                    2
                } else if runtime
                    .lock()
                    .map(|mut runtime| runtime.activate(hwnd))
                    .unwrap_or(false)
                {
                    1
                } else {
                    0
                };
                let posted = unsafe {
                    PostMessageW(
                        Some(HWND(target_hwnd as *mut c_void)),
                        WM_APP_ENGINE_COMPLETE,
                        WPARAM(result),
                        LPARAM(0),
                    )
                    .is_ok()
                };
                if !posted || shutting_down.load(Ordering::Acquire) {
                    if let Ok(mut runtime) = runtime.lock() {
                        runtime.stop_session();
                    }
                }
                if com_initialized {
                    unsafe { CoUninitialize() };
                }
            });
        if let Err(error) = spawn {
            self.engine_busy = false;
            self.pending_activation = None;
            self.last_error = Some(format!("Failed to start scaling worker: {error}"));
            self.sync_telemetry(true);
        } else if !auto_scale {
            self.toast("info", "Starting scaling session...");
        }
    }

    fn stop_async(&mut self) {
        if defer_stop_if_busy(self.engine_busy, &mut self.stop_after_engine_start) {
            return;
        }
        let runtime = Arc::clone(&self.runtime);
        let target_hwnd = self.hwnd.0 as isize;
        self.engine_busy = true;
        if let Err(error) = thread::Builder::new()
            .name("FlowScale stop".to_owned())
            .spawn(move || {
                if let Ok(mut runtime) = runtime.lock() {
                    runtime.stop_session();
                }
                let _ = unsafe {
                    PostMessageW(
                        Some(HWND(target_hwnd as *mut c_void)),
                        WM_APP_ENGINE_COMPLETE,
                        WPARAM(0),
                        LPARAM(0),
                    )
                };
            })
        {
            self.engine_busy = false;
            self.last_error = Some(format!("Failed to start stop worker: {error}"));
            self.sync_telemetry(true);
        }
    }

    fn apply_pending_runtime_settings(&mut self) {
        if self.pending_runtime_profile.is_none() && self.pending_runtime_config.is_none() {
            return;
        }
        if let Ok(mut runtime) = self.runtime.try_lock() {
            if let Some(config) = self.pending_runtime_config.take() {
                runtime.apply_engine_config(config);
            }
            if let Some(profile) = self.pending_runtime_profile.take() {
                runtime.apply_game_profile(&profile);
            }
        }
    }

    fn finish_activation(&mut self, result: usize) {
        self.engine_busy = false;
        if self.stop_after_engine_start {
            self.stop_after_engine_start = false;
            self.pending_activation = None;
            self.stop_async();
            return;
        }
        self.apply_pending_runtime_settings();
        let activated = result == 1;
        if let Some((hwnd, auto_scale)) = self.pending_activation.take() {
            if activated {
                self.auto_scaler.suppress_hwnd(hwnd);
            } else if auto_scale {
                self.auto_scaler.record_failure(hwnd, Instant::now());
                let message = if result == 2 {
                    "Failed to initialize COM for the scaling worker."
                } else {
                    "Auto-scale activation failed."
                };
                self.last_error = Some(message.to_owned());
                self.toast("error", message);
            } else {
                let message = if result == 2 {
                    "Failed to initialize COM for the scaling worker."
                } else {
                    "Failed to start scaling session for the foreground window."
                };
                self.last_error = Some(message.to_owned());
                self.toast("error", message);
            }
        }
        self.sync_telemetry(true);
    }

    fn apply_hotkey_binding(&mut self) -> bool {
        let binding = HotkeyBinding {
            vk: self.settings.hotkey.vk,
            ctrl: self.settings.hotkey.ctrl,
            alt: self.settings.hotkey.alt,
            shift: self.settings.hotkey.shift,
            win: self.settings.hotkey.win,
        };
        if let Some(hook) = &self.hotkey_hook {
            hook.update_hotkey(binding);
            self.hotkey_error = None;
            return true;
        }
        match Self::install_hotkey_hook(self.hwnd, binding) {
            Ok(hook) => {
                self.hotkey_hook = Some(hook);
                self.hotkey_error = None;
                self.sync_telemetry(true);
                true
            }
            Err(e) => {
                let msg = format!("Global hotkey hook failed: {e}");
                eprintln!("{msg}");
                self.hotkey_error = Some(msg.clone());
                self.toast("error", msg);
                self.sync_telemetry(true);
                false
            }
        }
    }

    fn find_selected_profile(settings: &AppSettings) -> Option<&GameProfile> {
        settings
            .profiles
            .iter()
            .find(|p| p.id == settings.selected_profile_id)
            .or_else(|| settings.profiles.first())
    }

    fn attach_webview(&mut self) -> Result<(), String> {
        let queue = Rc::clone(&self.ipc_queue);
        let hwnd_raw = self.hwnd.0 as isize;
        let wv = HostWebView::create(
            self.hwnd,
            self.settings.theme,
            Rc::new(move |json| {
                queue.borrow_mut().push_back(json.to_owned());
                unsafe {
                    let target = HWND(hwnd_raw as *mut c_void);
                    let _ = PostMessageW(Some(target), WM_APP_IPC, WPARAM(0), LPARAM(0));
                }
            }),
        )?;
        self.webview = Some(wv);
        Ok(())
    }

    fn send_event(&self, event: &HostEvent) {
        let Some(wv) = &self.webview else {
            return;
        };
        if let Ok(json) = serde_json::to_string(event) {
            wv.post_json(&json);
        }
    }

    fn toast(&self, kind: &str, message: impl Into<String>) {
        self.send_event(&HostEvent::Toast(ToastPayload {
            kind: kind.to_owned(),
            message: message.into(),
        }));
    }

    fn current_telemetry(&self, countdown: Option<u32>) -> UiTelemetryState {
        let Some((telemetry, is_active)) = with_runtime(&self.runtime, |runtime| {
            (runtime.telemetry(), runtime.is_active())
        }) else {
            return self.last_telemetry.clone().unwrap_or(UiTelemetryState {
                status: "idle".to_owned(),
                processing_backend: String::new(),
                cpu_ms: 0.0,
                processing_status: String::new(),
                countdown_seconds: None,
                target_title: None,
                foreground_title: None,
                input_width: None,
                input_height: None,
                output_width: None,
                output_height: None,
                scale_factor: None,
                captured_fps: None,
                presented_fps: None,
                error_message: self.last_error.clone(),
                engine: None,
            });
        };
        let effective_error = self.last_error.as_deref().or_else(|| {
            (!is_active)
                .then_some(self.hotkey_error.as_deref())
                .flatten()
        });
        telemetry_to_ui(&telemetry, countdown, effective_error)
    }

    fn emit_state_sync(&mut self) {
        if self.hardware_dirty {
            self.hardware = collect_hardware_info();
            self.hardware_dirty = false;
        }
        let startup = get_windows_startup(APP_STARTUP_NAME);
        let payload = StateSyncPayload {
            settings: settings_to_ui(&self.settings, startup),
            profiles: self.settings.profiles.iter().map(profile_to_ui).collect(),
            telemetry: self.current_telemetry(self.auto_scaler.active_countdown()),
            hardware: self.hardware.clone(),
            engine_config: engine_config_to_ui(&self.engine_config),
            is_admin: self.hardware.is_admin,
            windows_build: self.hardware.windows_build.clone(),
        };
        self.send_event(&HostEvent::StateSync(Box::new(payload)));
    }

    fn update_tray_status(&mut self, telemetry: &UiTelemetryState) {
        let tip = match telemetry.status.as_str() {
            "active" => match &telemetry.target_title {
                Some(title) if !title.is_empty() => format!("FlowScale - Scaling: {title}"),
                _ => "FlowScale - Scaling active".to_owned(),
            },
            "paused" => "FlowScale - Paused (focus)".to_owned(),
            "countdown" => match telemetry.countdown_seconds {
                Some(s) => format!("FlowScale - Starting in {s}s"),
                None => "FlowScale - Starting".to_owned(),
            },
            _ => "FlowScale - Idle".to_owned(),
        };
        if tip != self.last_tray_tooltip {
            if let Some(tray) = &mut self.tray {
                tray.set_tooltip(&tip);
            }
            self.last_tray_tooltip = tip;
        }
    }

    fn sync_telemetry(&mut self, force: bool) {
        let current = self.current_telemetry(self.auto_scaler.active_countdown());
        if !force && self.last_telemetry.as_ref() == Some(&current) {
            return;
        }
        self.update_tray_status(&current);
        let ui_visible =
            unsafe { IsWindowVisible(self.hwnd).as_bool() && !IsIconic(self.hwnd).as_bool() };
        let state_changed = self.last_telemetry.as_ref().is_none_or(|prev| {
            prev.status != current.status
                || prev.countdown_seconds != current.countdown_seconds
                || prev.target_title != current.target_title
                || prev.error_message != current.error_message
        });
        if force || ui_visible || state_changed {
            self.last_telemetry = Some(current.clone());
            self.send_event(&HostEvent::TelemetryTick(current));
        }
    }

    fn activate_hwnd(&mut self, target_hwnd_raw: isize) {
        if self.engine_busy {
            return;
        }
        let self_hwnd = self.hwnd.0 as isize;
        let resolved_hwnd = if target_hwnd_raw != 0 {
            target_hwnd_raw
        } else {
            let fg = get_foreground_window().0 as isize;
            if fg == self_hwnd {
                self.auto_scaler.reset();
                self.toast(
                    "warning",
                    "Switch focus to the target game or app window before starting scaling.",
                );
                self.sync_telemetry(true);
                return;
            }
            fg
        };

        if resolved_hwnd == 0 || resolved_hwnd == self_hwnd {
            self.toast("warning", "No target window detected in the foreground.");
            self.sync_telemetry(true);
            return;
        }

        let (title, exe_path) = get_window_title_and_exe(HWND(resolved_hwnd as *mut c_void));
        let profile = self
            .settings
            .find_matching_profile(&exe_path, &title)
            .or_else(|| Self::find_selected_profile(&self.settings))
            .cloned()
            .unwrap_or_default();

        self.last_error = None;
        self.activate_async(resolved_hwnd, &profile, false);
    }

    fn stop_scaling_internal(&mut self) {
        if defer_stop_if_busy(self.engine_busy, &mut self.stop_after_engine_start) {
            return;
        }
        self.auto_scaler.reset();
        let fg = get_foreground_window().0 as isize;
        if fg != 0 && fg != self.hwnd.0 as isize {
            self.auto_scaler.suppress_hwnd(fg);
        }
        self.last_error = None;
        self.stop_async();
    }

    fn save_settings_or_toast(&self) {
        if let Some(error) = &self.settings_load_error {
            self.toast(
                "error",
                format!("Settings were not saved because the existing file could not be loaded: {error}"),
            );
        } else if let Err(e) = self.persistence.save_settings(self.settings.clone()) {
            self.toast("error", format!("Failed to queue settings save: {e}"));
        }
    }

    fn emit_profile_save_result(
        &self,
        id: String,
        revision: String,
        profile: Option<UiScalingProfile>,
        message: Option<String>,
    ) {
        self.send_event(&HostEvent::ProfileUpserted(Box::new(ProfileUpsertedPayload {
            id,
            revision,
            ok: profile.is_some(),
            profile,
            message,
        })));
    }

    fn save_profile_upsert(&self, profile: &GameProfile, revision: String) {
        let ui_profile = profile_to_ui(profile);
        if let Some(error) = &self.settings_load_error {
            self.emit_profile_save_result(
                profile.id.clone(),
                revision,
                None,
                Some(format!("Existing settings file was preserved: {error}")),
            );
        } else if let Err(error) =
            self.persistence
                .save_profile(self.settings.clone(), ui_profile, revision.clone())
        {
            self.emit_profile_save_result(profile.id.clone(), revision, None, Some(error));
        }
    }

    fn restore_window(&mut self) {
        unsafe {
            let cmd = if IsIconic(self.hwnd).as_bool() {
                SW_RESTORE
            } else {
                SW_SHOW
            };
            let _ = ShowWindow(self.hwnd, cmd);
            let _ = SetForegroundWindow(self.hwnd);
        }
        self.sync_telemetry(true);
    }

    fn handle_config(&mut self, action: ConfigAction) -> Result<Option<&'static str>, String> {
        let path = AppSettings::default_settings_path().with_file_name("configs.json");
        let mut library = ConfigLibrary::read(&path)?;
        let message = match action {
            ConfigAction::List => None,
            ConfigAction::Import { document } => {
                library.import(PortableConfig::parse(&document)?)?;
                configs::write_json(&path, &library)?;
                Some("Config imported.")
            }
            ConfigAction::Delete { id } => {
                library.delete(id)?;
                configs::write_json(&path, &library)?;
                Some("Config removed from the loader.")
            }
            ConfigAction::Load { id, profile_id } => {
                if let Some(error) = &self.settings_load_error {
                    return Err(format!("Cannot update the profile: {error}"));
                }
                let mut next = self.settings.clone();
                let slot = next.profiles.iter_mut().find(|profile| profile.id == profile_id)
                    .ok_or("The target profile no longer exists.")?;
                let loaded = library.get(id)?.apply(slot)?;
                *slot = loaded.clone();
                self.persistence.commit_settings(next.clone())?;
                self.emit_pending_profile_results();
                self.settings = next;
                if self.settings.selected_profile_id == profile_id {
                    self.apply_runtime_profile(&loaded);
                }
                self.emit_state_sync();
                Some("Config loaded.")
            }
            ConfigAction::Export { .. } => return Ok(None), // Handled outside the controller borrow.
        };
        self.send_event(&HostEvent::ConfigsResult(bridge::ConfigsResult { entries: Some(library.list()) }));
        Ok(message)
    }

    fn handle_command(&mut self, cmd: UiCommand) {
        match cmd {
            UiCommand::GetInitialState(_) => {
                self.emit_state_sync();
                if let Some(err) = self.settings_load_error.clone() {
                    self.toast(
                        "error",
                        format!("Settings file was preserved but could not be loaded: {err}"),
                    );
                }
                if let Some(err) = self.engine_config_load_error.clone() {
                    self.toast(
                        "error",
                        format!("config.ini was preserved but could not be loaded: {err}"),
                    );
                }
                if let Some(err) = self.hotkey_error.clone() {
                    self.toast("error", err);
                }
            }
            UiCommand::SaveSettings(ui_settings) => {
                if let Err(e) = validate_ui_theme(&ui_settings.theme) {
                    self.toast("error", e);
                    self.emit_state_sync();
                    return;
                }
                let prev_pref = self.settings.gpu_preference;
                let prev_theme = self.settings.theme;
                let prev_startup = self.settings.start_at_windows_startup;
                let prev_profile_id = self.settings.selected_profile_id.clone();

                apply_ui_settings(&mut self.settings, &ui_settings);
                if self.settings.selected_profile_id != prev_profile_id {
                    if let Some(profile) = Self::find_selected_profile(&self.settings).cloned() {
                        self.apply_runtime_profile(&profile);
                    }
                }

                if self.settings.theme != prev_theme {
                    set_dwm_window_theme(self.hwnd, self.settings.theme);
                    if let Some(wv) = &self.webview {
                        wv.apply_theme_background(self.settings.theme);
                    }
                }
                if self.settings.start_at_windows_startup != prev_startup {
                    if let Ok(exe) = std::env::current_exe() {
                        if let Err(e) = set_windows_startup(
                            APP_STARTUP_NAME,
                            &exe.to_string_lossy(),
                            self.settings.start_at_windows_startup,
                        ) {
                            self.toast("error", format!("Startup registration failed: {e}"));
                        }
                    }
                }
                if self.settings.gpu_preference != prev_pref
                    && !set_windows_gpu_preference(self.settings.gpu_preference)
                {
                    self.toast(
                        "warning",
                        "Could not update Windows DirectX GPU preference.",
                    );
                }
                self.apply_hotkey_binding();
                self.save_settings_or_toast();
            }
            UiCommand::SelectProfile(payload) => {
                if self.settings.profiles.iter().any(|p| p.id == payload.id) {
                    self.settings.selected_profile_id = payload.id;
                    if let Some(p) = Self::find_selected_profile(&self.settings).cloned() {
                        self.apply_runtime_profile(&p);
                    }
                    self.save_settings_or_toast();
                }
            }
            UiCommand::UpsertProfile(ui_profile) => {
                if ui_profile
                    .client_revision
                    .as_deref()
                    .is_some_and(|revision| revision.is_empty() || revision.len() > 64)
                {
                    self.toast("error", "Invalid profile revision token.");
                    self.emit_state_sync();
                    return;
                }
                if let Err(e) = validate_ui_profile(&ui_profile) {
                    self.toast("error", e);
                    if let Some(revision) = ui_profile.client_revision.clone() {
                        self.emit_profile_save_result(
                            ui_profile.id.clone(),
                            revision,
                            None,
                            Some("Profile values were rejected by the host.".to_owned()),
                        );
                    }
                    self.emit_state_sync();
                    return;
                }
                let existing = self
                    .settings
                    .profiles
                    .iter()
                    .find(|p| p.id == ui_profile.id);
                let game = ui_to_game_profile(&ui_profile, existing);
                if let Some(slot) = self.settings.profiles.iter_mut().find(|p| p.id == game.id) {
                    *slot = game.clone();
                } else {
                    self.settings.profiles.push(game.clone());
                }
                self.settings.ensure_default_profile();
                if self.settings.selected_profile_id == game.id {
                    self.apply_runtime_profile(&game);
                }
                if let Some(revision) = ui_profile.client_revision {
                    self.save_profile_upsert(&game, revision);
                } else {
                    self.save_settings_or_toast();
                }
            }
            UiCommand::DeleteProfile(payload) => {
                if payload.id == "default" {
                    return;
                }
                self.settings.profiles.retain(|p| p.id != payload.id);
                self.settings.ensure_default_profile();
                if let Some(p) = Self::find_selected_profile(&self.settings).cloned() {
                    self.apply_runtime_profile(&p);
                }
                self.save_settings_or_toast();
            }
            UiCommand::DuplicateProfile(payload) => {
                let Some(src) = self
                    .settings
                    .profiles
                    .iter()
                    .find(|p| p.id == payload.id)
                    .cloned()
                else {
                    return;
                };
                let base_slug = slugify_profile_id(&format!("{}-copy", src.title));
                let mut new_id = base_slug.clone();
                let mut n = 2usize;
                while self.settings.profiles.iter().any(|p| p.id == new_id) {
                    new_id = format!("{base_slug}-{n}");
                    n += 1;
                }
                let mut copy = src.clone();
                copy.id = new_id.clone();
                copy.title = format!("{} copy", src.title);
                copy.auto_scale = false;
                self.settings.profiles.push(copy.clone());
                self.settings.selected_profile_id = new_id;
                self.apply_runtime_profile(&copy);
                self.save_settings_or_toast();
                self.emit_state_sync();
                self.toast("success", format!("Duplicated {}", src.title));
            }
            UiCommand::ActivateScaling(payload) => {
                self.activate_hwnd(payload.hwnd.unwrap_or(0));
            }
            UiCommand::StopScaling(_) => {
                self.stop_scaling_internal();
            }
            UiCommand::CancelCountdown(_) => {
                let fg = get_foreground_window().0 as isize;
                if fg != 0 && fg != self.hwnd.0 as isize {
                    self.auto_scaler.suppress_hwnd(fg);
                } else {
                    self.auto_scaler.reset();
                }
                self.sync_telemetry(true);
            }
            UiCommand::SetHotkey(ui_hk) => match ui_to_hotkey(&ui_hk) {
                Ok(hk) => {
                    self.settings.hotkey = hk;
                    let hook_ok = self.apply_hotkey_binding();
                    self.save_settings_or_toast();
                    if hook_ok {
                        self.toast("success", format!("Hotkey updated to {}", ui_hk.display));
                    }
                }
                Err(e) => {
                    self.toast("error", e);
                    self.emit_state_sync();
                }
            },
            UiCommand::Configs(action) => match self.handle_config(action) {
                Ok(Some(message)) => self.toast("success", message),
                Ok(None) => {},
                Err(error) => {
                    self.toast("error", error);
                    self.send_event(&HostEvent::ConfigsResult(bridge::ConfigsResult { entries: None }));
                }
            },
            UiCommand::PickExecutable(_) => {
                // Processed in `process_ipc_queue` without holding a controller borrow.
            }
            UiCommand::ImportLosslessXml(_) => {
                // Processed cleanly in `process_ipc_queue` when falling back to file dialog.
            }
            UiCommand::SaveEngineConfig(ui_cfg) => {
                if let Some(error) = &self.engine_config_load_error {
                    self.toast(
                        "error",
                        format!("config.ini was not saved because the existing file could not be loaded: {error}"),
                    );
                    return;
                }
                let next = ui_to_engine_config(&ui_cfg, &self.engine_config);
                self.engine_config = next.clone();
                self.apply_runtime_config(next.clone());
                match self.persistence.save_engine_config(next) {
                    Ok(()) => self.toast("success", "Engine config.ini save queued"),
                    Err(e) => self.toast("error", e),
                }
            }
            UiCommand::ResetEngineConfig(_) => {
                let def = EngineConfig::default();
                self.engine_config = def.clone();
                self.apply_runtime_config(def.clone());
                self.emit_state_sync();
                match self.persistence.save_engine_config(def) {
                    Ok(()) => {
                        self.toast(
                            "info",
                            format!(
                                "Reset {} (restart FlowScale to reload it)",
                                config_ini_path().display()
                            ),
                        );
                    }
                    Err(e) => self.toast("error", e),
                }
            }
            UiCommand::SetWindowsGpuPreference(payload) => {
                let pref = payload.preference.clamp(0, 2);
                if self.settings.gpu_preference != pref {
                    self.settings.gpu_preference = pref;
                    self.settings.gpu_preference_change_count += 1;
                }
                self.save_settings_or_toast();
                if set_windows_gpu_preference(pref) {
                    self.toast("success", "Windows DirectX GPU preference updated");
                } else {
                    self.toast(
                        "error",
                        "Failed to write Windows UserGpuPreferences registry key",
                    );
                    self.emit_state_sync();
                }
            }
            UiCommand::RelaunchAsAdmin(_) => {
                // Processed in `process_ipc_queue` without holding a controller borrow.
            }
            UiCommand::OpenExternalUrl(payload) => {
                if let Err(e) = open_external_url(self.hwnd, &payload.url) {
                    self.toast("error", e);
                }
            }
        }
    }

    fn apply_imported_xml(
        &mut self,
        imported_hk: Option<flowscale_config::HotkeyConfig>,
        imported_profiles: Vec<GameProfile>,
    ) {
        if let Some(hk) = imported_hk {
            self.settings.hotkey = hk;
            self.apply_hotkey_binding();
        }

        let mut added_count = 0usize;
        for mut gp in imported_profiles {
            if gp.id == "default" {
                if let Some(def) = self
                    .settings
                    .profiles
                    .iter_mut()
                    .find(|p| p.id == "default")
                {
                    *def = gp;
                }
                continue;
            }
            if self.settings.profiles.iter().any(|p| p.id == gp.id) {
                let mut n = 2usize;
                let base = gp.id.clone();
                while self
                    .settings
                    .profiles
                    .iter()
                    .any(|p| p.id == format!("{base}-{n}"))
                {
                    n += 1;
                }
                gp.id = format!("{base}-{n}");
            }
            self.settings.profiles.push(gp);
            added_count += 1;
        }
        self.settings.ensure_default_profile();
        if let Some(profile) = Self::find_selected_profile(&self.settings).cloned() {
            self.apply_runtime_profile(&profile);
        }
        self.save_settings_or_toast();
        self.emit_state_sync();
        self.toast(
            "success",
            format!("Imported Settings.xml ({added_count} custom profile(s))"),
        );
    }

    fn emit_pending_profile_results(&self) {
        for result in self.persistence.take_profile_results() {
            match result.profile {
                Ok(profile) => {
                    self.emit_profile_save_result(result.id, result.revision, Some(profile), None)
                }
                Err(error) => {
                    self.emit_profile_save_result(result.id, result.revision, None, Some(error))
                }
            }
        }
    }

    fn handle_timer_tick(&mut self) {
        self.emit_pending_profile_results();
        for error in self.persistence.take_errors() {
            self.toast("error", format!("Failed to save configuration: {error}"));
        }
        if self.engine_busy {
            return;
        }
        self.apply_pending_runtime_settings();
        let now = Instant::now();
        let _ = self.fg_watcher.poll_change();
        let fg = self.fg_watcher.current().clone();
        let is_active = self.runtime_is_active();
        let main_hwnd = self.hwnd.0 as isize;
        if fg.hwnd != 0 && fg.hwnd != main_hwnd {
            self.last_external_hwnd = fg.hwnd;
        }

        match self.auto_scaler.tick(
            now,
            fg.hwnd,
            &fg.exe_path,
            &fg.title,
            is_active,
            main_hwnd,
            &self.settings,
        ) {
            AutoScaleAction::None => {
                self.sync_telemetry(false);
            }
            AutoScaleAction::Countdown(sec) => {
                let t = self.current_telemetry(Some(sec));
                self.update_tray_status(&t);
                self.last_telemetry = Some(t.clone());
                self.send_event(&HostEvent::CountdownTick(bridge::CountdownPayload {
                    seconds: sec,
                }));
                self.send_event(&HostEvent::TelemetryTick(t));
            }
            AutoScaleAction::Trigger(hwnd, profile) => {
                self.last_error = None;
                self.activate_async(hwnd, &profile, true);
            }
        }
    }

    fn handle_hotkey(&mut self) {
        if defer_stop_if_busy(self.engine_busy, &mut self.stop_after_engine_start) {
        } else if self.runtime_is_active() {
            self.stop_scaling_internal();
        } else {
            let fg = get_foreground_window().0 as isize;
            if fg == 0 || fg == self.hwnd.0 as isize {
                self.toast(
                    "warning",
                    "Focus a target game or app window before pressing the scaling hotkey.",
                );
            } else {
                self.activate_hwnd(fg);
            }
        }
    }

    fn persist_window_geometry(&mut self) {
        if unsafe { IsIconic(self.hwnd) }.as_bool() {
            return;
        }
        let maximized = unsafe { IsZoomed(self.hwnd) }.as_bool();
        self.settings.window_maximized = maximized;
        if !maximized {
            let mut rc = RECT::default();
            if unsafe { GetWindowRect(self.hwnd, &mut rc) }.is_ok() {
                let w = (rc.right - rc.left).clamp(800, 3840);
                let h = (rc.bottom - rc.top).clamp(560, 2160);
                self.settings.window_width = w;
                self.settings.window_height = h;
            }
        }
        if self.settings_load_error.is_none() {
            if let Err(error) = self.persistence.save_settings(self.settings.clone()) {
                eprintln!("Failed to queue settings save: {error}");
            }
        }
    }
}

fn process_ipc_queue(state: &Rc<RefCell<Option<AppController>>>) {
    let batch = {
        let Ok(ctrl_opt) = state.try_borrow() else {
            return;
        };
        let Some(ctrl) = ctrl_opt.as_ref() else {
            return;
        };
        let mut queue = ctrl.ipc_queue.borrow_mut();
        take_ipc_batch(&mut queue)
    };
    for raw in batch {
        let cmd = match serde_json::from_str::<UiCommand>(&raw) {
            Ok(cmd) => cmd,
            Err(_) => {
                if let Some((id, revision)) = malformed_profile_revision(&raw) {
                    if let Ok(mut guard) = state.try_borrow_mut() {
                        if let Some(ctrl) = guard.as_mut() {
                            let message = "Malformed profile payload was rejected by the host.";
                            ctrl.toast("error", message);
                            ctrl.emit_profile_save_result(
                                id,
                                revision,
                                None,
                                Some(message.to_owned()),
                            );
                            ctrl.emit_state_sync();
                        }
                    }
                }
                continue;
            }
        };

        // Run native file dialogs outside any mutable borrow so nested modal WM_TIMER ticks stay safe.
        match cmd {
            UiCommand::Configs(ConfigAction::Export { profile_id }) => {
                let snapshot = state.borrow().as_ref().map(|ctrl| {
                    let config = ctrl.settings.profiles.iter().find(|p| p.id == profile_id)
                        .ok_or_else(|| "The target profile no longer exists.".to_owned())
                        .and_then(PortableConfig::from_profile);
                    (ctrl.hwnd, config)
                });
                if let Some((hwnd, config)) = snapshot {
                    let result = config.and_then(|config| {
                        let Some(path) = win_host::save_config_dialog(hwnd) else { return Ok(false); };
                        let path = PathBuf::from(path);
                        let settings = AppSettings::default_settings_path();
                        let protected = [settings.clone(), settings.with_file_name("configs.json"), config_ini_path()];
                        if protected.iter().any(|file| configs::same_destination(file, &path)) {
                            return Err("Choose a location outside FlowScale's own configuration files.".into());
                        }
                        configs::write_json(&path, &config)?;
                        Ok(true)
                    });
                    if let Some(ctrl) = state.borrow().as_ref() {
                        match result {
                            Ok(true) => ctrl.toast("success", "Config exported."),
                            Ok(false) => {},
                            Err(error) => ctrl.toast("error", error),
                        }
                        ctrl.send_event(&HostEvent::ConfigsResult(bridge::ConfigsResult { entries: None }));
                    }
                }
            }
            UiCommand::PickExecutable(_) => {
                let hwnd = state.borrow().as_ref().map(|c| c.hwnd).unwrap_or_default();
                if let Some(path) = pick_file_dialog(
                    hwnd,
                    "Select application executable",
                    &[("Executables (*.exe)", "*.exe"), ("All Files (*.*)", "*.*")],
                ) {
                    if let Some(ctrl) = state.borrow().as_ref() {
                        ctrl.send_event(&HostEvent::PickedExecutable(PickedPathPayload { path }));
                    }
                }
            }
            UiCommand::ImportLosslessXml(_) => {
                let default_res = import_lossless_default_xml();
                match default_res {
                    Ok((hk, profiles)) => {
                        if let Some(ctrl) = state.borrow_mut().as_mut() {
                            ctrl.apply_imported_xml(hk, profiles);
                        }
                    }
                    Err(_) => {
                        let hwnd = state.borrow().as_ref().map(|c| c.hwnd).unwrap_or_default();
                        if let Some(xml_path) = pick_file_dialog(
                            hwnd,
                            "Select Settings.xml to import",
                            &[("XML Files (*.xml)", "*.xml"), ("All Files (*.*)", "*.*")],
                        ) {
                            match import_lossless_settings_xml(&xml_path) {
                                Ok((hk, profiles)) => {
                                    if let Some(ctrl) = state.borrow_mut().as_mut() {
                                        ctrl.apply_imported_xml(hk, profiles);
                                    }
                                }
                                Err(e) => {
                                    if let Some(ctrl) = state.borrow().as_ref() {
                                        ctrl.toast("error", e);
                                    }
                                }
                            }
                        } else if let Some(ctrl) = state.borrow().as_ref() {
                            ctrl.toast("info", "Settings.xml import canceled.");
                        }
                    }
                }
            }
            UiCommand::RelaunchAsAdmin(_) => {
                let hwnd = {
                    let mut guard = state.borrow_mut();
                    if let Some(ctrl) = guard.as_mut() {
                        ctrl.save_settings_or_toast();
                        if let Some(guard_lock) = ctrl.single_instance.take() {
                            guard_lock.release_early();
                        }
                        ctrl.hwnd
                    } else {
                        continue;
                    }
                };
                match relaunch_current_exe_as_admin(Some(hwnd), None) {
                    Ok(()) => {
                        if let Some(ctrl) = state.borrow_mut().as_mut() {
                            ctrl.force_exit = true;
                        }
                        unsafe {
                            let _ = DestroyWindow(hwnd);
                        }
                    }
                    Err(e) => {
                        if let Some(ctrl) = state.borrow_mut().as_mut() {
                            ctrl.single_instance =
                                SingleInstanceGuard::acquire_or_activate_existing()
                                    .ok()
                                    .flatten();
                            ctrl.toast("error", e);
                        }
                    }
                }
            }
            other => {
                if let Some(ctrl) = state.borrow_mut().as_mut() {
                    ctrl.handle_command(other);
                }
            }
        }
    }
    let repost = state.try_borrow().ok().and_then(|guard| {
        guard
            .as_ref()
            .filter(|ctrl| !ctrl.ipc_queue.borrow().is_empty())
            .map(|ctrl| ctrl.hwnd)
    });
    if let Some(hwnd) = repost {
        unsafe {
            let _ = PostMessageW(Some(hwnd), WM_APP_IPC, WPARAM(0), LPARAM(0));
        }
    }
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<Option<AppController>>;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }

    // Safety: pointer lives for the full lifetime of `hwnd` inside `main`.
    let state = Rc::from_raw(ptr);
    let result = dispatch_window_message(&state, hwnd, msg, wparam, lparam);
    let _ = Rc::into_raw(state);
    result
}

unsafe fn dispatch_window_message(
    state: &Rc<RefCell<Option<AppController>>>,
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if let Ok(mut guard) = state.try_borrow_mut() {
        if let Some(ctrl) = guard.as_mut() {
            if msg == ctrl.restore_msg && ctrl.restore_msg != 0 {
                ctrl.restore_window();
                return LRESULT(0);
            }
            if msg == ctrl.taskbar_created_msg && ctrl.taskbar_created_msg != 0 {
                let tip = ctrl.last_tray_tooltip.clone();
                let recreated = ctrl.tray.as_mut().is_some_and(|tray| tray.recreate(&tip));
                if !recreated && !IsWindowVisible(hwnd).as_bool() {
                    ctrl.restore_window();
                }
                return LRESULT(0);
            }
        }
    }

    match msg {
        WM_SIZE => {
            if let Ok(mut guard) = state.try_borrow_mut() {
                if let Some(ctrl) = guard.as_mut() {
                    if wparam.0 == SIZE_MINIMIZED as usize {
                        if should_hide_to_tray(ctrl.settings.minimize_to_tray, ctrl.tray_active()) {
                            let _ = ShowWindow(hwnd, SW_HIDE);
                        }
                    } else {
                        ctrl.settings.window_maximized = wparam.0 == SIZE_MAXIMIZED as usize;
                        if let Some(wv) = &ctrl.webview {
                            wv.resize_to_parent(hwnd);
                        }
                        ctrl.sync_telemetry(true);
                    }
                }
            }
            LRESULT(0)
        }
        WM_DISPLAYCHANGE => {
            if let Ok(mut guard) = state.try_borrow_mut() {
                if let Some(ctrl) = guard.as_mut() {
                    ctrl.hardware_dirty = true;
                    ctrl.emit_state_sync();
                }
            }
            LRESULT(0)
        }
        WM_TIMER => {
            if wparam.0 == TIMER_ID {
                if let Ok(mut guard) = state.try_borrow_mut() {
                    if let Some(ctrl) = guard.as_mut() {
                        ctrl.handle_timer_tick();
                    }
                }
            }
            LRESULT(0)
        }
        WM_APP_HOTKEY => {
            if let Ok(mut guard) = state.try_borrow_mut() {
                if let Some(ctrl) = guard.as_mut() {
                    ctrl.handle_hotkey();
                }
            }
            LRESULT(0)
        }
        WM_APP_IPC => {
            process_ipc_queue(state);
            LRESULT(0)
        }
        WM_APP_ENGINE_COMPLETE => {
            if let Ok(mut guard) = state.try_borrow_mut() {
                if let Some(ctrl) = guard.as_mut() {
                    ctrl.finish_activation(wparam.0);
                }
            }
            LRESULT(0)
        }
        WM_APP_TRAY => {
            let ev = (lparam.0 & 0xFFFF) as u32;
            match ev {
                WM_LBUTTONUP | WM_LBUTTONDBLCLK => {
                    if let Ok(mut guard) = state.try_borrow_mut() {
                        if let Some(ctrl) = guard.as_mut() {
                            ctrl.restore_window();
                        }
                    }
                }
                WM_RBUTTONUP | WM_CONTEXTMENU => {
                    let menu_state = state.try_borrow().ok().and_then(|g| {
                        g.as_ref()
                            .filter(|ctrl| ctrl.tray_active())
                            .map(|ctrl| ctrl.runtime_is_active())
                    });
                    let chosen = menu_state.and_then(|active| show_tray_context_menu(hwnd, active));
                    if let Some(cmd) = chosen {
                        match cmd {
                            TRAY_CMD_SHOW => {
                                if let Ok(mut g) = state.try_borrow_mut() {
                                    if let Some(ctrl) = g.as_mut() {
                                        ctrl.restore_window();
                                    }
                                }
                            }
                            TRAY_CMD_TOGGLE_SCALE => {
                                if let Ok(mut g) = state.try_borrow_mut() {
                                    if let Some(ctrl) = g.as_mut() {
                                        if ctrl.runtime_is_active() {
                                            ctrl.stop_scaling_internal();
                                        } else {
                                            let target = ctrl.last_external_hwnd;
                                            let valid_target = target != 0
                                                && target != ctrl.hwnd.0 as isize
                                                && unsafe {
                                                    IsWindow(Some(HWND(target as *mut c_void)))
                                                }
                                                .as_bool();
                                            ctrl.activate_hwnd(if valid_target {
                                                target
                                            } else {
                                                0
                                            });
                                        }
                                    }
                                }
                            }
                            TRAY_CMD_EXIT => {
                                if let Ok(mut g) = state.try_borrow_mut() {
                                    if let Some(ctrl) = g.as_mut() {
                                        ctrl.force_exit = true;
                                        ctrl.persist_window_geometry();
                                    }
                                }
                                let _ = DestroyWindow(hwnd);
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_SETTINGCHANGE => {
            if let Ok(guard) = state.try_borrow() {
                if let Some(ctrl) = guard.as_ref() {
                    if ctrl.settings.theme == ThemeMode::System {
                        set_dwm_window_theme(hwnd, ThemeMode::System);
                        if let Some(wv) = &ctrl.webview {
                            wv.apply_theme_background(ThemeMode::System);
                        }
                    }
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_CLOSE => {
            let hide_only = {
                let Ok(mut guard) = state.try_borrow_mut() else {
                    return LRESULT(0);
                };
                if let Some(ctrl) = guard.as_mut() {
                    ctrl.persist_window_geometry();
                    should_hide_to_tray(ctrl.settings.close_to_tray, ctrl.tray_active())
                        && !ctrl.force_exit
                } else {
                    false
                }
            };
            if hide_only {
                let _ = ShowWindow(hwnd, SW_HIDE);
            } else {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            let _ = KillTimer(Some(hwnd), TIMER_ID);
            if let Ok(mut guard) = state.try_borrow_mut() {
                if let Some(mut ctrl) = guard.take() {
                    ctrl.shutting_down.store(true, Ordering::Release);
                    let runtime = Arc::clone(&ctrl.runtime);
                    let shutdown_complete = Arc::clone(&ctrl.shutdown_complete);
                    if thread::Builder::new()
                        .name("FlowScale shutdown".to_owned())
                        .spawn(move || {
                            if let Ok(mut runtime) = runtime.lock() {
                                runtime.uninit();
                            }
                            shutdown_complete.store(true, Ordering::Release);
                        })
                        .is_err()
                    {
                        if let Ok(mut runtime) = ctrl.runtime.lock() {
                            runtime.uninit();
                        }
                        ctrl.shutdown_complete.store(true, Ordering::Release);
                    }
                    drop(ctrl.hotkey_hook.take());
                    drop(ctrl.webview.take());
                    drop(ctrl.tray.take());
                }
            }
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn run_headless_smoke() -> Result<(), String> {
    let settings = AppSettings::load_or_default();
    let cfg = load_engine_config();
    let hw = collect_hardware_info();
    let mut rt = EngineRuntime::new();
    if !rt.init(None) {
        return Err("EngineRuntime::init returned false".to_owned());
    }
    rt.apply_engine_config(cfg);
    if let Some(first) = settings.profiles.first() {
        rt.apply_game_profile(first);
    }
    let telemetry = telemetry_to_ui(&rt.telemetry(), None, None);
    if telemetry.status != "idle" || hw.dwm_refresh_rate <= 0.0 {
        return Err("Unexpected smoke test telemetry/hardware output".to_owned());
    }
    rt.uninit();
    Ok(())
}

fn main() {
    if std::env::args().any(|a| a == "--headless-smoke") {
        if let Err(err) = run_headless_smoke() {
            eprintln!("headless smoke failed: {err}");
            std::process::exit(1);
        }
        return;
    }

    enable_dpi_awareness();
    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };

    let no_elevate_retry = std::env::args().any(|a| a == "--no-elevate-retry");
    let boot_settings = AppSettings::load_or_default();
    if boot_settings.start_as_admin
        && !flowscale_core::is_admin()
        && !no_elevate_retry
        && relaunch_current_exe_as_admin(None, Some("--no-elevate-retry")).is_ok()
    {
        unsafe { CoUninitialize() };
        return;
    }

    let Ok(Some(single_instance)) = SingleInstanceGuard::acquire_or_activate_existing() else {
        unsafe { CoUninitialize() };
        return;
    };

    let restore_msg = restore_window_message_id();
    let taskbar_created_msg = taskbar_created_message_id();
    let initial_w = boot_settings.window_width.clamp(800, 3840);
    let initial_h = boot_settings.window_height.clamp(560, 2160);

    let hmod = unsafe { GetModuleHandleW(None) }.unwrap_or_default();
    let big_icon = load_app_icon(0);
    let small_icon = load_app_icon(16);
    let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default();

    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(wnd_proc),
        hInstance: hmod.into(),
        hIcon: big_icon,
        hIconSm: small_icon,
        hCursor: cursor,
        lpszClassName: WINDOW_CLASS_NAME,
        ..Default::default()
    };
    unsafe {
        RegisterClassExW(&wc);
    }

    let title_w = to_wide_null("FlowScale");
    let hwnd = match unsafe {
        CreateWindowExW(
            Default::default(),
            WINDOW_CLASS_NAME,
            PCWSTR(title_w.as_ptr()),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            initial_w,
            initial_h,
            None,
            None,
            Some(hmod.into()),
            None,
        )
    } {
        Ok(h) => h,
        Err(_) => {
            unsafe { CoUninitialize() };
            return;
        }
    };

    if restore_msg != 0 {
        unsafe {
            let _ = ChangeWindowMessageFilterEx(hwnd, restore_msg, MSGFLT_ALLOW, None);
        }
    }
    if taskbar_created_msg != 0 {
        unsafe {
            let _ = ChangeWindowMessageFilterEx(hwnd, taskbar_created_msg, MSGFLT_ALLOW, None);
        }
    }

    if big_icon != HICON::default() {
        unsafe {
            let _ = send_message_w_icon(hwnd, ICON_BIG, big_icon);
        }
    }
    if small_icon != HICON::default() {
        unsafe {
            let _ = send_message_w_icon(hwnd, ICON_SMALL, small_icon);
        }
    }

    let state = Rc::new(RefCell::new(Some(AppController::new(
        hwnd,
        restore_msg,
        taskbar_created_msg,
        single_instance,
    ))));
    let shutdown_complete = Arc::clone(
        &state
            .borrow()
            .as_ref()
            .expect("controller initialized")
            .shutdown_complete,
    );
    let raw_ptr = Rc::into_raw(Rc::clone(&state));
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, raw_ptr as isize);
    }

    let initial_theme = state
        .borrow()
        .as_ref()
        .map_or(ThemeMode::System, |c| c.settings.theme);
    set_dwm_window_theme(hwnd, initial_theme);

    let attach_res = {
        let mut guard = state.borrow_mut();
        guard
            .as_mut()
            .map_or(Err("Controller missing".to_owned()), |ctrl| {
                ctrl.attach_webview()
            })
    };
    if let Err(err) = attach_res {
        show_error_box(
            "FlowScale",
            &format!("Failed to initialize Microsoft Edge WebView2 runtime.\n\n{err}"),
        );
        if let Some(ctrl) = state.borrow_mut().as_mut() {
            ctrl.force_exit = true;
        }
        unsafe {
            let _ = DestroyWindow(hwnd);
            let _ = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            drop(Rc::from_raw(raw_ptr));
            CoUninitialize();
        }
        std::process::exit(1);
    }

    unsafe {
        let _ = SetTimer(Some(hwnd), TIMER_ID, TIMER_INTERVAL_MS, None);
    }

    let (start_min, start_max, tray_active) =
        state.borrow().as_ref().map_or((false, false, false), |c| {
            (
                c.settings.start_minimized,
                c.settings.window_maximized,
                c.tray_active(),
            )
        });
    if !should_hide_to_tray(start_min, tray_active) {
        unsafe {
            let _ = ShowWindow(hwnd, if start_max { SW_SHOWMAXIMIZED } else { SW_SHOW });
        }
    }

    let mut msg = MSG::default();
    loop {
        let res = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if res.0 <= 0 {
            break;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    if let Some(ctrl) = state.borrow_mut().take() {
        if let Err(error) = ctrl.persistence.flush() {
            eprintln!("Failed to flush configuration saves: {error}");
        }
        for error in ctrl.persistence.take_errors() {
            eprintln!("Failed to save configuration during shutdown: {error}");
        }
        ctrl.shutting_down.store(true, Ordering::Release);
        match ctrl.runtime.lock() {
            Ok(mut runtime) => runtime.uninit(),
            Err(poisoned) => poisoned.into_inner().uninit(),
        }
        ctrl.shutdown_complete.store(true, Ordering::Release);
    }

    while !shutdown_complete.load(Ordering::Acquire) {
        thread::sleep(Duration::from_millis(10));
    }

    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        let _ = Rc::from_raw(raw_ptr);
        CoUninitialize();
    }
}

unsafe fn send_message_w_icon(hwnd: HWND, slot: u32, icon: HICON) -> LRESULT {
    windows::Win32::UI::WindowsAndMessaging::SendMessageW(
        hwnd,
        WM_SETICON,
        Some(WPARAM(slot as usize)),
        Some(LPARAM(icon.0 as isize)),
    )
}

#[cfg(test)]
mod runtime_worker_tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn owner_runtime_access_and_stop_request_do_not_wait_for_worker() {
        let runtime = Arc::new(Mutex::new(EngineRuntime::new()));
        let worker_runtime = Arc::clone(&runtime);
        let (locked_tx, locked_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let _guard = worker_runtime.lock().unwrap();
            locked_tx.send(()).unwrap();
            let _ = release_rx.recv();
        });
        locked_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        let owner_runtime = Arc::clone(&runtime);
        let (result_tx, result_rx) = mpsc::channel();
        let owner = thread::spawn(move || {
            let telemetry = with_runtime(&owner_runtime, EngineRuntime::telemetry).is_some();
            let config = with_runtime_mut(&owner_runtime, |runtime| {
                runtime.apply_engine_config(EngineConfig::default());
            })
            .is_some();
            result_tx.send((telemetry, config)).unwrap();
        });
        let owner_result = result_rx.recv_timeout(Duration::from_millis(250));
        release_tx.send(()).unwrap();
        owner.join().unwrap();
        worker.join().unwrap();
        assert_eq!(owner_result.unwrap(), (false, false));

        let mut stop_after_start = false;
        assert!(defer_stop_if_busy(true, &mut stop_after_start));
        assert!(stop_after_start);
    }

    #[test]
    fn ipc_batch_is_bounded_and_preserves_fifo_order() {
        let mut queue = (0..(IPC_BATCH_LIMIT + 3))
            .map(|n| n.to_string())
            .collect::<VecDeque<_>>();
        let first = take_ipc_batch(&mut queue);
        assert_eq!(first.len(), IPC_BATCH_LIMIT);
        assert_eq!(first.first().map(String::as_str), Some("0"));
        assert_eq!(first.last().map(String::as_str), Some("31"));
        assert_eq!(queue.into_iter().collect::<Vec<_>>(), ["32", "33", "34"]);
    }

    #[test]
    fn malformed_upsert_with_revision_can_be_rejected_without_orphaning_draft() {
        let raw = r#"{"type":"upsert_profile","payload":{"id":"game","client_revision":"9","scaling_type":42}}"#;
        assert_eq!(
            malformed_profile_revision(raw),
            Some(("game".to_owned(), "9".to_owned()))
        );
        assert_eq!(
            malformed_profile_revision(r#"{"type":"save_settings","payload":{}}"#),
            None
        );
        assert_eq!(
            malformed_profile_revision(
                r#"{"type":"upsert_profile","payload":{"id":"game","client_revision":""}}"#
            ),
            None
        );
    }

    #[test]
    fn persistence_worker_flushes_the_latest_ordered_snapshot() {
        let path = std::env::temp_dir()
            .join(format!("flowscale-worker-{}", std::process::id()))
            .join("settings.json");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
        let worker = PersistenceWorker::new();
        let first = AppSettings { window_width: 900, ..AppSettings::default() };
        let last = AppSettings { window_width: 1300, ..AppSettings::default() };
        worker
            .sender
            .send(PersistenceJob::Settings(path.clone(), first, None))
            .unwrap();
        worker
            .sender
            .send(PersistenceJob::Settings(path.clone(), last, None))
            .unwrap();
        worker.flush().unwrap();
        assert_eq!(
            AppSettings::load_from_path(&path).unwrap().window_width,
            1300
        );
        drop(worker);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn dropping_persistence_worker_drains_queued_save() {
        let path = std::env::temp_dir()
            .join(format!("flowscale-worker-drop-{}", std::process::id()))
            .join("settings.json");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
        let worker = PersistenceWorker::new();
        let settings = AppSettings { window_width: 1377, ..AppSettings::default() };
        worker
            .sender
            .send(PersistenceJob::Settings(path.clone(), settings, None))
            .unwrap();
        drop(worker);
        assert_eq!(
            AppSettings::load_from_path(&path).unwrap().window_width,
            1377
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn delayed_writer_does_not_block_host_queue_heartbeat() {
        let path=std::env::temp_dir().join(format!("flowscale-delayed-save-{}",std::process::id())).join("settings.json");
        let worker=PersistenceWorker::new();
        let (started_tx,started_rx)=mpsc::channel();
        let (release_tx,release_rx)=mpsc::channel();
        worker.sender.send(PersistenceJob::Pause(started_tx,release_rx)).unwrap();
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let before=std::time::Instant::now();
        let last = AppSettings { window_width: 1444, ..AppSettings::default() };
        worker.sender.send(PersistenceJob::Settings(path.clone(),last,None)).unwrap();
        let queued_in=before.elapsed();
        let mut heartbeat=0;
        let mut queue=(0..96).map(|n|n.to_string()).collect();
        for _ in 0..3 { heartbeat+=take_ipc_batch(&mut queue).len(); }
        let not_written=!path.exists();
        release_tx.send(()).unwrap();
        worker.flush().unwrap();
        assert_eq!(heartbeat,96);
        assert!(not_written && queued_in<Duration::from_millis(50));
        assert_eq!(AppSettings::load_from_path(&path).unwrap().window_width,1444);
        drop(worker);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
