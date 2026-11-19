# FlowScale capture, compute pipeline, and frame pacing

## 1. Capture backends and `FrameRingQueue`

FlowScale supports three Windows capture backends in `crates/flowscale-capture`:

1. `DxgiDuplicationCapture` (`CaptureApi::Dxgi`) acquires frames from the target window's monitor, copies the cropped region on the GPU, and drops pointer-only updates when `dedup = 1`. It recreates lost duplication with bounded backoff and pauses while Windows denies access to the desktop or disconnects the session.
2. `WgcCapture` (`CaptureApi::Wgc`) uses a free-threaded, two-buffer frame pool. It copies the requested client crop into an owned BGRA texture, recreates the pool on content-size changes, and reports when Windows closes the capture item.
3. `GdiCapture` (`CaptureApi::Gdi`) captures target client surfaces with `PrintWindow` or `BitBlt` into a D3D11 staging texture and copies to GPU textures for legacy windows that resist WGC or DXGI duplication.

All backends supply QPC timestamps. WGC queues its owned textures in a bounded `FrameRingQueue`, evicting the oldest unread frame if the consumer falls behind. DXGI and GDI acquire synchronously.

## 2. Frametime smoothing and queue draining

The render thread derives `dt` from consecutive capture timestamps. Timeouts hold the last frame without sampling frametime. Out-of-order timestamps are discarded, and long gaps preserve the cadence estimate while breaking the interpolation pair.

1. Queue draining (`queue_draining_momentum`) tracks an EMA `smoothed_error` over `(q_len - queue_target)` and multiplies `avg_dt` by `(1.0 - smoothed_error * 0.05).clamp(0.85, 1.15)` so overfull queues drain gradually without stutter.
2. Spike reset (`frametime_buffer_reset_multiplier`) clears history after 3+ samples when an accepted delta is far from the average.
3. Moving average (`frametime_buffer_size`) tracks recent deltas in `FrametimeTracker` to produce a steady input interval `avg_dt`.

## 3. D3D11 compute and optical-flow pipeline

1. Device setup (`D3d11Context`) creates a D3D11 (`11_1` or `11_0`) BGRA device, probes `TypedUAVLoadAdditionalFormats`, reserves VRAM (`SetVideoMemoryReservation`), and sets `SetGPUThreadPriority`.
2. Optical-flow motion estimation (`ComputePipeline`) allocates a `DXGI_FORMAT_R32G32_FLOAT` motion grid sized from the capture dimensions and FlowScale setting (`25%..100%`). The shader caches samples in group-shared memory where the tile fits, then uses coarse-to-fine search and subpixel refinement. The search radius is 8 for Balanced or 5 for Performance.
3. Interpolation (`InterpolationScheduler`) supports Fixed (`X2`/`X3`/`X4` or LSFG3 `1..20x`) and Adaptive target FPS scheduling, dispatches `cs_frame_interp` across scheduled phases (`alpha`), passes through a single real frame when captured FPS is below `base_framerate_threshold`, and treats step `alpha` as real (`alpha = 1.0`) when `(1.0 - alpha) <= real_timestamp_tolerance`.
4. Spatial upscaling (`cs_5_0`) dispatches the matching compute kernel into `DstRect` (`AspectRatio` or `Fullscreen`, centered with clear background outside `DstRect`):
   - `cs_nearest` (`Off`, `Nearest`, and `Integer` with step snapping)
   - `cs_bilinear_sharp` (`SharpBilinear`)
   - `cs_bicubic_cas` (`BicubicCAS`)
   - `cs_spatial_easu` (`Fsr`, `Original`/`Optimized` subtype)
   - `cs_nis_sharpen` (`Nis`)
   - `cs_ls1_detail` (`Ls1`, `Balanced`/`Performance` subtype, `0..4` sharpness)
   - `cs_anime4k_restore` (`Anime4k`, `S..UL` strength + optional `Vrs` toggle)
   - `cs_sgsr_mobile` (`Sgsr`)
   - `cs_xbr_pixel` (`Xbr`)
5. The scaling session keeps the native Windows pointer at its original size and position. WGC disables cursor capture, and DXGI switches to WGC or GDI if a captured frame may contain a second copy of the visible pointer. The FPS overlay draws on the swap chain before presentation. Each presentation uses a pacing deadline, including the first interpolation step and frames without generation. Device loss ends the session and reports a capture error through the status callback.

## 4. Hybrid waitable-timer and QPC spin pacing (`HybridPacer`)

`HybridPacer` combines high-resolution `SetWaitableTimerEx` sleeping up to a short spin margin before the target deadline with a `QueryPerformanceCounter` spin loop for the final microseconds, keeping pacing tight without full CPU saturation.
