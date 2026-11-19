# FlowScale architecture

## 1. System overview

FlowScale is a clean-room Windows desktop scaling and frame-generation application written in Rust. It captures a target game or application window using Windows.Graphics.Capture (`WGC`), DXGI Desktop Duplication, or GDI BitBlt, runs spatial upscaling and optical-flow interpolation compute shaders (`cs_5_0`) on D3D11, and presents output on a click-through, topmost Win32 overlay swapchain (`DXGI_SWAP_EFFECT_FLIP_DISCARD`).

The desktop UI is written in React and TypeScript with `@astryxdesign/core` (`apps/flowscale-ui`), compiled into `dist/`, and embedded inside `flowscale.exe` (`crates/flowscale-app`), which serves it locally to Microsoft Edge WebView2 over `https://app.flowscale.local/`.

## 2. Workspace crate map

| Crate | Responsibility |
|---|---|
| `flowscale-app` | Standalone Windows GUI binary (`flowscale.exe`): single-instance mutex, WebView2 host, `Shell_NotifyIconW` tray menu, `GetOpenFileNameW` dialogs, auto-scale foreground countdown, and JSON IPC bridge. |
| `flowscale-abi` | Clean-room C ABI (`__cdecl`) types, enums (`ScalingMode`, `ScalingType`, `FrameGenerationType`, `CaptureApi`, `SyncMode`, `StatusCode`, `ErrorCode`), `ScalingProfile`, and `StatusCallback` signature. |
| `flowscale-config` | `config.ini` and `%LocalAppData%\FlowScale\settings.json` persistence, `Settings.xml` profile importer, glob matcher, Windows `Run` startup helper, `FrametimeTracker`, `QueueDrainingController`, `InterpolationScheduler`, and `HybridPacer`. |
| `flowscale-capture` | `CaptureBackend` trait, `FrameRingQueue`, `DxgiDuplicationCapture` (with dedup), `WgcCapture` (`Direct3D11CaptureFramePool::CreateFreeThreaded`), and `GdiCapture` (`PrintWindow` / `BitBlt` staged to D3D11). |
| `flowscale-gpu` | `D3d11Context`, `OverlaySwapChain` (`FLIP_DISCARD`, `ALLOW_TEARING`, HDR10 `R16G16B16A16_FLOAT` or SDR `B8G8R8A8_UNORM`), D2D1 software cursor + DirectWrite FPS counter, 2 optical-flow compute shaders, and 9 spatial upscaler compute kernels. |
| `flowscale-overlay` | Topmost click-through overlay + optional backdrop windows, `WindowWatcher` (`SetWinEventHook`), `GlobalHotkeyHook` (`WH_KEYBOARD_LL`), `ForegroundProcessWatcher`, `CursorSpeedGuard`, DXGI hardware probing, and cursor RGBA capture. |
| `flowscale-core` | Engine runtime (`EngineRuntime`) and `cdylib` (`flowscale_core.dll`) C ABI exports (`Init`, `UnInit`, `Activate`, `ApplySettings`, `GetAdapterNames`, `GetDisplayNames`, `GetDwmRefreshRate`, `GetForegroundWindowEx`, `IsWindowsBuildAtLeast`, `SetWindowsSettings`, `SetDriverSettings`). |
| `flowscale-cli` | Standalone CLI (`flowscale-cli.exe`) for adapter/display inspection, DWM timing, GPU smoke checks, pacing benchmarks, and CLI scaling tests. |

## 3. Session lifecycle

1. Application startup (`flowscale-app`) acquires a per-session Windows mutex (`Local\FlowScale_SingleInstance_v1`), loads `settings.json` and `config.ini`, initializes `EngineRuntime`, registers `GlobalHotkeyHook`, creates the tray icon, and hosts the embedded WebView2 UI.
2. Activation (`Activate`, hotkey, or foreground auto-scale) resolves the target `HWND` and matches `GameProfile` rules by process path or window title, refusing self-capture if the foreground window belongs to FlowScale.
3. Engine setup (`EngineRuntime::activate`) resizes bordered custom-mode windows when configured, creates overlay and backdrop windows, initializes the D3D11 device and `ComputePipeline` sized from `config.lsfg.flow_scale` or `profile.frame_generation_flow_scale` (`25..=100`%), and initializes the capture backend (`DXGI`, `WGC`, or `GDI`).
4. The runtime loop and `WindowWatcher` monitor focus loss (`TempDisabled` temporary overlay hide and cursor-clip release), resize relocation, minimize/restore, and target destruction (`stop_session`), pushing telemetry ticks to the UI and tray tooltip.
5. Teardown signals worker threads, joins the render and capture threads, and restores `ClipCursor`, `CursorSpeedGuard` mouse speed, resized target window bounds, and `OverlayWindow`.

## 4. Threading model

- Main UI thread pumps Win32 messages, WebView2 events, tray interactions, IPC commands, and `WM_TIMER` foreground auto-scale checks.
- Hotkey thread runs a dedicated `WH_KEYBOARD_LL` message loop inside `GlobalHotkeyHook` with key-hold debouncing and live rebinding.
- Render thread (`THREAD_PRIORITY_HIGHEST`) pops frames from `FrameRingQueue`, runs optical-flow estimation and interpolation, executes the active spatial upscaler (`cs_5_0`) into `DstRect`, composites software cursor and DirectWrite FPS overlays, runs `HybridPacer` waits across interpolated steps, calls `Present`, and flushes D3D11 when `config.rendering.flush` is enabled.
- Capture producer delivers `WgcCapture` frames through `Direct3D11CaptureFramePool::CreateFreeThreaded`, or runs dedicated capture threads in DXGI Desktop Duplication and GDI modes.
