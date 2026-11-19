// Session log: aggregates 200 ms telemetry ticks into run stats + an event list for the Log panel.
// Plain JS (with sessionLog.d.ts) so `node --test` can exercise it like profileDrafts.js.

const EVENT_LIMIT = 300;
/** Output within ±5% of the target counts as stable. */
const STABLE_TOLERANCE = 0.05;
/** Output below 90% of the target is logged as a dip. */
const DIP_RATIO = 0.9;
/** Ticks further apart than this (UI hidden / app suspended) are not counted as run time. */
const MAX_TICK_GAP_MS = 1000;
/** EMA weight per 200 ms tick (~1 s window) for the capture rate the target is derived from. */
const CAPTURE_EMA_ALPHA = 0.2;
/** Capture above this multiple of its EMA is a burst (catch-up frames), not a new target. */
const BURST_RATIO = 1.3;

const RUNNING = new Set(['active', 'paused']);
const COUNTERS = ['submitted_outputs', 'dropped_outputs', 'expired_output_slots', 'latency_wait_timeouts', 'occluded_outputs'];

const newStat = () => ({n: 0, sum: 0, min: Infinity, max: -Infinity});

function push(stat, value) {
  if (!Number.isFinite(value)) return;
  stat.n += 1;
  stat.sum += value;
  stat.min = Math.min(stat.min, value);
  stat.max = Math.max(stat.max, value);
}

function newRun() {
  return {
    startedAt: null,
    activeMs: 0,
    pausedMs: 0,
    sampledMs: 0,
    stableMs: 0,
    lostFrames: 0,
    inDip: false,
    captureEma: 0,
    worstLow: {fps: Infinity, at: 0},
    worstGen: {ms: -Infinity, at: 0},
    stats: {
      captured: newStat(), presented: newStat(), target: newStat(), low1: newStat(),
      gen: newStat(), cap: newStat(), present: newStat(), queue: newStat(), latencyWait: newStat(),
      motion: newStat(), interp: newStat(), upscale: newStat(),
    },
    counters: Object.fromEntries(COUNTERS.map((k) => [k, 0])),
    lastCounters: null,
    backend: '',
    resolution: '',
  };
}

export function createSessionLog() {
  return {status: 'idle', lastAt: null, runs: 0, run: newRun(), events: []};
}

/** Output FPS the active profile asks for, given the measured capture rate. */
export function targetOutputFps(profile, capturedFps) {
  switch (profile.frame_gen_engine) {
    case 'Off':
      return capturedFps;
    case 'LSFG2':
      return capturedFps * Number(profile.lsfg2_mode.slice(1));
    case 'LSFG3':
      if (profile.lsfg3_mode === 'Adaptive') return profile.lsfg_target_fps;
      return capturedFps * profile.lsfg_multiplier;
    default:
      return capturedFps * profile.lsfg_multiplier;
  }
}

function addEvent(log, now, level, text) {
  const at = log.run.startedAt === null ? 0 : now - log.run.startedAt;
  log.events.push({run: log.runs, at, level, text});
  if (log.events.length > EVENT_LIMIT) log.events.splice(0, log.events.length - EVENT_LIMIT);
}

const fps = (v) => `${v.toFixed(1)} FPS`;

/** Folds one telemetry tick into the log. `now` is a monotonic ms clock (performance.now()). */
export function recordTick(log, t, profile, now) {
  const wasRunning = RUNNING.has(log.status);
  const isRunning = RUNNING.has(t.status);
  const gap = log.lastAt === null ? 0 : Math.min(now - log.lastAt, MAX_TICK_GAP_MS);
  log.lastAt = now;

  if (isRunning && !wasRunning) {
    log.run = newRun();
    log.run.startedAt = now;
    log.runs += 1;
    addEvent(log, now, 'info', `Run ${log.runs} started on ${t.target_title ?? 'target window'} with profile "${profile.title}"`);
  }
  if (t.status !== log.status) {
    if (t.status === 'paused') addEvent(log, now, 'warn', `Paused: target lost focus (${t.foreground_title || 'unknown window'})`);
    else if (t.status === 'active' && log.status === 'paused') addEvent(log, now, 'info', 'Resumed');
    else if (t.status === 'error') addEvent(log, now, 'error', `Error: ${t.error_message || 'engine error'}`);
    else if (!isRunning && wasRunning) addEvent(log, now, 'info', 'Scaling stopped');
    log.status = t.status;
  }
  if (!isRunning) return;

  const run = log.run;
  if (t.status === 'paused') {
    run.pausedMs += gap;
    run.inDip = false;
    return;
  }
  run.activeMs += gap;

  const resolution = t.input_width && t.output_width
    ? `${t.input_width}×${t.input_height} → ${t.output_width}×${t.output_height}`
    : '';
  if (resolution && resolution !== run.resolution) {
    addEvent(log, now, 'info', `Resolution ${resolution}`);
    run.resolution = resolution;
  }

  const e = t.engine;
  if (e) {
    if (e.actual_capture_api && e.actual_capture_api !== run.backend) {
      const fallback = e.actual_capture_api !== e.requested_capture_api;
      addEvent(log, now, fallback ? 'warn' : 'info', fallback
        ? `Capture fell back ${e.requested_capture_api} → ${e.actual_capture_api}: ${e.capture_fallback_reason || 'backend unavailable'}`
        : `Capture backend ${e.actual_capture_api}`);
      run.backend = e.actual_capture_api;
    }
    // Engine counters are cumulative across runs: the first tick of a run is only the baseline.
    for (const key of run.lastCounters ? COUNTERS : []) {
      const prev = run.lastCounters[key];
      // Counters restart from zero when the engine rebuilds the session; count the fresh value then.
      const delta = e[key] >= prev ? e[key] - prev : e[key];
      run.counters[key] += delta;
      if (key === 'dropped_outputs' && delta > 0) {
        addEvent(log, now, 'warn', `${delta} output frame${delta > 1 ? 's' : ''} dropped${e.last_output_drop_reason ? `: ${e.last_output_drop_reason}` : ''}`);
      }
    }
    run.lastCounters = Object.fromEntries(COUNTERS.map((k) => [k, e[k]]));
  }

  const captured = t.captured_fps ?? 0;
  const presented = t.presented_fps ?? 0;
  if (captured <= 0 || presented <= 0) return;

  // Target follows the smoothed capture rate so a catch-up burst (e.g. 144 for a tick) never asks for 288.
  const burst = run.captureEma > 0 && captured > run.captureEma * BURST_RATIO;
  run.captureEma = run.captureEma > 0 ? run.captureEma + CAPTURE_EMA_ALPHA * (captured - run.captureEma) : captured;
  const target = targetOutputFps(profile, Math.min(captured, run.captureEma));
  const s = run.stats;
  push(s.captured, captured);
  push(s.presented, presented);
  push(s.target, target);
  run.sampledMs += gap;
  if (Math.abs(presented - target) <= target * STABLE_TOLERANCE) run.stableMs += gap;
  run.lostFrames += Math.max(0, target - presented) * (gap / 1000);

  const dipping = !burst && presented < target * DIP_RATIO;
  if (dipping && !run.inDip) addEvent(log, now, 'warn', `Dip: ${fps(presented)} out vs ${fps(target)} target (capture ${fps(captured)})`);
  if (!dipping && run.inDip) addEvent(log, now, 'info', `Recovered: ${fps(presented)}`);
  run.inDip = dipping;

  if (!e) return;
  push(s.low1, e.presented_1pct_low_fps);
  push(s.cap, e.cap_ms);
  push(s.present, e.present_ms);
  push(s.queue, e.queue_depth);
  push(s.latencyWait, e.latency_wait_ms);
  if (e.gpu_ms > 0) {
    push(s.gen, e.gpu_ms);
    // Per-frame extremes over the engine's 128-frame window catch spikes between 200 ms ticks.
    s.gen.min = Math.min(s.gen.min, e.gpu_ms_min > 0 ? e.gpu_ms_min : e.gpu_ms);
    s.gen.max = Math.max(s.gen.max, e.gpu_ms_max);
  }
  const [motion, interp, upscale] = e.gpu_stage_ms ?? [];
  if (motion != null) push(s.motion, motion);
  if (interp != null) push(s.interp, interp);
  if (upscale != null) push(s.upscale, upscale);
  if (e.presented_1pct_low_fps > 0 && e.presented_1pct_low_fps < run.worstLow.fps) {
    run.worstLow = {fps: e.presented_1pct_low_fps, at: now - run.startedAt};
  }
  if (e.gpu_ms_max > run.worstGen.ms) run.worstGen = {ms: e.gpu_ms_max, at: now - run.startedAt};
}

export function formatClock(ms) {
  const total = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(total / 3600);
  const m = String(Math.floor((total % 3600) / 60)).padStart(2, '0');
  const sec = String(total % 60).padStart(2, '0');
  return h > 0 ? `${h}:${m}:${sec}` : `${m}:${sec}`;
}

const dash = '—';
const avg = (s) => (s.n ? s.sum / s.n : NaN);
const num = (v, digits = 1, unit = '') => (Number.isFinite(v) ? `${v.toFixed(digits)}${unit}` : dash);
const triple = (s, digits, unit) => (s.n ? `${num(avg(s), digits, unit)} avg · ${num(s.min, digits)} min · ${num(s.max, digits)} max` : dash);
const pct = (part, whole) => (whole > 0 ? `${((part / whole) * 100).toFixed(1)}%` : dash);
const onOff = (v) => (v ? 'On' : 'Off');

function frameGenLabel(p) {
  if (p.frame_gen_engine === 'Off') return 'Off';
  if (p.frame_gen_engine === 'LSFG3') {
    return p.lsfg3_mode === 'Adaptive' ? `FlowGen 3 Adaptive → ${p.lsfg_target_fps} FPS` : `FlowGen 3 Fixed ×${p.lsfg_multiplier}`;
  }
  if (p.frame_gen_engine === 'LSFG2') return `FlowGen 2 ${p.lsfg2_mode}`;
  return `FlowGen 1 ×${p.lsfg_multiplier}`;
}

/** Report sections shared by the Log panel and the copied text. */
export function summarize(log, profile, now) {
  const run = log.run;
  const s = run.stats;
  const running = RUNNING.has(log.status);
  const c = run.counters;
  return [
    {
      title: 'Run',
      rows: [
        ['Status', log.status],
        ['Run time', run.startedAt === null ? dash : `${formatClock(run.activeMs)} active · ${formatClock(run.pausedMs)} paused`],
        ['Elapsed', run.startedAt === null ? dash : formatClock((running ? now : log.lastAt ?? now) - run.startedAt)],
        ['Capture backend', run.backend || dash],
        ['Resolution', run.resolution || dash],
      ],
    },
    {
      title: 'Frame rate',
      rows: [
        ['Captured (game)', triple(s.captured, 1, ' FPS')],
        ['Output (generated)', triple(s.presented, 1, ' FPS')],
        ['Target output', s.target.n ? num(avg(s.target), 1, ' FPS') : dash],
        ['Stable at target (±5%)', pct(run.stableMs, run.sampledMs)],
        ['1% low', s.low1.n ? `${num(avg(s.low1), 1, ' FPS')} avg · ${num(s.low1.min, 1, ' FPS')} worst` : dash],
        ['Worst 1% low at', Number.isFinite(run.worstLow.fps) ? `${num(run.worstLow.fps, 1, ' FPS')} @ ${formatClock(run.worstLow.at)}` : dash],
      ],
    },
    {
      title: 'Frames lost',
      rows: [
        ['Missed vs target (est.)', s.target.n ? `${Math.round(run.lostFrames)} frames` : dash],
        ['Dropped outputs', `${c.dropped_outputs} of ${c.submitted_outputs + c.dropped_outputs} (${pct(c.dropped_outputs, c.submitted_outputs + c.dropped_outputs)})`],
        ['Expired output slots', String(c.expired_output_slots)],
        ['Latency-wait timeouts', String(c.latency_wait_timeouts)],
        ['Occluded presents', String(c.occluded_outputs)],
        ['Last drop reason', lastDropReason(log)],
      ],
    },
    {
      title: 'Generation time (GPU)',
      rows: [
        ['Total per frame', s.gen.n ? `${num(avg(s.gen), 2, ' ms')} avg · ${num(s.gen.min, 2, ' ms')} fastest · ${num(s.gen.max, 2, ' ms')} slowest` : dash],
        ['Slowest frame at', Number.isFinite(run.worstGen.ms) && run.worstGen.ms > 0 ? `${num(run.worstGen.ms, 2, ' ms')} @ ${formatClock(run.worstGen.at)}` : dash],
        ['Budget at target', s.target.n ? `${num(1000 / avg(s.target), 2, ' ms')} per output frame` : dash],
        ['Load vs budget', loadHint(s)],
        ['Motion estimation', triple(s.motion, 2, ' ms')],
        ['Interpolation', triple(s.interp, 2, ' ms')],
        ['Upscale', triple(s.upscale, 2, ' ms')],
      ],
    },
    {
      title: 'Pipeline',
      rows: [
        ['Capture interval', triple(s.cap, 2, ' ms')],
        ['Present call', triple(s.present, 2, ' ms')],
        ['Latency wait', triple(s.latencyWait, 2, ' ms')],
        ['Queue depth', triple(s.queue, 1, '')],
      ],
    },
    {
      title: 'Active settings',
      rows: [
        ['Profile', profile.title],
        ['Upscaler', profile.scaling_type === 'Off' ? 'Off' : `${profile.scaling_type === 'LS1' ? 'FlowSharp' : profile.scaling_type}${profile.scaling_subtype ? ` ${profile.scaling_subtype}` : ''} · ${profile.scaling_mode === 'Custom' ? `×${profile.scale_factor}` : 'Auto'} · ${profile.fit_mode} · sharpness ${profile.sharpness}`],
        ['Frame generation', frameGenLabel(profile)],
        ['Flow scale / quality', `${profile.flow_scale}% · ${profile.lsfg_performance}`],
        ['Capture API (requested)', profile.capture_api],
        ['Sync / latency', `${profile.sync_mode} · max frame latency ${profile.max_frame_latency} · queue target ${profile.queue_target}`],
        ['G-Sync / HDR / VRS', `${onOff(profile.gsync_support)} / ${onOff(profile.hdr_support)} / ${onOff(profile.vrs)}`],
        ['GPU / display', `${profile.preferred_gpu || 'Auto'} · ${profile.output_display || 'Auto'}`],
      ],
    },
  ];
}

/** Generation time is shared with the game on one GPU: past ~40% of the frame budget (or one frame over it) spikes cause drops. */
function loadHint(s) {
  if (!s.gen.n || !s.target.n) return dash;
  const budget = 1000 / avg(s.target);
  const share = avg(s.gen) / budget;
  const label = `${(share * 100).toFixed(0)}% avg`;
  return share > 0.4 || s.gen.max > budget
    ? `${label} · heavy: lower Flow scale (e.g. 100% → 50%)`
    : `${label} · OK`;
}

function lastDropReason(log) {
  for (let i = log.events.length - 1; i >= 0; i -= 1) {
    if (log.events[i].run !== log.runs) break;
    const text = log.events[i].text;
    const idx = text.indexOf('dropped: ');
    if (idx >= 0) return text.slice(idx + 'dropped: '.length);
  }
  return dash;
}

/** Events of the current run only (older runs restart the clock and would read as duplicates). */
export function currentRunEvents(log) {
  return log.events.filter((ev) => ev.run === log.runs);
}

/** Plain-text report for the clipboard. */
export function formatReport(log, profile, now) {
  const lines = [`FlowScale session log (run ${log.runs || 0})`, ''];
  for (const section of summarize(log, profile, now)) {
    lines.push(`[${section.title}]`);
    for (const [label, value] of section.rows) lines.push(`${label}: ${value}`);
    lines.push('');
  }
  if (log.runs > 1) lines.push(`Earlier runs this session: ${log.runs - 1} (not included)`, '');
  lines.push('[Events]');
  for (const ev of currentRunEvents(log)) lines.push(`${formatClock(ev.at)}  ${ev.level.toUpperCase().padEnd(5)}  ${ev.text}`);
  return lines.join('\n');
}
