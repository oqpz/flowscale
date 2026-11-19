//! Global low-level keyboard hotkey hook (`WH_KEYBOARD_LL`), VK mapping,
//! and foreground window title/executable watcher.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use windows::core::PWSTR;
use windows::Win32::Foundation::{
    CloseHandle, HANDLE, HWND, LPARAM, LRESULT, STILL_ACTIVE, WPARAM,
};
use windows::Win32::System::Threading::{
    GetCurrentThreadId, GetExitCodeProcess, OpenProcess, QueryFullProcessImageNameW,
    PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VkKeyScanW, VK_CONTROL, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MENU,
    VK_RCONTROL, VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, GetWindowTextW, GetWindowThreadProcessId,
    PeekMessageW, PostThreadMessageW, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx,
    HC_ACTION, KBDLLHOOKSTRUCT, MSG, PM_NOREMOVE, WH_KEYBOARD_LL, WM_APP, WM_KEYDOWN, WM_KEYUP,
    WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_USER,
};

use crate::window::get_foreground_window;

/// Global toggle hotkey descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotkeyBinding {
    pub vk: u32,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
}

impl Default for HotkeyBinding {
    fn default() -> Self {
        Self {
            vk: 0x53, // 'S'
            ctrl: true,
            alt: true,
            shift: false,
            win: false,
        }
    }
}

/// Resolves a key name (e.g. `"S"`, `"F10"`, `"Space"`) to a Win32 virtual-key code.
pub fn key_name_to_vk(name: &str) -> Option<u32> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return None;
    }
    let upper = trimmed.to_ascii_uppercase();
    if let Some(num_str) = upper.strip_prefix('F') {
        if let Ok(n) = num_str.parse::<u32>() {
            if (1..=24).contains(&n) {
                return Some(0x6F + n);
            }
        }
    }
    if let Some(num_str) = upper.strip_prefix("NUMPAD") {
        if let Ok(n) = num_str.parse::<u32>() {
            if n <= 9 {
                return Some(0x60 + n);
            }
        }
    }
    let named = match upper.as_str() {
        "SPACE" => Some(0x20),
        "TAB" => Some(0x09),
        "ENTER" | "RETURN" => Some(0x0D),
        "ESC" | "ESCAPE" => Some(0x1B),
        "BACKSPACE" | "BACK" => Some(0x08),
        "INSERT" => Some(0x2D),
        "DELETE" => Some(0x2E),
        "HOME" => Some(0x24),
        "END" => Some(0x23),
        "PAGEUP" | "PRIOR" => Some(0x21),
        "PAGEDOWN" | "NEXT" => Some(0x22),
        "LEFT" => Some(0x25),
        "UP" => Some(0x26),
        "RIGHT" => Some(0x27),
        "DOWN" => Some(0x28),
        "PAUSE" => Some(0x13),
        "CAPSLOCK" => Some(0x14),
        _ => None,
    };
    if named.is_some() {
        return named;
    }

    let mut utf16 = trimmed.encode_utf16();
    if let (Some(ch), None) = (utf16.next(), utf16.next()) {
        let scan = unsafe { VkKeyScanW(ch) };
        if scan != -1 {
            return Some((scan & 0xFF) as u32);
        }
        if ch <= 0x7F && (ch as u8).is_ascii_alphanumeric() {
            return Some(u32::from((ch as u8).to_ascii_uppercase()));
        }
    }
    None
}

/// Converts a Win32 virtual-key code into a human-readable key label.
pub fn vk_to_key_name(vk: u32) -> String {
    match vk {
        0x30..=0x39 | 0x41..=0x5A => String::from(vk as u8 as char),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        0x60..=0x69 => format!("Numpad{}", vk - 0x60),
        0x20 => "Space".to_owned(),
        0x09 => "Tab".to_owned(),
        0x0D => "Enter".to_owned(),
        0x1B => "Esc".to_owned(),
        0x08 => "Backspace".to_owned(),
        0x2D => "Insert".to_owned(),
        0x2E => "Delete".to_owned(),
        0x24 => "Home".to_owned(),
        0x23 => "End".to_owned(),
        0x21 => "PageUp".to_owned(),
        0x22 => "PageDown".to_owned(),
        0x25 => "Left".to_owned(),
        0x26 => "Up".to_owned(),
        0x27 => "Right".to_owned(),
        0x28 => "Down".to_owned(),
        0x13 => "Pause".to_owned(),
        0x14 => "CapsLock".to_owned(),
        _ => format!("VK_0x{vk:02X}"),
    }
}

/// Queries `(window_title, full_exe_path)` for `hwnd`. Returns empty strings on failure.
pub fn get_window_title_and_exe(hwnd: HWND) -> (String, String) {
    if hwnd.is_invalid() || hwnd.0.is_null() {
        return (String::new(), String::new());
    }
    let title = get_window_title(hwnd);
    let pid = get_window_pid(hwnd);
    (title, get_process_exe_path(pid))
}

fn get_window_title(hwnd: HWND) -> String {
    let mut title_buf = [0u16; 512];
    let title_len = unsafe { GetWindowTextW(hwnd, &mut title_buf) };
    if title_len > 0 {
        String::from_utf16_lossy(&title_buf[..title_len as usize])
    } else {
        String::new()
    }
}

fn get_window_pid(hwnd: HWND) -> u32 {
    let mut pid = 0;
    unsafe {
        let _ = GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }
    pid
}

fn open_process_exe(pid: u32) -> Option<(isize, String)> {
    if pid == 0 {
        return None;
    }
    let Ok(proc_handle) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) })
    else {
        return None;
    };
    let mut path_buf = [0u16; 1024];
    let mut len = path_buf.len() as u32;
    let result = unsafe {
        QueryFullProcessImageNameW(
            proc_handle,
            PROCESS_NAME_WIN32,
            PWSTR(path_buf.as_mut_ptr()),
            &mut len,
        )
    };
    if result.is_ok() && len > 0 {
        Some((
            proc_handle.0 as isize,
            String::from_utf16_lossy(&path_buf[..len as usize]),
        ))
    } else {
        unsafe {
            let _ = CloseHandle(proc_handle);
        }
        None
    }
}

fn get_process_exe_path(pid: u32) -> String {
    let Some((handle, path)) = open_process_exe(pid) else {
        return String::new();
    };
    unsafe {
        let _ = CloseHandle(HANDLE(handle as *mut _));
    }
    path
}

fn cached_identity_matches(
    cached_hwnd: isize,
    cached_pid: u32,
    hwnd: isize,
    pid: u32,
    process_alive: bool,
) -> bool {
    process_alive && (cached_hwnd, cached_pid) == (hwnd, pid)
}

#[derive(Debug)]
struct CachedProcess {
    hwnd: isize,
    pid: u32,
    path: String,
    handle: isize,
}

impl Drop for CachedProcess {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(HANDLE(self.handle as *mut _));
        }
    }
}

fn process_is_alive(handle: isize) -> bool {
    let mut code = 0;
    unsafe { GetExitCodeProcess(HANDLE(handle as *mut _), &mut code) }.is_ok()
        && code == STILL_ACTIVE.0 as u32
}

/// Foreground window identity snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ForegroundWindowInfo {
    pub hwnd: isize,
    pub title: String,
    pub exe_path: String,
}

/// Tracks changes to the system foreground window and resolves its title + exe path.
#[derive(Debug, Default)]
pub struct ForegroundProcessWatcher {
    last: ForegroundWindowInfo,
    last_process: Option<CachedProcess>,
}

impl ForegroundProcessWatcher {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `Some(info)` when the foreground window or its title/exe changes.
    pub fn poll_change(&mut self) -> Option<ForegroundWindowInfo> {
        let fg = get_foreground_window();
        let hwnd = fg.0 as isize;
        let title = get_window_title(fg);
        let pid = if hwnd == 0 { 0 } else { get_window_pid(fg) };
        let cache_valid = self.last_process.as_ref().is_some_and(|cached| {
            cached_identity_matches(
                cached.hwnd,
                cached.pid,
                hwnd,
                pid,
                process_is_alive(cached.handle),
            )
        });
        if !cache_valid {
            self.last_process = open_process_exe(pid).map(|(handle, path)| CachedProcess {
                hwnd,
                pid,
                path,
                handle,
            });
        }
        let exe_path = self
            .last_process
            .as_ref()
            .filter(|cached| cached.hwnd == hwnd && cached.pid == pid)
            .map_or_else(String::new, |cached| cached.path.clone());
        let next = ForegroundWindowInfo {
            hwnd,
            title,
            exe_path,
        };
        if next != self.last {
            self.last = next.clone();
            Some(next)
        } else {
            None
        }
    }

    pub fn current(&self) -> &ForegroundWindowInfo {
        &self.last
    }
}

const WM_FLOWSCALE_HOTKEY_TRIGGER: u32 = WM_APP + 1;

struct HookEntry {
    id: u64,
    binding: Mutex<HotkeyBinding>,
    key_held: AtomicBool,
    thread_id: AtomicU32,
    callback: Arc<dyn Fn() + Send + Sync>,
}

fn hook_entries() -> &'static Mutex<Vec<Arc<HookEntry>>> {
    static ENTRIES: OnceLock<Mutex<Vec<Arc<HookEntry>>>> = OnceLock::new();
    ENTRIES.get_or_init(|| Mutex::new(Vec::new()))
}

fn is_vk_down(vk: u16) -> bool {
    (unsafe { GetAsyncKeyState(i32::from(vk)) } as u16 & 0x8000) != 0
}

fn modifier_used_in_binding(b: &HotkeyBinding, vk: u32) -> bool {
    (b.ctrl
        && (vk == u32::from(VK_CONTROL.0)
            || vk == u32::from(VK_LCONTROL.0)
            || vk == u32::from(VK_RCONTROL.0)))
        || (b.alt
            && (vk == u32::from(VK_MENU.0)
                || vk == u32::from(VK_LMENU.0)
                || vk == u32::from(VK_RMENU.0)))
        || (b.shift
            && (vk == u32::from(VK_SHIFT.0)
                || vk == u32::from(VK_LSHIFT.0)
                || vk == u32::from(VK_RSHIFT.0)))
        || (b.win && (vk == u32::from(VK_LWIN.0) || vk == u32::from(VK_RWIN.0)))
}

unsafe extern "system" fn ll_keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 && lparam.0 != 0 {
        let kb = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        let msg = wparam.0 as u32;
        let is_down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
        let is_up = msg == WM_KEYUP || msg == WM_SYSKEYUP;

        let snapshot: Vec<Arc<HookEntry>> = { hook_entries().lock().unwrap().clone() };
        if is_down {
            let ctrl = is_vk_down(VK_CONTROL.0);
            let alt = is_vk_down(VK_MENU.0);
            let shift = is_vk_down(VK_SHIFT.0);
            let win = is_vk_down(VK_LWIN.0) || is_vk_down(VK_RWIN.0);

            for entry in &snapshot {
                let b = *entry.binding.lock().unwrap();
                if kb.vkCode == b.vk
                    && ctrl == b.ctrl
                    && alt == b.alt
                    && shift == b.shift
                    && win == b.win
                    && !entry.key_held.swap(true, Ordering::SeqCst)
                {
                    let tid = entry.thread_id.load(Ordering::Acquire);
                    if tid != 0 {
                        let _ = PostThreadMessageW(
                            tid,
                            WM_FLOWSCALE_HOTKEY_TRIGGER,
                            WPARAM(entry.id as usize),
                            LPARAM(0),
                        );
                    }
                }
            }
        } else if is_up {
            for entry in &snapshot {
                let b = *entry.binding.lock().unwrap();
                if kb.vkCode == b.vk || modifier_used_in_binding(&b, kb.vkCode) {
                    entry.key_held.store(false, Ordering::SeqCst);
                }
            }
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

/// RAII global low-level keyboard hotkey hook (`WH_KEYBOARD_LL`) supporting key-hold debounce
/// and live dynamic hotkey updates.
pub struct GlobalHotkeyHook {
    entry: Arc<HookEntry>,
    thread_id: Arc<AtomicU32>,
    thread: Option<JoinHandle<()>>,
}

struct HookRegistryGuard(Option<u64>);

impl HookRegistryGuard {
    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for HookRegistryGuard {
    fn drop(&mut self) {
        if let Some(id) = self.0.take() {
            hook_entries().lock().unwrap().retain(|item| item.id != id);
        }
    }
}

impl GlobalHotkeyHook {
    /// Installs the global `WH_KEYBOARD_LL` hook on a dedicated message-loop thread.
    pub fn start(
        binding: HotkeyBinding,
        on_trigger: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self, String> {
        Self::start_with_spawn(binding, on_trigger, |run| {
            thread::Builder::new()
                .name("flowscale-hotkey".into())
                .spawn(run)
        })
    }

    fn start_with_spawn(
        binding: HotkeyBinding,
        on_trigger: impl Fn() + Send + Sync + 'static,
        spawn_fn: impl FnOnce(Box<dyn FnOnce() + Send>) -> std::io::Result<JoinHandle<()>>,
    ) -> Result<Self, String> {
        Self::start_with_spawn_timeout(binding, on_trigger, Duration::from_secs(2), spawn_fn)
    }

    fn start_with_spawn_timeout(
        binding: HotkeyBinding,
        on_trigger: impl Fn() + Send + Sync + 'static,
        timeout: Duration,
        spawn_fn: impl FnOnce(Box<dyn FnOnce() + Send>) -> std::io::Result<JoinHandle<()>>,
    ) -> Result<Self, String> {
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let entry = Arc::new(HookEntry {
            id,
            binding: Mutex::new(binding),
            key_held: AtomicBool::new(false),
            thread_id: AtomicU32::new(0),
            callback: Arc::new(on_trigger),
        });
        {
            hook_entries().lock().unwrap().push(Arc::clone(&entry));
        }
        let mut reg_guard = HookRegistryGuard(Some(id));

        let thread_id = Arc::new(AtomicU32::new(0));
        let cancel_startup = Arc::new(AtomicBool::new(false));
        let cancel_for_thread = Arc::clone(&cancel_startup);
        let tid_clone = Arc::clone(&thread_id);
        let entry_for_thread = Arc::clone(&entry);
        let (tx, rx) = mpsc::channel::<Result<(), String>>();

        let handle = spawn_fn(Box::new(move || {
            if cancel_for_thread.load(Ordering::Acquire) {
                return;
            }
            let tid = unsafe { GetCurrentThreadId() };
            // Ensure this thread's Win32 message queue exists before signaling readiness so
            // early PostThreadMessageW calls from the hook or Drop never fail.
            let mut init_msg = MSG::default();
            unsafe {
                let _ = PeekMessageW(&mut init_msg, None, WM_USER, WM_USER, PM_NOREMOVE);
            }
            entry_for_thread.thread_id.store(tid, Ordering::Release);
            tid_clone.store(tid, Ordering::SeqCst);

            let hook =
                unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(ll_keyboard_proc), None, 0) };
            match hook {
                Ok(hhook) => {
                    if cancel_for_thread.load(Ordering::Acquire) || tx.send(Ok(())).is_err() {
                        unsafe {
                            let _ = UnhookWindowsHookEx(hhook);
                        }
                        return;
                    }
                    let mut msg = MSG::default();
                    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
                        if msg.message == WM_FLOWSCALE_HOTKEY_TRIGGER && msg.hwnd.is_invalid() {
                            let target_id = msg.wParam.0 as u64;
                            let cb = hook_entries()
                                .lock()
                                .unwrap()
                                .iter()
                                .find(|item| item.id == target_id)
                                .map(|item| Arc::clone(&item.callback));
                            if let Some(cb) = cb {
                                cb();
                            }
                            continue;
                        }
                        unsafe {
                            let _ = TranslateMessage(&msg);
                            DispatchMessageW(&msg);
                        }
                    }
                    unsafe {
                        let _ = UnhookWindowsHookEx(hhook);
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(format!("SetWindowsHookExW failed: {e}")));
                }
            }
        }))
        .map_err(|e| format!("failed to spawn hotkey thread: {e}"))?;

        match rx.recv_timeout(timeout) {
            Ok(Ok(())) => {
                reg_guard.disarm();
                Ok(Self {
                    entry,
                    thread_id,
                    thread: Some(handle),
                })
            }
            Ok(Err(e)) => {
                let _ = handle.join();
                Err(e)
            }
            Err(e) => {
                cancel_startup.store(true, Ordering::Release);
                drop(handle);
                Err(format!("hotkey thread startup aborted: {e}"))
            }
        }
    }

    /// Dynamically updates the active hotkey binding without reinstalling the Win32 hook.
    pub fn update_hotkey(&self, binding: HotkeyBinding) {
        *self.entry.binding.lock().unwrap() = binding;
        self.entry.key_held.store(false, Ordering::SeqCst);
    }

    pub fn binding(&self) -> HotkeyBinding {
        *self.entry.binding.lock().unwrap()
    }
}

impl Drop for GlobalHotkeyHook {
    fn drop(&mut self) {
        hook_entries()
            .lock()
            .unwrap()
            .retain(|item| item.id != self.entry.id);
        let tid = self.thread_id.load(Ordering::SeqCst);
        if tid != 0 {
            unsafe {
                let _ = PostThreadMessageW(tid, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_failure_cleans_up_registry_and_callback() {
        let sentinel = Arc::new(());
        let weak = Arc::downgrade(&sentinel);
        let before = hook_entries().lock().unwrap().len();

        let res = GlobalHotkeyHook::start_with_spawn(
            HotkeyBinding::default(),
            move || {
                let _keep = &sentinel;
            },
            |_| Err(std::io::Error::other("simulated spawn failure")),
        );
        assert!(res.is_err());
        assert_eq!(hook_entries().lock().unwrap().len(), before);
        assert!(
            weak.upgrade().is_none(),
            "callback state leaked in hook_entries"
        );
    }

    #[test]
    fn executable_cache_misses_when_window_or_process_identity_changes() {
        assert!(cached_identity_matches(10, 20, 10, 20, true));
        assert!(!cached_identity_matches(10, 20, 11, 20, true));
        assert!(!cached_identity_matches(10, 20, 10, 21, true));
        assert!(!cached_identity_matches(10, 20, 10, 20, false));
    }

    #[test]
    fn delayed_hotkey_startup_returns_within_timeout_and_cleans_registry() {
        let binding = HotkeyBinding {
            vk: u32::MAX,
            ..HotkeyBinding::default()
        };
        let started = std::time::Instant::now();
        let result = GlobalHotkeyHook::start_with_spawn_timeout(
            binding,
            || {},
            Duration::from_millis(30),
            |run| {
                thread::Builder::new().spawn(move || {
                    thread::sleep(Duration::from_millis(100));
                    run();
                })
            },
        );
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_millis(80));
        thread::sleep(Duration::from_millis(110));
        assert!(hook_entries().lock().unwrap().iter().all(|entry| entry
            .binding
            .lock()
            .unwrap()
            .vk
            != binding.vk));
    }
}
