//! Clean-room frame capture backends (DXGI Desktop Duplication, Windows.Graphics.Capture, & GDI)
//! and bounded dual-queue ring buffer with Win32 event synchronization.

use std::collections::VecDeque;
use std::fmt;
use std::mem::size_of;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use flowscale_abi::CaptureApi;
use windows::core::{IInspectable, Interface, Result as WinResult};
use windows::Foundation::{TimeSpan, TypedEventHandler};
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::{CloseHandle, E_ACCESSDENIED, HANDLE, HWND, RECT, WAIT_OBJECT_0};
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11DeviceContext, ID3D11Multithread, ID3D11Texture2D,
    D3D11_BIND_SHADER_RESOURCE, D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_CPU_ACCESS_WRITE,
    D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_FLAG_DO_NOT_WAIT, D3D11_MAP_READ, D3D11_MAP_WRITE,
    D3D11_REQ_TEXTURE2D_U_OR_V_DIMENSION, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_MODE_ROTATION,
    DXGI_MODE_ROTATION_IDENTITY, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    IDXGIDevice, IDXGIOutput1, IDXGIOutputDuplication, DXGI_ERROR_ACCESS_LOST,
    DXGI_ERROR_INVALID_CALL, DXGI_ERROR_SESSION_DISCONNECTED, DXGI_ERROR_WAIT_TIMEOUT,
    DXGI_ERROR_WAS_STILL_DRAWING, DXGI_OUTDUPL_FRAME_INFO, DXGI_OUTDUPL_MOVE_RECT,
};
use windows::Win32::Graphics::Gdi::{
    BitBlt, ClientToScreen, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject,
    GetDC, GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS, HBITMAP, HDC, SRCCOPY,
};
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Threading::{CreateEventW, ResetEvent, SetEvent, WaitForSingleObject};
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::System::WinRT::{
    RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED, RO_INIT_SINGLETHREADED,
};
use windows::Win32::UI::HiDpi::{
    SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::{GetClientRect, IsWindow};

/// Errors returned by capture backends and region validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureError {
    LargeOffset { width: i32, height: i32 },
    DxgiInit(String),
    AccessLost,
    DxgiDesktopUnavailable,
    DxgiUnsupportedRotation { rotation: DXGI_MODE_ROTATION },
    DxgiRecoveryFailed(String),
    WgcItemClosed,
    WgcUnsupported(String),
    DeviceError(String),
}

impl fmt::Display for CaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LargeOffset { width, height } => {
                write!(f, "capture offset exceeds window size ({width}x{height})")
            }
            Self::DxgiInit(msg) => write!(f, "DXGI duplication init failed: {msg}"),
            Self::AccessLost => write!(f, "DXGI duplication access lost"),
            Self::DxgiDesktopUnavailable => write!(f, "DXGI cannot access the current desktop"),
            Self::DxgiUnsupportedRotation { rotation } => write!(
                f,
                "DXGI output rotation {:?} requires upright WGC capture",
                rotation.0
            ),
            Self::DxgiRecoveryFailed(msg) => {
                write!(f, "DXGI duplication recovery failed: {msg}")
            }
            Self::WgcItemClosed => write!(f, "Windows closed the WGC capture item"),
            Self::WgcUnsupported(msg) => write!(f, "WGC unsupported or init failed: {msg}"),
            Self::DeviceError(msg) => write!(f, "D3D11 capture error: {msg}"),
        }
    }
}

impl std::error::Error for CaptureError {}

/// Validated capture sub-rectangle inside a window or output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureRegion {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
}

impl CaptureRegion {
    /// Computes a cropped capture region from `(left, top, right, bottom)` and inward offsets.
    pub fn from_rect_and_offsets(
        rect: (i32, i32, i32, i32),
        offset_left: i32,
        offset_top: i32,
        offset_right: i32,
        offset_bottom: i32,
    ) -> Result<Self, CaptureError> {
        let left = i64::from(rect.0) + i64::from(offset_left.max(0));
        let top = i64::from(rect.1) + i64::from(offset_top.max(0));
        let right = i64::from(rect.2) - i64::from(offset_right.max(0));
        let bottom = i64::from(rect.3) - i64::from(offset_bottom.max(0));
        let width = right - left;
        let height = bottom - top;
        if !(1..=i64::from(i32::MAX)).contains(&width)
            || !(1..=i64::from(i32::MAX)).contains(&height)
        {
            return Err(CaptureError::LargeOffset {
                width: width.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
                height: height.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
            });
        }
        Ok(Self {
            left: left as i32,
            top: top as i32,
            width: width as u32,
            height: height as u32,
        })
    }
}

/// A captured frame (GPU texture + QPC timestamp).
#[derive(Debug, Clone)]
pub struct CapturedFrame {
    pub texture: Option<ID3D11Texture2D>,
    pub width: u32,
    pub height: u32,
    pub timestamp_qpc: i64,
    pub is_duplicate: bool,
}

impl CapturedFrame {
    /// Returns the actual GPU texture format, or `None` for metadata-only frames.
    pub fn format(&self) -> Option<DXGI_FORMAT> {
        self.texture
            .as_ref()
            .map(|texture| texture_desc(texture).Format)
    }

    /// Creates a metadata-only frame (useful for pacing and ring-queue tests).
    pub fn metadata_only(width: u32, height: u32, timestamp_qpc: i64, is_duplicate: bool) -> Self {
        Self {
            texture: None,
            width,
            height,
            timestamp_qpc,
            is_duplicate,
        }
    }
}

/// RAII wrapper around a Win32 auto-reset or manual-reset event `HANDLE`.
pub struct Win32Event {
    handle: HANDLE,
}

unsafe impl Send for Win32Event {}
unsafe impl Sync for Win32Event {}

impl Win32Event {
    /// Creates an unnamed Win32 event.
    pub fn new(manual_reset: bool, initial_state: bool) -> WinResult<Self> {
        let handle = unsafe { CreateEventW(None, manual_reset, initial_state, None)? };
        Ok(Self { handle })
    }

    /// Signals the event.
    pub fn signal(&self) {
        unsafe {
            let _ = SetEvent(self.handle);
        }
    }

    /// Resets the event to non-signaled.
    pub fn reset(&self) {
        unsafe {
            let _ = ResetEvent(self.handle);
        }
    }

    /// Waits up to `timeout_ms` milliseconds for the event to become signaled.
    pub fn wait(&self, timeout_ms: u32) -> bool {
        unsafe { WaitForSingleObject(self.handle, timeout_ms) == WAIT_OBJECT_0 }
    }
}

impl Drop for Win32Event {
    fn drop(&mut self) {
        if !self.handle.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.handle);
            }
        }
    }
}

/// Outcome of pushing a frame into [`FrameRingQueue`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushOutcome {
    Enqueued,
    EvictedOldest,
}

/// Bounded dual-queue ring buffer (`free_pool` + `ready_queue`) with Win32 event notification.
pub struct FrameRingQueue {
    capacity: usize,
    state: Mutex<QueueState>,
    frame_ready: Win32Event,
}

struct QueueState {
    ready: VecDeque<CapturedFrame>,
    free_pool: VecDeque<ID3D11Texture2D>,
    superseded_count: u32,
}

impl FrameRingQueue {
    /// Creates a new bounded ring queue with `capacity >= 1`.
    pub fn new(capacity: usize) -> WinResult<Self> {
        let cap = capacity.max(1);
        Ok(Self {
            capacity: cap,
            state: Mutex::new(QueueState {
                ready: VecDeque::with_capacity(cap),
                free_pool: VecDeque::with_capacity(cap),
                superseded_count: 0,
            }),
            frame_ready: Win32Event::new(false, false)?,
        })
    }

    /// Pushes a captured frame. If full, evicts the oldest ready frame (returning its texture to `free_pool`).
    pub fn push_frame(&self, frame: CapturedFrame) -> PushOutcome {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let outcome = if guard.ready.len() >= self.capacity {
            if let Some(evicted) = guard.ready.pop_front() {
                if let Some(tex) = evicted.texture {
                    if guard.free_pool.len() < self.capacity {
                        guard.free_pool.push_back(tex);
                    }
                }
                guard.superseded_count = guard.superseded_count.saturating_add(1);
            }
            PushOutcome::EvictedOldest
        } else {
            PushOutcome::Enqueued
        };
        guard.ready.push_back(frame);
        self.frame_ready.signal();
        outcome
    }

    /// Pops the next ready frame, waiting up to `timeout_ms` if the queue is currently empty.
    pub fn pop_frame(&self, timeout_ms: u32) -> Option<CapturedFrame> {
        {
            let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(frame) = guard.ready.pop_front() {
                if guard.ready.is_empty() {
                    self.frame_ready.reset();
                }
                return Some(frame);
            }
            // With no frame queued, a pending signal can be a capture error or closure.
        }
        if timeout_ms == 0 || !self.frame_ready.wait(timeout_ms) {
            return None;
        }
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let frame = guard.ready.pop_front();
        if guard.ready.is_empty() {
            self.frame_ready.reset();
        }
        frame
    }

    /// Drains all currently queued frames and returns only the newest one, returning skipped
    /// textures directly to `free_pool`. Never blocks.
    pub fn pop_latest(&self) -> Option<CapturedFrame> {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut latest = guard.ready.pop_front()?;
        while let Some(newer) = guard.ready.pop_front() {
            if let Some(old_tex) = latest.texture.take() {
                if guard.free_pool.len() < self.capacity {
                    guard.free_pool.push_back(old_tex);
                }
            }
            guard.superseded_count = guard.superseded_count.saturating_add(1);
            latest = newer;
        }
        self.frame_ready.reset();
        Some(latest)
    }

    /// Returns a reusable texture from the free pool if available.
    pub fn take_free_texture(&self) -> Option<ID3D11Texture2D> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .free_pool
            .pop_front()
    }

    /// Returns a consumed texture back to the free pool (up to `capacity`).
    pub fn recycle_texture(&self, tex: ID3D11Texture2D) {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if guard.free_pool.len() < self.capacity {
            guard.free_pool.push_back(tex);
        }
    }

    /// Number of frames currently waiting in the ready queue.
    pub fn ready_len(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .ready
            .len()
    }

    /// Number of reusable textures currently sitting in the free pool.
    pub fn free_pool_len(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .free_pool
            .len()
    }

    /// Non-blocking peek at the oldest ready frame's QPC timestamp (the next frame `pop_frame` would return).
    pub fn peek_next_timestamp_qpc(&self) -> Option<i64> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .ready
            .front()
            .map(|frame| frame.timestamp_qpc)
    }

    /// Non-blocking peek at the newest ready frame's QPC timestamp (the frame `pop_latest` would return).
    pub fn peek_latest_timestamp_qpc(&self) -> Option<i64> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .ready
            .back()
            .map(|frame| frame.timestamp_qpc)
    }

    /// Returns true when the ready queue exceeds `queue_target` (triggering queue draining).
    pub fn should_drain(&self, queue_target: usize) -> bool {
        self.ready_len() > queue_target
    }

    /// Drops oldest frames until `ready_len() <= queue_target`, returning how many were dropped.
    pub fn drain_to(&self, queue_target: usize) -> usize {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut dropped = 0;
        while guard.ready.len() > queue_target {
            if let Some(evicted) = guard.ready.pop_front() {
                if let Some(tex) = evicted.texture {
                    if guard.free_pool.len() < self.capacity {
                        guard.free_pool.push_back(tex);
                    }
                }
                guard.superseded_count = guard.superseded_count.saturating_add(1);
                dropped += 1;
            }
        }
        if guard.ready.is_empty() {
            self.frame_ready.reset();
        }
        dropped
    }

    /// Drains and returns the number of frames discarded by overflow, draining, or `pop_latest`.
    pub fn take_superseded_count(&self) -> u32 {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut guard.superseded_count)
    }

    fn note_superseded(&self) {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        guard.superseded_count = guard.superseded_count.saturating_add(1);
    }

    /// Clears both ready and free queues.
    pub fn clear(&self) {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        guard.ready.clear();
        guard.free_pool.clear();
        guard.superseded_count = 0;
        self.frame_ready.reset();
    }

    /// Wakes a thread blocked in [`Self::pop_frame`] so it can observe shutdown immediately.
    pub fn wake_waiter(&self) {
        self.frame_ready.signal();
    }
}

/// Common trait implemented by all capture backends.
pub trait CaptureBackend: Send {
    fn api(&self) -> CaptureApi;
    fn start(&mut self) -> Result<(), CaptureError>;
    fn acquire_frame(&mut self, timeout_ms: u32) -> Result<Option<CapturedFrame>, CaptureError>;
    /// Non-blocking acquire of the next ready frame (`acquire_frame(0)`).
    fn try_acquire_frame(&mut self) -> Result<Option<CapturedFrame>, CaptureError> {
        self.acquire_frame(0)
    }
    /// Non-blocking drain returning only the freshest queued frame (recycling older ones).
    fn drain_latest_frame(&mut self) -> Result<Option<CapturedFrame>, CaptureError> {
        self.acquire_frame(0)
    }
    /// Non-blocking check for whether at least one frame is queued and ready.
    fn has_ready_frame(&self) -> bool {
        self.ready_len() > 0
    }
    /// Non-blocking peek at the next queued frame's QPC timestamp without consuming it.
    fn peek_next_timestamp_qpc(&self) -> Option<i64> {
        None
    }
    /// Non-blocking peek at the newest queued frame's QPC timestamp without consuming it.
    fn peek_latest_timestamp_qpc(&self) -> Option<i64> {
        None
    }
    /// Returns a finished frame's GPU texture back to the backend's reuse pool.
    fn recycle_frame_texture(&self, _texture: ID3D11Texture2D) {}
    /// Returns a thread-safe handle to the backend's [`FrameRingQueue`] for cross-thread
    /// texture recycling and waiter wakeup while a producer thread owns `&mut self`.
    fn frame_recycler(&self) -> Option<Arc<FrameRingQueue>> {
        None
    }
    fn cursor_embedded(&self) -> bool {
        false
    }
    fn take_discontinuity(&mut self) -> bool {
        false
    }
    /// Drains and returns any queued frames discarded internally by the backend since the last call.
    fn take_superseded_count(&self) -> u32 {
        0
    }
    /// DXGI desktop updates represented by `AccumulatedFrames`, distinct from queue eviction.
    fn take_accumulated_frames(&self) -> u64 {
        0
    }
    /// Requested and effective WGC session properties, when this backend is WGC.
    fn wgc_session_status(&self) -> Option<WgcSessionStatus> {
        None
    }
    fn ready_len(&self) -> usize;
    fn stop(&mut self);
}

fn alloc_bgra_copy_texture(
    device: &ID3D11Device,
    queue: &FrameRingQueue,
    width: u32,
    height: u32,
) -> Result<ID3D11Texture2D, CaptureError> {
    alloc_capture_copy_texture(device, queue, width, height, DXGI_FORMAT_B8G8R8A8_UNORM)
}

fn alloc_capture_copy_texture(
    device: &ID3D11Device,
    queue: &FrameRingQueue,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
) -> Result<ID3D11Texture2D, CaptureError> {
    if let Some(tex) = queue.take_free_texture() {
        let desc = texture_desc(&tex);
        if desc.Width == width && desc.Height == height && desc.Format == format {
            return Ok(tex);
        }
    }
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width.max(1),
        Height: height.max(1),
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let mut tex = None;
    unsafe {
        device
            .CreateTexture2D(&desc, None, Some(&mut tex))
            .map_err(|e| CaptureError::DeviceError(e.to_string()))?;
    }
    tex.ok_or_else(|| CaptureError::DeviceError("CreateTexture2D returned None".into()))
}

fn texture_desc(texture: &ID3D11Texture2D) -> D3D11_TEXTURE2D_DESC {
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe { texture.GetDesc(&mut desc) };
    desc
}

/// DXGI Desktop Duplication capture backend (`IDXGIOutputDuplication`).
pub struct DxgiDuplicationCapture {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    output1: IDXGIOutput1,
    region: CaptureRegion,
    dedup: bool,
    duplication: Option<IDXGIOutputDuplication>,
    queue: Arc<FrameRingQueue>,
    cursor_embedded: bool,
    last_mouse_update_time: i64,
    started: bool,
    discontinuity_pending: bool,
    last_timestamp_qpc: AtomicI64,
    accumulated_frames: AtomicU64,
    /// Staging copy of sampled rows plus the previous frame's rows, to drop pixel-identical frames.
    probe: Option<ID3D11Texture2D>,
    probe_prev: Vec<u8>,
}

unsafe impl Send for DxgiDuplicationCapture {}

impl DxgiDuplicationCapture {
    pub fn new(
        device: ID3D11Device,
        context: ID3D11DeviceContext,
        output1: IDXGIOutput1,
        region: CaptureRegion,
        queue_capacity: usize,
    ) -> Result<Self, CaptureError> {
        let queue = Arc::new(
            FrameRingQueue::new(queue_capacity)
                .map_err(|e| CaptureError::DxgiInit(e.to_string()))?,
        );
        Ok(Self {
            device,
            context,
            output1,
            region,
            dedup: false,
            duplication: None,
            queue,
            cursor_embedded: false,
            last_mouse_update_time: 0,
            started: false,
            discontinuity_pending: false,
            last_timestamp_qpc: AtomicI64::new(0),
            accumulated_frames: AtomicU64::new(0),
            probe: None,
            probe_prev: Vec::new(),
        })
    }

    /// Whether `tex` differs from the previous accepted frame on `PROBE_ROWS` evenly spaced rows.
    /// Windows behind the game make DXGI report new desktop frames whose game pixels are identical;
    /// counting those inflates captured FPS (~80 for a 60 FPS game) and breaks pacing.
    fn pixels_changed(&mut self, tex: &ID3D11Texture2D, width: u32, height: u32) -> bool {
        let rows = PROBE_ROWS.min(height);
        let reusable = self.probe.as_ref().is_some_and(|p| {
            let d = texture_desc(p);
            d.Width == width && d.Height == rows
        });
        if !reusable {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: width,
                Height: rows,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_STAGING,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                ..Default::default()
            };
            let mut probe = None;
            if unsafe { self.device.CreateTexture2D(&desc, None, Some(&mut probe)) }.is_err() {
                return true;
            }
            self.probe = probe;
            self.probe_prev.clear();
        }
        let Some(probe) = self.probe.clone() else { return true };
        for i in 0..rows {
            let y = (2 * i + 1) * height / (2 * rows);
            let row = D3D11_BOX { left: 0, top: y, front: 0, right: width, bottom: y + 1, back: 1 };
            unsafe { self.context.CopySubresourceRegion(&probe, 0, 0, i, 0, tex, 0, Some(&row)) };
        }
        unsafe { self.context.Flush() };
        let deadline = Instant::now() + PROBE_READBACK_BUDGET;
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        loop {
            match unsafe {
                self.context.Map(&probe, 0, D3D11_MAP_READ, D3D11_MAP_FLAG_DO_NOT_WAIT.0 as u32, Some(&mut mapped))
            } {
                Ok(()) => break,
                Err(e) if e.code() == DXGI_ERROR_WAS_STILL_DRAWING && Instant::now() < deadline => thread::yield_now(),
                Err(_) => return true, // Readback not ready: never drop a possibly real frame.
            }
        }
        let row_bytes = width as usize * 4;
        let data = mapped.pData as *const u8;
        let pitch = mapped.RowPitch as usize;
        let sampled = (0..rows as usize)
            .map(|i| unsafe { std::slice::from_raw_parts(data.add(i * pitch), row_bytes) });
        let changed = probe_rows_changed(&mut self.probe_prev, sampled, row_bytes);
        unsafe { self.context.Unmap(&probe, 0) };
        changed
    }

    /// Enables or disables duplicate-frame filtering (`LastPresentTime == 0 && AccumulatedFrames == 0`).
    pub fn with_dedup(mut self, dedup: bool) -> Self {
        self.dedup = dedup;
        self
    }

    fn create_duplication(&self) -> WinResult<IDXGIOutputDuplication> {
        unsafe { self.output1.DuplicateOutput(&self.device) }
    }

    fn ensure_upright(&self, duplication: &IDXGIOutputDuplication) -> Result<(), CaptureError> {
        let rotation = unsafe { duplication.GetDesc() }.Rotation;
        if dxgi_rotation_requires_wgc(rotation) {
            Err(CaptureError::DxgiUnsupportedRotation { rotation })
        } else {
            Ok(())
        }
    }

    fn update_cursor_state(&mut self, info: &DXGI_OUTDUPL_FRAME_INFO) {
        (self.last_mouse_update_time, self.cursor_embedded) = dxgi_cursor_state(
            self.last_mouse_update_time,
            self.cursor_embedded,
            info.LastMouseUpdateTime,
            info.PointerPosition.Visible.as_bool(),
        );
    }
}

const DXGI_RECOVERY_BACKOFF_MS: [u64; 6] = [10, 25, 50, 100, 200, 400];

fn recovery_delay_ms(attempt: usize) -> Option<u64> {
    DXGI_RECOVERY_BACKOFF_MS.get(attempt).copied()
}

fn dxgi_cursor_state(
    last_mouse_update_time: i64,
    cursor_embedded: bool,
    update_time: i64,
    pointer_visible: bool,
) -> (i64, bool) {
    if update_time != 0 && update_time != last_mouse_update_time {
        (update_time, !pointer_visible)
    } else {
        (last_mouse_update_time, cursor_embedded)
    }
}

fn is_duplication_recoverable(code: windows::core::HRESULT) -> bool {
    matches!(
        code,
        DXGI_ERROR_ACCESS_LOST | DXGI_ERROR_INVALID_CALL | DXGI_ERROR_SESSION_DISCONNECTED
    )
}

fn dxgi_rotation_requires_wgc(rotation: DXGI_MODE_ROTATION) -> bool {
    rotation != DXGI_MODE_ROTATION_IDENTITY
}

fn desktop_unavailable(code: windows::core::HRESULT) -> bool {
    matches!(code, E_ACCESSDENIED | DXGI_ERROR_SESSION_DISCONNECTED)
}

fn crop_to_texture(region: CaptureRegion, width: u32, height: u32) -> Option<(u32, u32, u32, u32)> {
    let source_left = i64::from(region.left);
    let source_top = i64::from(region.top);
    let left = source_left.max(0);
    let top = source_top.max(0);
    let right = (source_left + i64::from(region.width)).min(i64::from(width));
    let bottom = (source_top + i64::from(region.height)).min(i64::from(height));
    if left >= right || top >= bottom {
        return None;
    }
    Some((
        left as u32,
        top as u32,
        (right - left) as u32,
        (bottom - top) as u32,
    ))
}

fn qpc_from_100ns(timestamp_100ns: i64, qpc_frequency: i64) -> Option<i64> {
    if timestamp_100ns <= 0 || qpc_frequency <= 0 {
        return None;
    }
    (i128::from(timestamp_100ns) * i128::from(qpc_frequency) / 10_000_000)
        .try_into()
        .ok()
}

fn has_complete_dib_rows(read_rows: i32, requested_rows: u32) -> bool {
    requested_rows > 0 && i64::from(read_rows) == i64::from(requested_rows)
}

const PROBE_ROWS: u32 = 32;
/// Max time `pixels_changed` waits for the probe readback before keeping the frame.
const PROBE_READBACK_BUDGET: Duration = Duration::from_millis(1);

/// Compares sampled rows against `prev` (then stores them); a size change counts as changed.
fn probe_rows_changed<'a>(
    prev: &mut Vec<u8>,
    rows: impl ExactSizeIterator<Item = &'a [u8]>,
    row_bytes: usize,
) -> bool {
    let total = rows.len() * row_bytes;
    let mut changed = prev.len() != total;
    prev.resize(total, 0);
    for (row, stored) in rows.zip(prev.chunks_exact_mut(row_bytes)) {
        changed |= row != stored;
        stored.copy_from_slice(row);
    }
    changed
}

fn rects_touch_region(rects: impl IntoIterator<Item = RECT>, region: CaptureRegion) -> bool {
    let (left, top) = (i64::from(region.left), i64::from(region.top));
    let (right, bottom) = (left + i64::from(region.width), top + i64::from(region.height));
    rects.into_iter().any(|r| {
        i64::from(r.left) < right && i64::from(r.right) > left && i64::from(r.top) < bottom && i64::from(r.bottom) > top
    })
}

/// Whether this DXGI frame changed pixels inside the captured region. Desktop updates elsewhere
/// (other windows, clocks, consoles) otherwise count as phantom game frames and inflate FPS.
fn frame_touches_region(dupl: &IDXGIOutputDuplication, info: &DXGI_OUTDUPL_FRAME_INFO, region: CaptureRegion) -> bool {
    let bytes = info.TotalMetadataBufferSize;
    if bytes == 0 {
        return true;
    }
    let mut used = 0u32;
    let mut dirty = vec![RECT::default(); bytes as usize / size_of::<RECT>() + 1];
    let dirty_bytes = (dirty.len() * size_of::<RECT>()) as u32;
    if unsafe { dupl.GetFrameDirtyRects(dirty_bytes, dirty.as_mut_ptr(), &mut used) }.is_err() {
        return true;
    }
    if rects_touch_region(dirty[..used as usize / size_of::<RECT>()].iter().copied(), region) {
        return true;
    }
    let mut moves = vec![DXGI_OUTDUPL_MOVE_RECT::default(); bytes as usize / size_of::<DXGI_OUTDUPL_MOVE_RECT>() + 1];
    let move_bytes = (moves.len() * size_of::<DXGI_OUTDUPL_MOVE_RECT>()) as u32;
    if unsafe { dupl.GetFrameMoveRects(move_bytes, moves.as_mut_ptr(), &mut used) }.is_err() {
        return true;
    }
    rects_touch_region(
        moves[..used as usize / size_of::<DXGI_OUTDUPL_MOVE_RECT>()].iter().map(|m| m.DestinationRect),
        region,
    )
}

fn poll_dxgi_frame(timeout_ms: u32, mut acquire: impl FnMut(u32) -> WinResult<()>) -> WinResult<()> {
    let deadline = Instant::now() + Duration::from_millis(u64::from(timeout_ms));
    loop {
        // A blocking DXGI acquire can stall rendering on the same D3D11 device.
        match acquire(0) {
            Err(error) if error.code() == DXGI_ERROR_WAIT_TIMEOUT => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(error);
                }
                thread::sleep(remaining.min(Duration::from_millis(1)));
            }
            result => return result,
        }
    }
}

impl CaptureBackend for DxgiDuplicationCapture {
    fn api(&self) -> CaptureApi {
        CaptureApi::Dxgi
    }

    fn start(&mut self) -> Result<(), CaptureError> {
        let duplication = self
            .create_duplication()
            .map_err(|e| CaptureError::DxgiInit(e.to_string()))?;
        self.ensure_upright(&duplication)?;
        self.duplication = Some(duplication);
        self.started = true;
        self.cursor_embedded = false;
        self.last_mouse_update_time = 0;
        self.discontinuity_pending = false;
        self.last_timestamp_qpc.store(0, Ordering::Relaxed);
        self.accumulated_frames.store(0, Ordering::Relaxed);
        self.probe_prev.clear();
        Ok(())
    }

    fn acquire_frame(&mut self, timeout_ms: u32) -> Result<Option<CapturedFrame>, CaptureError> {
        if !self.started {
            return Err(CaptureError::DxgiInit("not started".into()));
        }
        let mut recovery_attempts = 0;
        let mut last_recovery_error = String::from("DXGI duplication access lost");
        loop {
            if self.duplication.is_none() {
                let Some(delay_ms) = recovery_delay_ms(recovery_attempts) else {
                    return Err(CaptureError::DxgiRecoveryFailed(last_recovery_error));
                };
                recovery_attempts += 1;
                thread::sleep(Duration::from_millis(delay_ms));
                unsafe { self.device.GetDeviceRemovedReason() }
                    .map_err(|error| CaptureError::DeviceError(error.to_string()))?;
                match self.create_duplication() {
                    Ok(duplication) => {
                        self.ensure_upright(&duplication)?;
                        self.duplication = Some(duplication);
                        self.cursor_embedded = false;
                        self.last_mouse_update_time = 0;
                        self.discontinuity_pending = true;
                    }
                    Err(error) if desktop_unavailable(error.code()) => {
                        return Err(CaptureError::DxgiDesktopUnavailable);
                    }
                    Err(error) => {
                        last_recovery_error = error.to_string();
                        continue;
                    }
                }
            }
            let dupl = self
                .duplication
                .as_ref()
                .cloned()
                .ok_or_else(|| CaptureError::DxgiInit("not started".into()))?;
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut resource = None;
            if let Err(error) = poll_dxgi_frame(timeout_ms, |wait| unsafe {
                dupl.AcquireNextFrame(wait, &mut info, &mut resource)
            })
            {
                if error.code() == DXGI_ERROR_WAIT_TIMEOUT {
                    return Ok(None);
                }
                if is_duplication_recoverable(error.code()) {
                    last_recovery_error = error.to_string();
                    drop(dupl);
                    self.duplication = None;
                    continue;
                }
                return Err(CaptureError::DeviceError(error.to_string()));
            }

            self.update_cursor_state(&info);
            self.accumulated_frames
                .fetch_add(u64::from(info.AccumulatedFrames), Ordering::Relaxed);
            let is_duplicate = info.AccumulatedFrames == 0 || !frame_touches_region(&dupl, &info, self.region);
            let result = (|| {
                if self.dedup && info.LastPresentTime == 0 && info.AccumulatedFrames == 0 {
                    return Ok(None);
                }
                let Some(resource) = resource else {
                    return Ok(None);
                };
                let src_tex: ID3D11Texture2D = resource
                    .cast()
                    .map_err(|e| CaptureError::DeviceError(e.to_string()))?;
                let source_desc = texture_desc(&src_tex);
                let Some((left, top, width, height)) =
                    crop_to_texture(self.region, source_desc.Width, source_desc.Height)
                else {
                    return Ok(None);
                };
                let raw_qpc = if info.LastPresentTime != 0 {
                    info.LastPresentTime
                } else {
                    qpc_now()
                };
                let timestamp_qpc = clamp_monotonic_qpc(raw_qpc, &self.last_timestamp_qpc);
                if is_duplicate {
                    return Ok(Some(CapturedFrame::metadata_only(
                        width,
                        height,
                        timestamp_qpc,
                        true,
                    )));
                }
                let dst_tex = alloc_bgra_copy_texture(&self.device, &self.queue, width, height)?;
                let src_box = D3D11_BOX {
                    left,
                    top,
                    front: 0,
                    right: left + width,
                    bottom: top + height,
                    back: 1,
                };
                unsafe {
                    self.context.CopySubresourceRegion(
                        &dst_tex,
                        0,
                        0,
                        0,
                        0,
                        &src_tex,
                        0,
                        Some(&src_box),
                    );
                }
                if !self.pixels_changed(&dst_tex, width, height) {
                    self.queue.recycle_texture(dst_tex);
                    return Ok(Some(CapturedFrame::metadata_only(width, height, timestamp_qpc, true)));
                }
                Ok(Some(CapturedFrame {
                    texture: Some(dst_tex),
                    width,
                    height,
                    timestamp_qpc,
                    is_duplicate: false,
                }))
            })();

            if let Err(error) = unsafe { dupl.ReleaseFrame() } {
                if is_duplication_recoverable(error.code()) {
                    last_recovery_error = error.to_string();
                    drop(dupl);
                    self.duplication = None;
                    continue;
                } else {
                    return Err(CaptureError::DeviceError(error.to_string()));
                }
            }
            return result;
        }
    }

    fn recycle_frame_texture(&self, texture: ID3D11Texture2D) {
        self.queue.recycle_texture(texture);
    }

    fn frame_recycler(&self) -> Option<Arc<FrameRingQueue>> {
        Some(Arc::clone(&self.queue))
    }

    fn cursor_embedded(&self) -> bool {
        self.cursor_embedded
    }

    fn take_accumulated_frames(&self) -> u64 {
        self.accumulated_frames.swap(0, Ordering::Relaxed)
    }

    fn take_discontinuity(&mut self) -> bool {
        std::mem::take(&mut self.discontinuity_pending)
    }

    fn ready_len(&self) -> usize {
        self.queue.ready_len()
    }

    fn peek_next_timestamp_qpc(&self) -> Option<i64> {
        self.queue.peek_next_timestamp_qpc()
    }

    fn peek_latest_timestamp_qpc(&self) -> Option<i64> {
        self.queue.peek_latest_timestamp_qpc()
    }

    fn stop(&mut self) {
        self.duplication = None;
        self.started = false;
        self.discontinuity_pending = false;
        self.last_timestamp_qpc.store(0, Ordering::Relaxed);
        self.queue.clear();
    }
}

/// Windows.Graphics.Capture (`WGC`) backend using `Direct3D11CaptureFramePool`.
pub struct WgcCapture {
    device: ID3D11Device,
    hwnd: HWND,
    region: CaptureRegion,
    hdr_capture: bool,
    session_status: WgcSessionStatus,
    queue: Arc<FrameRingQueue>,
    pool: Option<Direct3D11CaptureFramePool>,
    frame_token: Option<i64>,
    session: Option<GraphicsCaptureSession>,
    closed_handler: Option<(GraphicsCaptureItem, i64)>,
    async_error: Arc<Mutex<Option<CaptureError>>>,
    multithread: Option<ID3D11Multithread>,
    multithread_was_protected: bool,
}

unsafe impl Send for WgcCapture {}

/// Requested and read-back values for the WGC session setters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WgcSessionStatus {
    pub requested_cursor_capture_enabled: bool,
    pub actual_cursor_capture_enabled: Option<bool>,
    pub cursor_capture_error: Option<String>,
    pub requested_border_required: bool,
    pub actual_border_required: Option<bool>,
    pub border_error: Option<String>,
    pub requested_min_update_interval_100ns: i64,
    pub actual_min_update_interval_100ns: Option<i64>,
    pub min_update_interval_error: Option<String>,
}

impl WgcCapture {
    pub fn new(
        device: ID3D11Device,
        hwnd: HWND,
        region: CaptureRegion,
        hide_cursor: bool,
        queue_capacity: usize,
    ) -> Result<Self, CaptureError> {
        Self::new_with_format(device, hwnd, region, hide_cursor, queue_capacity, false)
    }

    /// Creates a WGC capture that requests a native FP16 frame pool and copy textures.
    pub fn new_hdr(
        device: ID3D11Device,
        hwnd: HWND,
        region: CaptureRegion,
        hide_cursor: bool,
        queue_capacity: usize,
    ) -> Result<Self, CaptureError> {
        Self::new_with_format(device, hwnd, region, hide_cursor, queue_capacity, true)
    }

    fn new_with_format(
        device: ID3D11Device,
        hwnd: HWND,
        region: CaptureRegion,
        hide_cursor: bool,
        queue_capacity: usize,
        hdr_capture: bool,
    ) -> Result<Self, CaptureError> {
        let queue = Arc::new(
            FrameRingQueue::new(queue_capacity)
                .map_err(|e| CaptureError::WgcUnsupported(e.to_string()))?,
        );
        Ok(Self {
            device,
            hwnd,
            region,
            hdr_capture,
            session_status: WgcSessionStatus {
                requested_cursor_capture_enabled: !hide_cursor,
                actual_cursor_capture_enabled: None,
                cursor_capture_error: None,
                requested_border_required: false,
                actual_border_required: None,
                border_error: None,
                requested_min_update_interval_100ns: 10_000,
                actual_min_update_interval_100ns: None,
                min_update_interval_error: None,
            },
            queue,
            pool: None,
            frame_token: None,
            session: None,
            closed_handler: None,
            async_error: Arc::new(Mutex::new(None)),
            multithread: None,
            multithread_was_protected: false,
        })
    }

    pub fn session_status(&self) -> &WgcSessionStatus {
        &self.session_status
    }

    fn pixel_format(&self) -> DirectXPixelFormat {
        wgc_pixel_format(self.hdr_capture)
    }

    fn create_winrt_device(&self) -> Result<IDirect3DDevice, CaptureError> {
        let dxgi_dev: IDXGIDevice = self
            .device
            .cast()
            .map_err(|e| CaptureError::WgcUnsupported(e.to_string()))?;
        let inspectable = unsafe {
            CreateDirect3D11DeviceFromDXGIDevice(&dxgi_dev)
                .map_err(|e| CaptureError::WgcUnsupported(e.to_string()))?
        };
        inspectable
            .cast()
            .map_err(|e| CaptureError::WgcUnsupported(e.to_string()))
    }
    fn take_async_error(&self) -> Result<(), CaptureError> {
        if let Some(error) = self
            .async_error
            .lock()
            .map_err(|e| CaptureError::WgcUnsupported(e.to_string()))?
            .take()
        {
            return Err(error);
        }
        Ok(())
    }
}

fn qpc_now() -> i64 {
    let mut qpc = 0i64;
    unsafe {
        let _ = QueryPerformanceCounter(&mut qpc);
    }
    qpc
}

fn wgc_pixel_format(hdr: bool) -> DirectXPixelFormat {
    if hdr {
        DirectXPixelFormat::R16G16B16A16Float
    } else {
        DirectXPixelFormat::B8G8R8A8UIntNormalized
    }
}

fn capture_copy_format(hdr: bool) -> DXGI_FORMAT {
    if hdr {
        DXGI_FORMAT_R16G16B16A16_FLOAT
    } else {
        DXGI_FORMAT_B8G8R8A8_UNORM
    }
}

fn clamp_monotonic_qpc(raw_qpc: i64, last_qpc: &AtomicI64) -> i64 {
    let mut prev = last_qpc.load(Ordering::Relaxed);
    loop {
        let next = if prev > 0 && raw_qpc <= prev {
            prev.saturating_add(1)
        } else {
            raw_qpc
        };
        match last_qpc.compare_exchange_weak(prev, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(actual) => prev = actual,
        }
    }
}

struct WinRtInitialization(bool);

impl WinRtInitialization {
    fn new() -> Result<Self, CaptureError> {
        match unsafe { RoInitialize(RO_INIT_MULTITHREADED) } {
            Ok(()) => Ok(Self(true)),
            Err(error) if error.code() == windows::Win32::Foundation::RPC_E_CHANGED_MODE => {
                match unsafe { RoInitialize(RO_INIT_SINGLETHREADED) } {
                    Ok(()) => Ok(Self(true)),
                    Err(error)
                        if error.code() == windows::Win32::Foundation::RPC_E_CHANGED_MODE =>
                    {
                        Ok(Self(false))
                    }
                    Err(error) => Err(CaptureError::WgcUnsupported(error.to_string())),
                }
            }
            Err(error) => Err(CaptureError::WgcUnsupported(error.to_string())),
        }
    }
}

impl Drop for WinRtInitialization {
    fn drop(&mut self) {
        if self.0 {
            unsafe { RoUninitialize() };
        }
    }
}

struct ThreadDpiAwarenessGuard(Option<DPI_AWARENESS_CONTEXT>);

impl ThreadDpiAwarenessGuard {
    fn per_monitor_v2() -> Result<Self, CaptureError> {
        let previous =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        if previous.0.is_null() {
            return Err(CaptureError::WgcUnsupported(
                "could not set capture callback to Per-Monitor V2 DPI".into(),
            ));
        }
        Ok(Self(Some(previous)))
    }
}

impl Drop for ThreadDpiAwarenessGuard {
    fn drop(&mut self) {
        if let Some(previous) = self.0.take() {
            unsafe {
                let _ = SetThreadDpiAwarenessContext(previous);
            }
        }
    }
}

fn wgc_capture_box(
    client_screen: (i32, i32, i32, i32),
    frame_bounds: (i32, i32, i32, i32),
    region: CaptureRegion,
    texture_size: (u32, u32),
) -> Option<D3D11_BOX> {
    let client_width = i64::from(client_screen.2) - i64::from(client_screen.0);
    let client_height = i64::from(client_screen.3) - i64::from(client_screen.1);
    if region.left < 0
        || region.top < 0
        || i64::from(region.left) + i64::from(region.width) > client_width
        || i64::from(region.top) + i64::from(region.height) > client_height
    {
        return None;
    }
    let left =
        (i64::from(client_screen.0) - i64::from(frame_bounds.0) + i64::from(region.left)).max(0);
    let top =
        (i64::from(client_screen.1) - i64::from(frame_bounds.1) + i64::from(region.top)).max(0);
    let raw_right = left + i64::from(region.width);
    let raw_bottom = top + i64::from(region.height);
    let tex_w = i64::from(texture_size.0);
    let tex_h = i64::from(texture_size.1);
    if raw_right > tex_w + 2 || raw_bottom > tex_h + 2 {
        return None;
    }
    let right = raw_right.min(tex_w);
    let bottom = raw_bottom.min(tex_h);
    if right <= left || bottom <= top {
        return None;
    }
    Some(D3D11_BOX {
        left: left as u32,
        top: top as u32,
        front: 0,
        right: right as u32,
        bottom: bottom as u32,
        back: 1,
    })
}

#[allow(clippy::type_complexity)] // Client and frame bounds in screen coordinates.
fn wgc_window_rects(
    hwnd: HWND,
) -> Result<((i32, i32, i32, i32), (i32, i32, i32, i32)), CaptureError> {
    let _dpi = ThreadDpiAwarenessGuard::per_monitor_v2()?;
    let mut client = windows::Win32::Foundation::RECT::default();
    unsafe { GetClientRect(hwnd, &mut client) }
        .map_err(|e| CaptureError::WgcUnsupported(e.to_string()))?;
    let mut origin = windows::Win32::Foundation::POINT {
        x: client.left,
        y: client.top,
    };
    if !unsafe { ClientToScreen(hwnd, &mut origin) }.as_bool() {
        return Err(CaptureError::WgcUnsupported(
            "ClientToScreen failed while resolving WGC crop".into(),
        ));
    }
    let client_screen = (
        origin.x,
        origin.y,
        origin.x + (client.right - client.left),
        origin.y + (client.bottom - client.top),
    );
    let mut bounds = windows::Win32::Foundation::RECT::default();
    unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&mut bounds as *mut windows::Win32::Foundation::RECT).cast(),
            size_of::<windows::Win32::Foundation::RECT>() as u32,
        )
    }
    .map_err(|e| CaptureError::WgcUnsupported(e.to_string()))?;
    Ok((
        client_screen,
        (bounds.left, bounds.top, bounds.right, bounds.bottom),
    ))
}

impl CaptureBackend for WgcCapture {
    fn api(&self) -> CaptureApi {
        CaptureApi::Wgc
    }

    fn start(&mut self) -> Result<(), CaptureError> {
        if self.pool.is_some() || self.session.is_some() {
            return Err(CaptureError::WgcUnsupported("already started".into()));
        }
        if let Ok(mut pending) = self.async_error.lock() {
            *pending = None;
        }
        let _winrt = WinRtInitialization::new()?;
        if !GraphicsCaptureSession::IsSupported()
            .map_err(|e| CaptureError::WgcUnsupported(e.to_string()))?
        {
            return Err(CaptureError::WgcUnsupported(
                "GraphicsCaptureSession::IsSupported returned false".into(),
            ));
        }

        // The free-threaded pool can still be unwinding a Windows worker after Close.
        // Pin the module after activation, even if creating the window item later fails.
        use windows::Win32::System::LibraryLoader::{
            GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_PIN,
        };
        let mut capture_module = Default::default();
        let _ = unsafe {
            GetModuleHandleExW(
                GET_MODULE_HANDLE_EX_FLAG_PIN,
                windows::core::w!("GraphicsCapture.dll"),
                &mut capture_module,
            )
        };

        let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
            .map_err(|e| CaptureError::WgcUnsupported(e.to_string()))?;
        let item: GraphicsCaptureItem = unsafe {
            interop
                .CreateForWindow(self.hwnd)
                .map_err(|e| CaptureError::WgcUnsupported(e.to_string()))?
        };
        let winrt_dev = self.create_winrt_device()?;
        let size = item
            .Size()
            .map_err(|e| CaptureError::WgcUnsupported(e.to_string()))?;

        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &winrt_dev,
            self.pixel_format(),
            3,
            size,
        )
        .map_err(|e| CaptureError::WgcUnsupported(e.to_string()))?;

        let queue_clone = Arc::clone(&self.queue);
        let device = self.device.clone();
        let context = unsafe { self.device.GetImmediateContext() }
            .map_err(|e| CaptureError::WgcUnsupported(e.to_string()))?;
        let callback_context = context.clone();
        let hwnd = self.hwnd.0 as isize;
        let region = self.region;
        let pixel_format = self.pixel_format();
        let copy_format = capture_copy_format(self.hdr_capture);
        let frame_size = Arc::new(Mutex::new((size.Width, size.Height)));
        let frame_size_callback = Arc::clone(&frame_size);
        let async_error = Arc::clone(&self.async_error);
        let last_timestamp_qpc = Arc::new(AtomicI64::new(0));
        let mut qpc_frequency = 0i64;
        unsafe { QueryPerformanceFrequency(&mut qpc_frequency) }
            .map_err(|e| CaptureError::WgcUnsupported(e.to_string()))?;
        if qpc_frequency <= 0 {
            return Err(CaptureError::WgcUnsupported(
                "QueryPerformanceFrequency returned an invalid value".into(),
            ));
        }
        let frame_token = pool
            .FrameArrived(
                &TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new(
                    move |sender, _| {
                        if let Some(pool_ref) = sender.as_ref() {
                            let mut latest = None;
                            let mut skipped_frames = 0u32;
                            let mut drain_err: Option<CaptureError> = None;
                            while let Ok(frame) = pool_ref.TryGetNextFrame() {
                                match frame.ContentSize() {
                                    Ok(sz) if sz.Width > 0 && sz.Height > 0 => {
                                        if let Some((prev_frame, _)) = latest.replace((frame, sz)) {
                                            let _ = prev_frame.Close();
                                            skipped_frames = skipped_frames.saturating_add(1);
                                        }
                                    }
                                    Ok(_) => {
                                        let _ = frame.Close();
                                    }
                                    Err(e) => {
                                        let _ = frame.Close();
                                        drain_err =
                                            Some(CaptureError::WgcUnsupported(e.to_string()));
                                        break;
                                    }
                                }
                            }
                            if let Some(error) = drain_err {
                                if let Some((prev_frame, _)) = latest.take() {
                                    let _ = prev_frame.Close();
                                }
                                if let Ok(mut pending) = async_error.lock() {
                                    *pending = Some(error);
                                }
                                queue_clone.frame_ready.signal();
                            } else if let Some((frame, content_size)) = latest {
                                let mut closed = false;
                                let result = (|| -> Result<(), CaptureError> {
                                    let mut current_size = frame_size_callback
                                        .lock()
                                        .map_err(|e| CaptureError::WgcUnsupported(e.to_string()))?;
                                    let resized =
                                        *current_size != (content_size.Width, content_size.Height);
                                    if resized {
                                        queue_clone.clear();
                                    } else {
                                        for _ in 0..skipped_frames {
                                            queue_clone.note_superseded();
                                        }
                                    }
                                    // A static window may only produce one frame for a resize.
                                    let copied =
                                        (|| -> Result<Option<CapturedFrame>, CaptureError> {
                                            let hwnd = HWND(hwnd as *mut std::ffi::c_void);
                                            let (client_screen, frame_bounds) =
                                                match wgc_window_rects(hwnd) {
                                                    Err(_)
                                                        if !unsafe { IsWindow(Some(hwnd)) }
                                                            .as_bool() =>
                                                    {
                                                        return Ok(None)
                                                    }
                                                    result => result?,
                                                };
                                            let surface = frame.Surface().map_err(|e| {
                                                CaptureError::DeviceError(e.to_string())
                                            })?;
                                            let access = surface
                                                .cast::<IDirect3DDxgiInterfaceAccess>()
                                                .map_err(|e| {
                                                    CaptureError::DeviceError(e.to_string())
                                                })?;
                                            let src_tex =
                                                unsafe { access.GetInterface::<ID3D11Texture2D>() }
                                                    .map_err(|e| {
                                                        CaptureError::DeviceError(e.to_string())
                                                    })?;
                                            let src_desc = texture_desc(&src_tex);
                                            if src_desc.Format != copy_format {
                                                return Err(CaptureError::DeviceError(format!(
                                                    "WGC frame format {} did not match requested copy format {}",
                                                    src_desc.Format.0, copy_format.0
                                                )));
                                            }
                                            let Some(src_box) = wgc_capture_box(
                                                client_screen,
                                                frame_bounds,
                                                region,
                                                (
                                                    src_desc.Width.min(content_size.Width as u32),
                                                    src_desc.Height.min(content_size.Height as u32),
                                                ),
                                            ) else {
                                                return Ok(None);
                                            };
                                            let width = src_box.right - src_box.left;
                                            let height = src_box.bottom - src_box.top;
                                            let dst_tex = alloc_capture_copy_texture(
                                                &device,
                                                &queue_clone,
                                                width,
                                                height,
                                                copy_format,
                                            )?;
                                            unsafe {
                                                callback_context.CopySubresourceRegion(
                                                    &dst_tex,
                                                    0,
                                                    0,
                                                    0,
                                                    0,
                                                    &src_tex,
                                                    0,
                                                    Some(&src_box),
                                                );
                                            }
                                            let raw_qpc = frame
                                                .SystemRelativeTime()
                                                .ok()
                                                .and_then(|time| {
                                                    qpc_from_100ns(time.Duration, qpc_frequency)
                                                })
                                                .unwrap_or_else(qpc_now);
                                            let timestamp_qpc =
                                                clamp_monotonic_qpc(raw_qpc, &last_timestamp_qpc);
                                            Ok(Some(CapturedFrame {
                                                texture: Some(dst_tex),
                                                width,
                                                height,
                                                timestamp_qpc,
                                                is_duplicate: false,
                                            }))
                                        })();
                                    if resized {
                                        frame.Close().map_err(|e| {
                                            CaptureError::WgcUnsupported(e.to_string())
                                        })?;
                                        closed = true;
                                        let dxgi_device: IDXGIDevice =
                                            device.cast().map_err(|e| {
                                                CaptureError::WgcUnsupported(e.to_string())
                                            })?;
                                        let inspectable = unsafe {
                                            CreateDirect3D11DeviceFromDXGIDevice(&dxgi_device)
                                        }
                                        .map_err(|e| CaptureError::WgcUnsupported(e.to_string()))?;
                                        let callback_device: IDirect3DDevice =
                                            inspectable.cast().map_err(|e| {
                                                CaptureError::WgcUnsupported(e.to_string())
                                            })?;
                                        pool_ref
                                            .Recreate(
                                                &callback_device,
                                                pixel_format,
                                                3,
                                                content_size,
                                            )
                                            .map_err(|e| {
                                                CaptureError::WgcUnsupported(e.to_string())
                                            })?;
                                        *current_size = (content_size.Width, content_size.Height);
                                    }
                                    if let Some(frame) = copied? {
                                        queue_clone.push_frame(frame);
                                    }
                                    Ok(())
                                })();
                                if !closed {
                                    let _ = frame.Close();
                                }
                                if let Err(error) = result {
                                    if let Ok(mut pending) = async_error.lock() {
                                        *pending = Some(error);
                                    }
                                    queue_clone.frame_ready.signal();
                                }
                            }
                        }
                        Ok(())
                    },
                ),
            )
            .map_err(|e| CaptureError::WgcUnsupported(e.to_string()))?;

        let session = pool
            .CreateCaptureSession(&item)
            .map_err(|e| CaptureError::WgcUnsupported(e.to_string()))?;
        let border_set = session.SetIsBorderRequired(self.session_status.requested_border_required);
        self.session_status.actual_border_required = session.IsBorderRequired().ok();
        self.session_status.border_error =
            border_set.err().map(|error| error.to_string()).or_else(|| {
                self.session_status
                    .actual_border_required
                    .is_none()
                    .then(|| "read-back failed".to_owned())
            });
        let cursor_set =
            session.SetIsCursorCaptureEnabled(self.session_status.requested_cursor_capture_enabled);
        self.session_status.actual_cursor_capture_enabled = session.IsCursorCaptureEnabled().ok();
        self.session_status.cursor_capture_error =
            cursor_set.err().map(|error| error.to_string()).or_else(|| {
                self.session_status
                    .actual_cursor_capture_enabled
                    .is_none()
                    .then(|| "read-back failed".to_owned())
            });
        let requested_interval = TimeSpan {
            Duration: self.session_status.requested_min_update_interval_100ns,
        };
        let interval_set = session.SetMinUpdateInterval(requested_interval);
        self.session_status.actual_min_update_interval_100ns = session
            .MinUpdateInterval()
            .ok()
            .map(|interval| interval.Duration);
        self.session_status.min_update_interval_error = interval_set
            .err()
            .map(|error| error.to_string())
            .or_else(|| {
                self.session_status
                    .actual_min_update_interval_100ns
                    .is_none()
                    .then(|| "read-back failed".to_owned())
            });

        let multithread: ID3D11Multithread = context
            .cast()
            .map_err(|e| CaptureError::WgcUnsupported(e.to_string()))?;
        let closed_error = Arc::clone(&self.async_error);
        let closed_queue = Arc::clone(&self.queue);
        let closed_token = item
            .Closed(
                &TypedEventHandler::<GraphicsCaptureItem, IInspectable>::new(move |_, _| {
                    if let Ok(mut pending) = closed_error.lock() {
                        *pending = Some(CaptureError::WgcItemClosed);
                    }
                    closed_queue.frame_ready.signal();
                    Ok(())
                }),
            )
            .map_err(|e| CaptureError::WgcUnsupported(e.to_string()))?;
        let multithread_was_protected = unsafe { multithread.GetMultithreadProtected() }.as_bool();
        if !multithread_was_protected {
            unsafe {
                let _ = multithread.SetMultithreadProtected(true);
            }
        }
        if let Err(error) = session.StartCapture() {
            let _ = item.RemoveClosed(closed_token);
            let _ = pool.RemoveFrameArrived(frame_token);
            if !multithread_was_protected {
                unsafe {
                    let _ = multithread.SetMultithreadProtected(false);
                }
            }
            let _ = session.Close();
            let _ = pool.Close();
            return Err(CaptureError::WgcUnsupported(error.to_string()));
        }

        self.pool = Some(pool);
        self.frame_token = Some(frame_token);
        self.session = Some(session);
        self.closed_handler = Some((item, closed_token));
        self.multithread = Some(multithread);
        self.multithread_was_protected = multithread_was_protected;
        Ok(())
    }

    fn acquire_frame(&mut self, timeout_ms: u32) -> Result<Option<CapturedFrame>, CaptureError> {
        self.take_async_error()?;
        let mut frame = self.queue.pop_latest();
        if frame.is_none() {
            frame = self.queue.pop_frame(timeout_ms);
            if let Some(newer) = self.queue.pop_latest() {
                if let Some(old_frame) = frame.take() {
                    if let Some(tex) = old_frame.texture {
                        self.queue.recycle_texture(tex);
                    }
                    self.queue.note_superseded();
                }
                frame = Some(newer);
            }
        }
        self.take_async_error()?;
        Ok(frame)
    }

    fn drain_latest_frame(&mut self) -> Result<Option<CapturedFrame>, CaptureError> {
        self.take_async_error()?;
        let frame = self.queue.pop_latest();
        self.take_async_error()?;
        Ok(frame)
    }

    fn recycle_frame_texture(&self, texture: ID3D11Texture2D) {
        self.queue.recycle_texture(texture);
    }

    fn frame_recycler(&self) -> Option<Arc<FrameRingQueue>> {
        Some(Arc::clone(&self.queue))
    }

    fn cursor_embedded(&self) -> bool {
        self.session_status
            .actual_cursor_capture_enabled
            .unwrap_or(false)
    }

    fn take_superseded_count(&self) -> u32 {
        self.queue.take_superseded_count()
    }

    fn wgc_session_status(&self) -> Option<WgcSessionStatus> {
        Some(self.session_status.clone())
    }

    fn ready_len(&self) -> usize {
        self.queue.ready_len()
    }

    fn peek_next_timestamp_qpc(&self) -> Option<i64> {
        self.queue.peek_next_timestamp_qpc()
    }

    fn peek_latest_timestamp_qpc(&self) -> Option<i64> {
        self.queue.peek_latest_timestamp_qpc()
    }

    fn stop(&mut self) {
        let _winrt = WinRtInitialization::new().ok();
        if let (Some(pool), Some(token)) = (&self.pool, self.frame_token.take()) {
            let _ = pool.RemoveFrameArrived(token);
        }
        if let Some((item, token)) = self.closed_handler.take() {
            let _ = item.RemoveClosed(token);
        }
        if let Some(session) = self.session.take() {
            let _ = session.Close();
        }
        if let Some(pool) = self.pool.take() {
            let _ = pool.Close();
        }
        if let Some(multithread) = self.multithread.take() {
            if !self.multithread_was_protected {
                unsafe {
                    let _ = multithread.SetMultithreadProtected(false);
                }
            }
        }
        self.queue.clear();
        if let Ok(mut pending) = self.async_error.lock() {
            *pending = None;
        }
    }
}

impl Drop for WgcCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

struct PrintWindowWorker {
    result: Receiver<Option<Vec<u8>>>,
    thread: thread::JoinHandle<()>,
}

static PRINT_WINDOW_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

struct PrintWindowPermit<'a>(&'a AtomicBool);

impl<'a> PrintWindowPermit<'a> {
    fn try_acquire() -> Option<Self> {
        Self::try_acquire_from(&PRINT_WINDOW_IN_FLIGHT)
    }

    fn try_acquire_from(in_flight: &'a AtomicBool) -> Option<Self> {
        in_flight
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| Self(in_flight))
    }
}

impl Drop for PrintWindowPermit<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// GDI fallback uses `PrintWindow` for occluded windows and `BitBlt` for visible regions.
/// The process-wide worker permit prevents repeated shutdowns from accumulating hung calls.
pub struct GdiCapture {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    pixels: GdiPixels,
    queue: Arc<FrameRingQueue>,
    staging_tex: Option<ID3D11Texture2D>,
    started: bool,
}

unsafe impl Send for GdiCapture {}

impl GdiCapture {
    pub fn new(
        device: ID3D11Device,
        context: ID3D11DeviceContext,
        hwnd: HWND,
        region: CaptureRegion,
        queue_capacity: usize,
    ) -> Result<Self, CaptureError> {
        let pixels = GdiPixels::new(hwnd, region)?;
        let queue = Arc::new(FrameRingQueue::new(queue_capacity)
            .map_err(|e| CaptureError::DeviceError(e.to_string()))?);
        Ok(Self { device, context, pixels, queue, staging_tex: None, started: false })
    }

    fn create_staging_texture(&self) -> Result<ID3D11Texture2D, CaptureError> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: self.pixels.region.width.max(1),
            Height: self.pixels.region.height.max(1),
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
            MiscFlags: 0,
        };
        let mut tex = None;
        unsafe {
            self.device
                .CreateTexture2D(&desc, None, Some(&mut tex))
                .map_err(|e| CaptureError::DeviceError(e.to_string()))?;
        }
        tex.ok_or_else(|| CaptureError::DeviceError("CreateTexture2D staging returned None".into()))
    }
}

/// CPU-accessible BGRA window capture, independent of a graphics device.
pub struct GdiPixels {
    hwnd: HWND,
    region: CaptureRegion,
    cached_dc: HDC,
    cached_bmp: HBITMAP,
    pixel_buf: Vec<u8>,
    print_worker: Option<PrintWindowWorker>,
}

impl GdiPixels {
    /// Uses one capture source consistently, avoiding alternating window and screen pixels.
    pub fn acquire_window(&mut self, timeout_ms: u32) -> Result<Option<&[u8]>, CaptureError> {
        if unsafe { !IsWindow(Some(self.hwnd)).as_bool() } {
            return Err(CaptureError::AccessLost);
        }
        let Some(pixels) = self.print_window_pixels(timeout_ms) else { return Ok(None); };
        self.pixel_buf = pixels;
        for pixel in self.pixel_buf.chunks_exact_mut(4) { pixel[3] = 255; }
        Ok(Some(&self.pixel_buf))
    }

    pub fn new(hwnd: HWND, region: CaptureRegion) -> Result<Self, CaptureError> {
        if !(1..=D3D11_REQ_TEXTURE2D_U_OR_V_DIMENSION).contains(&region.width)
            || !(1..=D3D11_REQ_TEXTURE2D_U_OR_V_DIMENSION).contains(&region.height)
        {
            return Err(CaptureError::DeviceError(
                "invalid GDI capture dimensions".into(),
            ));
        }

        Ok(Self { hwnd, region, cached_dc: HDC::default(), cached_bmp: HBITMAP::default(),
            pixel_buf: vec![0; region.width as usize * region.height as usize * 4], print_worker: None })
    }

    fn print_window_pixels(&mut self, timeout_ms: u32) -> Option<Vec<u8>> {
        if self.print_worker.is_none() {
            let permit = PrintWindowPermit::try_acquire()?;
            let hwnd = self.hwnd.0 as isize;
            let width = self.region.width;
            let height = self.region.height;
            let (sender, result) = mpsc::channel();
            // The permit is owned by the closure, so it remains held while a hung OS call runs.
            let worker = thread::Builder::new()
                .name("FlowScale PrintWindow".to_owned())
                .spawn(move || {
                    let _permit = permit;
                    let pixels =
                        print_window_bgra(HWND(hwnd as *mut std::ffi::c_void), width, height);
                    let _ = sender.send(pixels);
                })
                .ok()?;
            self.print_worker = Some(PrintWindowWorker {
                result,
                thread: worker,
            });
        }

        let worker = self.print_worker.as_ref()?;
        let received = if timeout_ms == 0 {
            worker.result.try_recv().ok()
        } else {
            worker
                .result
                .recv_timeout(Duration::from_millis(u64::from(timeout_ms.min(50))))
                .ok()
        };
        let worker_finished = self
            .print_worker
            .as_ref()
            .is_some_and(|worker| worker.thread.is_finished());
        if received.is_some() || worker_finished {
            if let Some(worker) = self.print_worker.take() {
                if worker.thread.is_finished() {
                    let _ = worker.thread.join();
                }
                // If the OS call is still unwinding, dropping the handle detaches the worker;
                // all GDI handles and pixel memory remain owned by its closure until it returns.
            }
        }
        received.flatten()
    }

    fn release_gdi_cache(&mut self) {
        if !self.cached_bmp.is_invalid() {
            unsafe {
                let _ = DeleteObject(self.cached_bmp.into());
            }
            self.cached_bmp = HBITMAP::default();
        }
        if !self.cached_dc.is_invalid() {
            unsafe {
                let _ = DeleteDC(self.cached_dc);
            }
            self.cached_dc = HDC::default();
        }
    }

    pub fn acquire(&mut self, timeout_ms: u32) -> Result<Option<&[u8]>, CaptureError> {
        if unsafe { !IsWindow(Some(self.hwnd)).as_bool() } {
            return Err(CaptureError::AccessLost);
        }
        let w = self.region.width.max(1);
        let h = self.region.height.max(1);

        let src_dc = unsafe { GetDC(Some(self.hwnd)) };
        if src_dc.is_invalid() {
            if timeout_ms > 0 {
                thread::sleep(Duration::from_millis(u64::from(timeout_ms.min(8))));
            }
            return Ok(None);
        }
        if self.cached_dc.is_invalid() {
            self.cached_dc = unsafe { CreateCompatibleDC(Some(src_dc)) };
        }
        if self.cached_dc.is_invalid() {
            unsafe {
                let _ = ReleaseDC(Some(self.hwnd), src_dc);
            }
            return Ok(None);
        }
        if self.cached_bmp.is_invalid() {
            self.cached_bmp = unsafe { CreateCompatibleBitmap(src_dc, w as i32, h as i32) };
        }
        if self.cached_bmp.is_invalid() {
            self.release_gdi_cache();
            unsafe {
                let _ = ReleaseDC(Some(self.hwnd), src_dc);
            }
            return Ok(None);
        }
        let mem_dc = self.cached_dc;
        let bmp = self.cached_bmp;

        let prev_obj = unsafe { SelectObject(mem_dc, bmp.into()) };
        let print_pixels = if self.region.left == 0 && self.region.top == 0 {
            self.print_window_pixels(timeout_ms)
        } else {
            None
        };
        let dib_ok = if let Some(pixels) = print_pixels {
            unsafe {
                let _ = SelectObject(mem_dc, prev_obj);
            }
            self.pixel_buf = pixels;
            true
        } else {
            let captured = unsafe {
                BitBlt(
                    mem_dc,
                    0,
                    0,
                    w as i32,
                    h as i32,
                    Some(src_dc),
                    self.region.left,
                    self.region.top,
                    SRCCOPY,
                )
                .is_ok()
            };
            unsafe {
                let _ = SelectObject(mem_dc, prev_obj);
            }
            if !captured {
                false
            } else {
                let mut bmi = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: w as i32,
                        biHeight: -(h as i32), // top-down BGRA
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB.0,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let lines = unsafe {
                    GetDIBits(
                        mem_dc,
                        bmp,
                        0,
                        h,
                        Some(self.pixel_buf.as_mut_ptr() as *mut _),
                        &mut bmi,
                        DIB_RGB_COLORS,
                    )
                };
                has_complete_dib_rows(lines, h)
            }
        };

        unsafe {
            let _ = ReleaseDC(Some(self.hwnd), src_dc);
        }

        if !dib_ok {
            if timeout_ms > 0 {
                thread::sleep(Duration::from_millis(u64::from(timeout_ms.min(8))));
            }
            return Ok(None);
        }

        // GDI PrintWindow/BitBlt leaves alpha 0 on many windows; force opaque alpha.
        for px in self.pixel_buf.chunks_exact_mut(4) {
            px[3] = 0xFF;
        }

        Ok(Some(&self.pixel_buf))
    }
}

impl Drop for GdiPixels {
    fn drop(&mut self) { self.release_gdi_cache(); }
}

fn print_window_bgra(hwnd: HWND, width: u32, height: u32) -> Option<Vec<u8>> {
    let src_dc = unsafe { GetDC(Some(hwnd)) };
    if src_dc.is_invalid() {
        return None;
    }
    let mem_dc = unsafe { CreateCompatibleDC(Some(src_dc)) };
    if mem_dc.is_invalid() {
        unsafe {
            let _ = ReleaseDC(Some(hwnd), src_dc);
        }
        return None;
    }
    let bitmap = unsafe { CreateCompatibleBitmap(src_dc, width as i32, height as i32) };
    if bitmap.is_invalid() {
        unsafe {
            let _ = DeleteDC(mem_dc);
            let _ = ReleaseDC(Some(hwnd), src_dc);
        }
        return None;
    }

    let previous = unsafe { SelectObject(mem_dc, bitmap.into()) };
    let result = (|| {
        if !unsafe { PrintWindow(hwnd, mem_dc, PRINT_WINDOW_FLAGS(3)) }.as_bool() {
            return None;
        }
        if previous.is_invalid() {
            return None;
        }
        unsafe {
            let _ = SelectObject(mem_dc, previous);
        }
        let byte_len = usize::try_from(width)
            .ok()?
            .checked_mul(height as usize)?
            .checked_mul(4)?;
        let mut pixels = vec![0; byte_len];
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                biHeight: -(height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let rows = unsafe {
            GetDIBits(
                mem_dc,
                bitmap,
                0,
                height,
                Some(pixels.as_mut_ptr().cast()),
                &mut info,
                DIB_RGB_COLORS,
            )
        };
        has_complete_dib_rows(rows, height).then_some(pixels)
    })();
    unsafe {
        if !previous.is_invalid() {
            let _ = SelectObject(mem_dc, previous);
        }
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(mem_dc);
        let _ = ReleaseDC(Some(hwnd), src_dc);
    }
    result
}

impl CaptureBackend for GdiCapture {
    fn api(&self) -> CaptureApi {
        CaptureApi::Gdi
    }

    fn start(&mut self) -> Result<(), CaptureError> {
        if self.pixels.hwnd.is_invalid()
            || self.pixels.hwnd.0.is_null()
            || unsafe { !IsWindow(Some(self.pixels.hwnd)).as_bool() }
        {
            return Err(CaptureError::DeviceError(
                "invalid HWND for GDI capture".into(),
            ));
        }
        self.staging_tex = Some(self.create_staging_texture()?);
        self.started = true;
        Ok(())
    }

    fn acquire_frame(&mut self, timeout_ms: u32) -> Result<Option<CapturedFrame>, CaptureError> {
        if !self.started {
            return Err(CaptureError::DeviceError("GDI capture not started".into()));
        }
        let w = self.pixels.region.width;
        let h = self.pixels.region.height;
        if self.pixels.acquire(timeout_ms)?.is_none() { return Ok(None); }

        let staging = self
            .staging_tex
            .as_ref()
            .ok_or_else(|| CaptureError::DeviceError("GDI staging texture missing".into()))?;
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            self.context
                .Map(staging, 0, D3D11_MAP_WRITE, 0, Some(&mut mapped))
                .map_err(|e| CaptureError::DeviceError(e.to_string()))?;
            let row_bytes = (w * 4) as usize;
            let dst_pitch = mapped.RowPitch as usize;
            let dst_base = mapped.pData.cast::<u8>();
            for y in 0..(h as usize) {
                let src_row = &self.pixels.pixel_buf[y * row_bytes..(y + 1) * row_bytes];
                std::ptr::copy_nonoverlapping(
                    src_row.as_ptr(),
                    dst_base.add(y * dst_pitch),
                    row_bytes,
                );
            }
            self.context.Unmap(staging, 0);
        }

        let dst_tex = alloc_bgra_copy_texture(&self.device, &self.queue, w, h)?;
        unsafe {
            self.context.CopyResource(&dst_tex, staging);
        }
        let mut qpc = 0i64;
        unsafe {
            let _ = QueryPerformanceCounter(&mut qpc);
        }
        Ok(Some(CapturedFrame {
            texture: Some(dst_tex),
            width: w,
            height: h,
            timestamp_qpc: qpc,
            is_duplicate: false,
        }))
    }

    fn recycle_frame_texture(&self, texture: ID3D11Texture2D) {
        self.queue.recycle_texture(texture);
    }

    fn frame_recycler(&self) -> Option<Arc<FrameRingQueue>> {
        Some(Arc::clone(&self.queue))
    }

    fn ready_len(&self) -> usize {
        self.queue.ready_len()
    }

    fn peek_next_timestamp_qpc(&self) -> Option<i64> {
        self.queue.peek_next_timestamp_qpc()
    }

    fn peek_latest_timestamp_qpc(&self) -> Option<i64> {
        self.queue.peek_latest_timestamp_qpc()
    }

    fn stop(&mut self) {
        self.started = false;
        self.staging_tex = None;
        self.pixels.release_gdi_cache();
        self.queue.clear();
    }
}

impl Drop for GdiCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_rows_flag_only_real_pixel_changes() {
        let (a, b) = ([1u8; 8], [2u8; 8]);
        let mut prev = Vec::new();
        assert!(probe_rows_changed(&mut prev, [&a[..], &a[..]].into_iter(), 8)); // first frame
        assert!(!probe_rows_changed(&mut prev, [&a[..], &a[..]].into_iter(), 8)); // phantom duplicate
        assert!(probe_rows_changed(&mut prev, [&a[..], &b[..]].into_iter(), 8)); // one row changed
        assert!(!probe_rows_changed(&mut prev, [&a[..], &b[..]].into_iter(), 8));
        assert!(probe_rows_changed(&mut prev, [&a[..]].into_iter(), 8)); // size change
    }

    #[test]
    fn dxgi_updates_outside_game_region_are_not_game_frames() {
        let region = CaptureRegion { left: 80, top: 80, width: 960, height: 540 };
        let rect = |left, top, right, bottom| RECT { left, top, right, bottom };
        // Taskbar clock / console window elsewhere on the desktop.
        assert!(!rects_touch_region([rect(1800, 1040, 1920, 1080), rect(1100, 0, 1900, 600)], region));
        // Touching edges without overlap.
        assert!(!rects_touch_region([rect(0, 0, 80, 1080), rect(1040, 80, 1100, 620)], region));
        // Any overlap with the game is a real frame.
        assert!(rects_touch_region([rect(1800, 1040, 1920, 1080), rect(1000, 600, 1100, 700)], region));
        assert!(!rects_touch_region([], region));
    }

    #[test]
    fn capture_region_rejects_extreme_offsets_without_overflow() {
        for rect in [(0, 0, 1920, 1080), (i32::MAX, i32::MAX, i32::MIN, i32::MIN)] {
            assert!(CaptureRegion::from_rect_and_offsets(
                rect,
                i32::MAX,
                i32::MAX,
                i32::MAX,
                i32::MAX
            )
            .is_err());
        }
        assert!(
            CaptureRegion::from_rect_and_offsets((i32::MIN, 0, i32::MAX, 1080), 0, 0, 0, 0)
                .is_err()
        );
        let region =
            CaptureRegion::from_rect_and_offsets((-1920, -1080, 0, 0), 10, 20, 30, 40).unwrap();
        assert_eq!(
            region,
            CaptureRegion {
                left: -1910,
                top: -1060,
                width: 1880,
                height: 1020
            }
        );
    }

    #[test]
    fn ring_queue_evicts_oldest_on_overflow() {
        let q = FrameRingQueue::new(2).unwrap();
        assert_eq!(
            q.push_frame(CapturedFrame::metadata_only(1280, 720, 100, false)),
            PushOutcome::Enqueued
        );
        assert_eq!(
            q.push_frame(CapturedFrame::metadata_only(1280, 720, 200, false)),
            PushOutcome::Enqueued
        );
        assert_eq!(
            q.push_frame(CapturedFrame::metadata_only(1280, 720, 300, false)),
            PushOutcome::EvictedOldest
        );
        assert_eq!(q.ready_len(), 2);

        let f1 = q.pop_frame(0).unwrap();
        let f2 = q.pop_frame(0).unwrap();
        assert_eq!(f1.timestamp_qpc, 200);
        assert_eq!(f2.timestamp_qpc, 300);
        assert!(q.pop_frame(0).is_none());
    }

    #[test]
    fn empty_queue_wait_preserves_capture_failure_wake() {
        let queue = FrameRingQueue::new(1).unwrap();
        queue.frame_ready.signal();
        let start = std::time::Instant::now();
        assert!(queue.pop_frame(250).is_none());
        assert!(start.elapsed() < Duration::from_millis(125));
        assert!(!queue.frame_ready.wait(0));
    }

    #[test]
    fn single_slot_queue_delivers_only_the_latest_unread_frame() {
        let queue = FrameRingQueue::new(1).unwrap();
        for timestamp in [10, 20, 30] {
            queue.push_frame(CapturedFrame::metadata_only(64, 64, timestamp, false));
        }
        assert_eq!(queue.pop_frame(0).unwrap().timestamp_qpc, 30);
        assert_eq!(queue.ready_len(), 0);
    }

    #[test]
    fn ring_queue_drain_to_target() {
        let q = FrameRingQueue::new(4).unwrap();
        for ts in [10, 20, 30, 40] {
            q.push_frame(CapturedFrame::metadata_only(1920, 1080, ts, false));
        }
        assert!(q.should_drain(1));
        assert_eq!(q.drain_to(1), 3);
        assert_eq!(q.ready_len(), 1);
        assert_eq!(q.pop_frame(0).unwrap().timestamp_qpc, 40);
    }

    #[test]
    fn capture_region_validates_offsets() {
        let ok =
            CaptureRegion::from_rect_and_offsets((100, 100, 1380, 820), 10, 20, 10, 20).unwrap();
        assert_eq!(ok.left, 110);
        assert_eq!(ok.top, 120);
        assert_eq!(ok.width, 1260);
        assert_eq!(ok.height, 680);

        let err = CaptureRegion::from_rect_and_offsets((0, 0, 100, 100), 60, 0, 50, 0).unwrap_err();
        assert!(matches!(
            err,
            CaptureError::LargeOffset {
                width: -10,
                height: 100
            }
        ));
    }

    #[test]
    fn dxgi_recovery_retries_access_lost_and_invalid_call_with_a_bound() {
        assert!(is_duplication_recoverable(DXGI_ERROR_ACCESS_LOST));
        assert!(is_duplication_recoverable(DXGI_ERROR_INVALID_CALL));
        assert!(is_duplication_recoverable(DXGI_ERROR_SESSION_DISCONNECTED));
        assert!(desktop_unavailable(DXGI_ERROR_SESSION_DISCONNECTED));
        assert!(desktop_unavailable(E_ACCESSDENIED));
        assert!(!desktop_unavailable(DXGI_ERROR_INVALID_CALL));
        assert!(!is_duplication_recoverable(DXGI_ERROR_WAIT_TIMEOUT));
        assert_eq!(
            (0..DXGI_RECOVERY_BACKOFF_MS.len())
                .map(recovery_delay_ms)
                .collect::<Vec<_>>(),
            [
                Some(10),
                Some(25),
                Some(50),
                Some(100),
                Some(200),
                Some(400)
            ]
        );
        assert_eq!(recovery_delay_ms(DXGI_RECOVERY_BACKOFF_MS.len()), None);
    }

    #[test]
    fn dxgi_cursor_visibility_is_cached_until_a_new_pointer_update() {
        assert_eq!(dxgi_cursor_state(0, true, 0, true), (0, true));
        assert_eq!(dxgi_cursor_state(0, true, 100, true), (100, false));
        assert_eq!(dxgi_cursor_state(100, false, 100, false), (100, false));
        assert_eq!(dxgi_cursor_state(100, false, 200, false), (200, true));
    }

    #[test]
    fn dxgi_crop_intersects_source_dimensions_without_oob() {
        assert_eq!(
            crop_to_texture(
                CaptureRegion {
                    left: 1200,
                    top: 600,
                    width: 400,
                    height: 300,
                },
                1280,
                720,
            ),
            Some((1200, 600, 80, 120))
        );
        assert_eq!(
            crop_to_texture(
                CaptureRegion {
                    left: 1280,
                    top: 0,
                    width: 400,
                    height: 300,
                },
                1280,
                720,
            ),
            None
        );
        assert_eq!(
            crop_to_texture(
                CaptureRegion {
                    left: -10,
                    top: 0,
                    width: 100,
                    height: 50,
                },
                1280,
                720,
            ),
            Some((0, 0, 90, 50))
        );
        assert_eq!(
            crop_to_texture(
                CaptureRegion {
                    left: -1920,
                    top: -120,
                    width: 1920,
                    height: 1080,
                },
                1920,
                1080,
            ),
            None
        );
        assert_eq!(
            crop_to_texture(
                CaptureRegion {
                    left: -20,
                    top: -30,
                    width: 100,
                    height: 80,
                },
                1920,
                1080,
            ),
            Some((0, 0, 80, 50))
        );
    }

    #[test]
    fn dxgi_rotation_only_allows_identity_surface_orientation() {
        use windows::Win32::Graphics::Dxgi::Common::{
            DXGI_MODE_ROTATION_IDENTITY, DXGI_MODE_ROTATION_ROTATE180,
            DXGI_MODE_ROTATION_ROTATE270, DXGI_MODE_ROTATION_ROTATE90,
        };

        assert!(!dxgi_rotation_requires_wgc(DXGI_MODE_ROTATION_IDENTITY));
        for rotation in [
            DXGI_MODE_ROTATION_ROTATE90,
            DXGI_MODE_ROTATION_ROTATE180,
            DXGI_MODE_ROTATION_ROTATE270,
        ] {
            assert!(dxgi_rotation_requires_wgc(rotation));
        }
    }

    #[test]
    fn wgc_hdr_selects_fp16_pool_and_copy_format_without_relabeling_frames() {
        assert_eq!(
            wgc_pixel_format(true),
            DirectXPixelFormat::R16G16B16A16Float
        );
        assert_eq!(capture_copy_format(true), DXGI_FORMAT_R16G16B16A16_FLOAT);
        assert_eq!(
            wgc_pixel_format(false),
            DirectXPixelFormat::B8G8R8A8UIntNormalized
        );
        assert_eq!(capture_copy_format(false), DXGI_FORMAT_B8G8R8A8_UNORM);
        assert_eq!(CapturedFrame::metadata_only(1, 1, 1, false).format(), None);
    }

    #[test]
    fn dropping_pending_printwindow_worker_does_not_join_it() {
        let (sender, result) = mpsc::channel();
        let worker_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            let _ = sender.send(None);
        });
        let worker = PrintWindowWorker {
            result,
            thread: worker_thread,
        };
        let started = std::time::Instant::now();
        drop(worker);
        assert!(started.elapsed() < Duration::from_millis(50));
    }

    #[test]
    fn printwindow_permit_allows_only_one_outstanding_call_and_releases_on_drop() {
        let in_flight = AtomicBool::new(false);
        let permit = PrintWindowPermit::try_acquire_from(&in_flight).unwrap();
        assert!(PrintWindowPermit::try_acquire_from(&in_flight).is_none());
        drop(permit);
        assert!(PrintWindowPermit::try_acquire_from(&in_flight).is_some());
    }

    #[test]
    fn wgc_crop_uses_client_origin_and_offsets_inside_window_surface() {
        let region = CaptureRegion {
            left: 12,
            top: 24,
            width: 640,
            height: 360,
        };
        let cropped =
            wgc_capture_box((100, 120, 900, 620), (80, 80, 920, 640), region, (840, 560)).unwrap();
        assert_eq!((cropped.left, cropped.top), (32, 64));
        assert_eq!((cropped.right, cropped.bottom), (672, 424));
        // The old crop still fits the backing texture after a client-area shrink.
        assert!(
            wgc_capture_box((100, 120, 700, 620), (80, 80, 920, 640), region, (840, 560)).is_none()
        );
        assert!(wgc_capture_box(
            (100, 120, 900, 620),
            (80, 80, 920, 640),
            CaptureRegion {
                left: 12,
                top: 24,
                width: 820,
                height: 360,
            },
            (840, 560),
        )
        .is_none());
    }

    #[test]
    fn wgc_system_time_converts_to_qpc_ticks() {
        assert_eq!(qpc_from_100ns(5_000_000, 20_000_000), Some(10_000_000));
        assert_eq!(qpc_from_100ns(0, 20_000_000), None);
        assert_eq!(qpc_from_100ns(1, 0), None);
    }

    #[test]
    fn win32_event_signal_and_wait() {
        let ev = Win32Event::new(false, false).unwrap();
        assert!(!ev.wait(0));
        ev.signal();
        assert!(ev.wait(10));
        // Auto-reset clears after wait.
        assert!(!ev.wait(0));
    }

    #[test]
    fn dxgi_poll_waits_outside_the_driver_and_preserves_errors() {
        let mut calls = 0;
        let start = Instant::now();
        let error = poll_dxgi_frame(8, |wait| {
            assert_eq!(wait, 0, "the driver must never block the shared device");
            calls += 1;
            Err(DXGI_ERROR_WAIT_TIMEOUT.into())
        }).unwrap_err();
        assert_eq!(error.code(), DXGI_ERROR_WAIT_TIMEOUT);
        assert!(start.elapsed() >= Duration::from_millis(8));
        assert!(calls >= 2);

        calls = 0;
        assert!(poll_dxgi_frame(0, |_| {
            calls += 1;
            Err(DXGI_ERROR_WAIT_TIMEOUT.into())
        }).is_err());
        assert_eq!(calls, 1, "zero timeout must be a single nonblocking attempt");

        calls = 0;
        poll_dxgi_frame(100, |_| {
            calls += 1;
            if calls < 3 { Err(DXGI_ERROR_WAIT_TIMEOUT.into()) } else { Ok(()) }
        }).unwrap();
        assert_eq!(calls, 3);

        calls = 0;
        let error = poll_dxgi_frame(100, |_| {
            calls += 1;
            Err(DXGI_ERROR_ACCESS_LOST.into())
        }).unwrap_err();
        assert_eq!(error.code(), DXGI_ERROR_ACCESS_LOST);
        assert_eq!(calls, 1, "recovery errors must reach the duplication owner");
    }

    #[test]
    fn dxgi_recreates_duplication_after_acquire_with_an_outstanding_frame() {
        use windows::Win32::Graphics::Direct3D::{
            D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0,
        };
        use windows::Win32::Graphics::Direct3D11::{
            D3D11CreateDevice, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
        };

        let mut device = None;
        let mut context = None;
        if unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                Default::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        }
        .is_err()
        {
            eprintln!("DXGI recovery fixture requires D3D11 hardware");
            return;
        }
        let device = device.unwrap();
        let dxgi: IDXGIDevice = device.cast().unwrap();
        let adapter = unsafe { dxgi.GetAdapter() }.unwrap();
        let Ok(output) = (unsafe { adapter.EnumOutputs(0) }) else {
            eprintln!("DXGI recovery fixture requires an attached output");
            return;
        };
        let region = CaptureRegion {
            left: 0,
            top: 0,
            width: 16,
            height: 16,
        };
        let mut capture = DxgiDuplicationCapture::new(
            device,
            context.unwrap(),
            output.cast().unwrap(),
            region,
            2,
        )
        .unwrap();
        if let Err(error) = capture.start() {
            eprintln!("DXGI recovery fixture requires desktop duplication: {error}");
            return;
        }

        let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource = None;
        let acquired = unsafe {
            capture.duplication.as_ref().unwrap().AcquireNextFrame(
                250,
                &mut frame_info,
                &mut resource,
            )
        };
        if let Err(error) = acquired {
            eprintln!("DXGI recovery fixture could not acquire a desktop frame: {error}");
            return;
        }
        drop(resource);

        // Deliberately leave the lease outstanding so the next AcquireNextFrame fails.
        capture
            .acquire_frame(250)
            .expect("DXGI should recreate the duplication");
        assert!(capture.take_discontinuity());
        assert!(!capture.take_discontinuity());
        eprintln!("DXGI outstanding-frame recovery exercised on hardware");
    }

    #[test]
    fn gdi_capture_and_wgc_resize_close_smoke() {
        use windows::core::w;
        use windows::Win32::Foundation::{HMODULE, LPARAM, LRESULT, WPARAM};
        use windows::Win32::Graphics::Direct3D::{
            D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0,
        };
        use windows::Win32::Graphics::Direct3D11::{
            D3D11CreateDevice, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
        };
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, PeekMessageW,
            RegisterClassExW, SetWindowPos, MSG, PM_REMOVE, SWP_NOACTIVATE, SWP_NOZORDER,
            SWP_SHOWWINDOW, WINDOW_EX_STYLE, WNDCLASSEXW, WS_POPUP,
        };

        unsafe extern "system" fn wndproc(
            hwnd: HWND,
            msg: u32,
            wparam: WPARAM,
            lparam: LPARAM,
        ) -> LRESULT {
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }

        let mut device = None;
        let mut context = None;
        let levels = [D3D_FEATURE_LEVEL_11_0];
        let created = unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&levels),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        };
        if created.is_err() {
            return;
        }
        let device = device.unwrap();
        let context = context.unwrap();

        let region = CaptureRegion {
            left: 0,
            top: 0,
            width: 64,
            height: 64,
        };

        let mut invalid =
            GdiCapture::new(device.clone(), context.clone(), HWND::default(), region, 2).unwrap();
        assert_eq!(invalid.api(), CaptureApi::Gdi);
        assert!(invalid.start().is_err());
        for dimension in [0, D3D11_REQ_TEXTURE2D_U_OR_V_DIMENSION + 1, u32::MAX] {
            assert!(GdiCapture::new(
                device.clone(),
                context.clone(),
                HWND::default(),
                CaptureRegion {
                    width: dimension,
                    height: dimension,
                    ..region
                },
                2
            )
            .is_err());
        }

        let cls = w!("FlowScaleGdiTestWnd");
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
                w!("GdiTest"),
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

        let mut gdi = GdiCapture::new(device.clone(), context, hwnd, region, 2).unwrap();
        assert_eq!(gdi.api(), CaptureApi::Gdi);
        gdi.start().expect("GdiCapture::start failed");
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        let frame = loop {
            if let Some(frame) = gdi
                .acquire_frame(0)
                .expect("GdiCapture::acquire_frame failed")
            {
                break frame;
            }
            unsafe {
                let mut msg = MSG::default();
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    DispatchMessageW(&msg);
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "no GDI PrintWindow frame"
            );
            thread::sleep(Duration::from_millis(1));
        };
        assert_eq!(frame.width, 64);
        assert_eq!(frame.height, 64);
        assert!(frame.texture.is_some());
        assert!(!frame.is_duplicate);
        gdi.stop();

        // WGC requires a shown HWND. Keep the fixture outside the desktop and avoid activation.
        unsafe {
            SetWindowPos(
                hwnd,
                None,
                -30_000,
                -30_000,
                64,
                64,
                SWP_NOACTIVATE | SWP_NOZORDER | SWP_SHOWWINDOW,
            )
            .unwrap();
        }
        let _winrt = WinRtInitialization::new().unwrap();
        let crop = CaptureRegion {
            left: 5,
            top: 7,
            width: 50,
            height: 42,
        };
        let mut wgc = WgcCapture::new(device, hwnd, crop, true, 2).unwrap();
        let wgc_started = wgc.start();
        if wgc_started.is_ok() {
            if let Some(frame) = wgc
                .acquire_frame(150)
                .expect("offscreen WGC acquisition failed")
            {
                assert_eq!((frame.width, frame.height), (50, 42));
                assert_eq!(frame.format(), Some(DXGI_FORMAT_B8G8R8A8_UNORM));
                let desc = texture_desc(&frame.texture.unwrap());
                assert_eq!(
                    (desc.Width, desc.Height, desc.Format),
                    (50, 42, DXGI_FORMAT_B8G8R8A8_UNORM)
                );
                eprintln!("WGC texture copy verified on an offscreen target");
            }
            let before_resize = qpc_now();
            unsafe {
                SetWindowPos(
                    hwnd,
                    None,
                    -30_000,
                    -30_000,
                    96,
                    80,
                    SWP_NOACTIVATE | SWP_NOZORDER,
                )
                .unwrap();
            }
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            loop {
                unsafe {
                    let mut msg = MSG::default();
                    while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                        DispatchMessageW(&msg);
                    }
                }
                if let Some(frame) = wgc
                    .acquire_frame(20)
                    .expect("WGC resize acquisition failed")
                {
                    if frame.timestamp_qpc > before_resize {
                        let desc = texture_desc(&frame.texture.unwrap());
                        assert_eq!((desc.Width, desc.Height), (50, 42));
                        break;
                    }
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "no WGC frame after resize"
                );
            }
            eprintln!("WGC resumed delivery after an offscreen resize");
        }
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
        match wgc_started {
            Ok(()) => {
                let deadline = std::time::Instant::now() + Duration::from_secs(2);
                loop {
                    unsafe {
                        let mut msg = MSG::default();
                        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                            DispatchMessageW(&msg);
                        }
                    }
                    match wgc.acquire_frame(20) {
                        Err(CaptureError::WgcItemClosed) => break,
                        Err(error) => panic!("unexpected WGC error after target close: {error}"),
                        Ok(_) => assert!(
                            std::time::Instant::now() < deadline,
                            "missing WGC Closed event"
                        ),
                    }
                }
                eprintln!("WGC Closed event verified on an offscreen target");
            }
            Err(error) => eprintln!("WGC offscreen fixture unavailable: {error}"),
        }
    }

    #[test]
    fn ring_queue_pop_latest_drains_backlog_and_recycles_textures() {
        use windows::Win32::Foundation::HMODULE;
        use windows::Win32::Graphics::Direct3D::{
            D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0,
        };
        use windows::Win32::Graphics::Direct3D11::{
            D3D11CreateDevice, D3D11_BIND_SHADER_RESOURCE, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
        };

        let queue = FrameRingQueue::new(3).expect("FrameRingQueue::new failed");
        let mut stale = CapturedFrame::metadata_only(64, 64, 100, false);
        stale.timestamp_qpc = 100;
        let mut middle = CapturedFrame::metadata_only(64, 64, 200, false);
        middle.timestamp_qpc = 200;
        let mut newest = CapturedFrame::metadata_only(64, 64, 300, false);
        newest.timestamp_qpc = 300;
        queue.push_frame(stale);
        queue.push_frame(middle);
        queue.push_frame(newest);
        assert_eq!(queue.ready_len(), 3);

        let drained = queue.pop_latest().expect("expected newest frame");
        assert_eq!(drained.timestamp_qpc, 300);
        assert_eq!(queue.ready_len(), 0);
        assert_eq!(queue.take_superseded_count(), 2);
        assert_eq!(queue.take_superseded_count(), 0);
        assert!(queue.pop_latest().is_none());

        let mut device = None;
        let levels = [D3D_FEATURE_LEVEL_11_0];
        let created = unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&levels),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                None,
            )
        };
        if created.is_ok() {
            let dev = device.unwrap();
            let desc = D3D11_TEXTURE2D_DESC {
                Width: 32,
                Height: 32,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: (D3D11_BIND_SHADER_RESOURCE.0) as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };
            let mut tex = None;
            unsafe { dev.CreateTexture2D(&desc, None, Some(&mut tex)) }.unwrap();
            let tex = tex.unwrap();
            queue.recycle_texture(tex);
            let reused = queue.take_free_texture();
            assert!(reused.is_some());
            assert!(queue.take_free_texture().is_none());
        }
    }

    #[test]
    fn ring_queue_single_slot_burst_evicts_and_bounds_free_pool() {
        let queue = FrameRingQueue::new(1).expect("FrameRingQueue::new(1) failed");
        assert!(queue.peek_next_timestamp_qpc().is_none());
        assert!(queue.peek_latest_timestamp_qpc().is_none());

        assert_eq!(
            queue.push_frame(CapturedFrame::metadata_only(1280, 720, 1_000, false)),
            PushOutcome::Enqueued
        );
        assert_eq!(queue.peek_next_timestamp_qpc(), Some(1_000));
        assert_eq!(queue.peek_latest_timestamp_qpc(), Some(1_000));

        // Second frame in a WGC burst evicts the unread frame when cap_capacity=1 (queue_target=0).
        assert_eq!(
            queue.push_frame(CapturedFrame::metadata_only(1280, 720, 2_000, false)),
            PushOutcome::EvictedOldest
        );
        assert_eq!(queue.ready_len(), 1);
        assert_eq!(queue.peek_next_timestamp_qpc(), Some(2_000));
        assert_eq!(queue.peek_latest_timestamp_qpc(), Some(2_000));
        assert_eq!(queue.take_superseded_count(), 1);
        assert_eq!(queue.take_superseded_count(), 0);
        assert!(queue.free_pool_len() <= 1);
        assert_eq!(queue.pop_frame(0).unwrap().timestamp_qpc, 2_000);
        assert_eq!(queue.ready_len(), 0);
    }

    #[test]
    fn ring_queue_capacity_two_and_three_preserve_burst_without_eviction() {
        for (queue_target, cap_capacity) in [(1usize, 2usize), (2usize, 3usize)] {
            let queue = FrameRingQueue::new(cap_capacity).unwrap();
            for i in 1..=cap_capacity {
                let ts = (i as i64) * 69_444; // ~144 Hz ticks at 10 MHz QPC
                assert_eq!(
                    queue.push_frame(CapturedFrame::metadata_only(1920, 1080, ts, false)),
                    PushOutcome::Enqueued,
                    "queue_target={queue_target} should buffer {cap_capacity} frames without eviction"
                );
            }
            assert_eq!(queue.ready_len(), cap_capacity);
            assert_eq!(queue.peek_next_timestamp_qpc(), Some(69_444));
            assert_eq!(
                queue.peek_latest_timestamp_qpc(),
                Some((cap_capacity as i64) * 69_444)
            );
            // Non-blocking peek must not consume frames.
            assert_eq!(queue.ready_len(), cap_capacity);
            let latest = queue.pop_latest().unwrap();
            assert_eq!(latest.timestamp_qpc, (cap_capacity as i64) * 69_444);
            assert_eq!(queue.ready_len(), 0);
        }
    }

    #[test]
    fn wgc_timestamp_monotonicity_clamps_regressions_and_duplicates() {
        let last = AtomicI64::new(0);
        assert_eq!(clamp_monotonic_qpc(10_000, &last), 10_000);
        // Duplicate timestamp (e.g. quantized SystemRelativeTime) must still advance strictly.
        assert_eq!(clamp_monotonic_qpc(10_000, &last), 10_001);
        // Regression (e.g. SystemRelativeTime after a qpc_now() fallback) must clamp above prev.
        assert_eq!(clamp_monotonic_qpc(9_500, &last), 10_002);
        // Normal forward progress resumes cleanly.
        assert_eq!(clamp_monotonic_qpc(80_000, &last), 80_000);
    }

    #[test]
    fn capture_backend_non_blocking_peek_and_readiness_defaults() {
        struct MockBackend {
            queue: FrameRingQueue,
        }
        impl CaptureBackend for MockBackend {
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
                Ok(self.queue.pop_frame(timeout_ms))
            }
            fn drain_latest_frame(&mut self) -> Result<Option<CapturedFrame>, CaptureError> {
                Ok(self.queue.pop_latest())
            }
            fn peek_next_timestamp_qpc(&self) -> Option<i64> {
                self.queue.peek_next_timestamp_qpc()
            }
            fn peek_latest_timestamp_qpc(&self) -> Option<i64> {
                self.queue.peek_latest_timestamp_qpc()
            }
            fn ready_len(&self) -> usize {
                self.queue.ready_len()
            }
            fn stop(&mut self) {
                self.queue.clear();
            }
        }

        let mut backend = MockBackend {
            queue: FrameRingQueue::new(2).unwrap(),
        };
        assert!(!backend.has_ready_frame());
        assert_eq!(backend.peek_next_timestamp_qpc(), None);
        assert_eq!(backend.peek_latest_timestamp_qpc(), None);
        assert!(backend.try_acquire_frame().unwrap().is_none());

        backend
            .queue
            .push_frame(CapturedFrame::metadata_only(1280, 720, 100, false));
        backend
            .queue
            .push_frame(CapturedFrame::metadata_only(1280, 720, 200, false));
        assert!(backend.has_ready_frame());
        assert_eq!(backend.peek_next_timestamp_qpc(), Some(100));
        assert_eq!(backend.peek_latest_timestamp_qpc(), Some(200));

        let first = backend.try_acquire_frame().unwrap().unwrap();
        assert_eq!(first.timestamp_qpc, 100);
        assert!(backend.has_ready_frame());
        assert_eq!(backend.peek_next_timestamp_qpc(), Some(200));
        let second = backend.try_acquire_frame().unwrap().unwrap();
        assert_eq!(second.timestamp_qpc, 200);
        assert!(!backend.has_ready_frame());
    }

    #[test]
    fn producer_consumer_continuous_polling_and_stop_cycles_no_deadlock_or_leak() {
        use std::sync::atomic::{AtomicBool, AtomicUsize};
        use windows::Win32::Foundation::HMODULE;
        use windows::Win32::Graphics::Direct3D::{
            D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0,
        };
        use windows::Win32::Graphics::Direct3D11::{
            D3D11CreateDevice, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
        };

        struct ThreadedMockBackend {
            queue: Arc<FrameRingQueue>,
            started: bool,
        }
        impl CaptureBackend for ThreadedMockBackend {
            fn api(&self) -> CaptureApi {
                CaptureApi::Wgc
            }
            fn start(&mut self) -> Result<(), CaptureError> {
                self.started = true;
                Ok(())
            }
            fn acquire_frame(
                &mut self,
                timeout_ms: u32,
            ) -> Result<Option<CapturedFrame>, CaptureError> {
                Ok(self.queue.pop_frame(timeout_ms))
            }
            fn recycle_frame_texture(&self, texture: ID3D11Texture2D) {
                self.queue.recycle_texture(texture);
            }
            fn frame_recycler(&self) -> Option<Arc<FrameRingQueue>> {
                Some(Arc::clone(&self.queue))
            }
            fn ready_len(&self) -> usize {
                self.queue.ready_len()
            }
            fn stop(&mut self) {
                self.started = false;
                self.queue.clear();
            }
        }

        let mut device = None;
        let levels = [D3D_FEATURE_LEVEL_11_0];
        let created = unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&levels),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                None,
            )
        };
        let device = created.ok().and(device);

        let queue = Arc::new(FrameRingQueue::new(2).unwrap());
        let mut backend: Box<dyn CaptureBackend> = Box::new(ThreadedMockBackend {
            queue: Arc::clone(&queue),
            started: false,
        });

        for cycle in 0..3 {
            backend.start().unwrap();
            let recycler = backend
                .frame_recycler()
                .expect("CaptureBackend::frame_recycler should be Some");
            let stop_flag = Arc::new(AtomicBool::new(false));
            let mailbox: Arc<Mutex<Option<CapturedFrame>>> = Arc::new(Mutex::new(None));
            let consumed = Arc::new(AtomicUsize::new(0));

            // Simulate WGC/DXGI source feeding frames at ~144 Hz while consumer recycles.
            let feeder_stop = Arc::clone(&stop_flag);
            let feeder_queue = Arc::clone(&queue);
            let feeder_device = device.clone();
            let feeder = thread::spawn(move || {
                let mut ts = (cycle as i64 + 1) * 100_000;
                while !feeder_stop.load(Ordering::Relaxed) {
                    ts += 1;
                    let tex = feeder_device
                        .as_ref()
                        .map(|dev| alloc_bgra_copy_texture(dev, &feeder_queue, 64, 64).unwrap());
                    feeder_queue.push_frame(CapturedFrame {
                        texture: tex,
                        width: 64,
                        height: 64,
                        timestamp_qpc: ts,
                        is_duplicate: false,
                    });
                    thread::sleep(Duration::from_millis(2));
                }
            });

            // Simulate @pacer-core's consumer holding `previous` and recycling via `recycler`.
            let consumer_stop = Arc::clone(&stop_flag);
            let consumer_mailbox = Arc::clone(&mailbox);
            let consumer_recycler = Arc::clone(&recycler);
            let consumer_count = Arc::clone(&consumed);
            let consumer = thread::spawn(move || {
                let mut previous: Option<CapturedFrame> = None;
                while !consumer_stop.load(Ordering::Relaxed) {
                    if let Some(frame) = consumer_mailbox.lock().unwrap().take() {
                        consumer_count.fetch_add(1, Ordering::Relaxed);
                        if let Some(mut prev) = previous.replace(frame) {
                            if let Some(tex) = prev.texture.take() {
                                consumer_recycler.recycle_texture(tex);
                            }
                        }
                    }
                    thread::sleep(Duration::from_millis(3));
                }
                if let Some(mut prev) = previous.take() {
                    if let Some(tex) = prev.texture.take() {
                        consumer_recycler.recycle_texture(tex);
                    }
                }
            });

            // Simulate @pacer-core's capture-producer thread owning `&mut Box<dyn CaptureBackend>`.
            let prod_stop = Arc::clone(&stop_flag);
            let prod_mailbox = Arc::clone(&mailbox);
            let prod_recycler = Arc::clone(&recycler);
            let producer = thread::spawn(move || {
                while !prod_stop.load(Ordering::Relaxed) {
                    if let Ok(Some(frame)) = backend.acquire_frame(8) {
                        if let Some(mut superseded) = prod_mailbox.lock().unwrap().replace(frame) {
                            if let Some(tex) = superseded.texture.take() {
                                prod_recycler.recycle_texture(tex);
                            }
                        }
                    }
                }
                backend
            });

            thread::sleep(Duration::from_millis(35));
            stop_flag.store(true, Ordering::Relaxed);
            recycler.wake_waiter();

            feeder.join().expect("feeder panicked");
            consumer.join().expect("consumer panicked");
            backend = producer.join().expect("producer panicked");
            if let Some(mut leftover) = mailbox.lock().unwrap().take() {
                if let Some(tex) = leftover.texture.take() {
                    recycler.recycle_texture(tex);
                }
            }
            backend.stop();

            assert!(consumed.load(Ordering::Relaxed) > 0);
            assert_eq!(queue.ready_len(), 0);
            assert_eq!(queue.free_pool_len(), 0);
        }

        // Verify wake_waiter() unblocks a long acquire_frame wait immediately.
        backend.start().unwrap();
        let recycler = backend.frame_recycler().unwrap();
        let wake_handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            recycler.wake_waiter();
        });
        let t0 = std::time::Instant::now();
        assert!(backend.acquire_frame(500).unwrap().is_none());
        assert!(
            t0.elapsed() < Duration::from_millis(150),
            "wake_waiter should unblock acquire_frame well before 500ms timeout"
        );
        wake_handle.join().unwrap();
        backend.stop();
    }

    #[test]
    fn wgc_acquire_frame_checks_async_error_first_and_drains_stale_frames() {
        use windows::Win32::Foundation::HMODULE;
        use windows::Win32::Graphics::Direct3D::{
            D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_11_0,
        };
        use windows::Win32::Graphics::Direct3D11::{
            D3D11CreateDevice, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
        };

        let mut device = None;
        let levels = [D3D_FEATURE_LEVEL_11_0];
        let mut created = unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&levels),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                None,
            )
        };
        if created.is_err() {
            created = unsafe {
                D3D11CreateDevice(
                    None,
                    D3D_DRIVER_TYPE_WARP,
                    HMODULE::default(),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    Some(&levels),
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    None,
                )
            };
        }
        let Ok(()) = created else {
            return;
        };
        let device = device.unwrap();
        let region = CaptureRegion {
            left: 0,
            top: 0,
            width: 64,
            height: 64,
        };
        let mut wgc = WgcCapture::new(device.clone(), HWND::default(), region, true, 3).unwrap();

        for ts in [100_i64, 200, 300] {
            let tex = alloc_bgra_copy_texture(&device, &wgc.queue, 64, 64).unwrap();
            wgc.queue.push_frame(CapturedFrame {
                texture: Some(tex),
                width: 64,
                height: 64,
                timestamp_qpc: ts,
                is_duplicate: false,
            });
        }
        let latest = wgc
            .acquire_frame(0)
            .unwrap()
            .expect("expected drained frame");
        assert_eq!(latest.timestamp_qpc, 300);
        assert_eq!(wgc.ready_len(), 0);
        assert!(wgc.queue.free_pool_len() >= 2);

        wgc.queue
            .push_frame(CapturedFrame::metadata_only(64, 64, 400, false));
        *wgc.async_error.lock().unwrap() = Some(CaptureError::WgcItemClosed);
        let t0 = std::time::Instant::now();
        let err = wgc.acquire_frame(200).unwrap_err();
        assert_eq!(err, CaptureError::WgcItemClosed);
        assert!(t0.elapsed() < Duration::from_millis(50));
    }
}
