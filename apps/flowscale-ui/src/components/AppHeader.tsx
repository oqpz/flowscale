import {useEffect} from 'react';
import {DocumentTextIcon, PlayIcon, StopIcon, XMarkIcon} from '@heroicons/react/24/outline';
import {useAnnounce} from '@astryxdesign/core/hooks';
import {
  formatHotkeyDisplay,
  type HotkeyBinding,
  type NavSection,
  type ScalingProfile,
  type TelemetryState,
} from '../types';
import {FlowButton} from './ui/FlowControls';
import {useSlidingPill} from './ui/useSlidingPill';

export interface AppHeaderProps {
  activeNav: NavSection;
  onNavChange: (nav: NavSection) => void;
  activeProfile: ScalingProfile;
  telemetry: TelemetryState;
  countdownSeconds: number | null;
  hotkey: HotkeyBinding;
  onStartCountdown: () => void;
  onCancelCountdown: () => void;
  onStopScaling: () => void;
  onOpenLog: () => void;
}

const NAV_TABS: Array<{id: NavSection; label: string}> = [
  {id: 'profiles', label: 'Profiles'},
  {id: 'settings', label: 'Settings'},
  {id: 'manual', label: 'Information'},
  {id: 'configs', label: 'Config'},
];

function getTelemetryDot(
  telemetry: TelemetryState,
  countdownSeconds: number | null,
): {variant: 'neutral' | 'accent' | 'success' | 'danger'; label: string} {
  if (countdownSeconds !== null || telemetry.status === 'countdown') {
    return {variant: 'accent', label: 'Countdown'};
  }
  switch (telemetry.status) {
    case 'active':
      return {variant: 'success', label: 'Active'};
    case 'paused':
      return {variant: 'neutral', label: 'Paused'};
    case 'error':
      return {variant: 'danger', label: 'Error'};
    default:
      return {variant: 'neutral', label: 'Idle'};
  }
}

export function AppHeader({
  activeNav,
  onNavChange,
  activeProfile,
  telemetry,
  countdownSeconds,
  hotkey,
  onStartCountdown,
  onCancelCountdown,
  onStopScaling,
  onOpenLog,
}: AppHeaderProps) {
  const announce = useAnnounce();
  const navRef = useSlidingPill<HTMLElement>(activeNav);
  const isCountingDown = countdownSeconds !== null || telemetry.status === 'countdown';
  const remainingSeconds = countdownSeconds ?? telemetry.countdown_seconds ?? 5;
  const dot = getTelemetryDot(telemetry, countdownSeconds);
  const kbdKeys = formatHotkeyDisplay(hotkey).split(' + ');

  useEffect(() => {
    if (telemetry.status === 'active') {
      announce(`Scaling active for ${telemetry.target_title ?? 'target window'}`);
    } else if (telemetry.status === 'paused') {
      announce('Scaling paused while target window is unfocused');
    } else if (telemetry.status === 'error') {
      announce(`Scaling failed: ${telemetry.error_message || 'engine error'}`, 'assertive');
    }
  }, [telemetry.status, telemetry.target_title, telemetry.error_message, announce]);

  const hasResolution =
    telemetry.input_width &&
    telemetry.input_height &&
    telemetry.output_width &&
    telemetry.output_height;

  return (
    <header style={{width: '100%', flexShrink: 0}}>
      <div className="fs-header">
        <div className="fs-brand">
          <img
            src="./flowscale-duck.png"
            alt="FlowScale"
            className="fs-brand-duck"
            width={32}
            height={32}
          />
          <div style={{minWidth: 0}}>
            <div className="fs-brand-title">Flow Scale</div>
            <div className="fs-brand-profile" title={activeProfile.title}>
              {activeProfile.title}
            </div>
          </div>
        </div>

        <nav ref={navRef} className="fs-nav-pills" role="tablist" aria-label="Primary sections">
          <span className="fs-slide-pill" aria-hidden="true" />
          {NAV_TABS.map((tab) => (
            <button
              key={tab.id}
              type="button"
              role="tab"
              aria-selected={activeNav === tab.id}
              data-text={tab.label}
              className="fs-nav-pill"
              onClick={() => onNavChange(tab.id)}
            >
              <span className="fs-nav-pill-label">{tab.label}</span>
            </button>
          ))}
        </nav>

        <div className="fs-header-actions">
          <div className="fs-status-chip" title="Engine status">
            <span className="fs-dot" data-variant={dot.variant} />
            {isCountingDown ? (
              <span>Scaling in {remainingSeconds}s</span>
            ) : telemetry.status === 'active' ? (
              <span>
                {telemetry.target_title ?? 'Target'}
                {hasResolution
                  ? ` · ${telemetry.input_width}×${telemetry.input_height} → ${telemetry.output_width}×${telemetry.output_height}`
                  : ''}
                {` · ${telemetry.captured_fps ?? '--'}/${telemetry.presented_fps ?? '--'} FPS`}
              </span>
            ) : telemetry.status === 'paused' ? (
              <span>Paused ({telemetry.foreground_title || 'unfocused'})</span>
            ) : telemetry.status === 'error' ? (
              <span style={{color: 'var(--fs-danger)'}}>Error</span>
            ) : (
              <span>Idle</span>
            )}
          </div>

          <FlowButton
            label="Log"
            variant="secondary"
            size="sm"
            title="Session log: frame rate, frames lost, generation time and active settings"
            icon={<DocumentTextIcon width={15} height={15} />}
            onClick={onOpenLog}
          />

          <div className="fs-kbd-group" aria-label={`Hotkey ${hotkey.display}`}>
            {kbdKeys.map((k) => (
              <kbd key={k} className="fs-kbd">
                {k}
              </kbd>
            ))}
          </div>

          {isCountingDown ? (
            <FlowButton
              label="Cancel"
              variant="secondary"
              size="sm"
              icon={<XMarkIcon width={15} height={15} />}
              onClick={onCancelCountdown}
            />
          ) : telemetry.status === 'active' || telemetry.status === 'paused' ? (
            <FlowButton
              label="Stop scaling"
              variant="danger"
              size="sm"
              icon={<StopIcon width={15} height={15} />}
              onClick={onStopScaling}
            />
          ) : (
            <FlowButton
              label="Scale"
              variant="primary"
              size="sm"
              icon={<PlayIcon width={15} height={15} />}
              onClick={onStartCountdown}
            />
          )}
        </div>
      </div>

      {isCountingDown && (
        <div className="fs-countdown-banner" role="status">
          <span>
            <strong>Scaling starts in {remainingSeconds}s</strong> — click into your windowed or borderless game window now.
          </span>
          <FlowButton label="Cancel" variant="secondary" size="sm" onClick={onCancelCountdown} />
        </div>
      )}

      {telemetry.status === 'error' && (
        <div
          role="alert"
          style={{
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'space-between',
            gap: 12,
            padding: '8px 20px',
            backgroundColor: 'var(--fs-danger-soft)',
            borderBottom: '1px solid rgba(239, 68, 68, 0.35)',
            fontSize: 12.5,
          }}
        >
          <span>
            <strong>Scaling failed:</strong>{' '}
            {telemetry.error_message || 'Could not capture or present target window'}
          </span>
          <FlowButton label="Retry" variant="secondary" size="sm" onClick={onStartCountdown} />
        </div>
      )}
    </header>
  );
}
