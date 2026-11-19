//! Cursor RGBA extraction via `GetCursorInfo` + `GetIconInfo` + `GetDIBits`, `ClipCursor`,
//! and RAII cursor-speed scaling (`SPI_GETMOUSESPEED` / `SPI_SETMOUSESPEED`).

use std::ffi::c_void;
use windows::Win32::Foundation::{POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, BITMAP, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    ClipCursor, GetCursorInfo, GetIconInfo, SystemParametersInfoW, CURSORINFO, CURSOR_SHOWING,
    ICONINFO, SPI_GETMOUSESPEED, SPI_SETMOUSESPEED, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
};

/// Extracted cursor bitmap in RGBA8 (premultiplied/straight alpha depending on icon) and hotspot.
/// `screen_x/y` use the calling thread's DPI coordinate space; use PMv2 when mapping to pixels.
#[derive(Debug, Clone)]
pub struct CursorSnapshot {
    pub visible: bool,
    pub screen_x: i32,
    pub screen_y: i32,
    pub hotspot_x: i32,
    pub hotspot_y: i32,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Captures the current system cursor position, visibility, and RGBA pixels.
pub fn extract_cursor_rgba() -> Option<CursorSnapshot> {
    let mut ci = CURSORINFO {
        cbSize: std::mem::size_of::<CURSORINFO>() as u32,
        ..Default::default()
    };
    unsafe { GetCursorInfo(&mut ci).ok()? };
    let visible = (ci.flags.0 & CURSOR_SHOWING.0) != 0;
    if !visible || ci.hCursor.is_invalid() {
        return Some(CursorSnapshot {
            visible: false,
            screen_x: ci.ptScreenPos.x,
            screen_y: ci.ptScreenPos.y,
            hotspot_x: 0,
            hotspot_y: 0,
            width: 0,
            height: 0,
            rgba: Vec::new(),
        });
    }

    let mut ii = ICONINFO::default();
    unsafe { GetIconInfo(ci.hCursor.into(), &mut ii).ok()? };

    let result = extract_icon_bitmap(&ii, ci.ptScreenPos);

    unsafe {
        if !ii.hbmMask.is_invalid() {
            let _ = DeleteObject(ii.hbmMask.into());
        }
        if !ii.hbmColor.is_invalid() {
            let _ = DeleteObject(ii.hbmColor.into());
        }
    }

    result
}

fn extract_icon_bitmap(ii: &ICONINFO, pos: POINT) -> Option<CursorSnapshot> {
    let hdc = unsafe { CreateCompatibleDC(None) };
    if hdc.is_invalid() {
        return None;
    }

    let snapshot = if !ii.hbmColor.is_invalid() {
        let mut bmp = BITMAP::default();
        let got = unsafe {
            GetObjectW(
                ii.hbmColor.into(),
                std::mem::size_of::<BITMAP>() as i32,
                Some(&mut bmp as *mut _ as *mut _),
            )
        };
        if got == 0 || bmp.bmWidth <= 0 || bmp.bmHeight <= 0 {
            None
        } else {
            let w = bmp.bmWidth as u32;
            let h = bmp.bmHeight as u32;
            let Some(byte_len) = checked_rgba_bytes(w, h) else {
                unsafe {
                    let _ = DeleteDC(hdc);
                }
                return None;
            };
            let mut bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: bmp.bmWidth,
                    biHeight: -bmp.bmHeight, // top-down
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bgra = vec![0u8; byte_len];
            let lines = unsafe {
                GetDIBits(
                    hdc,
                    ii.hbmColor,
                    0,
                    h,
                    Some(bgra.as_mut_ptr() as *mut _),
                    &mut bmi,
                    DIB_RGB_COLORS,
                )
            };
            if !has_complete_rows(lines, h as i32) {
                None
            } else {
                // Convert BGRA -> RGBA
                for px in bgra.chunks_exact_mut(4) {
                    px.swap(0, 2);
                }
                // Some legacy GDI color+mask cursors leave every alpha byte at 0 in hbmColor and
                // encode transparency in the 1x-height AND plane of hbmMask. Recover alpha so the
                // software cursor remains visible when scale_cursor is active.
                let all_zero_alpha = bgra.chunks_exact(4).all(|px| px[3] == 0);
                if all_zero_alpha && !ii.hbmMask.is_invalid() {
                    let mut mask_buf = vec![0u8; byte_len];
                    let mask_lines = unsafe {
                        GetDIBits(
                            hdc,
                            ii.hbmMask,
                            0,
                            h,
                            Some(mask_buf.as_mut_ptr() as *mut _),
                            &mut bmi,
                            DIB_RGB_COLORS,
                        )
                    };
                    if has_complete_rows(mask_lines, h as i32) {
                        for (px, mask_px) in bgra.chunks_exact_mut(4).zip(mask_buf.chunks_exact(4))
                        {
                            if mask_px[0] == 0 || px[0] != 0 || px[1] != 0 || px[2] != 0 {
                                px[3] = 255;
                            }
                        }
                    }
                }
                Some(CursorSnapshot {
                    visible: true,
                    screen_x: pos.x,
                    screen_y: pos.y,
                    hotspot_x: ii.xHotspot as i32,
                    hotspot_y: ii.yHotspot as i32,
                    width: w,
                    height: h,
                    rgba: bgra,
                })
            }
        }
    } else if !ii.hbmMask.is_invalid() {
        let mut bmp = BITMAP::default();
        let got = unsafe {
            GetObjectW(
                ii.hbmMask.into(),
                std::mem::size_of::<BITMAP>() as i32,
                Some(&mut bmp as *mut _ as *mut _),
            )
        };
        if got == 0 || bmp.bmWidth <= 0 || bmp.bmHeight < 2 || bmp.bmHeight % 2 != 0 {
            None
        } else {
            let w = bmp.bmWidth as u32;
            let full_h = bmp.bmHeight as u32;
            let h = full_h / 2;
            let Some(full_byte_len) = checked_rgba_bytes(w, full_h) else {
                unsafe {
                    let _ = DeleteDC(hdc);
                }
                return None;
            };
            let mut bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: bmp.bmWidth,
                    biHeight: -(full_h as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut mask_buf = vec![0u8; full_byte_len];
            let lines = unsafe {
                GetDIBits(
                    hdc,
                    ii.hbmMask,
                    0,
                    full_h,
                    Some(mask_buf.as_mut_ptr() as *mut _),
                    &mut bmi,
                    DIB_RGB_COLORS,
                )
            };
            if !has_complete_rows(lines, full_h as i32) {
                None
            } else {
                let half = full_byte_len / 2;
                let (and_plane, xor_plane) = mask_buf.split_at(half);
                let mut rgba = vec![0u8; half];
                for idx in (0..half).step_by(4) {
                    let and_v = and_plane[idx];
                    let xor_v = xor_plane[idx];
                    if and_v == 0 && xor_v == 0 {
                        // Black opaque
                        rgba[idx + 3] = 255;
                    } else if and_v == 0 && xor_v != 0 {
                        // White opaque
                        rgba[idx] = 255;
                        rgba[idx + 1] = 255;
                        rgba[idx + 2] = 255;
                        rgba[idx + 3] = 255;
                    } else if and_v != 0 && xor_v != 0 {
                        // Inverted (render as white outline)
                        rgba[idx] = 255;
                        rgba[idx + 1] = 255;
                        rgba[idx + 2] = 255;
                        rgba[idx + 3] = 180;
                    }
                }
                Some(CursorSnapshot {
                    visible: true,
                    screen_x: pos.x,
                    screen_y: pos.y,
                    hotspot_x: ii.xHotspot as i32,
                    hotspot_y: ii.yHotspot as i32,
                    width: w,
                    height: h,
                    rgba,
                })
            }
        }
    } else {
        None
    };

    unsafe {
        let _ = DeleteDC(hdc);
    }
    snapshot
}

const MAX_CURSOR_DIMENSION: u32 = 512;
const MAX_CURSOR_BYTES: usize = 1024 * 1024;

fn checked_rgba_bytes(width: u32, height: u32) -> Option<usize> {
    if width == 0 || height == 0 || width > MAX_CURSOR_DIMENSION || height > MAX_CURSOR_DIMENSION {
        return None;
    }
    let bytes = usize::try_from(width)
        .ok()?
        .checked_mul(usize::try_from(height).ok()?)?
        .checked_mul(4)?;
    (bytes <= MAX_CURSOR_BYTES).then_some(bytes)
}

fn has_complete_rows(read_rows: i32, requested_rows: i32) -> bool {
    requested_rows > 0 && read_rows == requested_rows
}

/// Confines the cursor to `rect`, or releases confinement when `None`.
pub fn set_cursor_clip(rect: Option<RECT>) -> windows::core::Result<()> {
    unsafe { ClipCursor(rect.as_ref().map(|r| r as *const RECT)) }
}

/// Queries the current Windows pointer speed (`1..=20`).
pub fn get_mouse_speed() -> Option<u32> {
    let mut speed: u32 = 0;
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETMOUSESPEED,
            0,
            Some((&mut speed as *mut u32).cast::<c_void>()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    if ok.is_ok() && (1..=20).contains(&speed) {
        Some(speed)
    } else {
        None
    }
}

fn set_mouse_speed(speed: u32) {
    let clamped = speed.clamp(1, 20);
    unsafe {
        let _ = SystemParametersInfoW(
            SPI_SETMOUSESPEED,
            0,
            Some(clamped as usize as *mut c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
    }
}

/// RAII guard that scales Windows pointer speed proportionally during active scaling
/// and unconditionally restores the original pointer speed on drop.
pub struct CursorSpeedGuard {
    original_speed: u32,
    adjusted_speed: u32,
    applied: bool,
}

impl CursorSpeedGuard {
    /// Captures the current mouse speed and applies `original_speed / scale_factor` (clamped `1..=20`).
    pub fn apply(scale_factor: f32) -> Option<Self> {
        let original_speed = get_mouse_speed()?;
        let sf = if scale_factor.is_finite() {
            scale_factor.max(1.0)
        } else {
            1.0
        };
        let adjusted_speed = ((original_speed as f32) / sf).round().clamp(1.0, 20.0) as u32;
        if adjusted_speed != original_speed {
            set_mouse_speed(adjusted_speed);
        }
        Some(Self {
            original_speed,
            adjusted_speed,
            applied: adjusted_speed != original_speed,
        })
    }

    pub fn original_speed(&self) -> u32 {
        self.original_speed
    }

    pub fn adjusted_speed(&self) -> u32 {
        self.adjusted_speed
    }

    /// Temporarily restores the original mouse speed (e.g., when target window loses focus).
    pub fn pause(&mut self) {
        if self.applied {
            set_mouse_speed(self.original_speed);
            self.applied = false;
        }
    }

    /// Re-applies the scaled mouse speed (e.g., when target window regains focus).
    pub fn resume(&mut self) {
        if !self.applied && self.adjusted_speed != self.original_speed {
            set_mouse_speed(self.adjusted_speed);
            self.applied = true;
        }
    }
}

impl Drop for CursorSpeedGuard {
    fn drop(&mut self) {
        if self.applied {
            set_mouse_speed(self.original_speed);
            self.applied = false;
        }
    }
}

#[cfg(test)]
mod bitmap_bounds_tests {
    use super::{checked_rgba_bytes, has_complete_rows};

    #[test]
    fn rejects_zero_excessive_and_overflowing_cursor_bitmaps() {
        assert_eq!(checked_rgba_bytes(0, 1), None);
        assert_eq!(checked_rgba_bytes(1, 0), None);
        assert_eq!(checked_rgba_bytes(513, 1), None);
        assert_eq!(checked_rgba_bytes(1, 513), None);
        assert_eq!(checked_rgba_bytes(u32::MAX, u32::MAX), None);
    }

    #[test]
    fn accepts_bounded_cursor_bitmap_and_requires_every_gdi_row() {
        assert_eq!(checked_rgba_bytes(32, 16), Some(2048));
        assert_eq!(checked_rgba_bytes(512, 512), Some(1024 * 1024));
        assert!(has_complete_rows(16, 16));
        assert!(!has_complete_rows(15, 16));
        assert!(!has_complete_rows(17, 16));
    }
}
