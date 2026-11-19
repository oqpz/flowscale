//! Clean-room C ABI types, enums, and profile structures for `flowscale`.

#![allow(clippy::too_many_arguments)]

#[cfg(not(windows))]
compile_error!("FlowScale currently supports Windows only.");

use serde::{Deserialize, Serialize};

/// Scaling dimension mode.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ScalingMode {
    #[default]
    Auto = 0,
    Custom = 1,
}

impl TryFrom<i32> for ScalingMode {
    type Error = i32;
    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Auto),
            1 => Ok(Self::Custom),
            other => Err(other),
        }
    }
}

/// Output aspect/fit mode.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ScalingFitMode {
    #[default]
    AspectRatio = 0,
    Fullscreen = 1,
}

impl TryFrom<i32> for ScalingFitMode {
    type Error = i32;
    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::AspectRatio),
            1 => Ok(Self::Fullscreen),
            other => Err(other),
        }
    }
}

/// Spatial upscaling algorithm.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ScalingType {
    #[default]
    Off = 0,
    Ls1 = 1,
    Fsr = 2,
    Nis = 3,
    Sgsr = 4,
    BicubicCas = 5,
    Anime4k = 6,
    Xbr = 7,
    SharpBilinear = 8,
    Integer = 9,
    Nearest = 10,
}

impl TryFrom<i32> for ScalingType {
    type Error = i32;
    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Off),
            1 => Ok(Self::Ls1),
            2 => Ok(Self::Fsr),
            3 => Ok(Self::Nis),
            4 => Ok(Self::Sgsr),
            5 => Ok(Self::BicubicCas),
            6 => Ok(Self::Anime4k),
            7 => Ok(Self::Xbr),
            8 => Ok(Self::SharpBilinear),
            9 => Ok(Self::Integer),
            10 => Ok(Self::Nearest),
            other => Err(other),
        }
    }
}

/// FSR variant subtype.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FsrSubtype {
    #[default]
    Original = 0,
    Optimized = 1,
}

/// LS1 quality/performance subtype.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Ls1Subtype {
    #[default]
    Balanced = 0,
    Performance = 1,
}

/// Anime4K preset size.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Anime4kSubtype {
    #[default]
    S = 0,
    M = 1,
    L = 2,
    Vl = 3,
    Ul = 4,
}

/// Frame generation engine selection.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FrameGenerationType {
    #[default]
    Off = 0,
    Lsfg3 = 1,
    Lsfg2 = 2,
    Lsfg1 = 3,
}

impl TryFrom<i32> for FrameGenerationType {
    type Error = i32;
    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Off),
            1 => Ok(Self::Lsfg3),
            2 | 4 => Ok(Self::Lsfg2),
            3 | 5 => Ok(Self::Lsfg1),
            other => Err(other),
        }
    }
}

/// Frame generation quality/performance preset.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FrameGenerationSize {
    #[default]
    Balanced = 0,
    Performance = 1,
}

impl TryFrom<i32> for FrameGenerationSize {
    type Error = i32;
    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Balanced),
            1 => Ok(Self::Performance),
            other => Err(other),
        }
    }
}

/// LSFG 2.x fixed multiplier mode.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Lsfg2Mode {
    #[default]
    X2 = 0,
    X3 = 1,
    X4 = 2,
}

/// LSFG 3.x scheduling mode.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Lsfg3Mode {
    #[default]
    Fixed = 0,
    Adaptive = 1,
}

/// Presentation sync mode.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SyncMode {
    Off = 0,
    #[default]
    Default = 1,
    Vsync1 = 2,
    Vsync2 = 3,
    Vsync3 = 4,
    Vsync4 = 5,
}

impl TryFrom<i32> for SyncMode {
    type Error = i32;
    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Off),
            1 => Ok(Self::Default),
            2 => Ok(Self::Vsync1),
            3 => Ok(Self::Vsync2),
            4 => Ok(Self::Vsync3),
            5 => Ok(Self::Vsync4),
            other => Err(other),
        }
    }
}

/// Frame capture backend API.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CaptureApi {
    #[default]
    Dxgi = 0,
    Wgc = 1,
    Gdi = 2,
}

impl TryFrom<i32> for CaptureApi {
    type Error = i32;
    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Dxgi),
            1 => Ok(Self::Wgc),
            2 => Ok(Self::Gdi),
            other => Err(other),
        }
    }
}

/// Scaling lifecycle status reported to the host callback.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum StatusCode {
    #[default]
    Disabled = 0,
    TempDisabled = 1,
    Active = 2,
}

/// Error codes reported to the host callback.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ErrorCode {
    #[default]
    NoCode = 0,
    ResizeFailed = 1,
    LargeOffset = 2,
    CaptureFailed = 3,
}

/// C ABI status listener callback signature (`__cdecl` / `extern "C"`).
pub type StatusCallback = unsafe extern "C" fn(
    status: i32,
    hwnd: isize,
    input_width: i32,
    input_height: i32,
    output_width: i32,
    output_height: i32,
    resized: u8,
    scale_factor: f32,
    error_code: i32,
);

/// Structured status event emitted by the core engine.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StatusEvent {
    pub status: StatusCode,
    pub hwnd: isize,
    pub input_width: i32,
    pub input_height: i32,
    pub output_width: i32,
    pub output_height: i32,
    pub resized: bool,
    pub scale_factor: f32,
    pub error_code: ErrorCode,
}

impl StatusEvent {
    /// Invokes a C ABI status callback if provided.
    ///
    /// # Safety
    /// `callback` must be a valid function pointer safe to call from the engine thread.
    pub unsafe fn emit(&self, callback: Option<StatusCallback>) {
        if let Some(cb) = callback {
            cb(
                self.status as i32,
                self.hwnd,
                self.input_width,
                self.input_height,
                self.output_width,
                self.output_height,
                u8::from(self.resized),
                self.scale_factor,
                self.error_code as i32,
            );
        }
    }
}

/// Complete scaling & frame generation profile passed via `ApplySettings`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScalingProfile {
    pub scaling_mode: ScalingMode,
    pub scaling_fit_mode: ScalingFitMode,
    pub scaling_type: ScalingType,
    pub scaling_subtype: i32,
    pub scale_factor: f32,
    pub resize_before_scale: bool,
    pub sharpness: i32,
    pub vrs: bool,
    pub frame_generation_type: FrameGenerationType,
    pub frame_generation_size: FrameGenerationSize,
    pub frame_generation_mode: i32,
    pub frame_generation_multiplier: f32,
    pub frame_generation_target: f32,
    pub frame_generation_flow_scale: i32,
    pub clip_cursor: bool,
    pub cursor_sensitivity: bool,
    pub hide_cursor: bool,
    pub scale_cursor: bool,
    pub sync_mode: SyncMode,
    pub max_frame_latency: i32,
    pub gsync_support: bool,
    pub hdr_support: bool,
    pub capture_api: CaptureApi,
    pub queue_target: i32,
    pub draw_fps: bool,
    pub gpu_id: i32,
    pub display_id: i32,
    pub capture_offset_left: i32,
    pub capture_offset_top: i32,
    pub capture_offset_right: i32,
    pub capture_offset_bottom: i32,
    pub multi_display_mode: bool,
    pub windowed_mode: bool,
    pub setup_phase: i32,
}

impl Default for ScalingProfile {
    fn default() -> Self {
        Self {
            scaling_mode: ScalingMode::Auto,
            scaling_fit_mode: ScalingFitMode::AspectRatio,
            scaling_type: ScalingType::Off,
            scaling_subtype: 0,
            scale_factor: 1.5,
            resize_before_scale: false,
            sharpness: 5,
            vrs: false,
            frame_generation_type: FrameGenerationType::Off,
            frame_generation_size: FrameGenerationSize::Balanced,
            frame_generation_mode: 0,
            frame_generation_multiplier: 2.0,
            frame_generation_target: 60.0,
            frame_generation_flow_scale: 100,
            clip_cursor: true,
            cursor_sensitivity: false,
            hide_cursor: false,
            scale_cursor: false,
            sync_mode: SyncMode::Default,
            max_frame_latency: 3,
            gsync_support: true,
            hdr_support: false,
            capture_api: CaptureApi::Dxgi,
            queue_target: 1,
            draw_fps: true,
            gpu_id: -1,
            display_id: -1,
            capture_offset_left: 0,
            capture_offset_top: 0,
            capture_offset_right: 0,
            capture_offset_bottom: 0,
            multi_display_mode: false,
            windowed_mode: false,
            setup_phase: 0,
        }
    }
}

fn clamp_finite_f32(value: f32, min: f32, max: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        fallback
    }
}

impl ScalingProfile {
    /// Constructs a [`ScalingProfile`] from the 33 scalar C ABI arguments of `ApplySettings`.
    pub fn from_abi_args(
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
    ) -> Self {
        let scaling_type_enum = ScalingType::try_from(scaling_type).unwrap_or_default();
        let clamped_scale = clamp_finite_f32(scale_factor, 1.0, 10.0, 1.0);
        let scale_factor = match scaling_type_enum {
            ScalingType::Integer => clamped_scale.round().clamp(1.0, 10.0),
            ScalingType::Xbr => {
                let tenths = (clamped_scale * 10.0).round() as i32;
                let even_tenths = tenths - (tenths % 2);
                ((even_tenths as f32) / 10.0).clamp(1.0, 10.0)
            }
            _ => clamped_scale,
        };
        Self {
            scaling_mode: ScalingMode::try_from(scaling_mode).unwrap_or_default(),
            scaling_fit_mode: ScalingFitMode::try_from(scaling_fit_mode).unwrap_or_default(),
            scaling_type: scaling_type_enum,
            scaling_subtype,
            scale_factor,
            resize_before_scale: resize_before_scale != 0,
            sharpness: if scaling_type_enum == ScalingType::Ls1 {
                sharpness.clamp(0, 4)
            } else {
                sharpness.clamp(0, 10)
            },
            vrs: vrs != 0,
            frame_generation_type: FrameGenerationType::try_from(frame_generation_type)
                .unwrap_or_default(),
            frame_generation_size: FrameGenerationSize::try_from(frame_generation_size)
                .unwrap_or_default(),
            frame_generation_mode,
            frame_generation_multiplier: clamp_finite_f32(
                frame_generation_multiplier,
                1.0,
                20.0,
                2.0,
            ),
            frame_generation_target: clamp_finite_f32(
                frame_generation_target,
                10.0,
                1000.0,
                120.0,
            ),
            frame_generation_flow_scale: frame_generation_flow_scale.clamp(25, 100),
            clip_cursor: clip_cursor != 0,
            cursor_sensitivity: cursor_sensitivity != 0,
            hide_cursor: hide_cursor != 0,
            scale_cursor: scale_cursor != 0,
            sync_mode: SyncMode::try_from(sync_mode).unwrap_or_default(),
            max_frame_latency: max_frame_latency.clamp(1, 15),
            gsync_support: gsync_support != 0,
            hdr_support: hdr_support != 0,
            capture_api: CaptureApi::try_from(capture_api).unwrap_or_default(),
            queue_target: queue_target.clamp(0, 2),
            draw_fps: draw_fps != 0,
            gpu_id: gpu_id.max(-1),
            display_id: display_id.max(-1),
            capture_offset_left: capture_offset_left.max(0),
            capture_offset_top: capture_offset_top.max(0),
            capture_offset_right: capture_offset_right.max(0),
            capture_offset_bottom: capture_offset_bottom.max(0),
            multi_display_mode: multi_display_mode != 0,
            windowed_mode: false,
            setup_phase,
        }
    }

    /// Returns the effective LSFG multiplier for fixed or adaptive mode.
    pub fn effective_lsfg_multiplier(&self, input_fps: f32) -> f32 {
        match self.frame_generation_type {
            FrameGenerationType::Off => 1.0,
            FrameGenerationType::Lsfg1 => 2.0,
            FrameGenerationType::Lsfg2 => match self.frame_generation_mode {
                1 => 3.0,
                2 => 4.0,
                _ => 2.0,
            },
            FrameGenerationType::Lsfg3 => {
                if self.frame_generation_mode == Lsfg3Mode::Adaptive as i32 {
                    if input_fps.is_finite() && input_fps > 0.0 && self.frame_generation_target.is_finite() {
                        (self.frame_generation_target / input_fps).clamp(1.0, 20.0)
                    } else {
                        1.0
                    }
                } else {
                    clamp_finite_f32(self.frame_generation_multiplier, 1.0, 20.0, 2.0)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicI32, Ordering};

    static LAST_STATUS: AtomicI32 = AtomicI32::new(-1);
    static LAST_ERROR: AtomicI32 = AtomicI32::new(-1);

    unsafe extern "C" fn test_cb(
        status: i32,
        _hwnd: isize,
        _iw: i32,
        _ih: i32,
        _ow: i32,
        _oh: i32,
        _resized: u8,
        _sf: f32,
        error_code: i32,
    ) {
        LAST_STATUS.store(status, Ordering::SeqCst);
        LAST_ERROR.store(error_code, Ordering::SeqCst);
    }

    #[test]
    fn enum_try_from_roundtrips() {
        assert_eq!(ScalingType::try_from(1), Ok(ScalingType::Ls1));
        assert_eq!(ScalingType::try_from(10), Ok(ScalingType::Nearest));
        assert_eq!(ScalingType::try_from(99), Err(99));
        assert_eq!(FrameGenerationType::try_from(4), Ok(FrameGenerationType::Lsfg2));
        assert_eq!(FrameGenerationType::try_from(5), Ok(FrameGenerationType::Lsfg1));
        assert_eq!(CaptureApi::try_from(1), Ok(CaptureApi::Wgc));
    }

    #[test]
    fn from_abi_args_clamps_and_maps() {
        let profile = ScalingProfile::from_abi_args(
            1, 1, 2, 1, 2.5, 1, 8, 0, 1, 1, 1, 3.0, 144.0, 50, 1, 0, 0, 1, 2, 4, 1, 1, 1, 2, 1,
            1, 2, 10, 20, 30, 40, 1, 0,
        );
        assert_eq!(profile.scaling_mode, ScalingMode::Custom);
        assert_eq!(profile.scaling_type, ScalingType::Fsr);
        assert_eq!(profile.frame_generation_type, FrameGenerationType::Lsfg3);
        assert_eq!(profile.capture_api, CaptureApi::Wgc);
        assert_eq!(profile.queue_target, 2);
        assert!((profile.effective_lsfg_multiplier(60.0) - 2.4).abs() < 1e-5);

        let xbr_profile = ScalingProfile::from_abi_args(
            1, 0, ScalingType::Xbr as i32, 0, 1.8, 0, 0, 0, 0, 0, 0, 2.0, 0.0, 50, 0, 0, 0, 0, 0,
            1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        );
        assert!((xbr_profile.scale_factor - 1.8).abs() < 1e-5);
    }

    #[test]
    fn status_event_emits_c_callback() {
        let ev = StatusEvent {
            status: StatusCode::Active,
            hwnd: 0x1234,
            input_width: 1280,
            input_height: 720,
            output_width: 2560,
            output_height: 1440,
            resized: true,
            scale_factor: 2.0,
            error_code: ErrorCode::NoCode,
        };
        unsafe { ev.emit(Some(test_cb)) };
        assert_eq!(LAST_STATUS.load(Ordering::SeqCst), 2);
        assert_eq!(LAST_ERROR.load(Ordering::SeqCst), 0);
    }
}
