import {type CSSProperties, useEffect, useState} from 'react';
import {ArrowPathIcon, CheckIcon, ClipboardDocumentIcon} from '@heroicons/react/24/outline';
import {currentRunEvents, formatClock, formatReport, summarize, type SessionLog} from '../sessionLog.js';
import type {ScalingProfile} from '../types';
import {FlowButton, FlowModal} from './ui/FlowControls';

const REFRESH_MS = 500;

export function SessionLogModal({
  isOpen,
  onClose,
  log,
  getProfile,
  onReset,
}: {
  isOpen: boolean;
  onClose: () => void;
  log: SessionLog;
  /** Profile the engine is running (read on each refresh, it can change mid-run). */
  getProfile: () => ScalingProfile;
  onReset: () => void;
}) {
  // The log lives in a ref fed by 200 ms ticks; re-read it on a timer only while the panel is open.
  const [now, setNow] = useState(() => performance.now());
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    if (!isOpen) return;
    setNow(performance.now());
    const id = window.setInterval(() => setNow(performance.now()), REFRESH_MS);
    return () => window.clearInterval(id);
  }, [isOpen]);

  useEffect(() => {
    if (!copied) return;
    const id = window.setTimeout(() => setCopied(false), 1400);
    return () => window.clearTimeout(id);
  }, [copied]);

  const profile = getProfile();
  const sections = summarize(log, profile, now);
  const events = currentRunEvents(log).reverse();

  return (
    <FlowModal
      isOpen={isOpen}
      wide
      title="Session log"
      subtitle={log.runs ? `Run ${log.runs} · ${log.status} · updates live` : 'Start scaling to record a run'}
      onClose={onClose}
      footer={
        <>
          <FlowButton
            label="Reset"
            variant="ghost"
            size="sm"
            icon={<ArrowPathIcon width={15} height={15} />}
            onClick={() => {
              onReset();
              setNow(performance.now());
            }}
          />
          <FlowButton
            label={copied ? 'Copied' : 'Copy report'}
            variant="secondary"
            size="sm"
            className={copied ? 'fs-pulse-success' : ''}
            icon={
              <span className="fs-icon-swap" data-swapped={copied || undefined}>
                <ClipboardDocumentIcon width={15} height={15} />
                <CheckIcon width={15} height={15} />
              </span>
            }
            onClick={() => {
              void navigator.clipboard
                ?.writeText(formatReport(log, profile, performance.now()))
                .then(() => setCopied(true));
            }}
          />
          <FlowButton label="Close" variant="primary" size="sm" onClick={onClose} />
        </>
      }
    >
      <div className="fs-log-body">
        <div className="fs-log-grid">
          {sections.map((section, idx) => (
            <section
              key={section.title}
              className="fs-log-section fs-stagger-row"
              style={{'--stagger-index': idx} as CSSProperties}
            >
              <h3 className="fs-log-heading">{section.title}</h3>
              <dl className="fs-log-rows">
                {section.rows.map(([label, value]) => (
                  <div key={label} className="fs-log-row">
                    <dt>{label}</dt>
                    <dd className="fs-tabular">{value}</dd>
                  </div>
                ))}
              </dl>
            </section>
          ))}
        </div>

        <section className="fs-log-section">
          <h3 className="fs-log-heading">Events ({events.length})</h3>
          {events.length === 0 ? (
            <p className="fs-card-subtitle">No events yet. Dips, drops, pauses and backend fallbacks show up here.</p>
          ) : (
            <ol className="fs-log-events">
              {events.map((ev, i) => (
                <li key={`${events.length - i}`} className="fs-log-event" data-level={ev.level}>
                  <span className="fs-log-time">{formatClock(ev.at)}</span>
                  <span>{ev.text}</span>
                </li>
              ))}
            </ol>
          )}
        </section>
      </div>
    </FlowModal>
  );
}
