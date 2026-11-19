import {type CSSProperties, useEffect, useMemo, useState} from 'react';
import {
  ChevronDoubleLeftIcon,
  DocumentDuplicateIcon,
  FolderOpenIcon,
  MagnifyingGlassIcon,
  PencilSquareIcon,
  PlusIcon,
  TrashIcon,
} from '@heroicons/react/24/outline';
import {clampNumber, createDefaultProfile, type ScalingProfile} from '../types';
import {isWebView2Available} from '../ipc';
import {FlowButton, FlowModal, FlowNumber, FlowSwitch} from './ui/FlowControls';

export interface ProfileSidebarProps {
  profiles: ScalingProfile[];
  selectedProfileId: string;
  dwmRefreshRate: number;
  pickedExecutablePath: string | null;
  onClearPickedExecutable: () => void;
  externalEditNonce: number;
  onSelectProfile: (id: string) => void;
  onUpsertProfile: (profile: ScalingProfile, selectAfter?: boolean) => void;
  onDuplicateProfile: (id: string) => void;
  onDeleteProfile: (id: string) => void;
  onPickExecutable: () => void;
  onCollapseSidebar?: () => void;
}

interface ProfileFormDraft {
  mode: 'add' | 'edit';
  targetId: string | null;
  title: string;
  process_pattern: string;
  auto_scale: boolean;
  auto_scale_delay_sec: number;
}

function extractStemTitle(filePath: string): string {
  const fileName = filePath.split(/[\\/]/).pop() ?? '';
  return fileName.replace(/\.[^.]+$/, '');
}

export function ProfileSidebar({
  profiles,
  selectedProfileId,
  dwmRefreshRate,
  pickedExecutablePath,
  onClearPickedExecutable,
  externalEditNonce,
  onSelectProfile,
  onUpsertProfile,
  onDuplicateProfile,
  onDeleteProfile,
  onPickExecutable,
  onCollapseSidebar,
}: ProfileSidebarProps) {
  const [searchQuery, setSearchQuery] = useState('');
  const [dialogOpen, setDialogOpen] = useState(false);
  const [deleteDialogOpen, setDeleteDialogOpen] = useState(false);
  const [titleError, setTitleError] = useState<string | null>(null);
  const [shakeKey, setShakeKey] = useState(0);
  const canBrowseExe = isWebView2Available();
  const [draft, setDraft] = useState<ProfileFormDraft>({
    mode: 'add',
    targetId: null,
    title: '',
    process_pattern: '',
    auto_scale: false,
    auto_scale_delay_sec: 5,
  });

  const selectedProfile = useMemo(
    () => profiles.find((p) => p.id === selectedProfileId) ?? profiles[0],
    [profiles, selectedProfileId],
  );

  const defaultProfile = useMemo(
    () => profiles.find((p) => p.is_default || p.id === 'default') ?? profiles[0],
    [profiles],
  );

  const customProfiles = useMemo(() => {
    const query = searchQuery.trim().toLowerCase();
    return profiles
      .filter((p) => !p.is_default && p.id !== 'default')
      .filter((p) => {
        if (!query) return true;
        return (
          p.title.toLowerCase().includes(query) ||
          p.process_pattern.toLowerCase().includes(query)
        );
      });
  }, [profiles, searchQuery]);

  const showPinnedDefault = useMemo(() => {
    if (!defaultProfile) return false;
    const query = searchQuery.trim().toLowerCase();
    if (!query) return true;
    return defaultProfile.title.toLowerCase().includes(query);
  }, [defaultProfile, searchQuery]);

  const openAddDialog = () => {
    setTitleError(null);
    setDraft({
      mode: 'add',
      targetId: null,
      title: '',
      process_pattern: '',
      auto_scale: false,
      auto_scale_delay_sec: 5,
    });
    setDialogOpen(true);
  };

  const openEditDialog = (profile: ScalingProfile) => {
    if (profile.is_default || profile.id === 'default') return;
    setTitleError(null);
    setDraft({
      mode: 'edit',
      targetId: profile.id,
      title: profile.title,
      process_pattern: profile.process_pattern,
      auto_scale: profile.auto_scale,
      auto_scale_delay_sec: profile.auto_scale_delay_sec,
    });
    setDialogOpen(true);
  };

  useEffect(() => {
    if (externalEditNonce > 0 && selectedProfile && !selectedProfile.is_default) {
      openEditDialog(selectedProfile);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [externalEditNonce]);

  useEffect(() => {
    if (!dialogOpen || !pickedExecutablePath) return;
    setDraft((prev) => ({
      ...prev,
      process_pattern: pickedExecutablePath,
      title: prev.title.trim() ? prev.title : extractStemTitle(pickedExecutablePath),
    }));
    onClearPickedExecutable();
  }, [pickedExecutablePath, dialogOpen, onClearPickedExecutable]);

  const handleSaveDraft = () => {
    const cleanTitle = draft.title.trim();
    if (!cleanTitle) {
      setTitleError('Enter a profile title');
      setShakeKey((k) => k + 1);
      return;
    }

    const delay = clampNumber(Math.round(draft.auto_scale_delay_sec), 0, 60);
    if (draft.mode === 'add') {
      const slug = cleanTitle
        .toLowerCase()
        .replace(/[^a-z0-9]+/g, '-')
        .replace(/(^-|-$)/g, '');
      const baseSeed = defaultProfile ?? createDefaultProfile(dwmRefreshRate);
      const nextProfile: ScalingProfile = {
        ...baseSeed,
        id: `${slug || 'profile'}-${Date.now().toString(36)}`,
        title: cleanTitle,
        is_default: false,
        process_pattern: draft.process_pattern.trim(),
        auto_scale: draft.auto_scale,
        auto_scale_delay_sec: delay,
      };
      onUpsertProfile(nextProfile, true);
    } else if (draft.targetId) {
      const existing = profiles.find((p) => p.id === draft.targetId);
      if (!existing) return;
      onUpsertProfile({
        ...existing,
        title: cleanTitle,
        process_pattern: draft.process_pattern.trim(),
        auto_scale: draft.auto_scale,
        auto_scale_delay_sec: delay,
      });
    }

    setDialogOpen(false);
  };

  const canModifySelected =
    Boolean(selectedProfile) && !selectedProfile.is_default && selectedProfile.id !== 'default';

  return (
    <div style={{display: 'flex', flexDirection: 'column', gap: 12, height: '100%'}}>
      <div style={{display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 6}}>
        <span style={{fontSize: 12, fontWeight: 600, color: 'var(--fs-text-secondary)', letterSpacing: '0.02em'}}>
          Profiles
        </span>
        <div style={{display: 'flex', alignItems: 'center', gap: 4}}>
          <FlowButton
            label="Add"
            variant="secondary"
            size="sm"
            icon={<PlusIcon width={14} height={14} />}
            onClick={openAddDialog}
          />
          <FlowButton
            variant="ghost"
            ariaLabel="Edit selected profile"
            title={canModifySelected ? 'Edit profile' : 'Default profile metadata is fixed'}
            disabled={!canModifySelected}
            icon={<PencilSquareIcon width={15} height={15} />}
            onClick={() => selectedProfile && openEditDialog(selectedProfile)}
          />
          <FlowButton
            variant="ghost"
            ariaLabel="Duplicate selected profile"
            title="Duplicate profile"
            disabled={!selectedProfile}
            icon={<DocumentDuplicateIcon width={15} height={15} />}
            onClick={() => selectedProfile && onDuplicateProfile(selectedProfile.id)}
          />
          <FlowButton
            variant="ghost"
            ariaLabel="Delete selected profile"
            title={canModifySelected ? 'Delete profile' : 'Default profile cannot be deleted'}
            disabled={!canModifySelected}
            icon={<TrashIcon width={15} height={15} />}
            onClick={() => setDeleteDialogOpen(true)}
          />
          {onCollapseSidebar && (
            <FlowButton
              variant="ghost"
              ariaLabel="Hide profiles sidebar"
              title="Hide profiles"
              icon={<ChevronDoubleLeftIcon width={15} height={15} />}
              onClick={onCollapseSidebar}
            />
          )}
        </div>
      </div>

      <div className="fs-input-wrap">
        <MagnifyingGlassIcon
          width={14}
          height={14}
          style={{position: 'absolute', left: 10, color: 'var(--fs-text-muted)', pointerEvents: 'none'}}
        />
        <input
          type="search"
          aria-label="Search profiles"
          placeholder="Search profiles..."
          value={searchQuery}
          onChange={(e) => setSearchQuery(e.target.value)}
          className="fs-input"
          style={{paddingLeft: 30}}
        />
      </div>

      <div className="fs-profile-list" style={{flex: 1, overflowY: 'auto'}}>
        {showPinnedDefault && defaultProfile && (
          <button
            type="button"
            className="fs-profile-item"
            aria-selected={selectedProfile?.id === defaultProfile.id}
            onClick={() => onSelectProfile(defaultProfile.id)}
          >
            <span
              className="fs-dot"
              data-variant={defaultProfile.auto_scale ? 'success' : 'neutral'}
            />
            <div className="fs-profile-item-meta">
              <div className="fs-profile-item-name">{defaultProfile.title}</div>
              <div className="fs-profile-item-sub">
                {defaultProfile.auto_scale ? 'Auto-scale enabled' : 'Global fallback'}
              </div>
            </div>
          </button>
        )}

        {showPinnedDefault && customProfiles.length > 0 && <div className="fs-divider" style={{margin: '6px 0'}} />}

        {customProfiles.length > 0 ? (
          customProfiles.map((profile, idx) => (
            <button
              key={profile.id}
              type="button"
              className="fs-profile-item fs-stagger-row"
              style={{'--stagger-index': idx} as CSSProperties}
              aria-selected={selectedProfile?.id === profile.id}
              onClick={() => onSelectProfile(profile.id)}
            >
              <span
                className="fs-dot"
                data-variant={profile.auto_scale ? 'success' : 'neutral'}
              />
              <div className="fs-profile-item-meta">
                <div className="fs-profile-item-name">{profile.title}</div>
                <div className="fs-profile-item-sub">
                  {profile.process_pattern || 'Unbound window pattern'}
                </div>
              </div>
            </button>
          ))
        ) : (
          !showPinnedDefault && (
            <div style={{padding: '20px 8px', textAlign: 'center', fontSize: 12.5, color: 'var(--fs-text-secondary)'}}>
              No matching profiles
            </div>
          )
        )}
      </div>

      <FlowModal
        isOpen={dialogOpen}
        title={draft.mode === 'add' ? 'New profile' : 'Edit profile'}
        subtitle="Match a Windows executable or window title pattern"
        onClose={() => setDialogOpen(false)}
        footer={
          <>
            <FlowButton label="Cancel" variant="ghost" onClick={() => setDialogOpen(false)} />
            <FlowButton
              label={draft.mode === 'add' ? 'Create profile' : 'Save changes'}
              variant="primary"
              onClick={handleSaveDraft}
            />
          </>
        }
      >
        <div key={titleError ? `err-${shakeKey}` : 'ok'} className={`fs-field ${titleError ? 'fs-shake-error' : ''}`}>
          <label className="fs-field-label" htmlFor="fs-profile-title">
            Title *
          </label>
          <input
            id="fs-profile-title"
            className="fs-input"
            aria-invalid={titleError ? 'true' : undefined}
            style={titleError ? {borderColor: 'var(--fs-danger)'} : undefined}
            placeholder="Elden Ring"
            value={draft.title}
            onChange={(e) => {
              setTitleError(null);
              setDraft((prev) => ({...prev, title: e.target.value}));
            }}
          />
          {titleError && (
            <span role="alert" style={{fontSize: 12, fontWeight: 500, color: 'var(--fs-danger)'}}>
              {titleError}
            </span>
          )}
        </div>

        <div className="fs-field">
          <label className="fs-field-label" htmlFor="fs-profile-pattern">
            Game executable or window title
          </label>
          <div style={{display: 'flex', gap: 8}}>
            <input
              id="fs-profile-pattern"
              className="fs-input"
              placeholder="e.g. eldenring.exe or *Elden Ring*"
              value={draft.process_pattern}
              onChange={(e) => setDraft((prev) => ({...prev, process_pattern: e.target.value}))}
            />
            <FlowButton
              label="Browse..."
              variant="secondary"
              disabled={!canBrowseExe}
              title={
                canBrowseExe
                  ? 'Pick a game .exe file'
                  : 'Native file picker requires FlowScale for Windows — type game.exe here in preview'
              }
              icon={<FolderOpenIcon width={15} height={15} />}
              onClick={onPickExecutable}
            />
          </div>
          <span className="fs-field-desc">
            {canBrowseExe
              ? 'Executable filename (eldenring.exe) or partial window title (*Elden Ring*)'
              : 'Enter game.exe or *Game Title* (Browse requires FlowScale for Windows)'}
          </span>
        </div>

        <FlowSwitch
          label="Auto-scale on focus"
          description="Start scaling automatically when the matched window enters the foreground"
          checked={draft.auto_scale}
          onChange={(checked) => setDraft((prev) => ({...prev, auto_scale: checked}))}
        />

        <FlowNumber
          label="Auto-scale delay"
          description="Wait time in seconds after the game gains focus"
          value={draft.auto_scale_delay_sec}
          min={0}
          max={60}
          step={1}
          units="s"
          disabled={!draft.auto_scale}
          disabledMessage="Enable Auto-scale above to configure a focus delay"
          onChange={(value) => setDraft((prev) => ({...prev, auto_scale_delay_sec: value}))}
        />
      </FlowModal>

      <FlowModal
        isOpen={deleteDialogOpen}
        title={`Delete ${selectedProfile?.title ?? 'profile'}?`}
        subtitle="This removes the profile configuration from FlowScale."
        onClose={() => setDeleteDialogOpen(false)}
        footer={
          <>
            <FlowButton label="Cancel" variant="ghost" onClick={() => setDeleteDialogOpen(false)} />
            <FlowButton
              label="Delete profile"
              variant="danger"
              onClick={() => {
                if (canModifySelected && selectedProfile) {
                  onDeleteProfile(selectedProfile.id);
                }
                setDeleteDialogOpen(false);
              }}
            />
          </>
        }
      >
        <div className="fs-card-subtitle">
          Running scaling sessions remain active until manually stopped.
        </div>
      </FlowModal>
    </div>
  );
}
