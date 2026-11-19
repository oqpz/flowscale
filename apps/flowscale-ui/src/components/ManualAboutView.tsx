import {
  ComputerDesktopIcon,
  CpuChipIcon,
  KeyIcon,
  ShieldCheckIcon,
  TvIcon,
} from '@heroicons/react/24/outline';
import {formatHotkeyDisplay, type HardwareInfo, type HotkeyBinding} from '../types';

export interface ManualAboutViewProps {
  hardware: HardwareInfo;
  hotkey: HotkeyBinding;
}

export function ManualAboutView({hardware, hotkey}: ManualAboutViewProps) {
  const adapterSummary =
    hardware.adapters.length > 0
      ? hardware.adapters
          .map((adapter) => `${adapter.name} (${adapter.dedicated_vram_mb} MB)`)
          .join(', ')
      : 'No adapters detected';

  const displaySummary =
    hardware.displays.length > 0
      ? hardware.displays
          .map((display) => {
            const specs = `${display.width}x${display.height}`;
            const base = display.name.includes(specs)
              ? display.name
              : `${display.name} (${specs} @ ${Math.round(display.refresh_rate)} Hz)`;
            return `${base}${display.hdr_capable ? ' · HDR' : ''}`;
          })
          .join(', ')
      : 'No displays detected';

  return (
    <div style={{display: 'flex', flexDirection: 'column', gap: 16, width: '100%'}}>
      {/* Quick Start for New Users */}
      <section className="fs-card">
        <div className="fs-card-header">
          <div>
            <h2 className="fs-card-title">Quick start</h2>
            <p className="fs-card-subtitle">Three steps to scale or generate frames on any game window</p>
          </div>
          <span className="fs-summary-chip">Hotkey: {formatHotkeyDisplay(hotkey)}</span>
        </div>

        <div className="fs-divider" />

        <div className="fs-grid-3">
          <div style={{padding: '10px 12px', borderRadius: 8, backgroundColor: 'var(--fs-bg-pill)', border: '1px solid var(--fs-border-hairline)'}}>
            <div style={{fontSize: 12, fontWeight: 700, color: 'var(--fs-accent)', marginBottom: 4}}>
              1 · Switch game to Windowed
            </div>
            <p className="fs-card-subtitle">
              Set your game to Windowed or Borderless Fullscreen (never Exclusive Fullscreen).
            </p>
          </div>

          <div style={{padding: '10px 12px', borderRadius: 8, backgroundColor: 'var(--fs-bg-pill)', border: '1px solid var(--fs-border-hairline)'}}>
            <div style={{fontSize: 12, fontWeight: 700, color: 'var(--fs-accent)', marginBottom: 4}}>
              2 · Pick Scaling or FlowGen 3
            </div>
            <p className="fs-card-subtitle">
              Choose a quick preset in Profiles (e.g. 2x Smooth Frame Gen or Sharp FlowSharp Upscale).
            </p>
          </div>

          <div style={{padding: '10px 12px', borderRadius: 8, backgroundColor: 'var(--fs-bg-pill)', border: '1px solid var(--fs-border-hairline)'}}>
            <div style={{fontSize: 12, fontWeight: 700, color: 'var(--fs-accent)', marginBottom: 4}}>
              3 · Click Scale or press Hotkey
            </div>
            <p className="fs-card-subtitle">
              Press Scale in the top right for a 5-second countdown, then click into your game.
            </p>
          </div>
        </div>
      </section>

      {/* Windows Configuration Guide */}
      <section className="fs-card">
        <h2 className="fs-card-title">Windows configuration guide</h2>
        <div className="fs-divider" />
        <div className="fs-grid-2">
          <div>
            <h3 style={{fontSize: 13.5, fontWeight: 600, marginBottom: 4}}>
              Scaling vs Resize before scaling
            </h3>
            <p className="fs-card-subtitle">
              Run games in Windowed or Borderless mode below monitor resolution. Enable Resize before scaling when a game locks its window size so FlowScale shrinks the client area before applying FlowSharp, FSR, NIS, SGSR, or Anime4K.
            </p>
          </div>

          <div>
            <h3 style={{fontSize: 13.5, fontWeight: 600, marginBottom: 4}}>
              FlowGen Fixed vs Adaptive
            </h3>
            <p className="fs-card-subtitle">
              Fixed multiplies every captured frame (1x to 20x in FlowGen 3; 2x/3x/4x in FlowGen 2). Cap in-game FPS to a clean divisor of your refresh rate. Adaptive targets a steady Target FPS (default {Math.round(hardware.dwm_refresh_rate)} Hz).
            </p>
          </div>

          <div>
            <h3 style={{fontSize: 13.5, fontWeight: 600, marginBottom: 4}}>
              Capture API: DXGI vs WGC vs GDI
            </h3>
            <p className="fs-card-subtitle">
              DXGI Desktop Duplication delivers lowest frame latency. Use Windows Graphics Capture (WGC) for Windows 11 24H2 borderless games, overlays, or multi-GPU laptops. Keep GDI strictly for legacy software windows.
            </p>
          </div>

          <div>
            <h3 style={{fontSize: 13.5, fontWeight: 600, marginBottom: 4}}>
              Hotkeys and system tray
            </h3>
            <p className="fs-card-subtitle">
              Press {formatHotkeyDisplay(hotkey)} anytime to toggle scaling on the focused window. Enable Minimize to tray / Close to tray in Settings for automatic per-game scaling in the background.
            </p>
          </div>
        </div>
      </section>

      {/* Live Hardware Diagnostics */}
      <section className="fs-card">
        <h2 className="fs-card-title">Hardware diagnostics</h2>
        <div className="fs-divider" />
        <div className="fs-grid-2">
          <div style={{display: 'flex', gap: 10, alignItems: 'flex-start'}}>
            <CpuChipIcon width={16} height={16} style={{color: 'var(--fs-text-secondary)', marginTop: 2, flexShrink: 0}} />
            <div>
              <div className="fs-field-label">Adapters</div>
              <div className="fs-card-subtitle fs-tabular">{adapterSummary}</div>
            </div>
          </div>

          <div style={{display: 'flex', gap: 10, alignItems: 'flex-start'}}>
            <TvIcon width={16} height={16} style={{color: 'var(--fs-text-secondary)', marginTop: 2, flexShrink: 0}} />
            <div>
              <div className="fs-field-label">Displays</div>
              <div className="fs-card-subtitle fs-tabular">{displaySummary}</div>
            </div>
          </div>

          <div style={{display: 'flex', gap: 10, alignItems: 'flex-start'}}>
            <ComputerDesktopIcon width={16} height={16} style={{color: 'var(--fs-text-secondary)', marginTop: 2, flexShrink: 0}} />
            <div>
              <div className="fs-field-label">DWM refresh rate</div>
              <div className="fs-card-subtitle fs-tabular">{Math.round(hardware.dwm_refresh_rate)} Hz</div>
            </div>
          </div>

          <div style={{display: 'flex', gap: 10, alignItems: 'flex-start'}}>
            <KeyIcon width={16} height={16} style={{color: 'var(--fs-text-secondary)', marginTop: 2, flexShrink: 0}} />
            <div>
              <div className="fs-field-label">Windows build</div>
              <div className="fs-card-subtitle fs-tabular">{hardware.windows_build}</div>
            </div>
          </div>

          <div style={{display: 'flex', gap: 10, alignItems: 'flex-start'}}>
            <ShieldCheckIcon width={16} height={16} style={{color: 'var(--fs-text-secondary)', marginTop: 2, flexShrink: 0}} />
            <div>
              <div className="fs-field-label">Elevation</div>
              <div className="fs-card-subtitle">{hardware.is_admin ? 'Administrator (Elevated)' : 'Standard user'}</div>
            </div>
          </div>
        </div>
      </section>

      <div style={{display: 'flex', justifyContent: 'space-between', padding: '0 4px', fontSize: 11.5, color: 'var(--fs-text-muted)', flexWrap: 'wrap', gap: 8}}>
        <span>FlowScale open-source release (MIT / Apache-2.0)</span>
        <span>FlowScale Windows Engine</span>
      </div>
    </div>
  );
}
