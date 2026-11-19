import {useEffect, useState} from 'react';
import {
  ArrowDownTrayIcon,
  ArrowPathIcon,
  CheckIcon,
  ChevronDownIcon,
  KeyIcon,
  ShieldCheckIcon,
} from '@heroicons/react/24/outline';
import {
  clampNumber,
  DEFAULT_HOTKEY,
  type EngineConfig,
  formatHotkeyDisplay,
  type GlobalSettings,
  type HotkeyBinding,
  type ThemeMode,
  type WindowsGpuPreference,
} from '../types';
import {
  FlowButton,
  FlowNumber,
  FlowSegmented,
  FlowSelect,
  FlowSwitch,
} from './ui/FlowControls';

export interface GlobalSettingsViewProps {
  settings: GlobalSettings;
  engineConfig: EngineConfig;
  isAdmin: boolean;
  onSaveSettings: (updated: GlobalSettings) => void;
  onSetHotkey: (hotkey: HotkeyBinding) => void;
  onSetWindowsGpuPreference: (preference: WindowsGpuPreference) => void;
  onRelaunchAsAdmin: () => void;
  onImportLosslessXml: () => void;
  onSaveEngineConfig: (config: EngineConfig) => void;
  onResetEngineConfig: () => void;
}

const GPU_PREFERENCE_OPTIONS: Array<{value: string; label: string; description: string}> = [
  {value: '0', label: 'Auto', description: 'Let Windows choose the DirectX graphics adapter'},
  {value: '2', label: 'High performance', description: 'Prefer discrete GPU for scaling and frame generation'},
  {value: '1', label: 'Power saving', description: 'Prefer integrated GPU to reduce power draw'},
];

const IGNORED_MODIFIER_KEYS = new Set(['Control', 'Alt', 'Shift', 'Meta', 'OS']);

const KEY_ALIAS_MAP: Record<string, string> = {
  ' ': 'Space',
  ArrowUp: 'UP',
  ArrowDown: 'DOWN',
  ArrowLeft: 'LEFT',
  ArrowRight: 'RIGHT',
};

function normalizeRecordedKey(rawKey: string, rawCode?: string): string | null {
  if (IGNORED_MODIFIER_KEYS.has(rawKey)) return null;
  if (rawCode && /^Numpad\d$/.test(rawCode)) return rawCode;
  if (KEY_ALIAS_MAP[rawKey]) return KEY_ALIAS_MAP[rawKey];
  if (rawKey.length === 1) return rawKey.toUpperCase();
  return rawKey;
}

export function GlobalSettingsView({
  settings,
  engineConfig,
  isAdmin,
  onSaveSettings,
  onSetHotkey,
  onSetWindowsGpuPreference,
  onRelaunchAsAdmin,
  onImportLosslessXml,
  onSaveEngineConfig,
  onResetEngineConfig,
}: GlobalSettingsViewProps) {
  const [hotkeyDraft, setHotkeyDraft] = useState<HotkeyBinding>(settings.hotkey);
  const [isRecordingHotkey, setIsRecordingHotkey] = useState(false);
  const [gpuPrefDraft, setGpuPrefDraft] = useState<WindowsGpuPreference>(
    settings.windows_gpu_preference,
  );
  const [iniDraft, setIniDraft] = useState<EngineConfig>(engineConfig);
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [hotkeySavedPulse, setHotkeySavedPulse] = useState(false);
  const [iniSavedPulse, setIniSavedPulse] = useState(false);

  useEffect(() => {
    setHotkeyDraft(settings.hotkey);
  }, [settings.hotkey]);

  useEffect(() => {
    setGpuPrefDraft(settings.windows_gpu_preference);
  }, [settings.windows_gpu_preference]);

  useEffect(() => {
    setIniDraft(engineConfig);
  }, [engineConfig]);

  useEffect(() => {
    if (!isRecordingHotkey) return;
    const onWindowKeyDown = (event: globalThis.KeyboardEvent) => {
      event.preventDefault();
      event.stopPropagation();

      if (event.key === 'Escape') {
        setIsRecordingHotkey(false);
        return;
      }

      const normalizedKey = normalizeRecordedKey(event.key, event.code);
      if (!normalizedKey) return;

      const nextBinding: HotkeyBinding = {
        ctrl: event.ctrlKey,
        alt: event.altKey,
        shift: event.shiftKey,
        key: normalizedKey,
        display: formatHotkeyDisplay({
          ctrl: event.ctrlKey,
          alt: event.altKey,
          shift: event.shiftKey,
          key: normalizedKey,
        }),
      };
      setHotkeyDraft(nextBinding);
      setIsRecordingHotkey(false);
    };
    window.addEventListener('keydown', onWindowKeyDown, true);
    return () => window.removeEventListener('keydown', onWindowKeyDown, true);
  }, [isRecordingHotkey]);

  const patchSettings = (fields: Partial<GlobalSettings>) => {
    onSaveSettings({...settings, ...fields});
  };

  const updateHotkeyModifiers = (mod: Partial<Pick<HotkeyBinding, 'ctrl' | 'alt' | 'shift'>>) => {
    const merged = {...hotkeyDraft, ...mod};
    setHotkeyDraft({
      ...merged,
      display: formatHotkeyDisplay(merged),
    });
  };

  const kbdKeys = formatHotkeyDisplay(hotkeyDraft).split(' + ');

  return (
    <div style={{display: 'flex', flexDirection: 'column', gap: 16, width: '100%'}}>
      {/* Global Hotkey */}
      <section className="fs-card">
        <div className="fs-card-header">
          <div>
            <h2 className="fs-card-title">Global hotkey</h2>
            <p className="fs-card-subtitle">
              Toggle scaling and frame generation on the active foreground window
            </p>
          </div>
          <div className="fs-kbd-group">
            {kbdKeys.map((k) => (
              <kbd key={k} className="fs-kbd">
                {k}
              </kbd>
            ))}
          </div>
        </div>

        <div className="fs-divider" />

        <div className="fs-grid-2" style={{alignItems: 'end'}}>
          <div className="fs-field">
            <label className="fs-field-label" htmlFor="fs-hotkey-display">
              Hotkey recorder
            </label>
            <div className="fs-input-wrap">
              <KeyIcon
                width={14}
                height={14}
                style={{position: 'absolute', left: 10, color: 'var(--fs-text-muted)', pointerEvents: 'none'}}
              />
              <input
                id="fs-hotkey-display"
                className="fs-input"
                style={{paddingLeft: 30}}
                readOnly
                value={isRecordingHotkey ? 'Listening for key input...' : hotkeyDraft.display}
              />
            </div>
            <span className="fs-field-desc">
              {isRecordingHotkey
                ? 'Press desired key combination now, or Escape to cancel'
                : 'Click Record then press a key combination'}
            </span>
          </div>

          <div style={{display: 'flex', gap: 8, flexWrap: 'wrap', paddingBottom: 18}}>
            <FlowButton
              label={isRecordingHotkey ? 'Listening...' : 'Record'}
              variant={isRecordingHotkey ? 'primary' : 'secondary'}
              onClick={() => setIsRecordingHotkey((prev) => !prev)}
            />
            <FlowButton
              label="Save hotkey"
              variant="primary"
              className={hotkeySavedPulse ? 'fs-pulse-success' : ''}
              icon={<CheckIcon width={14} height={14} />}
              onClick={() => {
                onSetHotkey(hotkeyDraft);
                setHotkeySavedPulse(true);
                window.setTimeout(() => setHotkeySavedPulse(false), 320);
              }}
            />
            <FlowButton
              label="Reset default"
              variant="ghost"
              onClick={() => {
                setHotkeyDraft({...DEFAULT_HOTKEY});
                onSetHotkey({...DEFAULT_HOTKEY});
              }}
            />
          </div>
        </div>

        <div className="fs-grid-3" style={{marginTop: 12}}>
          <FlowSwitch
            label="Ctrl"
            checked={hotkeyDraft.ctrl}
            onChange={(checked) => updateHotkeyModifiers({ctrl: checked})}
          />
          <FlowSwitch
            label="Alt"
            checked={hotkeyDraft.alt}
            onChange={(checked) => updateHotkeyModifiers({alt: checked})}
          />
          <FlowSwitch
            label="Shift"
            checked={hotkeyDraft.shift}
            onChange={(checked) => updateHotkeyModifiers({shift: checked})}
          />
        </div>
      </section>

      {/* Appearance & Startup */}
      <section className="fs-card">
        <div className="fs-card-header" style={{alignItems: 'center'}}>
          <div>
            <h2 className="fs-card-title">Appearance and startup</h2>
            <p className="fs-card-subtitle">
              Theme, Windows system tray integration, and launch behavior
            </p>
          </div>
          <div style={{minWidth: 220}}>
            <FlowSegmented
              label="Theme mode"
              value={settings.theme}
              options={[
                {value: 'system', label: 'System'},
                {value: 'light', label: 'Light'},
                {value: 'dark', label: 'Dark'},
              ]}
              onChange={(val) => patchSettings({theme: val as ThemeMode})}
            />
          </div>
        </div>

        <div className="fs-divider" />

        <div className="fs-grid-2">
          <FlowSwitch
            label="Minimize to tray"
            description="Hide FlowScale to the Windows notification area when minimized"
            checked={settings.minimize_to_tray}
            onChange={(checked) => patchSettings({minimize_to_tray: checked})}
          />
          <FlowSwitch
            label="Close to tray"
            description="Keep background auto-scale and hotkeys active on window close"
            checked={settings.close_to_tray}
            onChange={(checked) => patchSettings({close_to_tray: checked})}
          />
          <FlowSwitch
            label="Start minimized"
            description="Launch straight into the system tray without opening the window"
            checked={settings.start_minimized}
            onChange={(checked) => patchSettings({start_minimized: checked})}
          />
          <FlowSwitch
            label="Start at Windows startup"
            description="Register FlowScale to launch automatically on Windows sign-in"
            checked={settings.start_at_startup}
            onChange={(checked) => patchSettings({start_at_startup: checked})}
          />
        </div>

        <div className="fs-divider" />

        <div style={{display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 12, flexWrap: 'wrap'}}>
          <div style={{flex: 1, minWidth: 240}}>
            <FlowSwitch
              label="Start as administrator"
              description="Required when scaling elevated games or sending hotkeys over admin windows"
              checked={settings.start_as_admin}
              onChange={(checked) => patchSettings({start_as_admin: checked})}
            />
          </div>
          <div style={{display: 'flex', alignItems: 'center', gap: 10}}>
            <div className="fs-status-chip">
              <span className="fs-dot" data-variant={isAdmin ? 'success' : 'neutral'} />
              <span>{isAdmin ? 'Elevated' : 'Standard user'}</span>
            </div>
            <FlowButton
              label="Restart as administrator"
              variant="secondary"
              size="sm"
              disabled={isAdmin}
              title={isAdmin ? 'FlowScale is already running elevated' : 'Relaunch with UAC elevation'}
              icon={<ShieldCheckIcon width={14} height={14} />}
              onClick={onRelaunchAsAdmin}
            />
          </div>
        </div>
      </section>

      {/* Windows Graphics & Migration */}
      <section className="fs-card">
        <h2 className="fs-card-title">Windows GPU preference and migration</h2>
        <div className="fs-divider" />
        <div className="fs-grid-2" style={{alignItems: 'end'}}>
          <FlowSelect
            label="Windows GPU preference"
            description="Writes Windows Graphics Settings preference for FlowScale"
            value={String(gpuPrefDraft)}
            options={GPU_PREFERENCE_OPTIONS}
            onChange={(val) => setGpuPrefDraft(Number(val) as WindowsGpuPreference)}
          />
          <div style={{display: 'flex', gap: 8, flexWrap: 'wrap', paddingBottom: 18}}>
            <FlowButton
              label="Apply GPU setting"
              variant="secondary"
              onClick={() => onSetWindowsGpuPreference(gpuPrefDraft)}
            />
            <FlowButton
              label="Import Settings.xml"
              variant="secondary"
              icon={<ArrowDownTrayIcon width={14} height={14} />}
              onClick={onImportLosslessXml}
            />
          </div>
        </div>
      </section>

      {/* Advanced Engine Tuning (config.ini) */}
      <section className="fs-card">
        <button
          type="button"
          aria-expanded={advancedOpen}
          onClick={() => setAdvancedOpen((prev) => !prev)}
          className="fs-accordion-trigger"
        >
          <div>
            <h2 className="fs-card-title">Advanced engine tuning (config.ini)</h2>
            <p className="fs-card-subtitle">Rendering, capture queue, frame-gen thresholds, and FPS overlay</p>
          </div>
          <ChevronDownIcon
            width={16}
            height={16}
            className="fs-accordion-chevron"
          />
        </button>

        <div className="fs-accordion-panel" data-open={advancedOpen ? 'true' : 'false'}>
          <div className="fs-accordion-inner">
            <div className="fs-divider" />
            <div style={{display: 'flex', flexDirection: 'column', gap: 16}}>
              <FlowSwitch
                label="[rendering] flush"
                description="Flush the Direct3D 11 device context after each frame composition pass"
                checked={iniDraft.rendering.flush}
                onChange={(checked) =>
                  setIniDraft((prev) => ({...prev, rendering: {flush: checked}}))
                }
              />

              <div className="fs-grid-2">
                <FlowNumber
                  label="[capture] frametime_buffer_size"
                  description="Sample window size used to smooth frame-pacing estimates (default 15)"
                  value={iniDraft.capture.frametime_buffer_size}
                  min={1}
                  max={1024}
                  step={1}
                  onChange={(value) =>
                    setIniDraft((prev) => ({
                      ...prev,
                      capture: {
                        ...prev.capture,
                        frametime_buffer_size: clampNumber(Math.round(value), 1, 1024),
                      },
                    }))
                  }
                />
                <FlowNumber
                  label="[capture] frametime_buffer_reset_multiplier"
                  description="Spike threshold before resetting pacing history after a hitch (default 6.0)"
                  value={iniDraft.capture.frametime_buffer_reset_multiplier}
                  min={1}
                  max={10}
                  step={0.1}
                  onChange={(value) =>
                    setIniDraft((prev) => ({
                      ...prev,
                      capture: {
                        ...prev.capture,
                        frametime_buffer_reset_multiplier: clampNumber(value, 1, 10),
                      },
                    }))
                  }
                />
                <FlowNumber
                  label="[capture] queue_draining_momentum"
                  description="How aggressively queued capture frames drain under load (0 to 1)"
                  value={iniDraft.capture.queue_draining_momentum}
                  min={0}
                  max={1}
                  step={0.05}
                  onChange={(value) =>
                    setIniDraft((prev) => ({
                      ...prev,
                      capture: {
                        ...prev.capture,
                        queue_draining_momentum: clampNumber(value, 0, 1),
                      },
                    }))
                  }
                />
                <FlowSwitch
                  label="[capture] dedup"
                  description="Skip duplicate unchanged frames during capture"
                  checked={iniDraft.capture.dedup}
                  onChange={(checked) =>
                    setIniDraft((prev) => ({...prev, capture: {...prev.capture, dedup: checked}}))
                  }
                />
                <FlowNumber
                  label="[lsfg] ui_detection_rate"
                  description="Captured frame interval between static HUD/UI checks (1 = every frame)"
                  value={iniDraft.lsfg.ui_detection_rate}
                  min={1}
                  max={60}
                  step={1}
                  onChange={(value) =>
                    setIniDraft((prev) => ({
                      ...prev,
                      lsfg: {
                        ...prev.lsfg,
                        ui_detection_rate: clampNumber(Math.round(value), 1, 60),
                      },
                    }))
                  }
                />
                <FlowNumber
                  label="[lsfg] ui_detection_threshold"
                  description="Confidence cutoff for preserving static HUD text (0 to 1, default 0.5)"
                  value={iniDraft.lsfg.ui_detection_threshold}
                  min={0}
                  max={1}
                  step={0.05}
                  onChange={(value) =>
                    setIniDraft((prev) => ({
                      ...prev,
                      lsfg: {...prev.lsfg, ui_detection_threshold: clampNumber(value, 0, 1)},
                    }))
                  }
                />
                <FlowNumber
                  label="[lsfg] real_timestamp_tolerance"
                  description="Phase tolerance for snapping near-real frames directly (0 to 0.5, default 0.05)"
                  value={iniDraft.lsfg.real_timestamp_tolerance}
                  min={0}
                  max={0.5}
                  step={0.01}
                  onChange={(value) =>
                    setIniDraft((prev) => ({
                      ...prev,
                      lsfg: {...prev.lsfg, real_timestamp_tolerance: clampNumber(value, 0, 0.5)},
                    }))
                  }
                />
                <FlowNumber
                  label="[lsfg] base_framerate_threshold"
                  description="Minimum game FPS before temporarily pausing frame generation (default 10 FPS)"
                  value={iniDraft.lsfg.base_framerate_threshold}
                  min={0}
                  max={120}
                  step={1}
                  units="FPS"
                  onChange={(value) =>
                    setIniDraft((prev) => ({
                      ...prev,
                      lsfg: {
                        ...prev.lsfg,
                        base_framerate_threshold: clampNumber(Math.round(value), 0, 120),
                      },
                    }))
                  }
                />
              </div>

              <div className="fs-grid-3">
                <FlowNumber
                  label="[fps_counter] scale"
                  description="Overlay text size multiplier (0.5 to 3.0)"
                  value={iniDraft.fps_counter.scale}
                  min={0.5}
                  max={3}
                  step={0.1}
                  onChange={(value) =>
                    setIniDraft((prev) => ({
                      ...prev,
                      fps_counter: {...prev.fps_counter, scale: clampNumber(value, 0.5, 3)},
                    }))
                  }
                />
                <label className="fs-field">
                  <span className="fs-field-label">[fps_counter] color</span>
                  <input
                    className="fs-input"
                    value={iniDraft.fps_counter.color}
                    onChange={(e) =>
                      setIniDraft((prev) => ({
                        ...prev,
                        fps_counter: {...prev.fps_counter, color: e.target.value},
                      }))
                    }
                  />
                  <span className="fs-field-desc">Hex overlay text color (e.g. #D3D3D3)</span>
                </label>
                <label className="fs-field">
                  <span className="fs-field-label">[fps_counter] font</span>
                  <input
                    className="fs-input"
                    value={iniDraft.fps_counter.font}
                    onChange={(e) =>
                      setIniDraft((prev) => ({
                        ...prev,
                        fps_counter: {...prev.fps_counter, font: e.target.value},
                      }))
                    }
                  />
                  <span className="fs-field-desc">Installed Windows font family (e.g. Consolas)</span>
                </label>
                <FlowNumber
                  label="[fps_counter] offset_x"
                  description="Horizontal screen margin in pixels"
                  value={iniDraft.fps_counter.offset_x}
                  step={1}
                  units="px"
                  onChange={(value) =>
                    setIniDraft((prev) => ({
                      ...prev,
                      fps_counter: {...prev.fps_counter, offset_x: Math.round(value)},
                    }))
                  }
                />
                <FlowNumber
                  label="[fps_counter] offset_y"
                  description="Vertical screen margin in pixels"
                  value={iniDraft.fps_counter.offset_y}
                  step={1}
                  units="px"
                  onChange={(value) =>
                    setIniDraft((prev) => ({
                      ...prev,
                      fps_counter: {...prev.fps_counter, offset_y: Math.round(value)},
                    }))
                  }
                />
                <FlowSwitch
                  label="[fps_counter] show_captured"
                  description="Show both game capture FPS and generated output FPS"
                  checked={iniDraft.fps_counter.show_captured}
                  onChange={(checked) =>
                    setIniDraft((prev) => ({
                      ...prev,
                      fps_counter: {...prev.fps_counter, show_captured: checked},
                    }))
                  }
                />
              </div>

              <div style={{display: 'flex', justifyContent: 'flex-end', gap: 8}}>
                <FlowButton
                  label="Reset defaults"
                  variant="ghost"
                  icon={<ArrowPathIcon width={14} height={14} />}
                  onClick={onResetEngineConfig}
                />
                <FlowButton
                  label="Save config.ini"
                  variant="primary"
                  className={iniSavedPulse ? 'fs-pulse-success' : ''}
                  icon={<CheckIcon width={14} height={14} />}
                  onClick={() => {
                    onSaveEngineConfig(iniDraft);
                    setIniSavedPulse(true);
                    window.setTimeout(() => setIniSavedPulse(false), 320);
                  }}
                />
              </div>
            </div>
          </div>
        </div>
      </section>
    </div>
  );
}
