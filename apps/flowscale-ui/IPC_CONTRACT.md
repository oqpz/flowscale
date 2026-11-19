# FlowScale WebView2 IPC Contract

FlowScale uses Windows WebView2 `window.chrome.webview` JSON messaging for bidirectional communication between the React UI (`apps/flowscale-ui`) and the native Windows Rust host.

## Transport

| Direction | Mechanism |
|---|---|
| UI to Host | `window.chrome.webview.postMessage(JSON.stringify({ type, payload }))` |
| Host to UI | WebView2 `message` event carrying `{ type, payload }` via `PostWebMessageAsJson` or `PostWebMessageAsString` |
| Browser preview | When `window.chrome?.webview` is absent, `src/ipc.ts` handles commands against in-memory mock state |

## UI to Host commands

Every command envelope has the form:

```json
{
  "type": "<command_name>",
  "payload": {}
}
```

| Command `type` | Payload schema | Behavior |
| --- | --- | --- |
| `get_initial_state` | `{}` | Requests a full `state_sync` event on startup. |
| `save_settings` | `GlobalSettings` | Persists global settings (tray behavior, startup, theme, hotkey, Windows GPU preference, selected profile ID). |
| `select_profile` | `{ "id": string }` | Sets the active profile ID in host state. |
| `upsert_profile` | `ScalingProfile` plus optional `client_revision` | Creates or updates a profile (`Default` uses `"id": "default"`). When supplied, the host emits `profile_upserted` after the matching settings save completes. |
| `delete_profile` | `{ "id": string }` | Deletes a custom profile. Ignored if `"id" === "default"`. |
| `duplicate_profile` | `{ "id": string }` | Clones an existing profile as a new non-default profile and emits updated `state_sync`. |
| `activate_scaling` | `{ "hwnd"?: number }` | Starts capture, scaling, and optional frame generation on `hwnd` or the current foreground target window. |
| `stop_scaling` | `{}` | Stops the active scaling session and returns telemetry to `idle`. |
| `cancel_countdown` | `{}` | Cancels the foreground auto-scale countdown on the host (suppressing that window until focus changes) and refreshes telemetry. |
| `set_hotkey` | `HotkeyBinding` | Registers the global Windows hotkey `{ ctrl, alt, shift, key, display }`. |
| `pick_executable` | `{}` | Opens a native Windows file picker (`*.exe`) and responds with `picked_executable`. |
| `import_lossless_xml` | `{}` | Imports profiles and global configuration from a scaler `Settings.xml` file and emits updated `state_sync`. |
| `save_engine_config` | `EngineConfig` | Writes `[rendering]`, `[capture]`, `[lsfg]`, and `[fps_counter]` settings to `config.ini`. |
| `reset_engine_config` | `{}` | Restores default `config.ini` values and emits updated `state_sync`. |
| `set_windows_gpu_preference` | `{ "preference": 0 \| 1 \| 2 }` | Updates Windows DirectX GPU preference (`0` Auto, `1` Power saving, `2` High performance). |
| `relaunch_as_admin` | `{}` | Requests UAC elevation restart on Windows. |
| `open_external_url` | `{ "url": string }` | Opens an external URL in the default Windows browser. |

## Host to UI events

Every event envelope pushed by the host has the form:

```json
{
  "type": "<event_name>",
  "payload": {}
}
```

| Event `type` | Payload schema | Description |
| --- | --- | --- |
| `state_sync` | `{ settings, profiles, telemetry, hardware, engine_config, is_admin?, windows_build? }` | Full application snapshot replacing local UI state. |
| `telemetry_tick` | `TelemetryState` | Live status (`idle`, `countdown`, `active`, `paused`, `error`), target/foreground title, input/output resolution, `scale_factor`, `captured_fps`, `presented_fps`, and optional `error_message`. |
| `countdown_tick` | `{ "seconds": number }` | Optional host-driven countdown synchronization; the UI also runs a local cancelable 5-second countdown before dispatching `activate_scaling`. |
| `picked_executable` | `{ "path": string }` | Returns the selected `.exe` path from `pick_executable`. |
| `profile_upserted` | `{ id, revision, ok, profile?, message? }` | Acknowledges the matching profile revision after its settings save completes; failures include a message and cannot acknowledge newer revisions. |
| `toast` | `{ "kind": "info" \| "success" \| "warning" \| "error", "message": string }` | Displays an in-app toast notification. |

## Data shapes

### `ScalingProfile`

```ts
{
  client_revision?: string; // Sent only with upsert_profile; omitted in host snapshots.
  id: string;
  title: string;
  is_default: boolean;
  process_pattern: string;
  auto_scale: boolean;
  auto_scale_delay_sec: number;
  scaling_mode: "Auto" | "Custom";
  fit_mode: "AspectRatio" | "Fullscreen";
  scale_factor: number; // 1.0..10.0; integer when Integer, step 0.2 when XBR, else 3 decimals
  resize_before_scaling: boolean;
  windowed_mode: boolean;
  scaling_type:
    | "Off"
    | "LS1"
    | "FSR"
    | "NIS"
    | "SGSR"
    | "BicubicCAS"
    | "Anime4K"
    | "XBR"
    | "SharpBilinear"
    | "Integer"
    | "Nearest";
  scaling_subtype: string; // FSR: Original|Optimized; LS1: Balanced|Performance; Anime4K: S|M|L|VL|UL
  sharpness: number; // 0..4 for LS1, 0..10 otherwise
  vrs: boolean; // Active for Anime4K
  frame_gen_engine: "Off" | "LSFG3" | "LSFG2" | "LSFG1";
  cpu_vulkan?: boolean; // Default false. Experimental software Vulkan processing with CPU/RAM.
  lsfg3_mode: "Fixed" | "Adaptive";
  lsfg2_mode: "X2" | "X3" | "X4";
  lsfg_multiplier: number; // 1..20 when LSFG3 + Fixed
  lsfg_target_fps: number; // 10..1000 when LSFG3 + Adaptive
  flow_scale: number; // 25..100 step 5
  lsfg_performance: "Balanced" | "Performance";
  clip_cursor: boolean;
  adjust_cursor_speed: boolean;
  hide_cursor: boolean;
  scale_cursor: boolean;
  sync_mode: "Off" | "Default" | "Vsync1" | "Vsync2" | "Vsync3" | "Vsync4";
  max_frame_latency: number; // 1..15
  gsync_support: boolean;
  hdr_support: boolean;
  draw_fps: boolean;
  capture_api: "DXGI" | "WGC" | "GDI";
  queue_target: number; // 0..2
  crop_enabled: boolean;
  crop_left: number;
  crop_top: number;
  crop_right: number;
  crop_bottom: number;
  preferred_gpu: string; // "Auto", "gpu-N", numeric index, or exact DXGI adapter name
  output_display: string; // "Auto", "disp-N", numeric index, or exact DXGI display name
  multi_display_mode: boolean;
}
```

### `GlobalSettings`

```ts
{
  hotkey: { ctrl: boolean; alt: boolean; shift: boolean; key: string; display: string };
  theme: "system" | "light" | "dark";
  minimize_to_tray: boolean;
  close_to_tray: boolean;
  start_minimized: boolean;
  start_as_admin: boolean;
  start_at_startup: boolean;
  windows_gpu_preference: 0 | 1 | 2;
  selected_profile_id: string;
}
```

### `EngineConfig` (`config.ini`)

```ts
{
  rendering: { flush: boolean };
  capture: {
    frametime_buffer_size: number;
    frametime_buffer_reset_multiplier: number;
    queue_draining_momentum: number;
    dedup: boolean;
  };
  lsfg: {
    ui_detection_rate: number;
    ui_detection_threshold: number;
    real_timestamp_tolerance: number;
    base_framerate_threshold: number;
  };
  fps_counter: {
    scale: number;
    color: string;
    font: string;
    offset_x: number;
    offset_y: number;
    show_captured: boolean;
  };
}
```

### `TelemetryState` & `HardwareInfo`

```ts
type TelemetryState = {
  status: "idle" | "countdown" | "active" | "paused" | "error";
  countdown_seconds?: number;
  target_title?: string;
  foreground_title?: string;
  input_width?: number;
  input_height?: number;
  output_width?: number;
  output_height?: number;
  scale_factor?: number;
  captured_fps?: number;
  presented_fps?: number;
  processing_backend?: string; // CPU Vulkan identifies the selected software device.
  cpu_ms?: number; // Most recently completed CPU interpolation, including initial compilation.
  processing_status?: string; // Includes missed-deadline and unavailable-runtime explanations.
  error_message?: string;
};

type HardwareInfo = {
  adapters: Array<{ id: string; name: string; dedicated_vram_mb: number; is_default?: boolean }>;
  displays: Array<{
    id: string;
    name: string;
    width: number;
    height: number;
    refresh_rate: number;
    hdr_capable: boolean;
    is_primary?: boolean;
  }>;
  dwm_refresh_rate: number;
  windows_build: string;
  is_admin: boolean;
};
```
