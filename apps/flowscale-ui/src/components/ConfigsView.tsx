import {type CSSProperties, useRef, useState} from 'react';
import {isWebView2Available} from '../ipc';
import type {ConfigAction, ConfigSummary, ScalingProfile} from '../types';
import {FlowButton} from './ui/FlowControls';

interface ConfigsViewProps {
  entries: ConfigSummary[];
  profile: ScalingProfile;
  busy: boolean;
  onAction: (action: ConfigAction) => void;
  onError: (message: string) => void;
}

export function ConfigsView({entries, profile, busy, onAction, onError}: ConfigsViewProps) {
  const input = useRef<HTMLInputElement>(null);
  const reading = useRef(false);
  const [isReading, setIsReading] = useState(false);
  const [isDragOver, setIsDragOver] = useState(false);
  const available = isWebView2Available();
  const disabled = busy || isReading || !available;

  async function importFiles(files: FileList) {
    if (disabled || reading.current) return;
    if (files.length !== 1) {
      onError('Choose one JSON file at a time.');
      return;
    }
    const file = files[0];
    if (!file.name.toLowerCase().endsWith('.json') || file.size > 256 * 1024) {
      onError('Choose a JSON file no larger than 256 KiB.');
      return;
    }
    reading.current = true;
    setIsReading(true);
    try {
      onAction({action: 'import', document: await file.text()});
    } catch {
      onError('Could not read the selected config.');
    } finally {
      reading.current = false;
      setIsReading(false);
    }
  }

  return (
    <div style={{display: 'flex', flexDirection: 'column', gap: 16, width: '100%'}}>
      <section
        className="fs-card fs-dropzone"
        data-drag-over={isDragOver ? 'true' : 'false'}
        aria-label="Import or export a profile config"
        aria-busy={busy || isReading}
        onDragOver={(event) => {
          if (!event.dataTransfer.types.includes('Files')) return;
          event.preventDefault();
          event.dataTransfer.dropEffect = disabled ? 'none' : 'copy';
          if (!disabled) setIsDragOver(true);
        }}
        onDragLeave={() => setIsDragOver(false)}
        onDrop={(event) => {
          event.preventDefault();
          setIsDragOver(false);
          void importFiles(event.dataTransfer.files);
        }}
      >
        <div className="fs-card-header">
          <div>
            <h2 className="fs-card-title">Profile configs</h2>
            <p className="fs-card-subtitle" style={{marginTop: 4}}>
              {available
                ? 'Drop a JSON config file anywhere inside this card, or import/export manually.'
                : 'Preview mode detected — open FlowScale for Windows to import or save configs.'}
            </p>
          </div>
          <span className="fs-summary-chip">Target: {profile.title}</span>
        </div>

        <input
          ref={input}
          type="file"
          accept=".json,application/json"
          hidden
          onChange={(event) => {
            const files = event.currentTarget.files;
            if (files?.length) void importFiles(files);
            event.currentTarget.value = '';
          }}
        />

        <div style={{display: 'flex', gap: 10, marginTop: 14, flexWrap: 'wrap'}}>
          <FlowButton
            label="Import JSON"
            variant="primary"
            disabled={disabled}
            onClick={() => input.current?.click()}
          />
          <FlowButton
            label="Export active profile"
            variant="secondary"
            disabled={disabled}
            onClick={() => onAction({action: 'export', profile_id: profile.id})}
          />
        </div>
      </section>

      <section className="fs-card">
        <div className="fs-card-header">
          <div>
            <h3 className="fs-card-title">Saved configs</h3>
            <p className="fs-card-subtitle">Load a stored preset into {profile.title}</p>
          </div>
        </div>

        <div className="fs-divider" />

        {entries.length === 0 && !busy ? (
          <p className="fs-card-subtitle" style={{padding: '8px 0'}}>
            No saved configs yet. Export your current profile above to save one.
          </p>
        ) : (
          <div style={{display: 'flex', flexDirection: 'column', gap: 8}}>
            {entries.map((entry, idx) => (
              <div
                key={entry.id}
                className="fs-stagger-row"
                style={{
                  '--stagger-index': idx,
                  display: 'flex',
                  alignItems: 'center',
                  justifyContent: 'space-between',
                  gap: 12,
                  padding: '8px 12px',
                  borderRadius: 8,
                  backgroundColor: 'var(--fs-bg-pill)',
                  border: '1px solid var(--fs-border-hairline)',
                } as CSSProperties}
              >
                <span style={{fontSize: 13, fontWeight: 500, overflow: 'hidden', textOverflow: 'ellipsis'}}>
                  {entry.name}
                </span>
                <div style={{display: 'flex', gap: 6}}>
                  <FlowButton
                    label="Load"
                    variant="secondary"
                    size="sm"
                    disabled={disabled}
                    ariaLabel={`Load ${entry.name} into ${profile.title}`}
                    onClick={() => onAction({action: 'load', id: entry.id, profile_id: profile.id})}
                  />
                  <FlowButton
                    label="Delete"
                    variant="ghost"
                    size="sm"
                    disabled={disabled}
                    ariaLabel={`Delete ${entry.name}`}
                    onClick={() => onAction({action: 'delete', id: entry.id})}
                  />
                </div>
              </div>
            ))}
          </div>
        )}
      </section>
    </div>
  );
}
