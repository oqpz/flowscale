import {useCallback, useEffect, useMemo, useRef, useState} from 'react';
import {ChevronDoubleRightIcon} from '@heroicons/react/24/outline';
import {Theme} from '@astryxdesign/core/theme';
import {ToastViewport, useToast} from '@astryxdesign/core/Toast';
import {AppHeader} from './components/AppHeader';
import {AsciiDuckBackdrop, HonkCornerSignature} from './components/AsciiDuckBackdrop';
import {FlowButton, FlowCrossfade} from './components/ui/FlowControls';
import {GlobalSettingsView} from './components/GlobalSettingsView';
import {ConfigsView} from './components/ConfigsView';
import {ManualAboutView} from './components/ManualAboutView';
import {ProfileEditor} from './components/ProfileEditor';
import {ProfileSidebar} from './components/ProfileSidebar';
import {SessionLogModal} from './components/SessionLogModal';
import {createSessionLog, recordTick} from './sessionLog.js';
import {defaultThemeSlug, themes} from '../astryx-themes.js';
import {createIpcClient, createMockInitialState, type FlowScaleIpcClient} from './ipc';
import {acknowledgeProfileDraft, mergePendingProfiles, takePendingProfiles} from './profileDrafts.js';
import {
  type AppState,
  type EngineConfig,
  type GlobalSettings,
  type HotkeyBinding,
  type NavSection,
  type ConfigAction,
  type ConfigSummary,
  type ScalingProfile,
  type TelemetryState,
  type ThemeMode,
  type UiCommand,
  type WindowsGpuPreference,
} from './types';

const PROFILE_UPSERT_DEBOUNCE_MS = 180;

const NAV_ORDER: Record<NavSection, number> = {
  profiles: 0,
  settings: 1,
  manual: 2,
  configs: 3,
};

function isSameTelemetry(a: TelemetryState, b: TelemetryState): boolean {
  return (
    a.status === b.status &&
    a.countdown_seconds === b.countdown_seconds &&
    a.target_title === b.target_title &&
    a.foreground_title === b.foreground_title &&
    a.input_width === b.input_width &&
    a.input_height === b.input_height &&
    a.output_width === b.output_width &&
    a.output_height === b.output_height &&
    a.scale_factor === b.scale_factor &&
    a.captured_fps === b.captured_fps &&
    a.presented_fps === b.presented_fps &&
    a.processing_backend === b.processing_backend &&
    a.cpu_ms === b.cpu_ms &&
    a.processing_status === b.processing_status &&
    a.error_message === b.error_message
  );
}

function FlowScaleShell({onThemeModeSync}: {onThemeModeSync: (mode: ThemeMode) => void}) {
  const toast = useToast();

  const ipcRef = useRef<FlowScaleIpcClient | null>(null);
  const sendCommand = useCallback((command: UiCommand) => {
    ipcRef.current?.sendCommand(command);
  }, []);
  const pendingProfileUpsertsRef = useRef(
    new Map<string, {timerId: number; profile: ScalingProfile}>(),
  );
  const inFlightProfileUpsertsRef = useRef(
    new Map<string, {revision: string; profile: ScalingProfile}>(),
  );
  const nextProfileRevisionRef = useRef(0);
  const sendProfileUpsert = useCallback((profile: ScalingProfile) => {
    const revision = String(++nextProfileRevisionRef.current);
    inFlightProfileUpsertsRef.current.set(profile.id, {revision, profile});
    sendCommand({type: 'upsert_profile', payload: {...profile, client_revision: revision}});
  }, [sendCommand]);
  const flushPendingProfileUpserts = useCallback(() => {
    const pending = pendingProfileUpsertsRef.current;
    if (pending.size === 0) return;
    const entries = takePendingProfiles(pending);
    for (const {timerId, profile} of entries) {
      window.clearTimeout(timerId);
      sendProfileUpsert(profile);
    }
  }, [sendProfileUpsert]);

  const [state, setState] = useState<AppState>(() => createMockInitialState());
  const [telemetry, setTelemetry] = useState<TelemetryState>(() => ({status: 'idle'}));
  // Session log is fed every tick but kept out of React state so ticks never re-render the app.
  const sessionLogRef = useRef(createSessionLog());
  const [isLogOpen, setLogOpen] = useState(false);
  const stateRef = useRef(state);
  stateRef.current = state;
  const runningProfileIdRef = useRef<string | null>(null);
  const runningProfile = useCallback((t?: TelemetryState): ScalingProfile => {
    if (t?.engine?.active_profile_id) runningProfileIdRef.current = t.engine.active_profile_id;
    const {profiles, settings} = stateRef.current;
    return (
      profiles.find((p) => p.id === runningProfileIdRef.current) ??
      profiles.find((p) => p.id === settings.selected_profile_id) ??
      profiles[0]
    );
  }, []);
  const [activeNav, setActiveNav] = useState<NavSection>('profiles');
  const [navDirection, setNavDirection] = useState<'none' | 'left' | 'right'>('none');
  const [isSidebarCollapsed, setIsSidebarCollapsed] = useState<boolean>(() => {
    try {
      return window.localStorage.getItem('flowscale.profilesSidebarCollapsed') === 'true';
    } catch {
      return false;
    }
  });
  const toggleSidebarCollapsed = useCallback((collapsed: boolean) => {
    setIsSidebarCollapsed(collapsed);
    try {
      window.localStorage.setItem('flowscale.profilesSidebarCollapsed', collapsed ? 'true' : 'false');
    } catch {
      // Ignore storage access failures inside restricted webviews.
    }
  }, []);
  const [configs, setConfigs] = useState<ConfigSummary[]>([]);
  const [configsBusy, setConfigsBusy] = useState(false);
  const configPending = useRef(false);
  const runConfigAction = useCallback((action: ConfigAction) => {
    if (configPending.current) return;
    configPending.current = true;
    setConfigsBusy(true);
    if (action.action === 'load' || action.action === 'export') flushPendingProfileUpserts();
    sendCommand({type: 'configs', payload: action});
  }, [flushPendingProfileUpserts, sendCommand]);
  const [manualCountdownSeconds, setManualCountdownSeconds] = useState<number | null>(null);
  const [pickedExecutablePath, setPickedExecutablePath] = useState<string | null>(null);
  const [externalEditNonce, setExternalEditNonce] = useState(0);

  useEffect(() => {
    onThemeModeSync(state.settings.theme);
  }, [state.settings.theme, onThemeModeSync]);

  useEffect(() => {
    const client = createIpcClient();
    ipcRef.current = client;
    const unsubscribe = client.subscribe((event) => {
      switch (event.type) {
        case 'state_sync': {
          setState((prev) => ({
            ...prev,
            settings: event.payload.settings,
            profiles:
              event.payload.profiles.length > 0
                ? mergePendingProfiles(
                    event.payload.profiles,
                    pendingProfileUpsertsRef.current,
                    inFlightProfileUpsertsRef.current,
                  )
                : prev.profiles,
            hardware: {
              ...event.payload.hardware,
              is_admin: event.payload.is_admin ?? event.payload.hardware.is_admin,
              windows_build:
                event.payload.windows_build ?? event.payload.hardware.windows_build,
            },
            engine_config: event.payload.engine_config,
          }));
          setTelemetry((prev) =>
            isSameTelemetry(prev, event.payload.telemetry) ? prev : event.payload.telemetry,
          );
          break;
        }
        case 'telemetry_tick': {
          if (event.payload.status !== 'idle' && event.payload.status !== 'countdown') {
            setManualCountdownSeconds(null);
          }
          recordTick(sessionLogRef.current, event.payload, runningProfile(event.payload), performance.now());
          setTelemetry((prev) => (isSameTelemetry(prev, event.payload) ? prev : event.payload));
          break;
        }
        case 'countdown_tick': {
          if (event.payload.seconds > 0) {
            setTelemetry((prev) =>
              prev.status === 'countdown' && prev.countdown_seconds === event.payload.seconds
                ? prev
                : {
                    ...prev,
                    status: 'countdown',
                    countdown_seconds: event.payload.seconds,
                  },
            );
          }
          break;
        }
        case 'configs_result': {
          if (event.payload.entries !== null) setConfigs(event.payload.entries);
          configPending.current = false;
          setConfigsBusy(false);
          break;
        }
        case 'picked_executable': {
          setPickedExecutablePath(event.payload.path);
          break;
        }
        case 'profile_upserted': {
          if (!acknowledgeProfileDraft(
            inFlightProfileUpsertsRef.current,
            event.payload.id,
            event.payload.revision,
          )) break;
          if (!event.payload.ok || !event.payload.profile) {
            toast({body: event.payload.message ?? 'Profile save failed.', type: 'error'});
            client.sendCommand({type: 'get_initial_state', payload: {}});
            break;
          }
          const echoed = event.payload.profile;
          setState((prev) => {
            const profiles = prev.profiles.some((profile) => profile.id === echoed.id)
              ? prev.profiles.map((profile) => profile.id === echoed.id ? echoed : profile)
              : [...prev.profiles, echoed];
            return {
              ...prev,
              profiles: mergePendingProfiles(
                profiles,
                pendingProfileUpsertsRef.current,
                inFlightProfileUpsertsRef.current,
              ),
            };
          });
          break;
        }
        case 'toast': {
          toast({
            body: event.payload.message,
            type: event.payload.kind === 'error' ? 'error' : 'info',
          });
          break;
        }
      }
    });

    client.sendCommand({type: 'get_initial_state', payload: {}});

    return () => {
      flushPendingProfileUpserts();
      unsubscribe();
      client.dispose();
      if (ipcRef.current === client) {
        ipcRef.current = null;
      }
    };
  }, [flushPendingProfileUpserts, toast]);

  useEffect(() => {
    if (manualCountdownSeconds === null) return;
    if (manualCountdownSeconds <= 0) {
      setManualCountdownSeconds(null);
      flushPendingProfileUpserts();
      sendCommand({type: 'activate_scaling', payload: {}});
      return;
    }

    const timer = window.setTimeout(() => {
      setManualCountdownSeconds((prev) => (prev === null ? null : prev - 1));
    }, 1000);

    return () => {
      window.clearTimeout(timer);
    };
  }, [flushPendingProfileUpserts, manualCountdownSeconds, sendCommand]);

  const activeProfile = useMemo(() => {
    return (
      state.profiles.find((p) => p.id === state.settings.selected_profile_id) ??
      state.profiles[0]
    );
  }, [state.profiles, state.settings.selected_profile_id]);

  const handleSelectProfile = useCallback(
    (id: string) => {
      flushPendingProfileUpserts();
      setState((prev) => ({
        ...prev,
        settings: {...prev.settings, selected_profile_id: id},
      }));
      sendCommand({type: 'select_profile', payload: {id}});
      if (activeNav !== 'profiles') {
        setActiveNav('profiles');
      }
    },
    [activeNav, flushPendingProfileUpserts, sendCommand],
  );

  const handleUpsertProfile = useCallback(
    (updated: ScalingProfile, selectAfter = false) => {
      setState((prev) => {
        const exists = prev.profiles.some((p) => p.id === updated.id);
        const profiles = exists
          ? prev.profiles.map((p) => (p.id === updated.id ? updated : p))
          : [...prev.profiles, updated];
        return {
          ...prev,
          profiles,
          settings: selectAfter
            ? {...prev.settings, selected_profile_id: updated.id}
            : prev.settings,
        };
      });

      const pending = pendingProfileUpsertsRef.current;
      const existing = pending.get(updated.id);
      if (existing) {
        window.clearTimeout(existing.timerId);
        pending.delete(updated.id);
      }

      if (selectAfter) {
        flushPendingProfileUpserts();
        sendProfileUpsert(updated);
        sendCommand({type: 'select_profile', payload: {id: updated.id}});
        return;
      }

      const timerId = window.setTimeout(() => {
        const entry = pending.get(updated.id);
        if (entry && entry.timerId === timerId) {
          pending.delete(updated.id);
          sendProfileUpsert(entry.profile);
        }
      }, PROFILE_UPSERT_DEBOUNCE_MS);
      pending.set(updated.id, {timerId, profile: updated});
    },
    [flushPendingProfileUpserts, sendCommand, sendProfileUpsert],
  );

  const handleDuplicateProfile = useCallback(
    (id: string) => {
      flushPendingProfileUpserts();
      sendCommand({type: 'duplicate_profile', payload: {id}});
    },
    [flushPendingProfileUpserts, sendCommand],
  );

  const handleDeleteProfile = useCallback(
    (id: string) => {
      if (id === 'default') return;
      flushPendingProfileUpserts();
      inFlightProfileUpsertsRef.current.delete(id);
      setState((prev) => {
        const profiles = prev.profiles.filter((p) => p.id !== id);
        const selected =
          prev.settings.selected_profile_id === id
            ? 'default'
            : prev.settings.selected_profile_id;
        return {
          ...prev,
          profiles,
          settings: {...prev.settings, selected_profile_id: selected},
        };
      });
      sendCommand({type: 'delete_profile', payload: {id}});
    },
    [flushPendingProfileUpserts, sendCommand],
  );

  const handleSaveSettings = useCallback(
    (updated: GlobalSettings) => {
      setState((prev) => ({...prev, settings: updated}));
      sendCommand({type: 'save_settings', payload: updated});
    },
    [sendCommand],
  );

  const handleSetHotkey = useCallback(
    (hotkey: HotkeyBinding) => {
      setState((prev) => ({
        ...prev,
        settings: {...prev.settings, hotkey},
      }));
      sendCommand({type: 'set_hotkey', payload: hotkey});
    },
    [sendCommand],
  );

  const handleSetWindowsGpuPreference = useCallback(
    (preference: WindowsGpuPreference) => {
      setState((prev) => ({
        ...prev,
        settings: {...prev.settings, windows_gpu_preference: preference},
      }));
      sendCommand({type: 'set_windows_gpu_preference', payload: {preference}});
    },
    [sendCommand],
  );

  const handleSaveEngineConfig = useCallback(
    (config: EngineConfig) => {
      setState((prev) => ({...prev, engine_config: config}));
      sendCommand({type: 'save_engine_config', payload: config});
    },
    [sendCommand],
  );

  const handleClearPickedExecutable = useCallback(() => {
    setPickedExecutablePath(null);
  }, []);

  const handlePickExecutable = useCallback(() => {
    sendCommand({type: 'pick_executable', payload: {}});
  }, [sendCommand]);

  const handleEditorChange = useCallback(
    (updated: ScalingProfile) => {
      handleUpsertProfile(updated);
    },
    [handleUpsertProfile],
  );

  const handleEditMeta = useCallback(() => {
    setExternalEditNonce((n) => n + 1);
  }, []);

  const handleRelaunchAsAdmin = useCallback(() => {
    sendCommand({type: 'relaunch_as_admin', payload: {}});
  }, [sendCommand]);

  const handleImportLosslessXml = useCallback(() => {
    flushPendingProfileUpserts();
    inFlightProfileUpsertsRef.current.clear();
    sendCommand({type: 'import_lossless_xml', payload: {}});
  }, [flushPendingProfileUpserts, sendCommand]);

  const handleResetEngineConfig = useCallback(() => {
    sendCommand({type: 'reset_engine_config', payload: {}});
  }, [sendCommand]);

  const sidebarNode = useMemo(
    () => (
      <ProfileSidebar
        profiles={state.profiles}
        selectedProfileId={activeProfile.id}
        dwmRefreshRate={state.hardware.dwm_refresh_rate}
        pickedExecutablePath={pickedExecutablePath}
        onClearPickedExecutable={handleClearPickedExecutable}
        externalEditNonce={externalEditNonce}
        onSelectProfile={handleSelectProfile}
        onUpsertProfile={handleUpsertProfile}
        onDuplicateProfile={handleDuplicateProfile}
        onDeleteProfile={handleDeleteProfile}
        onPickExecutable={handlePickExecutable}
        onCollapseSidebar={() => toggleSidebarCollapsed(true)}
      />
    ),
    [
      state.profiles,
      activeProfile.id,
      state.hardware.dwm_refresh_rate,
      pickedExecutablePath,
      handleClearPickedExecutable,
      externalEditNonce,
      handleSelectProfile,
      handleUpsertProfile,
      handleDuplicateProfile,
      handleDeleteProfile,
      handlePickExecutable,
      toggleSidebarCollapsed,
    ],
  );

  const profileEditorNode = useMemo(
    () => (
      <ProfileEditor
        profile={activeProfile}
        hardware={state.hardware}
        onChange={handleEditorChange}
        onEditMeta={handleEditMeta}
      />
    ),
    [activeProfile, state.hardware, handleEditorChange, handleEditMeta],
  );

  const settingsNode = useMemo(
    () => (
      <GlobalSettingsView
        settings={state.settings}
        engineConfig={state.engine_config}
        isAdmin={state.hardware.is_admin}
        onSaveSettings={handleSaveSettings}
        onSetHotkey={handleSetHotkey}
        onSetWindowsGpuPreference={handleSetWindowsGpuPreference}
        onRelaunchAsAdmin={handleRelaunchAsAdmin}
        onImportLosslessXml={handleImportLosslessXml}
        onSaveEngineConfig={handleSaveEngineConfig}
        onResetEngineConfig={handleResetEngineConfig}
      />
    ),
    [
      state.settings,
      state.engine_config,
      state.hardware.is_admin,
      handleSaveSettings,
      handleSetHotkey,
      handleSetWindowsGpuPreference,
      handleRelaunchAsAdmin,
      handleImportLosslessXml,
      handleSaveEngineConfig,
      handleResetEngineConfig,
    ],
  );

  const manualNode = useMemo(
    () => <ManualAboutView hardware={state.hardware} hotkey={state.settings.hotkey} />,
    [state.hardware, state.settings.hotkey],
  );

  return (
    <div className="fs-shell">
      <div className="fs-foreground">
        <AppHeader
          activeNav={activeNav}
          onNavChange={(nav) => {
            if (configPending.current) return;
            if (nav !== activeNav) {
              setNavDirection(NAV_ORDER[nav] > NAV_ORDER[activeNav] ? 'right' : 'left');
            }
            setActiveNav(nav);
            if (nav === 'configs') runConfigAction({action: 'list'});
          }}
          activeProfile={activeProfile}
          telemetry={telemetry}
          countdownSeconds={manualCountdownSeconds}
          hotkey={state.settings.hotkey}
          onStartCountdown={() => setManualCountdownSeconds(5)}
          onCancelCountdown={() => {
            setManualCountdownSeconds(null);
            if (telemetry.status === 'countdown') {
              sendCommand({type: 'cancel_countdown', payload: {}});
            }
          }}
          onStopScaling={() => {
            setManualCountdownSeconds(null);
            sendCommand({type: 'stop_scaling', payload: {}});
          }}
          onOpenLog={() => setLogOpen(true)}
        />
        <SessionLogModal
          isOpen={isLogOpen}
          onClose={() => setLogOpen(false)}
          log={sessionLogRef.current}
          getProfile={runningProfile}
          onReset={() => Object.assign(sessionLogRef.current, createSessionLog())}
        />

        <div className="fs-main">
          {activeNav === 'profiles' && (
            <aside
              className="fs-sidebar"
              aria-label="Scaling profiles"
              data-collapsed={isSidebarCollapsed ? 'true' : 'false'}
              aria-hidden={isSidebarCollapsed || undefined}
            >
              {sidebarNode}
            </aside>
          )}

          <div className="fs-stage">
            <AsciiDuckBackdrop />
            <main className="fs-content-scroll">
              <div className="fs-content-inner">
                {activeNav === 'profiles' && isSidebarCollapsed && (
                  <div className="fs-profiles-reopen-row">
                    <FlowButton
                      label={`Show profiles · ${activeProfile.title}`}
                      variant="secondary"
                      size="sm"
                      ariaLabel="Show profiles sidebar"
                      icon={<ChevronDoubleRightIcon width={14} height={14} />}
                      onClick={() => toggleSidebarCollapsed(false)}
                    />
                  </div>
                )}
                <FlowCrossfade activeKey={activeNav} direction={navDirection}>
                  {activeNav === 'profiles' && profileEditorNode}
                  {activeNav === 'settings' && settingsNode}
                  {activeNav === 'manual' && manualNode}
                  {activeNav === 'configs' && (
                    <ConfigsView
                      entries={configs}
                      profile={activeProfile}
                      busy={configsBusy}
                      onAction={runConfigAction}
                      onError={(message) => toast({body: message, type: 'error'})}
                    />
                  )}
                </FlowCrossfade>
              </div>
            </main>
          </div>
        </div>
      </div>
      <HonkCornerSignature />
    </div>
  );
}

export function App() {
  const [themeMode, setThemeMode] = useState<ThemeMode>('system');

  const handleThemeModeSync = useCallback((nextMode: ThemeMode) => {
    setThemeMode((prev) => {
      if (prev === nextMode) return prev;
      const root = document.documentElement;
      root.classList.add('flowscale-theme-switching');
      if (nextMode === 'system') {
        root.removeAttribute('data-theme');
      } else {
        root.setAttribute('data-theme', nextMode);
      }
      window.requestAnimationFrame(() => {
        window.requestAnimationFrame(() => {
          root.classList.remove('flowscale-theme-switching');
        });
      });
      return nextMode;
    });
  }, []);

  return (
    <Theme theme={themes[defaultThemeSlug]} mode={themeMode}>
      <ToastViewport position="bottomEnd">
        <FlowScaleShell onThemeModeSync={handleThemeModeSync} />
      </ToastViewport>
    </Theme>
  );
}

export default App;
