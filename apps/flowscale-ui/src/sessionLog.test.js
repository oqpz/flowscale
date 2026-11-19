import {test} from 'node:test';
import assert from 'node:assert/strict';
import {createSessionLog, formatReport, recordTick, summarize, targetOutputFps} from './sessionLog.js';

const profile = {
  title: 'CS2', frame_gen_engine: 'LSFG3', lsfg3_mode: 'Fixed', lsfg_multiplier: 2, lsfg_target_fps: 144,
  lsfg2_mode: 'X2', scaling_type: 'Off', scaling_subtype: '', scaling_mode: 'Auto', scale_factor: 1, fit_mode: 'AspectRatio',
  sharpness: 0.5, flow_scale: 50, lsfg_performance: 'Balanced', capture_api: 'DXGI', sync_mode: 'Off',
  max_frame_latency: 1, queue_target: 1, gsync_support: false, hdr_support: false, vrs: false, preferred_gpu: '', output_display: '',
};

const engine = (over = {}) => ({
  presented_1pct_low_fps: 110, cap_ms: 16.6, gpu_ms: 2, gpu_ms_min: 1.5, gpu_ms_max: 3, gpu_stage_ms: [0.8, 1, null],
  present_ms: 0.3, queue_depth: 1, requested_capture_api: 'Dxgi', actual_capture_api: 'Dxgi', capture_fallback_reason: '',
  dxgi_accumulated_frames: 0, expired_output_slots: 0, latency_wait_timeouts: 0, latency_wait_ms: 0,
  submitted_outputs: 0, dropped_outputs: 0, last_output_drop_reason: '', occluded_outputs: 0, ...over,
});

const tick = (captured, presented, e) => ({status: 'active', target_title: 'cs2', captured_fps: captured, presented_fps: presented, engine: e});
const row = (sections, title, label) => sections.find((s) => s.title === title).rows.find(([l]) => l === label)[1];

test('target follows the frame-gen mode', () => {
  assert.equal(targetOutputFps(profile, 60), 120);
  assert.equal(targetOutputFps({...profile, lsfg3_mode: 'Adaptive'}, 60), 144);
  assert.equal(targetOutputFps({...profile, frame_gen_engine: 'Off'}, 60), 60);
  assert.equal(targetOutputFps({...profile, frame_gen_engine: 'LSFG2', lsfg2_mode: 'X3'}, 60), 180);
});

test('run tracks stability, missed frames, drops and generation-time extremes', () => {
  const log = createSessionLog();
  recordTick(log, tick(60, 120, engine({submitted_outputs: 100})), profile, 0);
  recordTick(log, tick(60, 120, engine({submitted_outputs: 220})), profile, 1000);
  // 1 s at 96 FPS against a 120 target: ~24 frames missed, a dip, 4 drops and a 9 ms frame.
  recordTick(log, tick(60, 96, engine({submitted_outputs: 316, dropped_outputs: 4, last_output_drop_reason: 'late', gpu_ms_max: 9})), profile, 2000);
  recordTick(log, tick(60, 120, engine({submitted_outputs: 436, dropped_outputs: 4})), profile, 3000);

  const s = summarize(log, profile, 3000);
  assert.equal(row(s, 'Frame rate', 'Stable at target (±5%)'), '66.7%');
  assert.equal(row(s, 'Frames lost', 'Missed vs target (est.)'), '24 frames');
  // First tick is the baseline (submitted 100 is from before this run): 336 submitted + 4 dropped.
  assert.equal(row(s, 'Frames lost', 'Dropped outputs'), '4 of 340 (1.2%)');
  assert.equal(row(s, 'Frames lost', 'Last drop reason'), 'late');
  assert.match(row(s, 'Generation time (GPU)', 'Total per frame'), /1\.50 ms fastest · 9\.00 ms slowest/);
  assert.equal(row(s, 'Generation time (GPU)', 'Slowest frame at'), '9.00 ms @ 00:02');
  const texts = log.events.map((e) => e.text);
  assert.ok(texts.some((t) => t.startsWith('Dip: 96.0 FPS')));
  assert.ok(texts.some((t) => t.startsWith('Recovered')));
  assert.match(formatReport(log, profile, 3000), /Frame generation: FlowGen 3 Fixed ×2/);
});

test('cumulative engine counters are a baseline at run start; resets and new runs do not double count', () => {
  const log = createSessionLog();
  recordTick(log, tick(60, 120, engine({dropped_outputs: 129})), profile, 0); // carried over from earlier runs
  assert.equal(log.run.counters.dropped_outputs, 0);
  assert.ok(!log.events.some((e) => e.text.includes('dropped')));
  recordTick(log, tick(60, 120, engine({dropped_outputs: 2})), profile, 200); // engine session rebuilt
  assert.equal(log.run.counters.dropped_outputs, 2);
  recordTick(log, {status: 'idle'}, profile, 400);
  recordTick(log, tick(60, 120, engine({dropped_outputs: 3})), profile, 600);
  recordTick(log, tick(60, 120, engine({dropped_outputs: 4})), profile, 800);
  assert.equal(log.runs, 2);
  assert.equal(log.run.counters.dropped_outputs, 1);
});

test('a capture burst neither raises the target nor logs a dip', () => {
  const log = createSessionLog();
  for (let i = 0; i < 10; i += 1) recordTick(log, tick(60, 120, engine()), profile, i * 200);
  recordTick(log, tick(144, 116, engine()), profile, 2000); // one catch-up tick
  const s = summarize(log, profile, 2000);
  assert.ok(Number.parseFloat(row(s, 'Frame rate', 'Target output')) < 125);
  assert.ok(!log.events.some((e) => e.text.startsWith('Dip')));
});

test('copied report only carries the current run', () => {
  const log = createSessionLog();
  recordTick(log, tick(60, 120, engine()), profile, 0);
  recordTick(log, {status: 'idle'}, profile, 200);
  recordTick(log, {...tick(60, 120, engine()), target_title: 'second'}, profile, 400);
  const report = formatReport(log, profile, 600);
  assert.match(report, /Earlier runs this session: 1/);
  assert.match(report, /Run 2 started on second/);
  assert.doesNotMatch(report, /Run 1 started/);
});

test('generation load hint flags a saturated GPU', () => {
  const log = createSessionLog();
  recordTick(log, tick(60, 120, engine({gpu_ms: 4, gpu_ms_max: 23})), profile, 0);
  recordTick(log, tick(60, 120, engine({gpu_ms: 4, gpu_ms_max: 23})), profile, 200);
  assert.match(row(summarize(log, profile, 200), 'Generation time (GPU)', 'Load vs budget'), /lower Flow scale/);
});
