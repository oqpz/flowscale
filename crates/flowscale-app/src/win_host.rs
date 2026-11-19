//! Win32 windowing, single-instance guard, tray icon, native dialogs, and embedded WebView2 host.

use std::path::PathBuf;
use std::ptr;
use std::rc::Rc;
use std::sync::mpsc;

use flowscale_config::ThemeMode;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    CreateCoreWebView2EnvironmentWithOptions, ICoreWebView2, ICoreWebView2Controller,
    ICoreWebView2Controller2, COREWEBVIEW2_COLOR, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
};
use webview2_com::{
    CoTaskMemPWSTR, CreateCoreWebView2ControllerCompletedHandler,
    CreateCoreWebView2EnvironmentCompletedHandler, NavigationStartingEventHandler,
    NewWindowRequestedEventHandler, WebMessageReceivedEventHandler,
    WebResourceRequestedEventHandler,
};
use windows::core::{w, Interface, PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, E_POINTER, HANDLE, HWND, LPARAM, POINT, RECT,
    WPARAM,
};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, GetSaveFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR,
    OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST, OPENFILENAMEW,
};
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Shell::{
    SHCreateMemStream, ShellExecuteW, Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP,
    NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NOTIFYICONDATAW,
    NOTIFYICON_VERSION_4,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, FindWindowW, GetClientRect, GetCursorPos,
    LoadImageW, MessageBoxW, PostMessageW, RegisterWindowMessageW, SetForegroundWindow,
    TrackPopupMenu, HICON, IMAGE_ICON, LR_DEFAULTSIZE, LR_SHARED, MB_ICONERROR, MB_OK,
    MF_SEPARATOR, MF_STRING, SW_SHOWNORMAL, TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_NONOTIFY,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_NULL,
};

pub const APP_ORIGIN: &str = "https://app.flowscale.local/";
pub const WINDOW_CLASS_NAME: PCWSTR = w!("FlowScaleMainWindow");

pub const TRAY_CMD_SHOW: u32 = 1001;
pub const TRAY_CMD_TOGGLE_SCALE: u32 = 1002;
pub const TRAY_CMD_EXIT: u32 = 1003;

pub mod embedded {
    include!(concat!(env!("OUT_DIR"), "/embedded_dist.rs"));
}

/// Maps `https://app.flowscale.local/...` URIs to relative paths inside `apps/flowscale-ui/dist`.
pub fn asset_path_from_uri(uri: &str) -> Option<&str> {
    let rest = uri.strip_prefix(APP_ORIGIN)?;
    let no_query = rest.split(['?', '#']).next().unwrap_or(rest);
    let clean = no_query.trim_start_matches('/');
    if clean.is_empty() {
        Some("index.html")
    } else {
        Some(clean)
    }
}

/// Restricts external shell launches to HTTP/HTTPS URLs.
pub fn is_safe_external_url(url: &str) -> bool {
    let trimmed = url.trim();
    (trimmed.starts_with("https://") || trimmed.starts_with("http://"))
        && !trimmed.chars().any(char::is_control)
}

pub fn enable_dpi_awareness() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

pub fn to_wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn restore_window_message_id() -> u32 {
    unsafe { RegisterWindowMessageW(w!("FlowScale_RestoreWindow_v1")) }
}

pub fn taskbar_created_message_id() -> u32 {
    unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) }
}

pub fn should_hide_to_tray(setting_enabled: bool, tray_active: bool) -> bool {
    setting_enabled && tray_active
}

/// Holds the single-instance mutex for the lifetime of the primary window.
pub struct SingleInstanceGuard {
    handle: HANDLE,
}

impl SingleInstanceGuard {
    /// Returns `Some(guard)` if this process owns the primary lock, or `None` after signaling
    /// the running instance to restore its window.
    pub fn acquire_or_activate_existing() -> Result<Option<Self>, String> {
        let handle = unsafe { CreateMutexW(None, false, w!("Local\\FlowScale_SingleInstance_v1")) }
            .map_err(|e| format!("CreateMutexW failed: {e}"))?;

        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            unsafe {
                let _ = CloseHandle(handle);
            }
            if let Ok(existing) = unsafe { FindWindowW(WINDOW_CLASS_NAME, None) } {
                let msg = restore_window_message_id();
                if msg != 0 {
                    unsafe {
                        let _ = PostMessageW(Some(existing), msg, WPARAM(0), LPARAM(0));
                    }
                }
            }
            return Ok(None);
        }

        Ok(Some(Self { handle }))
    }

    pub fn release_early(self) {
        drop(self);
    }
}

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        if !self.handle.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.handle);
            }
            self.handle = HANDLE::default();
        }
    }
}

// Windows resource IDs are encoded as low-valued pointers, not dereferenced addresses.
#[allow(clippy::manual_dangling_ptr)]
pub fn load_app_icon(size_px: i32) -> HICON {
    let hmod = unsafe { GetModuleHandleW(None) }.unwrap_or_default();
    let res_id = PCWSTR(1usize as *const u16);
    let flags = if size_px <= 0 {
        LR_DEFAULTSIZE | LR_SHARED
    } else {
        LR_SHARED
    };
    unsafe {
        LoadImageW(
            Some(hmod.into()),
            res_id,
            IMAGE_ICON,
            size_px,
            size_px,
            flags,
        )
    }
    .ok()
    .map(|h| HICON(h.0))
    .unwrap_or_default()
}

pub fn system_uses_dark_mode() -> bool {
    let mut val: u32 = 1;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut val as *mut u32).cast()),
            Some(&mut size),
        )
    };
    status.is_ok() && val == 0
}

pub fn set_dwm_window_theme(hwnd: HWND, mode: ThemeMode) -> bool {
    let dark = match mode {
        ThemeMode::Dark => true,
        ThemeMode::Light => false,
        ThemeMode::System => system_uses_dark_mode(),
    };
    let flag = windows::core::BOOL::from(dark);
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&flag as *const windows::core::BOOL).cast(),
            std::mem::size_of::<windows::core::BOOL>() as u32,
        )
        .is_ok()
    }
}

pub fn open_external_url(hwnd: HWND, url: &str) -> Result<(), String> {
    if !is_safe_external_url(url) {
        return Err("Refused to open non-HTTP/HTTPS URL".to_owned());
    }
    let wide = to_wide_null(url.trim());
    let res = unsafe {
        ShellExecuteW(
            Some(hwnd),
            w!("open"),
            PCWSTR(wide.as_ptr()),
            None,
            None,
            SW_SHOWNORMAL,
        )
    };
    if (res.0 as isize) <= 32 {
        Err(format!("ShellExecuteW failed with code {}", res.0 as isize))
    } else {
        Ok(())
    }
}

pub fn show_error_box(title: &str, message: &str) {
    let title_w = to_wide_null(title);
    let msg_w = to_wide_null(message);
    unsafe {
        let _ = MessageBoxW(
            None,
            PCWSTR(msg_w.as_ptr()),
            PCWSTR(title_w.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}

pub fn relaunch_current_exe_as_admin(hwnd: Option<HWND>, args: Option<&str>) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("current_exe failed: {e}"))?;
    let exe_w = to_wide_null(&exe.to_string_lossy());
    let args_w = args.map(to_wide_null);
    let args_pcwstr = args_w
        .as_ref()
        .map_or(PCWSTR::null(), |v| PCWSTR(v.as_ptr()));
    let dir_w = exe.parent().map(|p| to_wide_null(&p.to_string_lossy()));
    let dir_pcwstr = dir_w
        .as_ref()
        .map_or(PCWSTR::null(), |v| PCWSTR(v.as_ptr()));
    let res = unsafe {
        ShellExecuteW(
            hwnd,
            w!("runas"),
            PCWSTR(exe_w.as_ptr()),
            args_pcwstr,
            dir_pcwstr,
            SW_SHOWNORMAL,
        )
    };
    if (res.0 as isize) <= 32 {
        Err("Administrator elevation was canceled or failed".to_owned())
    } else {
        Ok(())
    }
}

pub fn pick_file_dialog(owner: HWND, title: &str, filters: &[(&str, &str)]) -> Option<String> {
    file_dialog(owner, title, filters, false)
}

pub fn save_config_dialog(owner: HWND) -> Option<String> {
    file_dialog(owner, "Export FlowScale config", &[("JSON Files (*.json)", "*.json")], true)
}

fn file_dialog(owner: HWND, title: &str, filters: &[(&str, &str)], save: bool) -> Option<String> {
    let mut filter_w = Vec::new();
    for (label, pattern) in filters {
        filter_w.extend(label.encode_utf16());
        filter_w.push(0);
        filter_w.extend(pattern.encode_utf16());
        filter_w.push(0);
    }
    filter_w.push(0);

    let title_w = to_wide_null(title);
    let mut path_buf = vec![0u16; 1024];
    let json_extension = to_wide_null("json");

    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner,
        lpstrFilter: PCWSTR(filter_w.as_ptr()),
        nFilterIndex: 1,
        lpstrFile: PWSTR(path_buf.as_mut_ptr()),
        nMaxFile: path_buf.len() as u32,
        lpstrTitle: PCWSTR(title_w.as_ptr()),
        lpstrDefExt: if save { PCWSTR(json_extension.as_ptr()) } else { PCWSTR::null() },
        Flags: OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR | if save { OFN_OVERWRITEPROMPT } else { OFN_FILEMUSTEXIST },
        ..Default::default()
    };

    let ok = unsafe { if save { GetSaveFileNameW(&mut ofn) } else { GetOpenFileNameW(&mut ofn) } }.as_bool();
    if !ok {
        return None;
    }
    let end = path_buf
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(path_buf.len());
    Some(String::from_utf16_lossy(&path_buf[..end]))
}

/// RAII wrapper around the Windows notification area icon (`Shell_NotifyIconW`).
pub struct TrayIcon {
    hwnd: HWND,
    callback_msg: u32,
    icon: HICON,
    added: bool,
}

impl TrayIcon {
    pub fn new(hwnd: HWND, callback_msg: u32, tooltip: &str) -> Self {
        let icon = load_app_icon(16);
        let mut tray = Self {
            hwnd,
            callback_msg,
            icon,
            added: false,
        };
        tray.add(tooltip);
        tray
    }

    fn build_nid(&self, tooltip: &str) -> NOTIFYICONDATAW {
        let mut nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: 1,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP,
            uCallbackMessage: self.callback_msg,
            hIcon: self.icon,
            ..Default::default()
        };
        let encoded: Vec<u16> = tooltip.encode_utf16().collect();
        let copy_len = encoded.len().min(nid.szTip.len().saturating_sub(1));
        nid.szTip[..copy_len].copy_from_slice(&encoded[..copy_len]);
        nid.szTip[copy_len] = 0;
        nid
    }

    pub fn is_active(&self) -> bool {
        self.added
    }

    pub fn recreate(&mut self, tooltip: &str) -> bool {
        self.added = false;
        self.add(tooltip);
        self.added
    }

    fn add(&mut self, tooltip: &str) {
        let mut nid = self.build_nid(tooltip);
        if unsafe { Shell_NotifyIconW(NIM_ADD, &nid) }.as_bool() {
            nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            unsafe {
                let _ = Shell_NotifyIconW(NIM_SETVERSION, &nid);
            }
            self.added = true;
        } else {
            self.added = false;
        }
    }

    pub fn set_tooltip(&mut self, tooltip: &str) {
        if !self.added {
            self.add(tooltip);
            return;
        }
        let nid = self.build_nid(tooltip);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
        }
    }
}

/// Displays the tray popup menu for `hwnd` without borrowing application controller state.
pub fn show_tray_context_menu(hwnd: HWND, is_scaling_active: bool) -> Option<u32> {
    let menu = unsafe { CreatePopupMenu() }.ok()?;
    let scale_label = if is_scaling_active {
        w!("Stop Scaling")
    } else {
        w!("Scale Foreground")
    };
    unsafe {
        let _ = AppendMenuW(
            menu,
            MF_STRING,
            TRAY_CMD_SHOW as usize,
            w!("Show FlowScale"),
        );
        let _ = AppendMenuW(menu, MF_STRING, TRAY_CMD_TOGGLE_SCALE as usize, scale_label);
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(menu, MF_STRING, TRAY_CMD_EXIT as usize, w!("Exit"));

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(
            menu,
            TPM_LEFTALIGN | TPM_BOTTOMALIGN | TPM_RIGHTBUTTON | TPM_NONOTIFY | TPM_RETURNCMD,
            pt.x,
            pt.y,
            None,
            hwnd,
            None,
        );
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        (cmd.0 > 0).then_some(cmd.0 as u32)
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        if self.added {
            let nid = self.build_nid("");
            unsafe {
                let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
            }
            self.added = false;
        }
    }
}

fn webview2_user_data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("FlowScale").join("WebView2")
}

/// Hosts the embedded Astryx UI inside an Edge WebView2 control.
pub struct HostWebView {
    controller: ICoreWebView2Controller,
    webview: ICoreWebView2,
}

impl HostWebView {
    pub fn create(
        hwnd: HWND,
        initial_theme: ThemeMode,
        on_message: Rc<dyn Fn(&str)>,
    ) -> Result<Self, String> {
        let user_data_dir = webview2_user_data_dir();
        let _ = std::fs::create_dir_all(&user_data_dir);
        let user_data_w = to_wide_null(&user_data_dir.to_string_lossy());

        let environment = {
            let (tx, rx) = mpsc::channel();
            CreateCoreWebView2EnvironmentCompletedHandler::wait_for_async_operation(
                Box::new(move |handler| unsafe {
                    CreateCoreWebView2EnvironmentWithOptions(
                        PCWSTR::null(),
                        PCWSTR(user_data_w.as_ptr()),
                        None,
                        &handler,
                    )
                    .map_err(webview2_com::Error::WindowsError)
                }),
                Box::new(move |error_code, env| {
                    error_code?;
                    tx.send(env.ok_or_else(|| windows::core::Error::from(E_POINTER)))
                        .map_err(|_| windows::core::Error::from(E_POINTER))?;
                    Ok(())
                }),
            )
            .map_err(|e| format!("WebView2 environment creation failed: {e}"))?;

            rx.recv()
                .map_err(|_| "WebView2 environment channel aborted".to_owned())?
                .map_err(|e| format!("WebView2 environment error: {e}"))?
        };

        let controller = {
            let (tx, rx) = mpsc::channel();
            let env_clone = environment.clone();
            CreateCoreWebView2ControllerCompletedHandler::wait_for_async_operation(
                Box::new(move |handler| unsafe {
                    env_clone
                        .CreateCoreWebView2Controller(hwnd, &handler)
                        .map_err(webview2_com::Error::WindowsError)
                }),
                Box::new(move |error_code, ctrl| {
                    error_code?;
                    tx.send(ctrl.ok_or_else(|| windows::core::Error::from(E_POINTER)))
                        .map_err(|_| windows::core::Error::from(E_POINTER))?;
                    Ok(())
                }),
            )
            .map_err(|e| format!("WebView2 controller creation failed: {e}"))?;

            rx.recv()
                .map_err(|_| "WebView2 controller channel aborted".to_owned())?
                .map_err(|e| format!("WebView2 controller error: {e}"))?
        };

        let webview = unsafe { controller.CoreWebView2() }
            .map_err(|e| format!("CoreWebView2 handle missing: {e}"))?;

        unsafe {
            let settings = webview
                .Settings()
                .map_err(|e| format!("WebView2 Settings failed: {e}"))?;
            let _ = settings.SetAreDefaultContextMenusEnabled(false);
            let _ = settings.SetAreDevToolsEnabled(cfg!(debug_assertions));
            let _ = settings.SetIsStatusBarEnabled(false);
            let _ = settings.SetIsZoomControlEnabled(false);
            let _ = webview.AddWebResourceRequestedFilter(
                w!("https://app.flowscale.local/*"),
                COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
            );
        }

        // Serve bundled dist/ assets straight from executable memory.
        let env_for_res = environment.clone();
        unsafe {
            let mut token = 0i64;
            webview
                .add_WebResourceRequested(
                    &WebResourceRequestedEventHandler::create(Box::new(move |_sender, args| {
                        let Some(args) = args else {
                            return Ok(());
                        };
                        let Ok(req) = args.Request() else {
                            return Ok(());
                        };
                        let mut uri_pw = PWSTR(ptr::null_mut());
                        if req.Uri(&mut uri_pw).is_err() {
                            return Ok(());
                        }
                        let uri = CoTaskMemPWSTR::from(uri_pw).to_string();
                        if let Some(rel) = asset_path_from_uri(&uri) {
                            if let Some((bytes, mime)) = embedded::get_embedded_asset(rel) {
                                if let Some(stream) = SHCreateMemStream(Some(bytes)) {
                                    let headers_w = to_wide_null(&format!(
                                        "Content-Type: {mime}\r\nCache-Control: no-cache"
                                    ));
                                    if let Ok(resp) = env_for_res.CreateWebResourceResponse(
                                        &stream,
                                        200,
                                        w!("OK"),
                                        PCWSTR(headers_w.as_ptr()),
                                    ) {
                                        let _ = args.SetResponse(&resp);
                                    }
                                }
                            } else if let Some(stream) = SHCreateMemStream(Some(&[])) {
                                if let Ok(resp) = env_for_res.CreateWebResourceResponse(
                                    &stream,
                                    404,
                                    w!("Not Found"),
                                    w!("Content-Type: text/plain; charset=utf-8\r\nCache-Control: no-cache"),
                                ) {
                                    let _ = args.SetResponse(&resp);
                                }
                            }
                        }
                        Ok(())
                    })),
                    &mut token,
                )
                .map_err(|e| format!("add_WebResourceRequested failed: {e}"))?;
        }

        // Block main-frame navigations outside the embedded origin and route HTTP/HTTPS links externally.
        unsafe {
            let mut token = 0i64;
            let _ = webview.add_NavigationStarting(
                &NavigationStartingEventHandler::create(Box::new(move |_sender, args| {
                    if let Some(args) = args {
                        let mut uri_pw = PWSTR(ptr::null_mut());
                        if args.Uri(&mut uri_pw).is_ok() {
                            let uri = CoTaskMemPWSTR::from(uri_pw).to_string();
                            if !uri.starts_with(APP_ORIGIN) {
                                let _ = args.SetCancel(true);
                                if is_safe_external_url(&uri) {
                                    let _ = open_external_url(hwnd, &uri);
                                }
                            }
                        }
                    }
                    Ok(())
                })),
                &mut token,
            );
        }

        // Route any popup or target="_blank" link safely to the Windows default browser.
        unsafe {
            let mut token = 0i64;
            let _ = webview.add_NewWindowRequested(
                &NewWindowRequestedEventHandler::create(Box::new(move |_sender, args| {
                    if let Some(args) = args {
                        let _ = args.SetHandled(true);
                        let mut uri_pw = PWSTR(ptr::null_mut());
                        if args.Uri(&mut uri_pw).is_ok() {
                            let url = CoTaskMemPWSTR::from(uri_pw).to_string();
                            let _ = open_external_url(hwnd, &url);
                        }
                    }
                    Ok(())
                })),
                &mut token,
            );
        }

        // Dispatch UI -> Host JSON commands.
        unsafe {
            let mut token = 0i64;
            webview
                .add_WebMessageReceived(
                    &WebMessageReceivedEventHandler::create(Box::new(move |_sender, args| {
                        if let Some(args) = args {
                            let mut raw_pw = PWSTR(ptr::null_mut());
                            if args.TryGetWebMessageAsString(&mut raw_pw).is_ok() {
                                let json = CoTaskMemPWSTR::from(raw_pw).to_string();
                                on_message(&json);
                            }
                        }
                        Ok(())
                    })),
                    &mut token,
                )
                .map_err(|e| format!("add_WebMessageReceived failed: {e}"))?;
        }

        let host = Self {
            controller,
            webview,
        };
        host.apply_theme_background(initial_theme);
        host.resize_to_parent(hwnd);
        unsafe {
            let _ = host.controller.SetIsVisible(true);
            let nav_w = to_wide_null("https://app.flowscale.local/index.html");
            host.webview
                .Navigate(PCWSTR(nav_w.as_ptr()))
                .map_err(|e| format!("WebView2 Navigate failed: {e}"))?;
        }
        Ok(host)
    }

    pub fn apply_theme_background(&self, mode: ThemeMode) {
        let dark = match mode {
            ThemeMode::Dark => true,
            ThemeMode::Light => false,
            ThemeMode::System => system_uses_dark_mode(),
        };
        let bg = if dark {
            COREWEBVIEW2_COLOR {
                A: 255,
                R: 13,
                G: 15,
                B: 14,
            }
        } else {
            COREWEBVIEW2_COLOR {
                A: 255,
                R: 250,
                G: 250,
                B: 250,
            }
        };
        if let Ok(ctrl2) = self.controller.cast::<ICoreWebView2Controller2>() {
            unsafe {
                let _ = ctrl2.SetDefaultBackgroundColor(bg);
            }
        }
    }

    pub fn resize_to_parent(&self, hwnd: HWND) {
        let mut rc = RECT::default();
        unsafe {
            if GetClientRect(hwnd, &mut rc).is_ok() {
                let _ = self.controller.SetBounds(rc);
            }
        }
    }

    pub fn post_json(&self, json: &str) {
        let wide = to_wide_null(json);
        unsafe {
            let _ = self.webview.PostWebMessageAsJson(PCWSTR(wide.as_ptr()));
        }
    }
}

impl Drop for HostWebView {
    fn drop(&mut self) {
        unsafe {
            let _ = self.controller.Close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_embedded_uri_paths_and_blocks_unsafe_urls() {
        assert_eq!(
            asset_path_from_uri("https://app.flowscale.local/"),
            Some("index.html")
        );
        assert_eq!(
            asset_path_from_uri("https://app.flowscale.local/index.html?v=1"),
            Some("index.html")
        );
        assert_eq!(
            asset_path_from_uri("https://app.flowscale.local/assets/app.js"),
            Some("assets/app.js")
        );
        assert_eq!(asset_path_from_uri("https://example.com/"), None);

        assert!(embedded::get_embedded_asset("index.html").is_some());
        assert!(embedded::get_embedded_asset("flowscale-logo.png").is_some());
        assert!(embedded::get_embedded_asset("flowscale-banner.png").is_some());

        assert!(is_safe_external_url("https://github.com/"));
        assert!(!is_safe_external_url("file:///C:/Windows/system32/cmd.exe"));
        assert!(!is_safe_external_url("javascript:alert(1)"));
    }

    #[test]
    fn taskbar_created_msg_and_tray_hide_guard() {
        assert_ne!(taskbar_created_message_id(), 0);
        assert!(should_hide_to_tray(true, true));
        assert!(!should_hide_to_tray(true, false));
        assert!(!should_hide_to_tray(false, true));
        assert!(!should_hide_to_tray(false, false));
    }
}
