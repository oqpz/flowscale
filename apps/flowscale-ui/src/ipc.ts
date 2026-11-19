import {
  type AppState,
  createDefaultEngineConfig,
  createDefaultProfile,
  DEFAULT_HOTKEY,
  type HostEvent,
  type ScalingProfile,
  type UiCommand,
} from './types';
import {isHostEventShape} from './ipcGuards.js';

interface WebView2MessageEvent {
  data: unknown;
}

interface WebView2Bridge {
  postMessage(message: string): void;
  addEventListener(type: 'message', listener: (event: WebView2MessageEvent) => void): void;
  removeEventListener(type: 'message', listener: (event: WebView2MessageEvent) => void): void;
}

declare global {
  interface Window {
    chrome?: {
      webview?: WebView2Bridge;
    };
  }
}

export function isWebView2Available(): boolean {
  return typeof window !== 'undefined' && Boolean(window.chrome?.webview);
}

export function createMockInitialState(): AppState {
  const dwmRefreshRate = 144;
  const defaultProfile = createDefaultProfile(dwmRefreshRate);
  const eldenProfile = createDefaultProfile(dwmRefreshRate, {
    id: 'elden-ring',
    title: 'Elden Ring',
    is_default: false,
    process_pattern: 'eldenring.exe',
    auto_scale: true,
    auto_scale_delay_sec: 4,
    scaling_type: 'LS1',
    scaling_subtype: 'Performance',
    sharpness: 3,
    frame_gen_engine: 'LSFG3',
    lsfg3_mode: 'Fixed',
    lsfg_multiplier: 2,
    flow_scale: 85,
    capture_api: 'DXGI',
    draw_fps: true,
  });
  const animePlayerProfile = createDefaultProfile(dwmRefreshRate, {
    id: 'mpv-player',
    title: 'mpv media player',
    is_default: false,
    process_pattern: 'mpv.exe',
    auto_scale: false,
    auto_scale_delay_sec: 5,
    scaling_type: 'Anime4K',
    scaling_subtype: 'VL',
    vrs: true,
    sharpness: 0,
    frame_gen_engine: 'LSFG3',
    lsfg3_mode: 'Adaptive',
    lsfg_target_fps: 144,
    flow_scale: 100,
    capture_api: 'WGC',
  });

  return {
    settings: {
      hotkey: {...DEFAULT_HOTKEY},
      theme: 'system',
      minimize_to_tray: true,
      close_to_tray: false,
      start_minimized: false,
      start_as_admin: false,
      start_at_startup: false,
      windows_gpu_preference: 2,
      selected_profile_id: 'default',
    },
    profiles: [defaultProfile, eldenProfile, animePlayerProfile],
    telemetry: {
      status: 'idle',
    },
    hardware: {
      adapters: [
        {
          id: 'gpu-0',
          name: 'NVIDIA GeForce RTX 4070 SUPER',
          dedicated_vram_mb: 12288,
          is_default: true,
        },
        {
          id: 'gpu-1',
          name: 'AMD Radeon Graphics (Integrated)',
          dedicated_vram_mb: 512,
          is_default: false,
        },
      ],
      displays: [
        {
          id: 'disp-0',
          name: '\\\\.\\DISPLAY1 (2560x1440 @ 144 Hz)',
          width: 2560,
          height: 1440,
          refresh_rate: 144,
          hdr_capable: true,
          is_primary: true,
        },
        {
          id: 'disp-1',
          name: '\\\\.\\DISPLAY2 (1920x1080 @ 60 Hz)',
          width: 1920,
          height: 1080,
          refresh_rate: 60,
          hdr_capable: false,
          is_primary: false,
        },
      ],
      dwm_refresh_rate: dwmRefreshRate,
      windows_build: 'Windows 11 24H2 (Build 26100)',
      is_admin: false,
    },
    engine_config: createDefaultEngineConfig(),
    is_host_connected: isWebView2Available(),
  };
}

function parseHostEvent(raw: unknown): HostEvent | null {
  try {
    const parsed = typeof raw === 'string' ? (JSON.parse(raw) as unknown) : raw;
    if (isHostEventShape(parsed)) {
      return parsed as HostEvent;
    }
    return null;
  } catch {
    return null;
  }
}

export interface FlowScaleIpcClient {
  sendCommand: (command: UiCommand) => void;
  subscribe: (listener: (event: HostEvent) => void) => () => void;
  dispose: () => void;
}

export function createIpcClient(): FlowScaleIpcClient {
  const listeners = new Set<(event: HostEvent) => void>();
  let mockState = createMockInitialState();
  let telemetryInterval: number | null = null;

  const emit = (event: HostEvent) => {
    for (const listener of listeners) {
      listener(event);
    }
  };

  const stopMockTelemetry = () => {
    if (telemetryInterval !== null) {
      window.clearInterval(telemetryInterval);
      telemetryInterval = null;
    }
  };

  const handleWebViewMessage = (event: WebView2MessageEvent) => {
    const parsed = parseHostEvent(event.data);
    if (parsed) {
      emit(parsed);
    }
  };

  const webview = typeof window !== 'undefined' ? window.chrome?.webview : undefined;
  if (webview) {
    webview.addEventListener('message', handleWebViewMessage);
  }

  const handleMockCommand = (command: UiCommand) => {
    switch (command.type) {
      case 'configs':
        emit({type: 'configs_result', payload: {entries: []}});
        break;
      case 'get_initial_state': {
        emit({
          type: 'state_sync',
          payload: {
            settings: mockState.settings,
            profiles: mockState.profiles,
            telemetry: mockState.telemetry,
            hardware: mockState.hardware,
            engine_config: mockState.engine_config,
            is_admin: mockState.hardware.is_admin,
            windows_build: mockState.hardware.windows_build,
          },
        });
        break;
      }
      case 'save_settings': {
        mockState = {...mockState, settings: command.payload};
        break;
      }
      case 'select_profile': {
        mockState = {
          ...mockState,
          settings: {...mockState.settings, selected_profile_id: command.payload.id},
        };
        break;
      }
      case 'upsert_profile': {
        const incoming = command.payload;
        const {client_revision, ...profile} = incoming;
        const exists = mockState.profiles.some((p) => p.id === profile.id);
        const profiles = exists
          ? mockState.profiles.map((p) => (p.id === profile.id ? profile : p))
          : [...mockState.profiles, profile];
        mockState = {...mockState, profiles};
        if (client_revision) {
          emit({
            type: 'profile_upserted',
            payload: {id: profile.id, revision: client_revision, ok: true, profile},
          });
        }
        break;
      }
      case 'delete_profile': {
        if (command.payload.id === 'default') return;
        const profiles = mockState.profiles.filter((p) => p.id !== command.payload.id);
        const selected =
          mockState.settings.selected_profile_id === command.payload.id
            ? 'default'
            : mockState.settings.selected_profile_id;
        mockState = {
          ...mockState,
          profiles,
          settings: {...mockState.settings, selected_profile_id: selected},
        };
        break;
      }
      case 'duplicate_profile': {
        const source = mockState.profiles.find((p) => p.id === command.payload.id);
        if (!source) return;
        const copyId = `${source.id}-copy-${Date.now().toString(36)}`;
        const duplicate: ScalingProfile = {
          ...source,
          id: copyId,
          title: `${source.title} copy`,
          is_default: false,
          auto_scale: false,
        };
        mockState = {
          ...mockState,
          profiles: [...mockState.profiles, duplicate],
          settings: {...mockState.settings, selected_profile_id: copyId},
        };
        emit({
          type: 'state_sync',
          payload: {
            settings: mockState.settings,
            profiles: mockState.profiles,
            telemetry: mockState.telemetry,
            hardware: mockState.hardware,
            engine_config: mockState.engine_config,
          },
        });
        emit({type: 'toast', payload: {kind: 'success', message: `Duplicated ${source.title}`}});
        break;
      }
      case 'activate_scaling': {
        stopMockTelemetry();
        const active =
          mockState.profiles.find((p) => p.id === mockState.settings.selected_profile_id) ??
          mockState.profiles[0];
        const targetName =
          active.title === 'Default'
            ? 'Borderless Game Window'
            : `${active.title} (${active.process_pattern || 'foreground window'})`;
        const multiplier =
          active.frame_gen_engine === 'LSFG3' && active.lsfg3_mode === 'Fixed'
            ? active.lsfg_multiplier
            : active.frame_gen_engine === 'LSFG2'
              ? Number(active.lsfg2_mode.slice(1))
              : 1;
        const baseFps = 60;
        const scaleFactor = active.scaling_mode === 'Custom' ? active.scale_factor : 1.333;
        let submitted = 0;
        let dropped = 0;
        const tick = () => {
          const jitter = Math.random() > 0.5 ? 0 : -1;
          const captured = baseFps + jitter;
          const dip = Math.random() < 0.08;
          const target =
            active.frame_gen_engine === 'LSFG3' && active.lsfg3_mode === 'Adaptive'
              ? active.lsfg_target_fps
              : captured * multiplier;
          const presented = dip ? Math.round(target * 0.82) : target;
          submitted += presented;
          if (dip) dropped += 3;
          const gen = 1.8 + Math.random() * 0.6;
          mockState = {
            ...mockState,
            telemetry: {
              status: 'active',
              target_title: targetName,
              input_width: 1920,
              input_height: 1080,
              output_width: 2560,
              output_height: 1440,
              scale_factor: scaleFactor,
              captured_fps: captured,
              presented_fps: presented,
              engine: {
                active_profile_id: active.id,
                presented_1pct_low_fps: dip ? presented * 0.7 : presented * 0.94,
                cap_ms: 1000 / captured,
                gpu_ms: gen,
                gpu_ms_min: gen - 0.4,
                gpu_ms_max: dip ? gen + 6 : gen + 0.9,
                gpu_stage_ms: [gen * 0.4, gen * 0.45, gen * 0.15],
                present_ms: 0.3,
                queue_depth: 1,
                requested_capture_api: 'Dxgi',
                actual_capture_api: 'Dxgi',
                capture_fallback_reason: '',
                dxgi_accumulated_frames: 0,
                expired_output_slots: 0,
                latency_wait_timeouts: 0,
                latency_wait_ms: 0.4,
                submitted_outputs: submitted,
                dropped_outputs: dropped,
                last_output_drop_reason: dropped ? 'output slot expired before present' : '',
                occluded_outputs: 0,
              },
            },
          };
          emit({type: 'telemetry_tick', payload: mockState.telemetry});
        };
        tick();
        telemetryInterval = window.setInterval(tick, 1000);
        break;
      }
      case 'stop_scaling': {
        stopMockTelemetry();
        mockState = {...mockState, telemetry: {status: 'idle'}};
        emit({type: 'telemetry_tick', payload: mockState.telemetry});
        break;
      }
      case 'cancel_countdown': {
        stopMockTelemetry();
        mockState = {...mockState, telemetry: {status: 'idle'}};
        emit({type: 'telemetry_tick', payload: mockState.telemetry});
        break;
      }
      case 'set_hotkey': {
        mockState = {
          ...mockState,
          settings: {...mockState.settings, hotkey: command.payload},
        };
        emit({
          type: 'toast',
          payload: {kind: 'success', message: `Hotkey updated to ${command.payload.display}`},
        });
        break;
      }
      case 'pick_executable': {
        emit({
          type: 'picked_executable',
          payload: {path: 'C:\\Program Files (x86)\\Steam\\steamapps\\common\\SampleGame\\game.exe'},
        });
        break;
      }
      case 'import_lossless_xml': {
        emit({
          type: 'toast',
          payload: {
            kind: 'info',
            message: 'Browser preview mode: native host imports Settings.xml',
          },
        });
        break;
      }
      case 'save_engine_config': {
        mockState = {...mockState, engine_config: command.payload};
        emit({type: 'toast', payload: {kind: 'success', message: 'Engine config.ini saved'}});
        break;
      }
      case 'reset_engine_config': {
        mockState = {...mockState, engine_config: createDefaultEngineConfig()};
        emit({
          type: 'state_sync',
          payload: {
            settings: mockState.settings,
            profiles: mockState.profiles,
            telemetry: mockState.telemetry,
            hardware: mockState.hardware,
            engine_config: mockState.engine_config,
          },
        });
        emit({type: 'toast', payload: {kind: 'info', message: 'Engine config.ini reset to defaults'}});
        break;
      }
      case 'set_windows_gpu_preference': {
        mockState = {
          ...mockState,
          settings: {...mockState.settings, windows_gpu_preference: command.payload.preference},
        };
        emit({
          type: 'toast',
          payload: {kind: 'success', message: 'Windows GPU preference applied'},
        });
        break;
      }
      case 'relaunch_as_admin': {
        mockState = {
          ...mockState,
          hardware: {...mockState.hardware, is_admin: true},
        };
        emit({
          type: 'state_sync',
          payload: {
            settings: mockState.settings,
            profiles: mockState.profiles,
            telemetry: mockState.telemetry,
            hardware: mockState.hardware,
            engine_config: mockState.engine_config,
            is_admin: true,
          },
        });
        emit({type: 'toast', payload: {kind: 'info', message: 'Administrator mode simulated in preview'}});
        break;
      }
      case 'open_external_url': {
        window.open(command.payload.url, '_blank', 'noopener,noreferrer');
        break;
      }
    }
  };

  return {
    sendCommand: (command) => {
      if (webview) {
        webview.postMessage(JSON.stringify(command));
        return;
      }
      handleMockCommand(command);
    },
    subscribe: (listener) => {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    dispose: () => {
      stopMockTelemetry();
      if (webview) {
        webview.removeEventListener('message', handleWebViewMessage);
      }
      listeners.clear();
    },
  };
}
