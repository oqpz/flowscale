export type NavSection = 'profiles' | 'settings' | 'manual' | 'configs';

export type ThemeMode = 'system' | 'light' | 'dark';

export type ScalingMode = 'Auto' | 'Custom';
export type FitMode = 'AspectRatio' | 'Fullscreen';

export type ScalingType =
  | 'Off'
  | 'LS1'
  | 'FSR'
  | 'NIS'
  | 'SGSR'
  | 'BicubicCAS'
  | 'Anime4K'
  | 'XBR'
  | 'SharpBilinear'
  | 'Integer'
  | 'Nearest';

export type FsrSubtype = 'Original' | 'Optimized';
export type Ls1Subtype = 'Balanced' | 'Performance';
export type Anime4kSubtype = 'S' | 'M' | 'L' | 'VL' | 'UL';

export type FrameGenEngine = 'Off' | 'LSFG3' | 'LSFG2' | 'LSFG1';
export type Lsfg3Mode = 'Fixed' | 'Adaptive';
export type Lsfg2Mode = 'X2' | 'X3' | 'X4';
export type LsfgPerformance = 'Balanced' | 'Performance';

export type SyncMode =
  | 'Off'
  | 'Default'
  | 'Vsync1'
  | 'Vsync2'
  | 'Vsync3'
  | 'Vsync4';

export type CaptureApi = 'DXGI' | 'WGC' | 'GDI';

/**
 * Windows GPU power preference integer matching DXGI/Windows Graphics Settings:
 * 0 = Auto (Let Windows decide), 1 = Power saving, 2 = High performance.
 */
export type WindowsGpuPreference = 0 | 1 | 2;

export interface ScalingProfile {
  client_revision?: string;
  id: string;
  title: string;
  is_default: boolean;
  process_pattern: string;
  auto_scale: boolean;
  auto_scale_delay_sec: number;

  // Scaling
  scaling_mode: ScalingMode;
  fit_mode: FitMode;
  scale_factor: number;
  resize_before_scaling: boolean;
  windowed_mode: boolean;
  scaling_type: ScalingType;
  scaling_subtype: string;
  sharpness: number;
  vrs: boolean;

  // Frame Generation
  frame_gen_engine: FrameGenEngine;
  cpu_vulkan?: boolean;
  lsfg3_mode: Lsfg3Mode;
  lsfg2_mode: Lsfg2Mode;
  lsfg_multiplier: number;
  lsfg_target_fps: number;
  flow_scale: number;
  lsfg_performance: LsfgPerformance;

  // Cursor
  clip_cursor: boolean;
  adjust_cursor_speed: boolean;
  hide_cursor: boolean;
  scale_cursor: boolean;

  // Rendering
  sync_mode: SyncMode;
  max_frame_latency: number;
  gsync_support: boolean;
  hdr_support: boolean;
  draw_fps: boolean;

  // Capture & Crop
  capture_api: CaptureApi;
  queue_target: number;
  crop_enabled: boolean;
  crop_left: number;
  crop_top: number;
  crop_right: number;
  crop_bottom: number;

  // GPU & Display
  preferred_gpu: string;
  output_display: string;
  multi_display_mode: boolean;
}

export interface HotkeyBinding {
  ctrl: boolean;
  alt: boolean;
  shift: boolean;
  key: string;
  display: string;
}

export interface GlobalSettings {
  hotkey: HotkeyBinding;
  theme: ThemeMode;
  minimize_to_tray: boolean;
  close_to_tray: boolean;
  start_minimized: boolean;
  start_as_admin: boolean;
  start_at_startup: boolean;
  windows_gpu_preference: WindowsGpuPreference;
  selected_profile_id: string;
}

export interface EngineConfig {
  rendering: {
    flush: boolean;
  };
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

export interface GpuAdapter {
  id: string;
  name: string;
  dedicated_vram_mb: number;
  is_default?: boolean;
}

export interface DisplayOutput {
  id: string;
  name: string;
  width: number;
  height: number;
  refresh_rate: number;
  hdr_capable: boolean;
  is_primary?: boolean;
}

export interface HardwareInfo {
  adapters: GpuAdapter[];
  displays: DisplayOutput[];
  dwm_refresh_rate: number;
  windows_build: string;
  is_admin: boolean;
}

export type TelemetryStatus = 'idle' | 'countdown' | 'active' | 'paused' | 'error';

export interface TelemetryState {
  status: TelemetryStatus;
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
  processing_backend?: string;
  cpu_ms?: number;
  processing_status?: string;
  error_message?: string;
  /** Raw engine counters (sent only while scaling runs); feeds the Session log panel. */
  engine?: EngineCounters;
}

/** Subset of the engine's `EngineTelemetry` used by the Session log. Counters are cumulative per run. */
export interface EngineCounters {
  active_profile_id: string;
  presented_1pct_low_fps: number;
  cap_ms: number;
  gpu_ms: number;
  gpu_ms_min: number;
  gpu_ms_max: number;
  gpu_stage_ms: [number | null, number | null, number | null];
  present_ms: number;
  queue_depth: number;
  requested_capture_api: string;
  actual_capture_api: string;
  capture_fallback_reason: string;
  dxgi_accumulated_frames: number;
  expired_output_slots: number;
  latency_wait_timeouts: number;
  latency_wait_ms: number;
  submitted_outputs: number;
  dropped_outputs: number;
  last_output_drop_reason: string;
  occluded_outputs: number;
}

export interface AppState {
  settings: GlobalSettings;
  profiles: ScalingProfile[];
  telemetry: TelemetryState;
  hardware: HardwareInfo;
  engine_config: EngineConfig;
  is_host_connected: boolean;
}

export interface ToastPayload {
  kind: 'info' | 'success' | 'warning' | 'error';
  message: string;
}

export type ConfigSummary = {id: number; name: string};
export type ConfigAction =
  | {action: 'list'}
  | {action: 'import'; document: string}
  | {action: 'export'; profile_id: string}
  | {action: 'load'; id: number; profile_id: string}
  | {action: 'delete'; id: number};

export type HostEvent =
  | {type: 'configs_result'; payload: {entries: ConfigSummary[] | null}}
  | {
      type: 'state_sync';
      payload: {
        settings: GlobalSettings;
        profiles: ScalingProfile[];
        telemetry: TelemetryState;
        hardware: HardwareInfo;
        engine_config: EngineConfig;
        is_admin?: boolean;
        windows_build?: string;
      };
    }
  | {type: 'telemetry_tick'; payload: TelemetryState}
  | {type: 'countdown_tick'; payload: {seconds: number}}
  | {type: 'picked_executable'; payload: {path: string}}
  | {
      type: 'profile_upserted';
      payload: {
        id: string;
        revision: string;
        ok: boolean;
        profile?: ScalingProfile;
        message?: string;
      };
    }
  | {type: 'toast'; payload: ToastPayload};

export type UiCommand =
  | {type: 'configs'; payload: ConfigAction}
  | {type: 'get_initial_state'; payload: Record<string, never>}
  | {type: 'save_settings'; payload: GlobalSettings}
  | {type: 'select_profile'; payload: {id: string}}
  | {type: 'upsert_profile'; payload: ScalingProfile}
  | {type: 'delete_profile'; payload: {id: string}}
  | {type: 'duplicate_profile'; payload: {id: string}}
  | {type: 'activate_scaling'; payload: {hwnd?: number}}
  | {type: 'stop_scaling'; payload: Record<string, never>}
  | {type: 'cancel_countdown'; payload: Record<string, never>}
  | {type: 'set_hotkey'; payload: HotkeyBinding}
  | {type: 'pick_executable'; payload: Record<string, never>}
  | {type: 'import_lossless_xml'; payload: Record<string, never>}
  | {type: 'save_engine_config'; payload: EngineConfig}
  | {type: 'reset_engine_config'; payload: Record<string, never>}
  | {type: 'set_windows_gpu_preference'; payload: {preference: WindowsGpuPreference}}
  | {type: 'relaunch_as_admin'; payload: Record<string, never>}
  | {type: 'open_external_url'; payload: {url: string}};

export const SCALING_SUBTYPES: Partial<Record<ScalingType, ReadonlyArray<{value: string; label: string}>>> = {
  FSR: [
    {value: 'Original', label: 'Original'},
    {value: 'Optimized', label: 'Optimized'},
  ],
  LS1: [
    {value: 'Balanced', label: 'Balanced'},
    {value: 'Performance', label: 'Performance'},
  ],
  Anime4K: [
    {value: 'S', label: 'S (Small)'},
    {value: 'M', label: 'M (Medium)'},
    {value: 'L', label: 'L (Large)'},
    {value: 'VL', label: 'VL (Very Large)'},
    {value: 'UL', label: 'UL (Ultra Large)'},
  ],
};

export const DEFAULT_HOTKEY: HotkeyBinding = {
  ctrl: true,
  alt: true,
  shift: false,
  key: 'S',
  display: 'Ctrl + Alt + S',
};

export function formatHotkeyDisplay(binding: Pick<HotkeyBinding, 'ctrl' | 'alt' | 'shift' | 'key'>): string {
  const parts: string[] = [];
  if (binding.ctrl) parts.push('Ctrl');
  if (binding.alt) parts.push('Alt');
  if (binding.shift) parts.push('Shift');
  parts.push(binding.key.toUpperCase());
  return parts.join(' + ');
}

export function formatHotkeyKbd(binding: Pick<HotkeyBinding, 'ctrl' | 'alt' | 'shift' | 'key'>): string {
  const parts: string[] = [];
  if (binding.ctrl) parts.push('ctrl');
  if (binding.alt) parts.push('alt');
  if (binding.shift) parts.push('shift');
  parts.push(binding.key.toLowerCase());
  return parts.join('+');
}

export function clampNumber(value: number, min: number, max: number): number {
  if (!Number.isFinite(value)) return min;
  return Math.min(max, Math.max(min, value));
}

export function normalizeScaleFactor(value: number, scalingType: ScalingType): number {
  const clamped = clampNumber(value, 1, 10);
  if (scalingType === 'Integer') {
    return Math.round(clamped);
  }
  if (scalingType === 'XBR') {
    const tenths = Math.round(clamped * 10);
    const evenTenths = tenths - (tenths % 2);
    return clampNumber(Number((evenTenths / 10).toFixed(1)), 1, 10);
  }
  return Number(clamped.toFixed(3));
}

export function getSharpnessMax(scalingType: ScalingType): number {
  return scalingType === 'LS1' ? 4 : 10;
}

export function normalizeSharpness(value: number, scalingType: ScalingType): number {
  const max = getSharpnessMax(scalingType);
  return Math.round(clampNumber(value, 0, max));
}

export function defaultSubtypeForType(scalingType: ScalingType, currentSubtype: string): string {
  const options = SCALING_SUBTYPES[scalingType];
  if (!options || options.length === 0) return 'None';
  if (options.some((opt) => opt.value === currentSubtype)) {
    return currentSubtype;
  }
  return options[0].value;
}

export function applyScalingTypeRules(profile: ScalingProfile, nextType: ScalingType): ScalingProfile {
  return {
    ...profile,
    scaling_type: nextType,
    scaling_subtype: defaultSubtypeForType(nextType, profile.scaling_subtype),
    scale_factor: normalizeScaleFactor(profile.scale_factor, nextType),
    sharpness: normalizeSharpness(profile.sharpness, nextType),
  };
}

export function createDefaultEngineConfig(): EngineConfig {
  return {
    rendering: {
      flush: true,
    },
    capture: {
      frametime_buffer_size: 15,
      frametime_buffer_reset_multiplier: 6.0,
      queue_draining_momentum: 0.01,
      dedup: false,
    },
    lsfg: {
      ui_detection_rate: 1,
      ui_detection_threshold: 0.5,
      real_timestamp_tolerance: 0.05,
      base_framerate_threshold: 10,
    },
    fps_counter: {
      scale: 1.0,
      color: '#D3D3D3',
      font: 'Consolas',
      offset_x: 0,
      offset_y: 0,
      show_captured: true,
    },
  };
}

export function createDefaultProfile(dwmRefreshRate = 60, overrides?: Partial<ScalingProfile>): ScalingProfile {
  const roundedRefresh = clampNumber(Math.round(dwmRefreshRate || 60), 10, 1000);
  return {
    id: 'default',
    title: 'Default',
    is_default: true,
    process_pattern: '',
    auto_scale: false,
    auto_scale_delay_sec: 5,
    scaling_mode: 'Auto',
    fit_mode: 'AspectRatio',
    scale_factor: 1.5,
    resize_before_scaling: false,
    windowed_mode: false,
    scaling_type: 'Off',
    scaling_subtype: 'None',
    sharpness: 5,
    vrs: false,
    frame_gen_engine: 'Off',
    lsfg3_mode: 'Fixed',
    lsfg2_mode: 'X2',
    lsfg_multiplier: 2,
    lsfg_target_fps: roundedRefresh,
    flow_scale: 100,
    lsfg_performance: 'Balanced',
    clip_cursor: true,
    adjust_cursor_speed: false,
    hide_cursor: false,
    scale_cursor: false,
    sync_mode: 'Default',
    max_frame_latency: 3,
    gsync_support: true,
    hdr_support: false,
    draw_fps: true,
    capture_api: 'DXGI',
    queue_target: 1,
    crop_enabled: false,
    crop_left: 0,
    crop_top: 0,
    crop_right: 0,
    crop_bottom: 0,
    preferred_gpu: 'Auto',
    output_display: 'Auto',
    multi_display_mode: false,
    ...overrides,
  };
}
