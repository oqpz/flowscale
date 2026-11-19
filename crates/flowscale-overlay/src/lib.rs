//! Clean-room Win32 overlay window, WinEvent watcher, global hotkey hook, DXGI/DWM info, and cursor utilities.

pub mod cursor;
pub mod dxgi_info;
pub mod hotkey;
pub mod window;

pub use cursor::{
    extract_cursor_rgba, get_mouse_speed, set_cursor_clip, CursorSnapshot, CursorSpeedGuard,
};
pub use dxgi_info::{
    enumerate_adapters, enumerate_displays, get_adapter_names_joined, get_display_names_joined,
    get_dwm_refresh_rate, get_refresh_rate_for_rect, is_windows_build_at_least,
    windows_build_number, AdapterInfo, DisplayInfo,
};
pub use hotkey::{
    get_window_title_and_exe, key_name_to_vk, vk_to_key_name, ForegroundProcessWatcher,
    ForegroundWindowInfo, GlobalHotkeyHook, HotkeyBinding,
};
pub use window::{
    get_foreground_window, resize_window_for_scale, OverlayWindow, WatchedWindowState,
    WindowEventKind, WindowEventWatcher, WindowWatcher,
};

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetLayeredWindowAttributes, GetWindowDisplayAffinity, LWA_ALPHA, WDA_EXCLUDEFROMCAPTURE,
    };

    #[test]
    fn windows_build_and_dwm_rate_are_sane() {
        assert!(is_windows_build_at_least(10240));
        assert!(!is_windows_build_at_least(999_999));
        let hz = get_dwm_refresh_rate();
        assert!((24.0..=1000.0).contains(&hz), "unexpected DWM hz: {hz}");
    }

    #[test]
    fn dxgi_enumeration_returns_displays() {
        let displays = enumerate_displays().expect("enumerate_displays failed");
        assert!(!displays.is_empty(), "expected at least 1 DXGI output");
        let adapters = enumerate_adapters(true).expect("enumerate_adapters failed");
        assert!(!adapters.is_empty(), "expected at least 1 DXGI adapter");
    }

    #[test]
    fn overlay_window_sets_affinity_and_alpha() {
        let rect = RECT {
            left: 0,
            top: 0,
            right: 320,
            bottom: 240,
        };
        let win = OverlayWindow::create(rect, true, false).expect("OverlayWindow::create failed");
        assert!(!win.hwnd().is_invalid());
        assert!(win.backdrop_hwnd().is_some());
        use windows::Win32::UI::WindowsAndMessaging::{SendMessageW, WM_DISPLAYCHANGE};
        assert!(!win.take_display_change());
        unsafe { SendMessageW(win.hwnd(), WM_DISPLAYCHANGE, None, None) };
        assert!(win.take_display_change());
        assert!(!win.take_display_change());

        for hwnd in [Some(win.hwnd()), win.backdrop_hwnd()]
            .into_iter()
            .flatten()
        {
            let mut affinity = 0u32;
            unsafe { GetWindowDisplayAffinity(hwnd, &mut affinity) }
                .expect("GetWindowDisplayAffinity failed");
            assert_eq!(affinity, WDA_EXCLUDEFROMCAPTURE.0);

            let mut alpha = 0u8;
            let mut flags =
                windows::Win32::UI::WindowsAndMessaging::LAYERED_WINDOW_ATTRIBUTES_FLAGS(0);
            unsafe { GetLayeredWindowAttributes(hwnd, None, Some(&mut alpha), Some(&mut flags)) }
                .expect("GetLayeredWindowAttributes failed");
            assert_eq!(alpha, 255);
            assert_eq!(flags, LWA_ALPHA);
        }
    }

    #[test]
    fn resize_window_for_scale_resizes_and_validates() {
        let rect = RECT {
            left: 0,
            top: 0,
            right: 640,
            bottom: 480,
        };
        let win = OverlayWindow::create(rect, false, false).expect("OverlayWindow::create failed");
        let (prev, new_rect) =
            resize_window_for_scale(win.hwnd(), 2.0).expect("resize_window_for_scale failed");
        assert_eq!(prev.right - prev.left, 640);
        assert!(new_rect.right - new_rect.left > 0);
        assert!(resize_window_for_scale(win.hwnd(), 0.5).is_err());
    }

    #[test]
    fn cursor_extraction_does_not_crash() {
        let _ = extract_cursor_rgba();
    }

    #[test]
    fn cursor_speed_guard_restores_on_pause_and_drop() {
        let Some(before) = get_mouse_speed() else {
            return;
        };
        {
            let mut guard = CursorSpeedGuard::apply(2.0).expect("CursorSpeedGuard::apply failed");
            assert_eq!(guard.original_speed(), before);
            guard.pause();
            assert_eq!(get_mouse_speed(), Some(before));
            guard.resume();
        }
        assert_eq!(get_mouse_speed(), Some(before));
    }

    #[test]
    fn hotkey_vk_mapping_and_hook_lifecycle() {
        assert_eq!(key_name_to_vk("S"), Some(0x53));
        assert_eq!(key_name_to_vk("F10"), Some(0x79));
        assert_eq!(key_name_to_vk("Space"), Some(0x20));
        assert_eq!(key_name_to_vk("CapsLock"), Some(0x14));
        assert_eq!(vk_to_key_name(0x53), "S");
        assert_eq!(vk_to_key_name(0x79), "F10");
        assert_eq!(vk_to_key_name(0x14), "CapsLock");

        let hook = GlobalHotkeyHook::start(HotkeyBinding::default(), || {})
            .expect("GlobalHotkeyHook::start failed");
        hook.update_hotkey(HotkeyBinding {
            vk: 0x79,
            ctrl: false,
            alt: true,
            shift: false,
            win: false,
        });
        assert_eq!(hook.binding().vk, 0x79);
    }

    #[test]
    fn window_watcher_and_title_exe_lookup() {
        use windows::core::w;
        use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassExW, WINDOW_EX_STYLE,
            WNDCLASSEXW, WS_POPUP,
        };

        unsafe extern "system" fn wndproc(
            hwnd: windows::Win32::Foundation::HWND,
            msg: u32,
            wparam: WPARAM,
            lparam: LPARAM,
        ) -> LRESULT {
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }

        let cls = w!("FlowScaleWatcherTestWnd");
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
                w!("WatcherTitle"),
                WS_POPUP,
                0,
                0,
                64,
                64,
                None,
                None,
                None,
                None,
            )
        }
        .expect("CreateWindowExW failed");

        let (title, exe) = get_window_title_and_exe(hwnd);
        assert_eq!(title, "WatcherTitle");
        assert!(!exe.is_empty());

        let watcher = WindowWatcher::new(hwnd, None, None);
        assert!(!watcher.is_destroyed());
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
        assert!(watcher.is_destroyed());
    }
}
