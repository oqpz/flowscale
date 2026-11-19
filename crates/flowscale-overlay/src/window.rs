//! Win32 topmost click-through overlay window, black backdrop, and `SetWinEventHook` watcher.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Arc, Mutex, Once, OnceLock};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND,
};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, GetStockObject, MonitorFromWindow, BLACK_BRUSH, MONITORINFO,
    MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::HiDpi::{
    AdjustWindowRectExForDpi, GetDpiForWindow, GetSystemMetricsForDpi,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetForegroundWindow, GetWindow,
    GetWindowLongPtrW, GetWindowLongW, GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindow,
    RegisterClassExW, SetLayeredWindowAttributes, SetWindowDisplayAffinity, SetWindowLongPtrW,
    SetWindowPos, ShowWindow, CHILDID_SELF, EVENT_OBJECT_DESTROY, EVENT_OBJECT_LOCATIONCHANGE,
    EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZEEND, EVENT_SYSTEM_MINIMIZESTART, GWLP_USERDATA,
    GWL_EXSTYLE, GWL_STYLE, GW_OWNER, HWND_TOPMOST, LWA_ALPHA, OBJID_WINDOW, SM_CXMINTRACK,
    SM_CYMINTRACK, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SW_HIDE,
    WDA_EXCLUDEFROMCAPTURE, WDA_NONE, WINDOW_EX_STYLE, WINDOW_STYLE, WINEVENT_OUTOFCONTEXT,
    WM_DISPLAYCHANGE, WM_ERASEBKGND, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
    WS_THICKFRAME,
};

static REGISTER_OVERLAY_CLASSES: Once = Once::new();
const OVERLAY_CLASS: PCWSTR = w!("FlowScaleOverlayClass");
const BACKDROP_CLASS: PCWSTR = w!("FlowScaleBackdropClass");

unsafe extern "system" fn overlay_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_DISPLAYCHANGE {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 1);
    } else if msg == WM_ERASEBKGND {
        return LRESULT(1);
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

unsafe extern "system" fn backdrop_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // Let DefWindowProcW erase with BLACK_BRUSH on WM_ERASEBKGND so letterbox bars stay black.
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

fn ensure_classes_registered(hinstance: HINSTANCE) {
    REGISTER_OVERLAY_CLASSES.call_once(|| unsafe {
        let wc_overlay = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(overlay_wnd_proc),
            hInstance: hinstance,
            lpszClassName: OVERLAY_CLASS,
            ..Default::default()
        };
        let _ = RegisterClassExW(&wc_overlay);

        let wc_backdrop = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(backdrop_wnd_proc),
            hInstance: hinstance,
            hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH(GetStockObject(BLACK_BRUSH).0),
            lpszClassName: BACKDROP_CLASS,
            ..Default::default()
        };
        let _ = RegisterClassExW(&wc_backdrop);
    });
}

struct HwndGuard(Option<HWND>);

impl HwndGuard {
    fn new(hwnd: HWND) -> Self {
        Self(Some(hwnd))
    }

    fn hwnd(&self) -> HWND {
        self.0.unwrap_or_default()
    }

    fn disarm(&mut self) -> Option<HWND> {
        self.0.take()
    }
}

impl Drop for HwndGuard {
    fn drop(&mut self) {
        if let Some(hwnd) = self.0.take() {
            if !hwnd.is_invalid() {
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
            }
        }
    }
}

/// Topmost click-through overlay window + optional black backdrop window.
pub struct OverlayWindow {
    overlay_hwnd: HWND,
    backdrop_hwnd: Option<HWND>,
}

impl OverlayWindow {
    /// Creates a topmost, click-through, non-activating overlay window (and optional backdrop).
    pub fn create(rect: RECT, with_backdrop: bool, show: bool) -> windows::core::Result<Self> {
        Self::create_with_setup(rect, with_backdrop, show, false, configure_overlay_window)
    }

    /// Creates a GDI-compatible window for software Vulkan presentation.
    pub fn create_software(rect: RECT, show: bool) -> windows::core::Result<Self> {
        Self::create_with_setup(rect, false, show, true, configure_overlay_window)
    }

    fn create_with_setup(
        rect: RECT,
        with_backdrop: bool,
        show: bool,
        software: bool,
        mut setup: impl FnMut(HWND) -> windows::core::Result<()>,
    ) -> windows::core::Result<Self> {
        let hmodule = unsafe { GetModuleHandleW(None)? };
        let hinstance: HINSTANCE = hmodule.into();
        ensure_classes_registered(hinstance);

        let ex_style =
            WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
        let overlay_ex_style = if software { ex_style } else { ex_style | WS_EX_NOREDIRECTIONBITMAP };
        let width = (rect.right - rect.left).max(1);
        let height = (rect.bottom - rect.top).max(1);

        let mut backdrop_guard = if with_backdrop {
            let hwnd = unsafe {
                CreateWindowExW(
                    ex_style,
                    BACKDROP_CLASS,
                    w!("FlowScale Backdrop"),
                    WS_POPUP,
                    rect.left,
                    rect.top,
                    width,
                    height,
                    None,
                    None,
                    Some(hinstance),
                    None,
                )?
            };
            let guard = HwndGuard::new(hwnd);
            disable_rounded_corners(hwnd);
            setup(hwnd)?;
            Some(guard)
        } else {
            None
        };

        let overlay_hwnd = unsafe {
            CreateWindowExW(
                overlay_ex_style,
                OVERLAY_CLASS,
                w!("FlowScale Overlay"),
                WS_POPUP,
                rect.left,
                rect.top,
                width,
                height,
                None,
                None,
                Some(hinstance),
                None,
            )?
        };
        let mut overlay_guard = HwndGuard::new(overlay_hwnd);
        disable_rounded_corners(overlay_hwnd);
        setup(overlay_hwnd)?;

        if let Some(bg) = backdrop_guard.as_ref().map(HwndGuard::hwnd) {
            unsafe {
                let _ = SetWindowPos(
                    bg,
                    Some(overlay_hwnd),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
        }

        let created = Self {
            overlay_hwnd: overlay_guard.disarm().unwrap_or_default(),
            backdrop_hwnd: backdrop_guard.as_mut().and_then(HwndGuard::disarm),
        };
        if show {
            created.set_visible(true);
        }

        Ok(created)
    }

    /// Returns the raw overlay `HWND`.
    pub fn hwnd(&self) -> HWND {
        self.overlay_hwnd
    }

    /// Consumes a display change notification delivered to the overlay window.
    pub fn take_display_change(&self) -> bool {
        unsafe {
            GetWindowLongPtrW(self.overlay_hwnd, GWLP_USERDATA) != 0
                && SetWindowLongPtrW(self.overlay_hwnd, GWLP_USERDATA, 0) != 0
        }
    }

    /// Returns the raw backdrop `HWND`, if created.
    pub fn backdrop_hwnd(&self) -> Option<HWND> {
        self.backdrop_hwnd
    }

    /// Toggles `WDA_EXCLUDEFROMCAPTURE` vs `WDA_NONE` on the overlay and backdrop windows.
    /// Disabling capture exclusion allows DWM Independent Flip / tearing under WGC capture.
    pub fn set_exclude_from_capture(&self, exclude: bool) -> windows::core::Result<()> {
        let affinity = if exclude {
            WDA_EXCLUDEFROMCAPTURE
        } else {
            WDA_NONE
        };
        unsafe {
            SetWindowDisplayAffinity(self.overlay_hwnd, affinity)?;
            if let Some(bg) = self.backdrop_hwnd {
                SetWindowDisplayAffinity(bg, affinity)?;
            }
        }
        Ok(())
    }

    /// Repositions the overlay and backdrop windows without activating them.
    pub fn set_bounds(&self, rect: RECT) -> windows::core::Result<()> {
        let width = (rect.right - rect.left).max(1);
        let height = (rect.bottom - rect.top).max(1);
        unsafe {
            SetWindowPos(
                self.overlay_hwnd,
                Some(HWND_TOPMOST),
                rect.left,
                rect.top,
                width,
                height,
                SWP_NOACTIVATE,
            )?;
            if let Some(bg) = self.backdrop_hwnd {
                SetWindowPos(
                    bg,
                    Some(self.overlay_hwnd),
                    rect.left,
                    rect.top,
                    width,
                    height,
                    SWP_NOACTIVATE,
                )?;
            }
            Ok(())
        }
    }

    /// Shows or hides the overlay (and backdrop) without stealing focus or inverting Z-order.
    pub fn set_visible(&self, visible: bool) {
        unsafe {
            if visible {
                let _ = SetWindowPos(
                    self.overlay_hwnd,
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
                if let Some(bg) = self.backdrop_hwnd {
                    let _ = SetWindowPos(
                        bg,
                        Some(self.overlay_hwnd),
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                    );
                }
            } else {
                if let Some(bg) = self.backdrop_hwnd {
                    let _ = ShowWindow(bg, SW_HIDE);
                }
                let _ = ShowWindow(self.overlay_hwnd, SW_HIDE);
            }
        }
    }
}

impl Drop for OverlayWindow {
    fn drop(&mut self) {
        unsafe {
            if !self.overlay_hwnd.is_invalid() {
                let _ = DestroyWindow(self.overlay_hwnd);
            }
            if let Some(bg) = self.backdrop_hwnd {
                if !bg.is_invalid() {
                    let _ = DestroyWindow(bg);
                }
            }
        }
    }
}

fn disable_rounded_corners(hwnd: HWND) {
    let pref = DWMWCP_DONOTROUND;
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &pref as *const _ as *const _,
            std::mem::size_of_val(&pref) as u32,
        );
    }
}

fn configure_overlay_window(hwnd: HWND) -> windows::core::Result<()> {
    unsafe {
        SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA)?;
        SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE)?;
    }
    Ok(())
}

/// Resizes and centers `hwnd` to `monitor_dim / scale_factor` for `resize_before_scale`.
///
/// Returns `(previous_rect, new_rect)` on success, or restores `previous_rect` and returns `Err`
/// if the target window refuses the resize.
pub fn resize_window_for_scale(hwnd: HWND, scale_factor: f32) -> Result<(RECT, RECT), String> {
    if hwnd.is_invalid() || hwnd.0.is_null() {
        return Err("invalid target HWND".to_owned());
    }
    if !scale_factor.is_finite() || scale_factor <= 1.0 {
        return Err(format!("invalid scale_factor {scale_factor} for resize"));
    }

    let mut prev_rect = RECT::default();
    unsafe {
        GetWindowRect(hwnd, &mut prev_rect).map_err(|e| format!("GetWindowRect failed: {e}"))?;
    }

    let hmon = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    let mut mi = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if unsafe { !GetMonitorInfoW(hmon, &mut mi).as_bool() } {
        return Err("GetMonitorInfoW failed".to_owned());
    }

    let mon = mi.rcMonitor;
    let mon_w = (mon.right - mon.left).max(1);
    let mon_h = (mon.bottom - mon.top).max(1);
    let client_w = ((mon_w as f32) / scale_factor).round().max(64.0) as i32;
    let client_h = ((mon_h as f32) / scale_factor).round().max(64.0) as i32;

    let style = WINDOW_STYLE(unsafe { GetWindowLongW(hwnd, GWL_STYLE) } as u32);
    let ex_style = WINDOW_EX_STYLE(unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32);
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
    let mut adj = RECT {
        left: 0,
        top: 0,
        right: client_w,
        bottom: client_h,
    };
    let _ = unsafe { AdjustWindowRectExForDpi(&mut adj, style, false, ex_style, dpi) };
    let nc_w = (adj.right - adj.left) - client_w;
    let nc_h = (adj.bottom - adj.top) - client_h;
    let target_w = (adj.right - adj.left).clamp(64, mon_w);
    let target_h = (adj.bottom - adj.top).clamp(64, mon_h);
    let (min_track_w, min_track_h) = if (style.0 & WS_THICKFRAME.0) != 0 {
        unsafe {
            (
                GetSystemMetricsForDpi(SM_CXMINTRACK, dpi).max(64),
                GetSystemMetricsForDpi(SM_CYMINTRACK, dpi).max(64),
            )
        }
    } else {
        (64, 64)
    };
    let expected_cw = (target_w.max(min_track_w).min(mon_w) - nc_w).max(1);
    let expected_ch = (target_h.max(min_track_h).min(mon_h) - nc_h).max(1);
    let target_x = mon.left + (mon_w - target_w) / 2;
    let target_y = mon.top + (mon_h - target_h) / 2;

    let restore = |reason: String| -> Result<(RECT, RECT), String> {
        unsafe {
            let _ = SetWindowPos(
                hwnd,
                None,
                prev_rect.left,
                prev_rect.top,
                (prev_rect.right - prev_rect.left).max(1),
                (prev_rect.bottom - prev_rect.top).max(1),
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        Err(reason)
    };

    if let Err(e) = unsafe {
        SetWindowPos(
            hwnd,
            None,
            target_x,
            target_y,
            target_w,
            target_h,
            SWP_NOZORDER | SWP_NOACTIVATE,
        )
    } {
        return restore(format!("SetWindowPos failed: {e}"));
    }

    let mut client = RECT::default();
    if let Err(e) = unsafe { GetClientRect(hwnd, &mut client) } {
        return restore(format!("GetClientRect failed: {e}"));
    }
    let cw = client.right - client.left;
    let ch = client.bottom - client.top;
    const RESIZE_TOLERANCE_PX: i32 = 8;
    if cw <= 0
        || ch <= 0
        || cw > mon_w
        || ch > mon_h
        || (cw - expected_cw).abs() > RESIZE_TOLERANCE_PX
        || (ch - expected_ch).abs() > RESIZE_TOLERANCE_PX
    {
        return restore(format!(
            "target window refused resize (client {}x{}, expected ~{}x{}, monitor {}x{})",
            cw, ch, expected_cw, expected_ch, mon_w, mon_h
        ));
    }

    let mut new_rect = RECT::default();
    let _ = unsafe { GetWindowRect(hwnd, &mut new_rect) };
    Ok((prev_rect, new_rect))
}

/// Returns the current foreground window handle.
pub fn get_foreground_window() -> HWND {
    unsafe { GetForegroundWindow() }
}

/// Events observed by [`WindowEventWatcher`] and [`WindowWatcher`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowEventKind {
    ForegroundChanged,
    LocationChanged,
    MinimizeStart,
    MinimizeEnd,
    Destroyed,
}

/// RAII wrapper around `SetWinEventHook` for foreground/geometry/minimize/destroy events.
pub struct WindowEventWatcher {
    hooks: Vec<HWINEVENTHOOK>,
}

fn is_target_or_ignored_foreground(
    fg_raw: isize,
    target_hwnd: isize,
    ignore_a: isize,
    ignore_b: isize,
) -> bool {
    if fg_raw == 0 {
        return false;
    }
    if fg_raw == target_hwnd
        || (ignore_a != 0 && fg_raw == ignore_a)
        || (ignore_b != 0 && fg_raw == ignore_b)
    {
        return true;
    }
    // Keep scaling active when an owned popup/dialog of the target momentarily owns focus.
    let fg = HWND(fg_raw as *mut std::ffi::c_void);
    matches!(unsafe { GetWindow(fg, GW_OWNER) }, Ok(owner) if owner.0 as isize == target_hwnd)
}

impl WindowEventWatcher {
    /// Installs out-of-context WinEvent hooks for foreground, minimize, location, and destroy changes.
    /// Scopes per-window object hooks to `target_pid` (`0` = global) to avoid desktop-wide callback noise.
    pub fn install(
        proc: unsafe extern "system" fn(HWINEVENTHOOK, u32, HWND, i32, i32, u32, u32),
    ) -> Self {
        Self::install_for_pid(proc, 0)
    }

    pub fn install_for_pid(
        proc: unsafe extern "system" fn(HWINEVENTHOOK, u32, HWND, i32, i32, u32, u32),
        target_pid: u32,
    ) -> Self {
        let ranges = [
            (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, 0u32),
            (
                EVENT_SYSTEM_MINIMIZESTART,
                EVENT_SYSTEM_MINIMIZEEND,
                target_pid,
            ),
            (
                EVENT_OBJECT_LOCATIONCHANGE,
                EVENT_OBJECT_LOCATIONCHANGE,
                target_pid,
            ),
            (EVENT_OBJECT_DESTROY, EVENT_OBJECT_DESTROY, target_pid),
        ];
        let mut hooks = Vec::new();
        for (min, max, pid) in ranges {
            let h = unsafe {
                SetWinEventHook(min, max, None, Some(proc), pid, 0, WINEVENT_OUTOFCONTEXT)
            };
            if !h.is_invalid() {
                hooks.push(h);
            }
        }
        Self { hooks }
    }

    /// Maps a Win32 `EVENT_*` constant to [`WindowEventKind`].
    pub fn classify_event(event: u32) -> Option<WindowEventKind> {
        match event {
            EVENT_SYSTEM_FOREGROUND => Some(WindowEventKind::ForegroundChanged),
            EVENT_OBJECT_LOCATIONCHANGE => Some(WindowEventKind::LocationChanged),
            EVENT_SYSTEM_MINIMIZESTART => Some(WindowEventKind::MinimizeStart),
            EVENT_SYSTEM_MINIMIZEEND => Some(WindowEventKind::MinimizeEnd),
            EVENT_OBJECT_DESTROY => Some(WindowEventKind::Destroyed),
            _ => None,
        }
    }
}

impl Drop for WindowEventWatcher {
    fn drop(&mut self) {
        for h in self.hooks.drain(..) {
            unsafe {
                let _ = UnhookWinEvent(h);
            }
        }
    }
}

/// Snapshot of watched target window state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchedWindowState {
    pub focused: bool,
    pub minimized: bool,
    pub destroyed: bool,
    pub location_dirty: bool,
    pub foreground_hwnd: isize,
}

#[derive(Debug)]
struct WatcherShared {
    target_hwnd: isize,
    ignore_hwnd_a: isize,
    ignore_hwnd_b: isize,
    focused: AtomicBool,
    minimized: AtomicBool,
    destroyed: AtomicBool,
    location_dirty: AtomicBool,
    foreground_hwnd: AtomicIsize,
}

fn watcher_registry() -> &'static Mutex<HashMap<isize, Arc<WatcherShared>>> {
    static REG: OnceLock<Mutex<HashMap<isize, Arc<WatcherShared>>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

unsafe extern "system" fn window_watcher_proc(
    hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _event_thread: u32,
    _event_time: u32,
) {
    let state = {
        let reg = watcher_registry().lock().unwrap();
        reg.get(&(hook.0 as isize)).cloned()
    };
    let Some(state) = state else {
        return;
    };
    let hwnd_raw = hwnd.0 as isize;
    match event {
        EVENT_SYSTEM_FOREGROUND => {
            state.foreground_hwnd.store(hwnd_raw, Ordering::Relaxed);
            let is_target = is_target_or_ignored_foreground(
                hwnd_raw,
                state.target_hwnd,
                state.ignore_hwnd_a,
                state.ignore_hwnd_b,
            );
            state.focused.store(is_target, Ordering::Relaxed);
        }
        EVENT_OBJECT_DESTROY => {
            if hwnd_raw == state.target_hwnd
                && id_object == OBJID_WINDOW.0
                && id_child == CHILDID_SELF as i32
            {
                state.destroyed.store(true, Ordering::Relaxed);
            }
        }
        EVENT_SYSTEM_MINIMIZESTART => {
            if hwnd_raw == state.target_hwnd && id_object == OBJID_WINDOW.0 {
                state.minimized.store(true, Ordering::Relaxed);
            }
        }
        EVENT_SYSTEM_MINIMIZEEND => {
            if hwnd_raw == state.target_hwnd && id_object == OBJID_WINDOW.0 {
                state.minimized.store(false, Ordering::Relaxed);
            }
        }
        EVENT_OBJECT_LOCATIONCHANGE
            if hwnd_raw == state.target_hwnd
                && id_object == OBJID_WINDOW.0
                && id_child == CHILDID_SELF as i32 =>
        {
            state.location_dirty.store(true, Ordering::Relaxed);
        }
        _ => {}
    }
}

/// Thread-safe `SetWinEventHook` watcher tracking focus, minimize, move, and destroy events
/// for a target window.
pub struct WindowWatcher {
    inner: WindowEventWatcher,
    state: Arc<WatcherShared>,
}

impl WindowWatcher {
    /// Starts watching `target_hwnd` (optionally treating overlay/backdrop HWNDs as non-unfocusing).
    pub fn new(target_hwnd: HWND, overlay_hwnd: Option<HWND>, backdrop_hwnd: Option<HWND>) -> Self {
        let target_raw = target_hwnd.0 as isize;
        let fg_raw = get_foreground_window().0 as isize;
        let ign_a = overlay_hwnd.map(|h| h.0 as isize).unwrap_or(0);
        let ign_b = backdrop_hwnd.map(|h| h.0 as isize).unwrap_or(0);
        let initially_focused = is_target_or_ignored_foreground(fg_raw, target_raw, ign_a, ign_b);
        let initially_minimized = unsafe { IsIconic(target_hwnd).as_bool() };
        let initially_destroyed =
            target_raw == 0 || unsafe { !IsWindow(Some(target_hwnd)).as_bool() };
        let mut target_pid = 0u32;
        if target_raw != 0 {
            unsafe {
                let _ = GetWindowThreadProcessId(target_hwnd, Some(&mut target_pid));
            }
        }

        let state = Arc::new(WatcherShared {
            target_hwnd: target_raw,
            ignore_hwnd_a: ign_a,
            ignore_hwnd_b: ign_b,
            focused: AtomicBool::new(initially_focused),
            minimized: AtomicBool::new(initially_minimized),
            destroyed: AtomicBool::new(initially_destroyed),
            location_dirty: AtomicBool::new(false),
            foreground_hwnd: AtomicIsize::new(fg_raw),
        });

        let inner = WindowEventWatcher::install_for_pid(window_watcher_proc, target_pid);
        {
            let mut reg = watcher_registry().lock().unwrap();
            for h in &inner.hooks {
                reg.insert(h.0 as isize, Arc::clone(&state));
            }
        }

        Self { inner, state }
    }

    /// Reads the latest state, cross-checking `IsWindow` / `IsIconic` / `GetForegroundWindow`
    /// so transitions remain accurate even before WinEvent messages flush.
    pub fn snapshot(&self) -> WatchedWindowState {
        let target = HWND(self.state.target_hwnd as *mut std::ffi::c_void);
        let alive = self.state.target_hwnd != 0 && unsafe { IsWindow(Some(target)).as_bool() };
        if !alive {
            self.state.destroyed.store(true, Ordering::Relaxed);
        } else {
            let iconic = unsafe { IsIconic(target).as_bool() };
            self.state.minimized.store(iconic, Ordering::Relaxed);
            let fg_raw = get_foreground_window().0 as isize;
            self.state.foreground_hwnd.store(fg_raw, Ordering::Relaxed);
            let is_target = is_target_or_ignored_foreground(
                fg_raw,
                self.state.target_hwnd,
                self.state.ignore_hwnd_a,
                self.state.ignore_hwnd_b,
            );
            self.state.focused.store(is_target, Ordering::Relaxed);
        }
        WatchedWindowState {
            focused: self.state.focused.load(Ordering::Relaxed),
            minimized: self.state.minimized.load(Ordering::Relaxed),
            destroyed: self.state.destroyed.load(Ordering::Relaxed),
            location_dirty: self.state.location_dirty.swap(false, Ordering::Relaxed),
            foreground_hwnd: self.state.foreground_hwnd.load(Ordering::Relaxed),
        }
    }

    pub fn is_destroyed(&self) -> bool {
        self.snapshot().destroyed
    }
}

impl Drop for WindowWatcher {
    fn drop(&mut self) {
        let mut reg = watcher_registry().lock().unwrap();
        for h in &self.inner.hooks {
            reg.remove(&(h.0 as isize));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::E_FAIL;
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, FillRect, GetPixel,
        SelectObject, WHITE_BRUSH,
    };
    use windows::Win32::UI::WindowsAndMessaging::{SendMessageW, WINDOWPOS, WM_WINDOWPOSCHANGING};

    #[test]
    fn backdrop_erases_background_to_black() {
        let rect = RECT {
            left: 0,
            top: 0,
            right: 64,
            bottom: 64,
        };
        let win = OverlayWindow::create(rect, true, false).expect("OverlayWindow::create failed");
        let bg = win.backdrop_hwnd().expect("backdrop_hwnd missing");

        unsafe {
            let mem_dc = CreateCompatibleDC(None);
            let bmp = CreateCompatibleBitmap(mem_dc, 64, 64);
            let old = SelectObject(mem_dc, bmp.into());
            let fill = RECT {
                left: 0,
                top: 0,
                right: 64,
                bottom: 64,
            };
            let white = windows::Win32::Graphics::Gdi::HBRUSH(GetStockObject(WHITE_BRUSH).0);
            FillRect(mem_dc, &fill, white);

            let res = SendMessageW(
                bg,
                WM_ERASEBKGND,
                Some(WPARAM(mem_dc.0 as usize)),
                Some(LPARAM(0)),
            );
            assert_ne!(res.0, 0, "backdrop WM_ERASEBKGND should erase background");
            let px = GetPixel(mem_dc, 4, 4);
            assert_eq!(px.0, 0, "backdrop should erase to black, got {:#x}", px.0);

            SelectObject(mem_dc, old);
            let _ = DeleteObject(bmp.into());
            let _ = DeleteDC(mem_dc);
        }
    }

    #[test]
    fn overlay_create_destroys_hwnds_on_setup_error() {
        let rect = RECT {
            left: 0,
            top: 0,
            right: 120,
            bottom: 80,
        };
        let mut seen = Vec::new();
        let res = OverlayWindow::create_with_setup(rect, true, false, false, |hwnd| {
            seen.push(hwnd);
            if seen.len() == 2 {
                Err(windows::core::Error::from(E_FAIL))
            } else {
                Ok(())
            }
        });
        assert!(res.is_err());
        assert_eq!(seen.len(), 2);
        for hwnd in seen {
            assert!(
                unsafe { !IsWindow(Some(hwnd)).as_bool() },
                "leaked HWND {hwnd:?} after create_with_setup error"
            );
        }
    }

    #[test]
    fn resize_window_for_scale_restores_when_target_refuses_resize() {
        unsafe extern "system" fn fixed_size_wndproc(
            hwnd: HWND,
            msg: u32,
            wparam: WPARAM,
            lparam: LPARAM,
        ) -> LRESULT {
            if msg == WM_WINDOWPOSCHANGING && lparam.0 != 0 {
                let wp = &mut *(lparam.0 as *mut WINDOWPOS);
                wp.flags |= SWP_NOSIZE;
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }

        let cls = w!("FlowScaleFixedSizeTestWnd");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(fixed_size_wndproc),
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
                w!("Fixed"),
                WS_POPUP,
                40,
                50,
                320,
                240,
                None,
                None,
                None,
                None,
            )
        }
        .expect("CreateWindowExW failed");
        let _guard = HwndGuard::new(hwnd);

        let err = resize_window_for_scale(hwnd, 2.0)
            .expect_err("expected Err when target refuses resize");
        assert!(err.contains("refused resize"), "unexpected err: {err}");

        let mut after = RECT::default();
        unsafe { GetWindowRect(hwnd, &mut after) }.expect("GetWindowRect failed");
        assert_eq!(
            (after.left, after.top, after.right, after.bottom),
            (40, 50, 360, 290)
        );
    }

    #[test]
    fn overlay_set_exclude_from_capture_toggles_both_hwnds() {
        use windows::Win32::UI::WindowsAndMessaging::GetWindowDisplayAffinity;

        let rect = RECT {
            left: 0,
            top: 0,
            right: 160,
            bottom: 120,
        };
        let win = OverlayWindow::create(rect, true, false).expect("OverlayWindow::create failed");
        let hwnds = [win.hwnd(), win.backdrop_hwnd().expect("backdrop missing")];

        win.set_exclude_from_capture(false).unwrap();
        for hwnd in hwnds {
            let mut affinity = u32::MAX;
            unsafe { GetWindowDisplayAffinity(hwnd, &mut affinity) }
                .expect("GetWindowDisplayAffinity failed");
            assert_eq!(affinity, WDA_NONE.0);
        }

        win.set_exclude_from_capture(true).unwrap();
        for hwnd in hwnds {
            let mut affinity = 0u32;
            unsafe { GetWindowDisplayAffinity(hwnd, &mut affinity) }
                .expect("GetWindowDisplayAffinity failed");
            assert_eq!(affinity, WDA_EXCLUDEFROMCAPTURE.0);
        }
    }
}
