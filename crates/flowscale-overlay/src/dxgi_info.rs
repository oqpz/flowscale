//! DXGI adapter/display enumeration, DWM refresh rate, and Windows build version helpers.

use serde::{Deserialize, Serialize};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Dwm::{DwmGetCompositionTimingInfo, DWM_TIMING_INFO};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1,
};
use windows::Win32::Graphics::Gdi::{
    EnumDisplaySettingsW, GetMonitorInfoW, MonitorFromRect, DEVMODEW, ENUM_CURRENT_SETTINGS,
    MONITORINFO, MONITORINFOEXW, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::System::SystemInformation::OSVERSIONINFOW;
use windows::Win32::UI::HiDpi::{
    SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::core::{w, PCWSTR};

/// Enumerated DXGI hardware/software adapter metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterInfo {
    pub index: usize,
    pub description: String,
    pub vendor_id: u32,
    pub device_id: u32,
    pub is_software: bool,
}

/// Enumerated DXGI display output metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayInfo {
    pub index: usize,
    pub device_name: String,
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl DisplayInfo {
    pub fn width(&self) -> i32 {
        (self.right - self.left).max(0)
    }

    pub fn height(&self) -> i32 {
        (self.bottom - self.top).max(0)
    }
}

/// Enumerates DXGI adapters (excluding software WARP adapters unless `include_software` is true).
pub fn enumerate_adapters(include_software: bool) -> windows::core::Result<Vec<AdapterInfo>> {
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1()? };
    let mut out = Vec::new();
    let mut i = 0u32;
    while let Ok(adapter) = unsafe { factory.EnumAdapters1(i) } {
        if let Ok(desc) = unsafe { adapter.GetDesc1() } {
            let is_sw = (desc.Flags & (DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32)) != 0;
            if include_software || !is_sw {
                let len = desc
                    .Description
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(desc.Description.len());
                let name = String::from_utf16_lossy(&desc.Description[..len]);
                out.push(AdapterInfo {
                    index: out.len(),
                    description: name,
                    vendor_id: desc.VendorId,
                    device_id: desc.DeviceId,
                    is_software: is_sw,
                });
            }
        }
        i += 1;
    }
    Ok(out)
}

struct ThreadPmv2Guard(DPI_AWARENESS_CONTEXT);

impl ThreadPmv2Guard {
    fn enter() -> Self {
        let prev = unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        Self(prev)
    }
}

impl Drop for ThreadPmv2Guard {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = SetThreadDpiAwarenessContext(self.0);
            }
        }
    }
}

/// Enumerates DXGI display outputs across all adapters in physical (PMv2) coordinates.
pub fn enumerate_displays() -> windows::core::Result<Vec<DisplayInfo>> {
    let _dpi_guard = ThreadPmv2Guard::enter();
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1()? };
    let mut out = Vec::new();
    let mut a_idx = 0u32;
    while let Ok(adapter) = unsafe { factory.EnumAdapters1(a_idx) } {
        let mut o_idx = 0u32;
        while let Ok(output) = unsafe { adapter.EnumOutputs(o_idx) } {
            if let Ok(desc) = unsafe { output.GetDesc() } {
                let len = desc
                    .DeviceName
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(desc.DeviceName.len());
                let device_name = String::from_utf16_lossy(&desc.DeviceName[..len]);
                let r = desc.DesktopCoordinates;
                out.push(DisplayInfo {
                    index: out.len(),
                    device_name,
                    left: r.left,
                    top: r.top,
                    right: r.right,
                    bottom: r.bottom,
                });
            }
            o_idx += 1;
        }
        a_idx += 1;
    }
    Ok(out)
}

/// Returns semicolon-delimited adapter names matching `GetAdapterNames()` convention.
pub fn get_adapter_names_joined() -> String {
    enumerate_adapters(false)
        .unwrap_or_default()
        .into_iter()
        .map(|a| a.description)
        .collect::<Vec<_>>()
        .join(";")
}

/// Returns semicolon-delimited display names matching `GetDisplayNames()` convention.
pub fn get_display_names_joined() -> String {
    enumerate_displays()
        .unwrap_or_default()
        .into_iter()
        .map(|d| format!("{} ({}x{})", d.device_name, d.width(), d.height()))
        .collect::<Vec<_>>()
        .join(";")
}

/// Queries DWM composition refresh rate in Hz (fallback 60.0 Hz on failure).
pub fn get_dwm_refresh_rate() -> f32 {
    let mut info = DWM_TIMING_INFO {
        cbSize: std::mem::size_of::<DWM_TIMING_INFO>() as u32,
        ..Default::default()
    };
    let hr = unsafe { DwmGetCompositionTimingInfo(HWND::default(), &mut info) };
    if hr.is_ok() && info.rateRefresh.uiDenominator > 0 {
        let hz = (info.rateRefresh.uiNumerator as f32) / (info.rateRefresh.uiDenominator as f32);
        if hz > 1.0 {
            return hz;
        }
    }
    60.0
}

/// Queries the current refresh rate of the monitor overlapping `(left, top, right, bottom)`,
/// falling back to `get_dwm_refresh_rate()` if unavailable.
pub fn get_refresh_rate_for_rect(left: i32, top: i32, right: i32, bottom: i32) -> f32 {
    let rect = RECT {
        left,
        top,
        right,
        bottom,
    };
    let monitor = unsafe { MonitorFromRect(&rect, MONITOR_DEFAULTTONEAREST) };
    if !monitor.is_invalid() {
        let mut mi = MONITORINFOEXW {
            monitorInfo: MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFOEXW>() as u32,
                ..Default::default()
            },
            ..Default::default()
        };
        if unsafe { GetMonitorInfoW(monitor, &mut mi as *mut _ as *mut MONITORINFO) }.as_bool() {
            let mut dm = DEVMODEW {
                dmSize: std::mem::size_of::<DEVMODEW>() as u16,
                ..Default::default()
            };
            let ok = unsafe {
                EnumDisplaySettingsW(
                    PCWSTR(mi.szDevice.as_ptr()),
                    ENUM_CURRENT_SETTINGS,
                    &mut dm,
                )
            };
            if ok.as_bool() && dm.dmDisplayFrequency > 1 {
                // `GetMonitorInfoW`/`EnumDisplaySettingsW` report integer Hz; prefer DWM fractional
                // timing when it matches within 1.5 Hz (e.g., 143.997 Hz vs 144 Hz, 59.94 vs 60).
                let dm_hz = dm.dmDisplayFrequency as f32;
                let dwm_hz = get_dwm_refresh_rate();
                if (dwm_hz - dm_hz).abs() < 1.5 {
                    return dwm_hz;
                }
                return dm_hz;
            }
        }
    }
    get_dwm_refresh_rate()
}

/// Returns the running Windows OS build number via `ntdll!RtlGetVersion`.
pub fn windows_build_number() -> Option<u32> {
    type RtlGetVersionFn = unsafe extern "system" fn(*mut OSVERSIONINFOW) -> i32;
    unsafe {
        let ntdll = GetModuleHandleW(w!("ntdll.dll")).ok()?;
        let proc = GetProcAddress(ntdll, windows::core::s!("RtlGetVersion"))?;
        let rtl_get_version: RtlGetVersionFn = std::mem::transmute(proc);
        let mut info = OSVERSIONINFOW {
            dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32,
            ..Default::default()
        };
        (rtl_get_version(&mut info) == 0).then_some(info.dwBuildNumber)
    }
}

/// Checks if the running Windows OS build number is at least `min_build` via `ntdll!RtlGetVersion`.
pub fn is_windows_build_at_least(min_build: u32) -> bool {
    windows_build_number().is_some_and(|b| b >= min_build)
}
