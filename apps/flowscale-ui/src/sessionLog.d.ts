import type {ScalingProfile, TelemetryState} from './types';

export interface SessionLogEvent {
  /** run number the event belongs to */
  run: number;
  /** ms since the run started */
  at: number;
  level: 'info' | 'warn' | 'error';
  text: string;
}

export interface SessionLog {
  status: TelemetryState['status'];
  lastAt: number | null;
  runs: number;
  events: SessionLogEvent[];
  run: unknown;
}

export interface SessionLogSection {
  title: string;
  rows: Array<[string, string]>;
}

export function createSessionLog(): SessionLog;
export function targetOutputFps(profile: ScalingProfile, capturedFps: number): number;
export function recordTick(log: SessionLog, t: TelemetryState, profile: ScalingProfile, now: number): void;
export function summarize(log: SessionLog, profile: ScalingProfile, now: number): SessionLogSection[];
export function formatReport(log: SessionLog, profile: ScalingProfile, now: number): string;
export function formatClock(ms: number): string;
export function currentRunEvents(log: SessionLog): SessionLogEvent[];
