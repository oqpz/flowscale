# FlowScale native C ABI and config schema

## 1. Exported C ABI (`__cdecl`, `flowscale_core.dll`)

All native exports use the C (`__cdecl` on x64 Windows) calling convention and accept primitive scalars, `BSTR` strings, or a function pointer callback.

| Export | Signature | Behavior |
|---|---|---|
| `Init` | `bool Init(StatusCallback cb)` | Stores `cb`, marks `EngineRuntime` initialized, and returns `true`. |
| `UnInit` | `void UnInit()` | Stops any active session, clears `cb`, and resets initialized state. |
| `Activate` | `bool Activate(HWND hwnd)` | Toggles scaling on/off for `hwnd` (or current foreground window if `NULL`). Returns new active state. |
| `ApplySettings` | `void ApplySettings(... 33 scalar args ...)` | Updates active `ScalingProfile` fields before or during a session. |
| `GetForegroundWindowEx` | `HWND GetForegroundWindowEx()` | Returns current foreground `HWND`. |
| `GetAdapterNames` | `BSTR GetAdapterNames()` | Returns `;`-delimited DXGI adapter descriptions (`SysAllocStringLen`). |
| `GetDisplayNames` | `BSTR GetDisplayNames()` | Returns `;`-delimited DXGI output device/display strings (`SysAllocStringLen`). |
| `GetDwmRefreshRate` | `float GetDwmRefreshRate()` | Queries `DwmGetCompositionTimingInfo` and returns `uiNumerator / uiDenominator`. |
| `IsWindowsBuildAtLeast` | `bool IsWindowsBuildAtLeast(uint32_t build)` | Checks OS build number against `build` (e.g., `19041` for Win10 2004, `22000` for Win11). |
| `SetWindowsSettings` | `bool SetWindowsSettings(int32_t gpuPref)` | Writes `GpuPreference={gpuPref};` under `HKCU\Software\Microsoft\DirectX\UserGpuPreferences`. |
| `SetDriverSettings` | `bool SetDriverSettings()` | Retained for C ABI parity with hosts expecting the NVAPI helper export. |

### `StatusCallback` signature

```c
typedef void (__cdecl *StatusCallback)(
    int32_t status,      // 0 = Disabled, 1 = TempDisabled, 2 = Active
    HWND    hwnd,
    int32_t inputWidth,
    int32_t inputHeight,
    int32_t outputWidth,
    int32_t outputHeight,
    bool    resized,
    float   scaleFactor,
    int32_t errorCode    // 0 = NoCode, 1 = ResizeFailed, 2 = LargeOffset, 3 = CaptureFailed
);
```

### `ApplySettings` parameter order (33 scalar arguments)

1. `scalingMode`: `int32` (`0: Auto`, `1: Custom`)
2. `scalingFitMode`: `int32` (`0: AspectRatio`, `1: Fullscreen`)
3. `scalingType`: `int32` (`0: Off`, `1: LS1`, `2: FSR`, `3: NIS`, `4: SGSR`, `5: BicubicCAS`, `6: Anime4K`, `7: XBR`, `8: SharpBilinear`, `9: Integer`, `10: Nearest`)
4. `scalingSubtype`: `int32` (`Ls1Subtype`, `FsrSubtype`, `Anime4kSubtype`)
5. `scaleFactor`: `float` (`1.0..10.0`)
6. `resizeBeforeScale`: `bool`
7. `sharpness`: `int32` (`0..4` for LS1, `0..10` for others)
8. `vrs`: `bool`
9. `frameGenerationType`: `int32` (`0: Off`, `1: LSFG3`, `2: LSFG2`, `3: LSFG1`)
10. `frameGenerationSize`: `int32` (`0: Balanced`, `1: Performance`)
11. `frameGenerationMode`: `int32` (`0: Fixed`, `1: Adaptive` for LSFG3; `0: X2`, `1: X3`, `2: X4` for LSFG2)
12. `frameGenerationMultiplier`: `float` (`1.0..20.0`)
13. `frameGenerationTarget`: `float` (`10.0..1000.0`)
14. `frameGenerationFlowScale`: `int32` (`25..100` percentage)
15. `clipCursor`: `bool`
16. `cursorSensitivity`: `bool`
17. `hideCursor`: `bool`
18. `scaleCursor`: `bool`
19. `syncMode`: `int32` (`0: Off`, `1: Default`, `2..5: VSync1..4`)
20. `maxFrameLatency`: `int32` (`1..15`)
21. `gsyncSupport`: `bool`
22. `hdrSupport`: `bool` (`R16G16B16A16_FLOAT` vs `B8G8R8A8_UNORM`)
23. `captureApi`: `int32` (`0: DXGI`, `1: WGC`, `2: GDI`)
24. `queueTarget`: `int32` (`0..2`)
25. `drawFps`: `bool`
26. `gpuId`: `int32` (`-1` for Auto, `0..` for hardware DXGI adapter index)
27. `displayId`: `int32` (`-1` for Auto, `0..` for DXGI output index)
28. `captureOffsetLeft`: `int32`
29. `captureOffsetTop`: `int32`
30. `captureOffsetRight`: `int32`
31. `captureOffsetBottom`: `int32`
32. `multiDisplayMode`: `bool`
33. `setupPhase`: `int32` (reserved host parameter)

## 2. `config.ini` and `settings.json`

`config.ini` is loaded from `./config.ini` if present or `%LocalAppData%\FlowScale\config.ini`:

| Section | Key | Default | Role |
|---|---|---|---|
| `[rendering]` | `flush` | `1` | Calls `ID3D11DeviceContext::Flush()` at the end of each render-loop iteration after `draw_and_present`. |
| `[capture]` | `frametime_buffer_size` | `15` | Rolling buffer size for smoothed input frame timing. |
| `[capture]` | `frametime_buffer_reset_multiplier` | `6` | Resets buffer after 3+ samples when `dt > avg * mult` or `dt * mult < avg`. |
| `[capture]` | `queue_draining_momentum` | `0.01` | EMA weight on `(queue_depth - queue_target)`, applying `(1.0 - smoothed_error * 0.05).clamp(0.85, 1.15)` to `avg_dt`. |
| `[capture]` | `dedup` | `0` | Duplicate-frame filtering in DXGI Desktop Duplication (`LastPresentTime == 0 && AccumulatedFrames == 0`). |
| `[lsfg]` | `ui_detection_rate` | `1` | Static-UI mask check frequency in frames. |
| `[lsfg]` | `ui_detection_threshold` | `0.5` | Confidence cutoff for static-UI masking. |
| `[lsfg]` | `real_timestamp_tolerance` | `0.05` | Marks step as a real frame (`alpha = 1.0`) when `(1.0 - alpha) <= tolerance`. |
| `[lsfg]` | `base_framerate_threshold` | `10` | Minimum captured FPS; returns a single real frame below threshold. |
| `[fps_counter]` | `scale` | `1.0` | Font size scale factor in `FpsOverlay`. |
| `[fps_counter]` | `color` | `0xD3D3D3` | RGB text color (`24-bit` hex). |
| `[fps_counter]` | `font` | `Consolas` | DirectWrite font family name. |
| `[fps_counter]` | `offset_x` / `offset_y` | `0` / `0` | Screen offset in pixels from top-left of overlay. |
| `[fps_counter]` | `show_captured` | `1` | Formats `"<captured> / <presented>"` when enabled, or `"<presented>"` only. |

UI application settings (`theme`, `hotkey`, tray behavior, startup toggle, window geometry, GPU preference, and per-game `GameProfile` list) are stored in `%LocalAppData%\FlowScale\settings.json`.
