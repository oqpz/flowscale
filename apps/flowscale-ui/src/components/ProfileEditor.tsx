import {useState} from 'react';
import {PencilSquareIcon} from '@heroicons/react/24/outline';
import {useSlidingPill} from './ui/useSlidingPill';
import {
  applyScalingTypeRules,
  clampNumber,
  getSharpnessMax,
  normalizeScaleFactor,
  normalizeSharpness,
  SCALING_SUBTYPES,
  type CaptureApi,
  type FitMode,
  type FrameGenEngine,
  type HardwareInfo,
  type Lsfg2Mode,
  type Lsfg3Mode,
  type LsfgPerformance,
  type ScalingMode,
  type ScalingProfile,
  type ScalingType,
  type SyncMode,
} from '../types';
import {
  FlowButton,
  FlowCrossfade,
  FlowNumber,
  FlowSegmented,
  FlowSelect,
  FlowSlider,
  FlowSwitch,
} from './ui/FlowControls';

export interface ProfileEditorProps {
  profile: ScalingProfile;
  hardware: HardwareInfo;
  onChange: (updated: ScalingProfile) => void;
  onEditMeta: () => void;
}

type EditorTab = 'overview' | 'scaling' | 'framegen' | 'display' | 'cursor';

const EDITOR_TABS: Array<{id: EditorTab; label: string}> = [
  {id: 'overview', label: 'Overview'},
  {id: 'scaling', label: 'Scaling'},
  {id: 'framegen', label: 'Frame Gen'},
  {id: 'display', label: 'Display & Capture'},
  {id: 'cursor', label: 'Cursor & Crop'},
];

const EDITOR_TAB_ORDER: Record<EditorTab, number> = {
  overview: 0,
  scaling: 1,
  framegen: 2,
  display: 3,
  cursor: 4,
};

const SCALING_TYPE_OPTIONS: Array<{value: ScalingType; label: string; description: string}> = [
  {value: 'Off', label: 'Off', description: 'Pass window frames without spatial upscaling'},
  {value: 'LS1', label: 'FlowSharp', description: 'FlowScale shader balanced for games'},
  {value: 'FSR', label: 'AMD FSR 1.0', description: 'Edge-adaptive spatial upscaling with RCAS'},
  {value: 'NIS', label: 'NVIDIA Image Scaling', description: 'Directional scaling and sharpening pass'},
  {value: 'SGSR', label: 'Snapdragon GSR', description: 'Single-pass spatial scaler and edge filter'},
  {value: 'BicubicCAS', label: 'Bicubic CAS', description: 'Bicubic sampling with contrast adaptive sharpening'},
  {value: 'Anime4K', label: 'Anime4K', description: 'Real-time restoration for hand-drawn animation'},
  {value: 'XBR', label: 'xBR', description: 'Pixel-art contour interpolation (0.2x steps)'},
  {value: 'SharpBilinear', label: 'Sharp Bilinear', description: 'Crisp texel edges with low shimmer'},
  {value: 'Integer', label: 'Integer', description: 'Exact integer pixel duplication (1x, 2x, 3x)'},
  {value: 'Nearest', label: 'Nearest Neighbor', description: 'Raw nearest pixel sampling'},
];

const FRAME_GEN_OPTIONS: Array<{value: FrameGenEngine; label: string; description: string}> = [
  {value: 'Off', label: 'Off', description: 'No synthetic intermediate frames'},
  {value: 'LSFG3', label: 'FlowGen 3', description: 'Newest optical-flow engine (Fixed 1-20x or Adaptive)'},
  {value: 'LSFG2', label: 'FlowGen 2', description: 'Classic X2 / X3 / X4 optical-flow mode'},
  {value: 'LSFG1', label: 'FlowGen 1', description: 'Legacy fixed 2x frame generation'},
];

const SYNC_MODE_OPTIONS: Array<{value: SyncMode; label: string}> = [
  {value: 'Off', label: 'Off (Allow tearing)'},
  {value: 'Default', label: 'Default (Tear-free)'},
  {value: 'Vsync1', label: 'VSync (1x refresh)'},
  {value: 'Vsync2', label: 'VSync 1/2 refresh'},
  {value: 'Vsync3', label: 'VSync 1/3 refresh'},
  {value: 'Vsync4', label: 'VSync 1/4 refresh'},
];

const CAPTURE_API_OPTIONS: Array<{value: CaptureApi; label: string; description: string}> = [
  {value: 'DXGI', label: 'DXGI Desktop Duplication', description: 'Lowest capture latency on Windows'},
  {value: 'WGC', label: 'Windows Graphics Capture', description: 'Recommended for Win 11 borderless windows'},
  {value: 'GDI', label: 'GDI Fallback', description: 'Compatibility capture for older windows'},
];

export function ProfileEditor({
  profile,
  hardware,
  onChange,
  onEditMeta,
}: ProfileEditorProps) {
  const [activeTab, setActiveTab] = useState<EditorTab>('overview');
  const subtabsRef = useSlidingPill<HTMLDivElement>(activeTab);
  const [tabDirection, setTabDirection] = useState<'none' | 'left' | 'right'>('none');
  const [presetPulse, setPresetPulse] = useState<string | null>(null);

  const isFg2xActive =
    !profile.cpu_vulkan &&
    profile.frame_gen_engine === 'LSFG3' &&
    profile.lsfg3_mode === 'Fixed' &&
    profile.lsfg_multiplier === 2 &&
    profile.scaling_type === 'LS1' &&
    profile.scaling_mode === 'Auto';

  const isUpscaleActive =
    !profile.cpu_vulkan &&
    profile.frame_gen_engine === 'Off' &&
    profile.scaling_type === 'LS1' &&
    profile.scaling_mode === 'Auto';

  const isAdaptiveActive =
    !profile.cpu_vulkan &&
    profile.frame_gen_engine === 'LSFG3' &&
    profile.lsfg3_mode === 'Adaptive' &&
    profile.scaling_type === 'Off';

  const isOffActive =
    !profile.cpu_vulkan &&
    profile.frame_gen_engine === 'Off' &&
    profile.scaling_type === 'Off';

  const patch = (fields: Partial<ScalingProfile>) => {
    onChange({...profile, ...fields});
  };

  const applyQuickPreset = (name: string, fields: Partial<ScalingProfile>) => {
    patch(fields);
    setPresetPulse(name);
    window.setTimeout(() => setPresetPulse(null), 320);
  };

  const subtypeOptions = [...(SCALING_SUBTYPES[profile.scaling_type] ?? [])];
  const sharpnessMax = getSharpnessMax(profile.scaling_type);
  const defaultAdaptiveFps = clampNumber(Math.round(hardware.dwm_refresh_rate || 60), 10, 1000);
  const scaleStep =
    profile.scaling_type === 'Integer' ? 1 : profile.scaling_type === 'XBR' ? 0.2 : 0.05;

  const gpuOptions = [
    {value: 'Auto', label: 'Auto'},
    ...hardware.adapters.map((adapter) => ({
      value: adapter.id,
      label: adapter.name,
      description: `${adapter.dedicated_vram_mb} MB VRAM`,
    })),
  ];

  const displayOptions = [
    {value: 'Auto', label: 'Auto'},
    ...hardware.displays.map((display) => ({
      value: display.id,
      label: display.name,
      description: `${display.width}x${display.height} @ ${Math.round(display.refresh_rate)} Hz${display.hdr_capable ? ' · HDR' : ''}`,
    })),
  ];

  const scalingSummary =
    profile.scaling_type === 'Off'
      ? 'Scaling: Off'
      : `Scaling: ${profile.scaling_type} (${profile.scaling_mode === 'Auto' ? 'Auto' : `${profile.scale_factor}x`})`;

  const frameGenSummary =
    profile.frame_gen_engine === 'Off'
      ? 'Frame Gen: Off'
      : profile.frame_gen_engine === 'LSFG3'
        ? profile.lsfg3_mode === 'Fixed'
          ? `FlowGen 3 · ${profile.lsfg_multiplier}x`
          : `FlowGen 3 · ${profile.lsfg_target_fps} FPS`
        : profile.frame_gen_engine === 'LSFG2'
          ? `FlowGen 2 · ${profile.lsfg2_mode}`
          : 'FlowGen 1 · 2x';

  return (
    <div style={{display: 'flex', flexDirection: 'column', gap: 16, width: '100%'}}>
      {/* Hero Summary Card */}
      <section className="fs-card">
        <div className="fs-card-header">
          <div>
            <div style={{display: 'flex', alignItems: 'center', gap: 8}}>
              <h2 className="fs-card-title" style={{fontSize: 18}}>{profile.title}</h2>
              <span
                className="fs-dot"
                data-variant={profile.is_default ? 'neutral' : profile.auto_scale ? 'success' : 'neutral'}
                title={profile.is_default ? 'Default' : profile.auto_scale ? 'Auto-scale on' : 'Manual'}
              />
            </div>
            <p className="fs-card-subtitle fs-tabular" style={{marginTop: 2}}>
              {profile.is_default
                ? 'Global fallback profile — used when no game rule matches'
                : `Rule: ${profile.process_pattern || 'None'} · Delay: ${profile.auto_scale_delay_sec}s`}
            </p>
          </div>

          <div style={{display: 'flex', alignItems: 'center', gap: 8, flexWrap: 'wrap'}}>
            <span className="fs-summary-chip">
              {scalingSummary} · {frameGenSummary} · {profile.cpu_vulkan ? 'GDI' : profile.capture_api}
            </span>
            {!profile.is_default && (
              <FlowButton
                label="Edit rule"
                variant="secondary"
                size="sm"
                icon={<PencilSquareIcon width={14} height={14} />}
                onClick={onEditMeta}
              />
            )}
          </div>
        </div>

        <div className="fs-divider" />

        {/* Quick Setup Presets for New Users */}
        <div style={{display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 10, flexWrap: 'wrap'}}>
          <span style={{fontSize: 12, color: 'var(--fs-text-secondary)', fontWeight: 500}}>
            Quick setup:
          </span>
          <div style={{display: 'flex', gap: 6, flexWrap: 'wrap'}}>
            <button
              type="button"
              aria-pressed={isFg2xActive}
              className={`fs-preset-pill ${presetPulse === 'fg2x' ? 'fs-pulse-success' : ''}`}
              onClick={() =>
                applyQuickPreset('fg2x', {
                  cpu_vulkan: false,
                  frame_gen_engine: 'LSFG3',
                  lsfg3_mode: 'Fixed',
                  lsfg_multiplier: 2,
                  scaling_type: 'LS1',
                  scaling_mode: 'Auto',
                  capture_api: 'DXGI',
                })
              }
            >
              Smooth 2x + FlowSharp
            </button>
            <button
              type="button"
              aria-pressed={isUpscaleActive}
              className={`fs-preset-pill ${presetPulse === 'upscale' ? 'fs-pulse-success' : ''}`}
              onClick={() =>
                applyQuickPreset('upscale', {
                  cpu_vulkan: false,
                  frame_gen_engine: 'Off',
                  scaling_type: 'LS1',
                  scaling_mode: 'Auto',
                  sharpness: 3,
                })
              }
            >
              Crisp Upscale Only
            </button>
            <button
              type="button"
              aria-pressed={isAdaptiveActive}
              className={`fs-preset-pill ${presetPulse === 'adaptive' ? 'fs-pulse-success' : ''}`}
              onClick={() =>
                applyQuickPreset('adaptive', {
                  cpu_vulkan: false,
                  frame_gen_engine: 'LSFG3',
                  lsfg3_mode: 'Adaptive',
                  lsfg_target_fps: defaultAdaptiveFps,
                  scaling_type: 'Off',
                  capture_api: 'DXGI',
                })
              }
            >
              Adaptive ({defaultAdaptiveFps} FPS)
            </button>
            <button
              type="button"
              aria-pressed={isOffActive}
              className={`fs-preset-pill ${presetPulse === 'reset' ? 'fs-pulse-success' : ''}`}
              onClick={() =>
                applyQuickPreset('reset', {
                  cpu_vulkan: false,
                  frame_gen_engine: 'Off',
                  scaling_type: 'Off',
                  crop_enabled: false,
                })
              }
            >
              Disable All
            </button>
          </div>
        </div>
      </section>

      {/* Focused Sub-Navigation Bar */}
      <div>
        <div ref={subtabsRef} className="fs-subtabs" role="tablist" aria-label="Profile settings sections">
          <span className="fs-slide-pill" aria-hidden="true" />
          {EDITOR_TABS.map((tab) => (
            <button
              key={tab.id}
              type="button"
              role="tab"
              aria-selected={activeTab === tab.id}
              data-text={tab.label}
              className="fs-subtab"
              onClick={() => {
                if (tab.id !== activeTab) {
                  setTabDirection(
                    EDITOR_TAB_ORDER[tab.id] > EDITOR_TAB_ORDER[activeTab] ? 'right' : 'left',
                  );
                }
                setActiveTab(tab.id);
              }}
            >
              {tab.label}
            </button>
          ))}
        </div>
      </div>

      {/* Sub-Tab Views (Overlapping Crossfade #1.17) */}
      <FlowCrossfade activeKey={activeTab} direction={tabDirection}>
        {activeTab === 'overview' && (
          <section className="fs-card">
            <h3 className="fs-card-title">Core profile settings</h3>
            <p className="fs-card-subtitle" style={{marginTop: 2}}>
              Set your game to Windowed or Borderless (never Exclusive Fullscreen), pick a preset or engine below, then press Scale in the top right.
            </p>
            <div className="fs-divider" />

            <div className="fs-grid-2">
              <FlowSelect
                label="Upscaler"
                description="Enlarges lower-res windowed games cleanly"
                value={profile.scaling_type}
                options={SCALING_TYPE_OPTIONS}
                onChange={(val) => onChange(applyScalingTypeRules(profile, val as ScalingType))}
              />

              <FlowSelect
                label="Frame generation engine"
                description="Synthesizes intermediate frames for higher smoothness"
                disabled={profile.cpu_vulkan}
                disabledMessage="Disable CPU Vulkan inside Frame Gen tab to switch engine"
                value={profile.frame_gen_engine}
                options={FRAME_GEN_OPTIONS}
                onChange={(val) =>
                  patch({
                    frame_gen_engine: val as FrameGenEngine,
                    lsfg_target_fps:
                      profile.lsfg_target_fps >= 10 ? profile.lsfg_target_fps : defaultAdaptiveFps,
                  })
                }
              />

              {profile.frame_gen_engine === 'LSFG3' && profile.lsfg3_mode === 'Fixed' && (
                <FlowNumber
                  label="Frame multiplier"
                  description="Multiply game FPS by this factor (e.g. 2x = 60 -> 120 FPS)"
                  disabled={profile.cpu_vulkan}
                  value={profile.lsfg_multiplier}
                  min={1}
                  max={20}
                  step={1}
                  units="x"
                  onChange={(val) => patch({lsfg_multiplier: clampNumber(Math.round(val), 1, 20)})}
                />
              )}

              {profile.frame_gen_engine === 'LSFG3' && profile.lsfg3_mode === 'Adaptive' && (
                <FlowNumber
                  label="Target FPS"
                  description={`Monitor default: ${defaultAdaptiveFps} Hz`}
                  value={profile.lsfg_target_fps}
                  min={10}
                  max={1000}
                  step={1}
                  units="FPS"
                  onChange={(val) => patch({lsfg_target_fps: clampNumber(Math.round(val), 10, 1000)})}
                />
              )}

              <FlowSelect
                label="Capture method"
                description="DXGI is fastest; choose WGC if a Win 11 borderless window stutters"
                disabled={profile.cpu_vulkan}
                value={profile.cpu_vulkan ? 'GDI' : profile.capture_api}
                options={CAPTURE_API_OPTIONS}
                onChange={(val) => patch({capture_api: val as CaptureApi})}
              />
            </div>

            <div className="fs-divider" />

            <div className="fs-grid-2">
              <FlowSwitch
                label="Draw FPS overlay"
                description="Show live captured and generated FPS counter on screen"
                checked={profile.draw_fps}
                onChange={(checked) => patch({draw_fps: checked})}
              />
              <FlowSwitch
                label="Auto-scale when game is focused"
                description={
                  profile.is_default
                    ? 'Create a custom game profile in the left sidebar to enable auto-scale'
                    : 'Automatically starts scaling after switching to the game window'
                }
                checked={!profile.is_default && profile.auto_scale}
                disabled={profile.is_default}
                disabledMessage="Default profile only scales manually or via hotkey"
                onChange={(checked) => patch({auto_scale: checked})}
              />
            </div>
          </section>
        )}

        {activeTab === 'scaling' && (
          <section className="fs-card">
            <h3 className="fs-card-title">Spatial scaling</h3>
            <p className="fs-card-subtitle">Configure upscaling shader, sizing behavior, and sharpness.</p>
            <div className="fs-divider" />

            <div className="fs-grid-2">
              <FlowSegmented
                label="Scaling mode"
                value={profile.scaling_mode}
                options={[
                  {value: 'Auto', label: 'Auto (Fill screen)'},
                  {value: 'Custom', label: 'Custom factor'},
                ]}
                onChange={(val) => patch({scaling_mode: val as ScalingMode})}
              />

              <FlowSegmented
                label="Fit mode"
                value={profile.fit_mode}
                options={[
                  {value: 'AspectRatio', label: 'Aspect ratio'},
                  {value: 'Fullscreen', label: 'Fullscreen'},
                ]}
                onChange={(val) => patch({fit_mode: val as FitMode})}
              />

              <FlowSelect
                label="Scaling type"
                value={profile.scaling_type}
                options={SCALING_TYPE_OPTIONS}
                onChange={(val) => onChange(applyScalingTypeRules(profile, val as ScalingType))}
              />

              {subtypeOptions.length > 0 && (
                <FlowSelect
                  label="Variant / Subtype"
                  value={profile.scaling_subtype}
                  options={subtypeOptions}
                  onChange={(val) => patch({scaling_subtype: val})}
                />
              )}

              <FlowNumber
                label="Scale factor"
                description={
                  profile.scaling_type === 'Integer'
                    ? 'Snaps to whole integers'
                    : profile.scaling_type === 'XBR'
                      ? 'Snaps to 0.2x steps'
                      : 'Range 1.000 to 10.000'
                }
                value={profile.scale_factor}
                min={1}
                max={10}
                step={scaleStep}
                units="x"
                disabled={profile.scaling_mode === 'Auto'}
                disabledMessage="Switch Scaling mode to Custom factor to edit"
                onChange={(val) =>
                  patch({scale_factor: normalizeScaleFactor(val, profile.scaling_type)})
                }
              />

              <FlowSlider
                label="Sharpness"
                description={`0 to ${sharpnessMax} (${profile.scaling_type === 'LS1' ? 'FlowSharp range' : 'standard range'})`}
                value={normalizeSharpness(profile.sharpness, profile.scaling_type)}
                min={0}
                max={sharpnessMax}
                step={1}
                disabled={profile.scaling_type === 'Off'}
                disabledMessage="Select a scaling algorithm first"
                onChange={(val) =>
                  patch({sharpness: normalizeSharpness(val, profile.scaling_type)})
                }
              />
            </div>

            <div className="fs-divider" />

            <div className="fs-grid-2">
              <FlowSwitch
                label="Resize before scaling"
                description="Shrink window proportionally before applying upscaler"
                checked={profile.resize_before_scaling}
                onChange={(checked) => patch({resize_before_scaling: checked})}
              />
              <FlowSwitch
                label="Windowed mode"
                description="Scale within windowed bounds without forcing fullscreen ownership"
                checked={profile.windowed_mode}
                onChange={(checked) => patch({windowed_mode: checked})}
              />
              {profile.scaling_type === 'Anime4K' && (
                <FlowSwitch
                  label="Variable rate shading (VRS)"
                  description="Optimize Anime4K pass shading rate on supported GPUs"
                  checked={profile.vrs}
                  onChange={(checked) => patch({vrs: checked})}
                />
              )}
            </div>
          </section>
        )}

        {activeTab === 'framegen' && (
          <section className="fs-card">
            <h3 className="fs-card-title">Frame generation</h3>
            <p className="fs-card-subtitle">Optical-flow motion estimation and intermediate frame synthesis.</p>
            <div className="fs-divider" />

            <div className="fs-grid-2">
              <FlowSelect
                label="Processing backend"
                value={profile.cpu_vulkan ? 'CPU' : 'GPU'}
                options={[
                  {value: 'GPU', label: 'GPU (Recommended)'},
                  {value: 'CPU', label: 'CPU Vulkan · Experimental'},
                ]}
                onChange={(val) =>
                  patch(
                    val === 'CPU'
                      ? {
                          cpu_vulkan: true,
                          frame_gen_engine: 'LSFG3',
                          lsfg3_mode: 'Fixed',
                          lsfg_multiplier: 2,
                          scaling_type: 'Off',
                          hdr_support: false,
                          crop_enabled: false,
                        }
                      : {cpu_vulkan: false},
                  )
                }
              />

              <FlowSelect
                label="Frame generation engine"
                description="FlowGen 3 works best for most games; FlowGen 2 offers fixed 2x/3x/4x fallback"
                disabled={profile.cpu_vulkan}
                value={profile.frame_gen_engine}
                options={FRAME_GEN_OPTIONS}
                onChange={(val) =>
                  patch({
                    frame_gen_engine: val as FrameGenEngine,
                    lsfg_target_fps:
                      profile.lsfg_target_fps >= 10 ? profile.lsfg_target_fps : defaultAdaptiveFps,
                  })
                }
              />
            </div>

            {profile.cpu_vulkan && (
              <p className="fs-card-subtitle" style={{marginTop: 10, padding: '8px 10px', borderRadius: 8, backgroundColor: 'var(--fs-bg-pill)'}}>
                CPU Vulkan renders frames using system RAM and CPU cores. Fixed 2x, SDR, native resolution, and GDI capture are locked in this mode.
              </p>
            )}

            {profile.frame_gen_engine !== 'Off' && (
              <>
                <div className="fs-divider" />
                <div className="fs-grid-2">
                  {profile.frame_gen_engine === 'LSFG3' && (
                    <FlowSegmented
                      label="FlowGen 3 mode"
                      disabled={profile.cpu_vulkan}
                      value={profile.lsfg3_mode}
                      options={[
                        {value: 'Fixed', label: 'Fixed'},
                        {value: 'Adaptive', label: 'Adaptive'},
                      ]}
                      onChange={(val) =>
                        patch({
                          lsfg3_mode: val as Lsfg3Mode,
                          lsfg_target_fps:
                            profile.lsfg_target_fps >= 10 ? profile.lsfg_target_fps : defaultAdaptiveFps,
                        })
                      }
                    />
                  )}

                  {profile.frame_gen_engine === 'LSFG2' && (
                    <FlowSegmented
                      label="FlowGen 2 multiplier"
                      value={profile.lsfg2_mode}
                      options={[
                        {value: 'X2', label: '2x'},
                        {value: 'X3', label: '3x'},
                        {value: 'X4', label: '4x'},
                      ]}
                      onChange={(val) => patch({lsfg2_mode: val as Lsfg2Mode})}
                    />
                  )}

                  {profile.frame_gen_engine === 'LSFG3' && profile.lsfg3_mode === 'Fixed' && (
                    <FlowNumber
                      label="Frame multiplier"
                      description="1x to 20x (2x doubles frame rate, e.g. 60 -> 120 FPS)"
                      disabled={profile.cpu_vulkan}
                      value={profile.lsfg_multiplier}
                      min={1}
                      max={20}
                      step={1}
                      units="x"
                      onChange={(val) => patch({lsfg_multiplier: clampNumber(Math.round(val), 1, 20)})}
                    />
                  )}

                  {profile.frame_gen_engine === 'LSFG3' && profile.lsfg3_mode === 'Adaptive' && (
                    <FlowNumber
                      label="Target FPS"
                      description={`Match monitor refresh (${defaultAdaptiveFps} Hz) for silky motion`}
                      value={profile.lsfg_target_fps}
                      min={10}
                      max={1000}
                      step={1}
                      units="FPS"
                      onChange={(val) => patch({lsfg_target_fps: clampNumber(Math.round(val), 10, 1000)})}
                    />
                  )}

                  <FlowSlider
                    label="Flow scale"
                    description="Optical-flow resolution % (lower to 50–75% at 1440p/4K to save GPU)"
                    disabled={profile.cpu_vulkan}
                    value={profile.flow_scale}
                    min={25}
                    max={100}
                    step={5}
                    formatValue={(v) => `${v}%`}
                    onChange={(val) =>
                      patch({flow_scale: clampNumber(Math.round(val / 5) * 5, 25, 100)})
                    }
                  />

                  <div className="fs-field">
                    <FlowSegmented
                      label="Model size"
                      disabled={profile.cpu_vulkan}
                      value={profile.lsfg_performance}
                      options={[
                        {value: 'Balanced', label: 'Balanced'},
                        {value: 'Performance', label: 'Performance'},
                      ]}
                      onChange={(val) => patch({lsfg_performance: val as LsfgPerformance})}
                    />
                    <span className="fs-field-desc">
                      Balanced = fewer artifacts; Performance = lighter on mid-range GPUs
                    </span>
                  </div>
                </div>
              </>
            )}
          </section>
        )}

        {activeTab === 'display' && (
          <section className="fs-card">
            <h3 className="fs-card-title">Display, presentation & capture</h3>
            <p className="fs-card-subtitle">Swapchain pacing, capture queue, HDR, and adapter routing.</p>
            <div className="fs-divider" />

            <div className="fs-grid-2">
              <FlowSelect
                label="Capture API"
                disabled={profile.cpu_vulkan}
                value={profile.cpu_vulkan ? 'GDI' : profile.capture_api}
                options={CAPTURE_API_OPTIONS}
                onChange={(val) => patch({capture_api: val as CaptureApi})}
              />

              <FlowNumber
                label="Queue target"
                description="Buffered capture frames (1 = balanced, 0 = lowest latency, 2 = smoothest)"
                value={profile.queue_target}
                min={0}
                max={2}
                step={1}
                onChange={(val) => patch({queue_target: clampNumber(Math.round(val), 0, 2)})}
              />

              <FlowSelect
                label="Sync mode"
                description="Controls screen tearing vs vsync pacing"
                value={profile.sync_mode}
                options={SYNC_MODE_OPTIONS}
                onChange={(val) => patch({sync_mode: val as SyncMode})}
              />

              <FlowNumber
                label="Max frame latency"
                description="Queued frames before display (3 = default, 1 = crispest mouse feel)"
                value={profile.max_frame_latency}
                min={1}
                max={15}
                step={1}
                onChange={(val) => patch({max_frame_latency: clampNumber(Math.round(val), 1, 15)})}
              />

              <FlowSelect
                label="Preferred GPU"
                value={profile.preferred_gpu}
                options={gpuOptions}
                onChange={(val) => patch({preferred_gpu: val})}
              />

              <FlowSelect
                label="Output display"
                value={profile.output_display}
                options={displayOptions}
                onChange={(val) => patch({output_display: val})}
              />
            </div>

            <div className="fs-divider" />

            <div className="fs-grid-2">
              <FlowSwitch
                label="G-Sync / FreeSync support"
                description="Use VRR-compatible flip presentation timing"
                checked={profile.gsync_support}
                onChange={(checked) => patch({gsync_support: checked})}
              />
              <FlowSwitch
                label="HDR support"
                description="Keep wide-gamut HDR colors intact when Windows HDR is active"
                checked={profile.hdr_support}
                onChange={(checked) => patch({hdr_support: checked})}
              />
              <FlowSwitch
                label="Draw FPS overlay"
                description="Show live captured and presented FPS counter"
                checked={profile.draw_fps}
                onChange={(checked) => patch({draw_fps: checked})}
              />
              <FlowSwitch
                label="Multi-display mode"
                description="Keep scaling active across monitor transitions"
                checked={profile.multi_display_mode}
                onChange={(checked) => patch({multi_display_mode: checked})}
              />
            </div>
          </section>
        )}

        {activeTab === 'cursor' && (
          <section className="fs-card">
            <h3 className="fs-card-title">Cursor & input crop</h3>
            <p className="fs-card-subtitle">Confine or scale mouse pointers and trim black bars before scaling.</p>
            <div className="fs-divider" />

            <div className="fs-grid-2">
              <FlowSwitch
                label="Clip cursor"
                description="Confine pointer inside the scaled game window"
                checked={profile.clip_cursor}
                onChange={(checked) => patch({clip_cursor: checked})}
              />
              <FlowSwitch
                label="Adjust cursor speed"
                description="Match cursor sensitivity to scaled resolution"
                checked={profile.adjust_cursor_speed}
                onChange={(checked) => patch({adjust_cursor_speed: checked})}
              />
              <FlowSwitch
                label="Hide cursor"
                description="Hide Windows cursor on scaled frames"
                checked={profile.hide_cursor}
                onChange={(checked) => patch({hide_cursor: checked})}
              />
              <FlowSwitch
                label="Scale cursor"
                description="Scale software cursor proportionally with viewport"
                checked={profile.scale_cursor}
                onChange={(checked) => patch({scale_cursor: checked})}
              />
            </div>

            <div className="fs-divider" />

            <div style={{display: 'flex', flexDirection: 'column', gap: 12}}>
              <FlowSwitch
                label="Crop input edges"
                description="Trim pixels from source window borders before scaling"
                checked={profile.crop_enabled}
                onChange={(checked) => patch({crop_enabled: checked})}
              />

              <div className="fs-grid-4">
                <FlowNumber
                  label="Left"
                  value={profile.crop_left}
                  min={0}
                  step={1}
                  units="px"
                  disabled={!profile.crop_enabled}
                  onChange={(val) => patch({crop_left: Math.max(0, Math.round(val))})}
                />
                <FlowNumber
                  label="Top"
                  value={profile.crop_top}
                  min={0}
                  step={1}
                  units="px"
                  disabled={!profile.crop_enabled}
                  onChange={(val) => patch({crop_top: Math.max(0, Math.round(val))})}
                />
                <FlowNumber
                  label="Right"
                  value={profile.crop_right}
                  min={0}
                  step={1}
                  units="px"
                  disabled={!profile.crop_enabled}
                  onChange={(val) => patch({crop_right: Math.max(0, Math.round(val))})}
                />
                <FlowNumber
                  label="Bottom"
                  value={profile.crop_bottom}
                  min={0}
                  step={1}
                  units="px"
                  disabled={!profile.crop_enabled}
                  onChange={(val) => patch({crop_bottom: Math.max(0, Math.round(val))})}
                />
              </div>
            </div>
          </section>
        )}
      </FlowCrossfade>
    </div>
  );
}
