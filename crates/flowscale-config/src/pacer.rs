//! Frame-time tracking, queue draining, interpolation scheduling, and hybrid waitable-timer + QPC pacing.

use std::collections::VecDeque;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Threading::{
    CreateWaitableTimerExW, SetWaitableTimerEx, WaitForSingleObject,
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, INFINITE, TIMER_ALL_ACCESS,
};

/// Rolling frametime window with outlier reset detection.
#[derive(Debug, Clone)]
pub struct FrametimeTracker {
    window_size: usize,
    reset_multiplier: f64,
    samples_sec: VecDeque<f64>,
    sum_sec: f64,
    fast_ema_sec: f64,
    last_timestamp_sec: Option<f64>,
    idle_streak: u8,
    fast_outlier_streak: u8,
}

impl FrametimeTracker {
    /// Creates a new tracker with the given rolling buffer size and spike reset multiplier.
    pub fn new(window_size: usize, reset_multiplier: f64) -> Self {
        let cap = window_size.clamp(1, 1024);
        let mult = if reset_multiplier.is_finite() {
            reset_multiplier.max(1.1)
        } else {
            6.0
        };
        Self {
            window_size: cap,
            reset_multiplier: mult,
            samples_sec: VecDeque::with_capacity(cap),
            sum_sec: 0.0,
            fast_ema_sec: 0.0,
            last_timestamp_sec: None,
            idle_streak: 0,
            fast_outlier_streak: 0,
        }
    }

    /// Records a new source frame timestamp in seconds and returns the accepted delta (if any).
    pub fn record_timestamp(&mut self, timestamp_sec: f64) -> Option<f64> {
        if !self.is_new_capture_timestamp(timestamp_sec) {
            return None;
        }
        let prev = self.last_timestamp_sec.replace(timestamp_sec)?;
        let dt = timestamp_sec - prev;
        if !dt.is_finite() || dt <= 0.0 {
            return None;
        }
        self.record_delta(dt).then_some(dt)
    }

    /// Records a capture timestamp without letting a single idle gap replace the cadence estimate,
    /// while still adapting if consecutive intervals stay slow.
    pub fn record_capture_timestamp(&mut self, timestamp_sec: f64) -> bool {
        if !self.is_new_capture_timestamp(timestamp_sec) {
            return false;
        }
        let Some(previous) = self.last_timestamp_sec.replace(timestamp_sec) else {
            self.idle_streak = 0;
            self.fast_outlier_streak = 0;
            return false;
        };
        let delta = timestamp_sec - previous;
        if !delta.is_finite() || delta <= 0.0 {
            return false;
        }
        let idle_gap = if self.samples_sec.is_empty() {
            0.25
        } else {
            (self.average_frametime_sec() * self.reset_multiplier).max(0.05)
        };
        if delta > idle_gap {
            self.idle_streak = self.idle_streak.saturating_add(1);
            self.fast_outlier_streak = 0;
            // Only adapt if slow intervals persist consecutively (legitimate FPS drop).
            if self.idle_streak < 2 || delta > 1.0 {
                return false;
            }
            self.samples_sec.clear();
            self.sum_sec = 0.0;
            self.fast_ema_sec = 0.0;
        } else {
            self.idle_streak = 0;
        }
        if delta < 0.0005 {
            return false;
        }
        // Skip single fast outliers from samples_sec without returning false (which would drop FG).
        let _ = self.record_delta(delta);
        true
    }

    /// Records a timestamp that superseded `superseded` unseen frames, learning their average interval.
    /// Without this, a consumer paced slower than the source sees every frame superseded and the
    /// cadence estimate can never recover (e.g. stuck at a 20 FPS menu rate after the game returns to 60).
    pub fn record_superseded_timestamp(&mut self, timestamp_sec: f64, superseded: u32) -> bool {
        if let Some(previous) = self.last_timestamp_sec.filter(|_| self.is_new_capture_timestamp(timestamp_sec)) {
            let frames = f64::from(superseded) + 1.0;
            let average_dt = (timestamp_sec - previous) / frames;
            // Frames piling up at a clearly faster rate prove the source sped up: adopt it now
            // instead of easing the window down over ~1 s (visible stutter after loading screens).
            // The 2x margin keeps DXGI's extra desktop-update frames from triggering it during steady play.
            if superseded > 0 && average_dt < self.pacing_frametime_sec() * 0.5 {
                self.samples_sec.clear();
                self.sum_sec = 0.0;
                self.fast_ema_sec = 0.0;
            }
            self.last_timestamp_sec = Some(timestamp_sec - average_dt);
        }
        self.record_capture_timestamp(timestamp_sec)
    }

    /// Whether a frame can advance the current capture sequence.
    pub fn is_new_capture_timestamp(&self, timestamp_sec: f64) -> bool {
        timestamp_sec.is_finite()
            && self
                .last_timestamp_sec
                .is_none_or(|previous| timestamp_sec > previous)
    }

    /// Starts a new run of timestamps while retaining the measured source cadence.
    pub fn break_capture_sequence(&mut self) {
        self.last_timestamp_sec = None;
        self.idle_streak = 0;
        self.fast_outlier_streak = 0;
    }

    /// Records a positive frametime delta in seconds and returns whether it was accepted.
    pub fn record_delta(&mut self, dt_sec: f64) -> bool {
        if !dt_sec.is_finite() || dt_sec < 0.0005 {
            return false;
        }
        if self.samples_sec.len() >= 3 {
            let avg = self.average_frametime_sec();
            if dt_sec > avg * self.reset_multiplier {
                self.samples_sec.clear();
                self.sum_sec = 0.0;
                self.fast_ema_sec = 0.0;
                self.fast_outlier_streak = 0;
            } else if dt_sec * self.reset_multiplier < avg {
                self.fast_outlier_streak = self.fast_outlier_streak.saturating_add(1);
                if self.fast_outlier_streak < 4 {
                    return false;
                }
                self.samples_sec.clear();
                self.sum_sec = 0.0;
                self.fast_ema_sec = 0.0;
                self.fast_outlier_streak = 0;
            } else {
                self.fast_outlier_streak = 0;
            }
        } else {
            self.fast_outlier_streak = 0;
        }
        if self.samples_sec.len() == self.window_size {
            if let Some(oldest) = self.samples_sec.pop_front() {
                self.sum_sec -= oldest;
            }
        }
        self.samples_sec.push_back(dt_sec);
        self.sum_sec += dt_sec;
        self.fast_ema_sec = if self.fast_ema_sec <= 0.0 {
            dt_sec
        } else {
            self.fast_ema_sec * 0.75 + dt_sec * 0.25
        };
        true
    }

    /// Average source frametime in seconds.
    pub fn average_frametime_sec(&self) -> f64 {
        if self.samples_sec.is_empty() {
            0.0
        } else {
            self.sum_sec / (self.samples_sec.len() as f64)
        }
    }

    /// Source frametime estimate for pacing: the rolling SMA while cadence is steady (EMA within
    /// 2%, absorbing capture-timestamp jitter), the fast EMA once cadence genuinely shifts.
    pub fn pacing_frametime_sec(&self) -> f64 {
        let avg = self.average_frametime_sec();
        if self.fast_ema_sec > 0.0 && (self.fast_ema_sec - avg).abs() > avg * 0.02 {
            self.fast_ema_sec
        } else {
            avg
        }
    }

    /// Most recently observed source timestamp in seconds, if any.
    pub fn last_timestamp_sec(&self) -> Option<f64> {
        self.last_timestamp_sec
    }

    /// Estimated source FPS from the rolling average.
    pub fn estimated_fps(&self) -> f64 {
        let avg = self.average_frametime_sec();
        if avg > 0.0 {
            1.0 / avg
        } else {
            0.0
        }
    }

    /// Number of samples currently in the rolling buffer.
    pub fn len(&self) -> usize {
        self.samples_sec.len()
    }

    /// Returns `true` if no frametime deltas have been recorded.
    pub fn is_empty(&self) -> bool {
        self.samples_sec.is_empty()
    }

    /// Clears all recorded samples and timestamp state.
    pub fn reset(&mut self) {
        self.samples_sec.clear();
        self.sum_sec = 0.0;
        self.fast_ema_sec = 0.0;
        self.last_timestamp_sec = None;
        self.idle_streak = 0;
        self.fast_outlier_streak = 0;
    }
}

/// Smooths capture queue draining using exponential momentum toward `queue_target`.
#[derive(Debug, Clone)]
pub struct QueueDrainingController {
    momentum: f64,
    target_depth: f64,
    smoothed_error: f64,
}

impl QueueDrainingController {
    /// Creates a controller with `queue_draining_momentum` and `queue_target`.
    pub fn new(momentum: f64, target_depth: usize) -> Self {
        let m = if momentum.is_finite() {
            momentum.clamp(0.0, 1.0)
        } else {
            0.05
        };
        Self {
            momentum: m,
            target_depth: target_depth as f64,
            smoothed_error: 0.0,
        }
    }

    /// Updates smoothed queue depth error and returns a fractional pacing multiplier
    /// (`< 1.0` drains slightly faster when queue is overfull; `> 1.0` slows slightly when empty).
    pub fn update(&mut self, current_depth: usize) -> f64 {
        let error = (current_depth as f64) - self.target_depth;
        self.smoothed_error = self.smoothed_error * (1.0 - self.momentum) + error * self.momentum;
        // Clamp pacing adjustment to +/-15% so frame pacing remains smooth.
        (1.0 - self.smoothed_error * 0.05).clamp(0.85, 1.15)
    }

    /// Returns the current smoothed queue error.
    pub fn smoothed_error(&self) -> f64 {
        self.smoothed_error
    }
}

/// Interpolation scheduling mode (Fixed multiplier or Adaptive target FPS).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InterpolationMode {
    Fixed { multiplier: f64 },
    Adaptive { target_fps: f64 },
}

/// Scheduled output frame decision for a single interval.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScheduledStep {
    /// Interpolation phase in `[0.0, 1.0]`. `1.0` denotes presenting the real source frame.
    pub alpha: f32,
    /// `true` if this output frame is an interpolated frame, `false` if it is a real frame.
    pub is_interpolated: bool,
    /// Target presentation delta from the previous output frame in seconds.
    pub output_dt_sec: f64,
}

/// Computes interpolation phases (`alpha`) for Fixed and Adaptive LSFG modes.
#[derive(Debug, Clone)]
pub struct InterpolationScheduler {
    mode: InterpolationMode,
    base_framerate_threshold: f64,
    real_timestamp_tolerance: f64,
    accumulator: f64,
}

impl InterpolationScheduler {
    /// Creates a new scheduler.
    pub fn new(
        mode: InterpolationMode,
        base_framerate_threshold: f64,
        real_timestamp_tolerance: f64,
    ) -> Self {
        Self {
            mode,
            base_framerate_threshold: base_framerate_threshold.max(0.0),
            real_timestamp_tolerance: real_timestamp_tolerance.max(0.0),
            accumulator: 0.0,
        }
    }

    /// Generates the sequence of output frame steps for one source frame interval `source_dt_sec`.
    /// Passes through one real frame below the base rate threshold or at the adaptive target.
    pub fn plan_interval(&mut self, source_dt_sec: f64) -> Vec<ScheduledStep> {
        if !source_dt_sec.is_finite() || source_dt_sec <= 0.0 {
            return Vec::new();
        }
        let source_fps = 1.0 / source_dt_sec;
        // 0.5% hysteresis prevents oscillating between passthrough and 2-step pulses near target_fps.
        if source_fps < self.base_framerate_threshold
            || matches!(self.mode, InterpolationMode::Adaptive { target_fps }
                if !target_fps.is_finite() || target_fps <= source_fps * 1.005)
        {
            self.reset_phase();
            return vec![ScheduledStep {
                alpha: 1.0,
                is_interpolated: false,
                output_dt_sec: source_dt_sec,
            }];
        }

        let ratio = match self.mode {
            InterpolationMode::Fixed { multiplier } => {
                if multiplier.is_finite() {
                    multiplier.clamp(1.0, 20.0)
                } else {
                    1.0
                }
            }
            InterpolationMode::Adaptive { target_fps } => {
                (target_fps / source_fps).clamp(1.0, 20.0)
            }
        };

        let phase = self.accumulator;
        self.accumulator += ratio;
        let total_frames = (self.accumulator + 1e-9).floor().max(1.0) as usize;
        self.accumulator = (self.accumulator - (total_frames as f64)).max(0.0);

        let out_dt = source_dt_sec / ratio;
        let mut steps = Vec::with_capacity(total_frames);
        for i in 1..=total_frames {
            let alpha = ((i as f64 - phase) / ratio).clamp(0.0, 1.0);
            let is_real = (1.0 - alpha) <= self.real_timestamp_tolerance;
            steps.push(ScheduledStep {
                alpha: if is_real { 1.0 } else { alpha as f32 },
                is_interpolated: !is_real,
                output_dt_sec: out_dt,
            });
        }
        steps
    }

    /// Discards fractional output phase when the source sequence is interrupted.
    pub fn reset_phase(&mut self) {
        self.accumulator = 0.0;
    }
}

/// Computes the next presentation deadline, absorbing jitter below one output interval
/// without accumulating phase drift, while resetting after a genuine stall (`>= interval`).
pub fn present_deadline(previous: Option<f64>, now: f64, interval: f64) -> f64 {
    let Some(previous) = previous else { return now };
    if !interval.is_finite() || interval <= 0.0 {
        return now;
    }
    let target = previous + interval;
    if target >= now {
        return target;
    }
    target + ((now - target) / interval).ceil() * interval
}

/// A single output clock; expired slots are skipped, never submitted in a catch-up burst.
#[derive(Debug, Default)]
pub struct OutputTimeline {
    last_deadline: Option<f64>,
    last_submission: Option<f64>,
    last_source: Option<i64>,
    output_id: u64,
    pub expired_slots: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct ScheduledOutput {
    pub output_id: u64,
    pub source_id: i64,
    pub source_time: f64,
    pub alpha: f32,
    pub deadline: f64,
}

impl OutputTimeline {
    pub fn continuing_from(output_id: u64, expired_slots: u64) -> Self { Self { output_id, expired_slots, ..Self::default() } }

    pub fn reset(&mut self) {
        self.last_deadline = None;
        self.last_submission = None;
        self.last_source = None;
    }

    pub fn record_submission(&mut self, submitted_at: f64) {
        self.last_submission = Some(submitted_at);
    }

    pub fn schedule(
        &mut self,
        ready_at: f64,
        interval: f64,
        source_id: i64,
        source_time: f64,
        alpha: f32,
    ) -> Option<ScheduledOutput> {
        if !ready_at.is_finite()
            || !source_time.is_finite()
            || !interval.is_finite()
            || interval <= 0.0
            || self.last_source.is_some_and(|last| source_id < last)
        {
            return None;
        }
        // Leave room for ordinary timer jitter while skipping a slot that would
        // otherwise follow a late submission almost immediately.
        let ready_at = self.last_submission.map_or(ready_at, |last| {
            ready_at.max(last + 0.75 * interval)
        });
        let deadline = self.last_deadline.map_or(ready_at + interval, |last| {
            present_deadline(Some(last), ready_at, interval)
        });
        if let Some(last) = self.last_deadline {
            self.expired_slots += (((deadline - last) / interval).round() as u64).saturating_sub(1);
        }
        self.last_deadline = Some(deadline);
        self.last_source = Some(source_id);
        self.output_id += 1;
        Some(ScheduledOutput {
            output_id: self.output_id,
            source_id,
            source_time,
            alpha,
            deadline,
        })
    }
}

/// Computes a capture-timestamp-anchored presentation deadline for `step_idx` of `total_steps`
/// spaced by `interval`, locking `Step 0` to `captured_at + lead_sec` and centering later steps
/// between the previous presentation and the next expected capture anchor.
pub fn anchored_present_deadline(
    captured_at: f64,
    lead_sec: f64,
    step_idx: usize,
    total_steps: usize,
    interval: f64,
    last_present: Option<f64>,
    now: f64,
) -> f64 {
    if !captured_at.is_finite() || !interval.is_finite() || interval <= 0.0 {
        return present_deadline(last_present, now, interval);
    }
    let anchor_base = captured_at + lead_sec.max(0.0);
    let ideal = anchor_base + (step_idx as f64) * interval;
    if step_idx == 0 || total_steps <= 1 {
        return ideal.max(now);
    }
    let next_capture_anchor = anchor_base + (total_steps as f64) * interval;
    let remaining_steps = total_steps.saturating_sub(step_idx).max(1) as f64;
    let target = if let Some(prev) = last_present {
        let balanced = prev + (next_capture_anchor - prev) / (remaining_steps + 1.0);
        balanced.max(ideal).min(prev + interval)
    } else {
        ideal
    };
    target.max(now)
}

/// Fixed-capacity frame-time and GPU profiler computing rolling `1% low` FPS with zero per-frame allocations.
#[derive(Debug, Clone)]
pub struct FrametimeProfiler {
    intervals_sec: [f64; 128],
    gpu_window_ms: [f32; 128],
    head: usize,
    len: usize,
    last_present_sec: Option<f64>,
    cap_ms: f32,
    gpu_ms: f32,
    present_ms: f32,
}

impl Default for FrametimeProfiler {
    fn default() -> Self {
        Self::new()
    }
}

impl FrametimeProfiler {
    /// Creates a new zero-allocation profiler with a 128-frame rolling window.
    pub fn new() -> Self {
        Self {
            intervals_sec: [0.0; 128],
            gpu_window_ms: [0.0; 128],
            head: 0,
            len: 0,
            last_present_sec: None,
            cap_ms: 0.0,
            gpu_ms: 0.0,
            present_ms: 0.0,
        }
    }

    /// Records a presented frame timestamp in seconds alongside `cap_ms`, `gpu_ms`, and `present_ms`.
    /// Returns the measured output interval in seconds when a previous timestamp was present.
    pub fn record_present_timestamp(
        &mut self,
        now_sec: f64,
        cap_ms: f32,
        gpu_ms: f32,
        present_ms: f32,
    ) -> Option<f64> {
        if !now_sec.is_finite()
            || self
                .last_present_sec
                .is_some_and(|previous| now_sec <= previous)
        {
            return None;
        }
        if cap_ms.is_finite() && cap_ms >= 0.0 {
            self.cap_ms = cap_ms;
        }
        if gpu_ms.is_finite() && gpu_ms >= 0.0 {
            self.gpu_ms = gpu_ms;
        }
        if present_ms.is_finite() && present_ms >= 0.0 {
            self.present_ms = present_ms;
        }
        let prev = self.last_present_sec.replace(now_sec)?;
        let dt = now_sec - prev;
        if !dt.is_finite() || dt <= 0.0 {
            return None;
        }
        self.intervals_sec[self.head] = dt;
        self.gpu_window_ms[self.head] = self.gpu_ms;
        self.head = (self.head + 1) % self.intervals_sec.len();
        self.len = (self.len + 1).min(self.intervals_sec.len());
        Some(dt)
    }

    /// Breaks the active timestamp run (e.g. on pause/resize) without counting the gap as a frame spike.
    pub fn break_sequence(&mut self) {
        self.last_present_sec = None;
    }

    /// Computes rolling realized output FPS from measured presentation intervals.
    pub fn average_fps(&self) -> f32 {
        if self.len < 4 {
            return 0.0;
        }
        let sum: f64 = self.intervals_sec[..self.len].iter().sum();
        if sum > 0.0 {
            ((self.len as f64) / sum) as f32
        } else {
            0.0
        }
    }

    /// Computes `1% low` presented FPS from the worst 1% of recorded output intervals.
    pub fn one_percent_low_fps(&self) -> f32 {
        if self.len == 0 {
            return 0.0;
        }
        let mut sorted = [0.0f64; 128];
        sorted[..self.len].copy_from_slice(&self.intervals_sec[..self.len]);
        sorted[..self.len].sort_by(|a, b| b.total_cmp(a));
        let worst_count = (self.len / 100).max(1);
        let avg_worst = sorted[..worst_count].iter().sum::<f64>() / (worst_count as f64);
        if avg_worst > 0.0 {
            (1.0 / avg_worst) as f32
        } else {
            0.0
        }
    }

    /// Latest source capture interval/latency in milliseconds.
    pub fn cap_ms(&self) -> f32 {
        self.cap_ms
    }

    /// Latest GPU compute duration (motion + interp + upscale) in milliseconds.
    pub fn gpu_ms(&self) -> f32 {
        self.gpu_ms
    }

    /// Latest swapchain `Present` duration in milliseconds.
    pub fn present_ms(&self) -> f32 {
        self.present_ms
    }

    /// Fastest and slowest GPU generation time (ms) across the rolling window; `(0, 0)` when empty.
    pub fn gpu_ms_range(&self) -> (f32, f32) {
        self.gpu_window_ms[..self.len]
            .iter()
            .fold(None, |range: Option<(f32, f32)>, &ms| {
                Some(range.map_or((ms, ms), |(lo, hi)| (lo.min(ms), hi.max(ms))))
            })
            .unwrap_or((0.0, 0.0))
    }
}

/// Hybrid frame pacer combining `SetWaitableTimerEx` coarse sleep with QPC spin-wait.
pub struct HybridPacer {
    timer: HANDLE,
    qpc_freq: i64,
    spin_threshold_sec: f64,
}

impl HybridPacer {
    /// Creates a new high-resolution waitable timer + QPC pacer.
    pub fn new() -> windows::core::Result<Self> {
        let mut freq = 0i64;
        unsafe { QueryPerformanceFrequency(&mut freq)? };
        let timer = unsafe {
            CreateWaitableTimerExW(
                None,
                None,
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                TIMER_ALL_ACCESS.0,
            )
            .or_else(|_| CreateWaitableTimerExW(None, None, 0, TIMER_ALL_ACCESS.0))?
        };
        Ok(Self {
            timer,
            qpc_freq: freq.max(1),
            spin_threshold_sec: 0.0015, // 1.5 ms spin tail
        })
    }

    /// Returns the current QPC timestamp in seconds.
    pub fn now_sec(&self) -> f64 {
        let mut count = 0i64;
        unsafe {
            let _ = QueryPerformanceCounter(&mut count);
        }
        self.qpc_seconds(count)
    }

    /// Converts a source timestamp from performance-counter ticks to QPC seconds.
    pub fn qpc_seconds(&self, ticks: i64) -> f64 {
        ticks as f64 / self.qpc_freq as f64
    }

    /// Returns the QPC spin-wait tail threshold in seconds.
    pub fn spin_threshold_sec(&self) -> f64 {
        self.spin_threshold_sec
    }

    /// Sleeps until `target_sec` (in QPC seconds) using `SetWaitableTimerEx` + QPC spin-wait.
    pub fn wait_until_sec(&self, target_sec: f64) {
        let now = self.now_sec();
        let remaining = target_sec - now;
        if remaining <= 0.0 {
            return;
        }

        if remaining > self.spin_threshold_sec {
            let coarse_sec = remaining - self.spin_threshold_sec;
            // Relative due time in negative 100-nanosecond intervals.
            let due_100ns = -((coarse_sec * 10_000_000.0) as i64).max(1);
            unsafe {
                if SetWaitableTimerEx(self.timer, &due_100ns, 0, None, None, None, 0).is_ok() {
                    WaitForSingleObject(self.timer, INFINITE);
                }
            }
        }

        while self.now_sec() < target_sec {
            std::hint::spin_loop();
        }
    }
}

impl Drop for HybridPacer {
    fn drop(&mut self) {
        if !self.timer.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.timer);
            }
        }
    }
}

// SAFETY: `HANDLE` is a kernel waitable timer owned exclusively by `HybridPacer` and closed on drop.
unsafe impl Send for HybridPacer {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage2_tracker_caps_untrusted_capacity() {
        assert_eq!(FrametimeTracker::new(usize::MAX, 6.0).window_size, 1024);
        assert_eq!(FrametimeTracker::new(0, 6.0).window_size, 1);
    }

    #[test]
    fn stage2_expired_deadlines_skip_slots_without_short_catchup() {
        let dt = 1.0 / 144.0;
        for late in [0.0005, 0.001, 0.004, 0.1] {
            let deadline = present_deadline(Some(1.0), 1.0 + dt + late, dt);
            assert!(deadline >= 1.0 + dt + late);
            let next = present_deadline(Some(deadline), deadline + 0.0003, dt);
            assert!((next - deadline - dt).abs() < 1e-9);
            assert!(
                ((deadline - 1.0) / dt).fract().abs() < 1e-9
                    || ((deadline - 1.0) / dt).fract() > 0.999999
            );
        }
    }

    #[test]
    fn late_submissions_skip_the_short_catch_up_slot() {
        let mut timeline = OutputTimeline::default();
        let dt = 1.0 / 142.0;
        let first = timeline.schedule(1.0, dt, 1, 1.0, 0.5).unwrap();
        let submitted = first.deadline + 0.0064;
        timeline.record_submission(submitted);
        let next = timeline.schedule(submitted + 0.0005, dt, 1, 1.0, 1.0).unwrap();
        assert!((next.deadline - first.deadline - 2.0 * dt).abs() < 1e-9);
        assert_eq!(timeline.expired_slots, 1);
    }

    #[test]
    fn stage2_replay_source_rates_jitter_bursts_stalls_and_gpu_work() {
        for hz in [60.0, 71.0, 72.0, 90.0] {
            for jitter in [0.0005, 0.001, 0.004] {
                for gpu in [0.0005, 0.0018, 0.004] {
                    for mode in [
                        InterpolationMode::Fixed { multiplier: 2.0 },
                        InterpolationMode::Adaptive { target_fps: 144.0 },
                    ] {
                        let mut timeline = OutputTimeline::default();
                        let mut scheduler = InterpolationScheduler::new(mode, 10.0, 0.05);
                        let mut ready: f64 = 1.0;
                        let mut last: Option<ScheduledOutput> = None;
                        for source in 1..=180 {
                            let captured = 1.0
                                + source as f64 / hz
                                + if source % 2 == 0 { jitter } else { -jitter };
                            if source == 90 {
                                ready += 0.1;
                                timeline.reset();
                                scheduler.reset_phase();
                            }
                            ready = ready.max(captured);
                            for step in scheduler.plan_interval(1.0 / hz) {
                                ready += gpu;
                                let ticket = timeline
                                    .schedule(
                                        ready,
                                        step.output_dt_sec,
                                        source,
                                        captured,
                                        step.alpha,
                                    )
                                    .unwrap();
                                if let Some(prev) = last {
                                    assert!(
                                        ticket.output_id > prev.output_id
                                            && ticket.source_id >= prev.source_id
                                    );
                                    assert!(
                                        ticket.deadline - prev.deadline
                                            >= step.output_dt_sec - 1e-9,
                                        "{hz} {jitter} {gpu}"
                                    );
                                }
                                assert!(
                                    ticket.deadline >= ready
                                        && ticket.alpha > 0.0
                                        && ticket.alpha <= 1.0
                                );
                                ready = ticket.deadline;
                                last = Some(ticket);
                            }
                            assert!(timeline
                                .schedule(ready, 1.0 / 144.0, source - 1, captured, 1.0)
                                .is_none());
                        }
                        assert!(timeline.output_id > 180);
                    }
                }
            }
        }
    }

    #[test]
    fn profiler_reports_fastest_and_slowest_generation_time_in_window() {
        let mut profiler = FrametimeProfiler::new();
        assert_eq!(profiler.gpu_ms_range(), (0.0, 0.0));
        for (i, gpu) in [2.0, 1.5, 4.0, 2.5].into_iter().enumerate() {
            let _ = profiler.record_present_timestamp(1.0 + i as f64 / 120.0, 0.0, gpu, 0.0);
        }
        // The first sample only seeds the timestamp, so its 2.0 ms never enters the window.
        assert_eq!(profiler.gpu_ms_range(), (1.5, 4.0));
    }

    #[test]
    fn rejected_timestamps_do_not_corrupt_the_next_interval() {
        let mut tracker = FrametimeTracker::new(4, 6.0);
        let mut profiler = FrametimeProfiler::new();
        assert_eq!(tracker.record_timestamp(1.0), None);
        assert_eq!(profiler.record_present_timestamp(1.0, 0.0, 0.0, 0.0), None);
        for bad in [f64::NAN, f64::INFINITY, 0.5, 1.0] {
            assert_eq!(tracker.record_timestamp(bad), None);
            assert_eq!(profiler.record_present_timestamp(bad, 0.0, 0.0, 0.0), None);
        }
        assert!((tracker.record_timestamp(1.01).unwrap() - 0.01).abs() < 1e-9);
        assert!(
            (profiler
                .record_present_timestamp(1.01, 0.0, 0.0, 0.0)
                .unwrap()
                - 0.01)
                .abs()
                < 1e-9
        );
    }

    #[test]
    fn invalid_fixed_multiplier_never_emits_nan_frames() {
        for multiplier in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut scheduler =
                InterpolationScheduler::new(InterpolationMode::Fixed { multiplier }, 10.0, 0.05);
            for _ in 0..2 {
                let steps = scheduler.plan_interval(1.0 / 60.0);
                assert_eq!(steps.len(), 1);
                assert_eq!(steps[0].alpha, 1.0);
                assert!(!steps[0].is_interpolated);
                assert!((steps[0].output_dt_sec - 1.0 / 60.0).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn qpc_capture_cadence_survives_an_idle_gap() {
        let pacer = HybridPacer::new().unwrap();
        let mut tracker = FrametimeTracker::new(8, 6.0);
        let period = pacer.qpc_freq / 50;
        assert!(!tracker.record_capture_timestamp(pacer.qpc_seconds(period)));
        for n in 2..=5 {
            assert!(tracker.record_capture_timestamp(pacer.qpc_seconds(n * period)));
        }
        assert!((tracker.estimated_fps() - 50.0).abs() < 0.01);
        assert!(!tracker.record_capture_timestamp(pacer.qpc_seconds(300 * period)));
        assert!(tracker.record_capture_timestamp(pacer.qpc_seconds(301 * period)));
        assert!(!tracker.record_capture_timestamp(pacer.qpc_seconds(299 * period)));
        assert!(!tracker.record_capture_timestamp(pacer.qpc_seconds(301 * period)));
        assert!(tracker.record_capture_timestamp(pacer.qpc_seconds(302 * period)));
        assert!((tracker.estimated_fps() - 50.0).abs() < 0.01);
        tracker.break_capture_sequence();
        assert!(!tracker.record_capture_timestamp(pacer.qpc_seconds(310 * period)));
        assert!((tracker.estimated_fps() - 50.0).abs() < 0.01);
    }

    #[test]
    fn tracker_adapts_to_persistent_slowdown_and_ignores_single_fast_burst() {
        let pacer = HybridPacer::new().unwrap();
        let mut tracker = FrametimeTracker::new(8, 4.0);
        let fast_p = pacer.qpc_freq / 120;
        for n in 1..=5 {
            tracker.record_capture_timestamp(pacer.qpc_seconds(n * fast_p));
        }
        assert!((tracker.estimated_fps() - 120.0).abs() < 0.1);

        // Single fast burst (< fast_p / 4) and NaN must not destroy 120 FPS cadence.
        tracker.record_delta(f64::NAN);
        tracker.record_delta(0.0005);
        assert!((tracker.estimated_fps() - 120.0).abs() < 0.1);

        // Persistent drop to 15 FPS (> idle_gap of ~33.3ms) adapts on consecutive slow frames.
        let slow_p = pacer.qpc_freq / 15;
        let t0 = 5 * fast_p;
        assert!(!tracker.record_capture_timestamp(pacer.qpc_seconds(t0 + slow_p)));
        assert!(tracker.record_capture_timestamp(pacer.qpc_seconds(t0 + 2 * slow_p)));
        assert!((tracker.estimated_fps() - 15.0).abs() < 0.1);
    }

    #[test]
    fn endpoint_tolerance_preserves_phase_and_slow_capture_resets_it() {
        let source_dt = 1.0 / 71.0;
        for (mode, tolerance_edge, frames) in [
            (InterpolationMode::Fixed { multiplier: 2.5 }, 0.2, 2),
            (InterpolationMode::Adaptive { target_fps: 144.0 }, 1.0 - 142.0 / 144.0, 2),
        ] {
            for margin in [-0.001, 0.001] {
                let mut scheduler = InterpolationScheduler::new(mode, 10.0, tolerance_edge + margin);
                let interval = scheduler.plan_interval(source_dt);
                assert_eq!(interval.len(), frames);
                assert!(interval[0].is_interpolated);
                let last = interval.last().unwrap();
                assert_eq!(last.is_interpolated, margin < 0.0);
                assert_eq!(last.alpha == 1.0, margin > 0.0);

                let slow = scheduler.plan_interval(1.0 / 9.9);
                assert_eq!(slow.len(), 1);
                assert_eq!(slow[0].alpha, 1.0);
                assert!(!slow[0].is_interpolated);
                assert_eq!(scheduler.accumulator, 0.0);

                let resumed = scheduler.plan_interval(1.0 / 10.1);
                assert!(resumed[0].is_interpolated);
                let mut fresh = InterpolationScheduler::new(mode, 10.0, tolerance_edge + margin);
                let expected = fresh.plan_interval(1.0 / 10.1);
                assert_eq!(
                    resumed.iter().map(|step| (step.alpha, step.is_interpolated)).collect::<Vec<_>>(),
                    expected.iter().map(|step| (step.alpha, step.is_interpolated)).collect::<Vec<_>>(),
                );
            }
        }
    }

    #[test]
    fn fixed_fractional_multiplier_uses_uniform_output_dt() {
        let mut sched =
            InterpolationScheduler::new(InterpolationMode::Fixed { multiplier: 2.5 }, 10.0, 0.05);
        let dt = 1.0 / 60.0;
        let s1 = sched.plan_interval(dt);
        let s2 = sched.plan_interval(dt);
        assert_eq!(s1.len(), 2);
        assert_eq!(s2.len(), 3);
        let expected_dt = dt / 2.5;
        for step in s1.iter().chain(s2.iter()) {
            assert!((step.output_dt_sec - expected_dt).abs() < 1e-9);
        }
    }

    #[test]
    fn fixed_2x_plan_interval_preserves_real_frame_and_uniform_step_dt() {
        let mut sched =
            InterpolationScheduler::new(InterpolationMode::Fixed { multiplier: 2.0 }, 10.0, 0.05);
        let source_dt = 1.0 / 72.0;
        let expected_out_dt = 1.0 / 144.0;
        for _ in 0..10 {
            let steps = sched.plan_interval(source_dt);
            assert_eq!(steps.len(), 2);
            assert!(steps[0].is_interpolated);
            assert!((steps[0].alpha - 0.5).abs() < 1e-6);
            assert!(!steps[1].is_interpolated);
            assert!((steps[1].alpha - 1.0).abs() < 1e-6);
            for step in &steps {
                assert!((step.output_dt_sec - expected_out_dt).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn present_deadline_absorbs_sub_interval_jitter_without_drift_and_resets_on_stall() {
        let dt = 1.0 / 144.0;
        let late = present_deadline(Some(1.0), 1.0 + dt + 0.004, dt);
        assert!((late - (1.0 + 2.0 * dt)).abs() < 1e-9);
        let next = present_deadline(Some(late), late + 0.0003, dt);
        assert!((next - late - dt).abs() < 1e-9);
    }

    #[test]
    fn frametime_profiler_computes_one_percent_low_and_timings() {
        let mut prof = FrametimeProfiler::new();
        assert_eq!(prof.one_percent_low_fps(), 0.0);

        let dt_144 = 1.0 / 144.0;
        let mut t = 1.0;
        assert!(prof.record_present_timestamp(t, 13.88, 1.1, 0.2).is_none());
        for i in 1..=100 {
            // Inject one slow 20ms frame (50 FPS) among 99 steady 144 Hz frames.
            let step = if i == 50 { 0.020 } else { dt_144 };
            t += step;
            let recorded = prof.record_present_timestamp(t, 13.88, 1.25, 0.35);
            assert!(recorded.is_some());
        }
        assert!((prof.one_percent_low_fps() - 50.0).abs() < 0.01);
        assert!((prof.cap_ms() - 13.88).abs() < 1e-4);
        assert!((prof.gpu_ms() - 1.25).abs() < 1e-4);
        assert!((prof.present_ms() - 0.35).abs() < 1e-4);

        // Breaking sequence skips the pause gap.
        prof.break_sequence();
        t += 1.0;
        assert!(prof.record_present_timestamp(t, 13.88, 1.0, 0.2).is_none());
    }

    #[test]
    fn tracker_ignores_submillisecond_clamped_bursts_and_requires_four_fast_outliers() {
        let mut tracker = FrametimeTracker::new(8, 4.0);
        let mut t = 1.0;
        assert!(!tracker.record_capture_timestamp(t));
        // Sub-0.5ms clamped-tick bursts must be rejected even before 3 samples exist.
        t += 0.0001;
        assert!(!tracker.record_capture_timestamp(t));
        assert!(tracker.is_empty());

        let dt_60 = 1.0 / 60.0;
        for _ in 0..4 {
            t += dt_60;
            assert!(tracker.record_capture_timestamp(t));
        }
        assert!((tracker.estimated_fps() - 60.0).abs() < 0.2);

        // Even 10 consecutive sub-0.5ms deltas must not corrupt 60 FPS cadence.
        for _ in 0..10 {
            t += 0.0001;
            assert!(!tracker.record_capture_timestamp(t));
        }
        assert!((tracker.estimated_fps() - 60.0).abs() < 0.2);

        // Up to 3 fast outliers (>= 0.5ms, < avg / 4) are skipped from samples_sec without
        // returning false from record_capture_timestamp (which would disable FG on that frame).
        let dt_500 = 0.002;
        for _ in 0..3 {
            t += dt_500;
            assert!(tracker.record_capture_timestamp(t));
        }
        assert_eq!(tracker.len(), 4);
        assert!((tracker.estimated_fps() - 60.0).abs() < 0.2);

        // The 4th consecutive fast interval adapts the cadence.
        t += dt_500;
        assert!(tracker.record_capture_timestamp(t));
        assert!((tracker.estimated_fps() - 500.0).abs() < 1.0);
    }

    #[test]
    fn superseded_timestamp_learns_average_interval_and_keeps_fg() {
        let mut tracker = FrametimeTracker::new(8, 6.0);
        let dt = 1.0 / 60.0;
        assert!(!tracker.record_superseded_timestamp(1.0, 1));
        assert!(tracker.record_capture_timestamp(1.0 + dt));
        assert!(tracker.record_capture_timestamp(1.0 + 2.0 * dt));
        assert!((tracker.estimated_fps() - 60.0).abs() < 0.01);

        assert!(tracker.record_superseded_timestamp(1.0 + 4.0 * dt, 1));
        assert_eq!(tracker.len(), 3);
        assert!((tracker.estimated_fps() - 60.0).abs() < 0.01);
        assert!(tracker.record_capture_timestamp(1.0 + 5.0 * dt));
        assert!((tracker.estimated_fps() - 60.0).abs() < 0.01);
    }

    #[test]
    fn pacing_frametime_tracks_tightening_cadence_faster_than_sma() {
        let mut tracker = FrametimeTracker::new(15, 6.0);
        let mut t = 1.0;
        let slow_dt = 1.0 / 68.0;
        let fast_dt = 1.0 / 74.0;
        for _ in 0..15 {
            t += slow_dt;
            tracker.record_capture_timestamp(t);
        }
        for _ in 0..4 {
            t += fast_dt;
            tracker.record_capture_timestamp(t);
        }
        assert!(
            tracker.pacing_frametime_sec() < tracker.average_frametime_sec(),
            "pacing_frametime_sec ({}) should respond faster than 15-frame SMA ({})",
            tracker.pacing_frametime_sec(),
            tracker.average_frametime_sec()
        );
    }

    #[test]
    fn jittered_60fps_fixed_2x_locks_to_uniform_120fps_steps() {
        let mut tracker = FrametimeTracker::new(15, 6.0);
        let mut sched = InterpolationScheduler::new(InterpolationMode::Fixed { multiplier: 2.0 }, 0.0, 0.01);
        let dt = 1.0 / 60.0;
        let mut t = 1.0;
        tracker.record_capture_timestamp(t);
        for i in 0..120 {
            // Realistic +/-0.8 ms DXGI present-timestamp jitter around a steady 60 FPS source.
            t += dt + if i % 2 == 0 { 0.0008 } else { -0.0008 };
            assert!(tracker.record_capture_timestamp(t));
            if i < 15 {
                continue;
            }
            let steps = sched.plan_interval(tracker.pacing_frametime_sec());
            assert_eq!(steps.len(), 2);
            assert!(steps[0].is_interpolated && !steps[1].is_interpolated);
            for step in steps {
                assert!((step.output_dt_sec - dt / 2.0).abs() < 0.00004, "out_dt {}", step.output_dt_sec);
            }
        }
    }

    #[test]
    fn consumer_paced_at_menu_rate_recovers_when_source_returns_to_60() {
        let mut tracker = FrametimeTracker::new(15, 6.0);
        let mut sched = InterpolationScheduler::new(InterpolationMode::Fixed { multiplier: 2.0 }, 0.0, 0.01);
        let source_dt = 1.0 / 60.0;
        for i in 0..=15 {
            tracker.record_capture_timestamp(1.0 + f64::from(i) / 20.0); // learned 20 FPS menu cadence
        }
        let start = 1.0 + 15.0 / 20.0;
        let (mut now, mut last_seen) = (start, start);
        // Must be back at the game's rate within 0.15 s, not ~1 s of visible stutter.
        while now - start < 0.15 {
            // The render loop spends one planned interval per source frame, then takes the newest frame.
            now += sched.plan_interval(tracker.pacing_frametime_sec()).iter().map(|s| s.output_dt_sec).sum::<f64>();
            let newest = start + ((now - start) / source_dt).floor() * source_dt;
            if newest <= last_seen {
                continue;
            }
            let superseded = (((newest - last_seen) / source_dt).round() as u32).saturating_sub(1);
            tracker.record_superseded_timestamp(newest, superseded);
            last_seen = newest;
        }
        let pacing = tracker.pacing_frametime_sec();
        assert!((pacing / source_dt - 1.0).abs() < 0.05, "stuck at {:.1} FPS", 1.0 / pacing);
    }

    #[test]
    fn tracker_and_scheduler_recover_full_rate_after_loading_stall() {
        let mut tracker = FrametimeTracker::new(15, 6.0);
        let mut sched = InterpolationScheduler::new(InterpolationMode::Fixed { multiplier: 2.0 }, 0.0, 0.01);
        let (fast, menu) = (1.0 / 72.0, 1.0 / 18.0);
        let mut t = 1.0;
        tracker.record_capture_timestamp(t);
        for _ in 0..30 {
            t += fast;
            tracker.record_capture_timestamp(t);
        }
        // 3 s loading stall, then an 18 FPS menu.
        t += 3.0;
        assert!(!tracker.record_capture_timestamp(t));
        sched.reset_phase();
        for _ in 0..30 {
            t += menu;
            tracker.record_capture_timestamp(t);
        }
        // Back in game at 72 FPS: pacing must return to ~144 output within ~0.3 s, not stay pinned.
        for _ in 0..20 {
            t += fast;
            assert!(tracker.record_capture_timestamp(t));
        }
        assert!((tracker.pacing_frametime_sec() / fast - 1.0).abs() < 0.05, "pacing {}", tracker.pacing_frametime_sec());
        let steps = sched.plan_interval(tracker.pacing_frametime_sec());
        assert_eq!(steps.len(), 2);
        assert!(steps.iter().all(|s| (s.output_dt_sec * 144.0 - 1.0).abs() < 0.05));

        let mut timeline = OutputTimeline::default();
        timeline.schedule(t, menu, 1, t, 1.0).unwrap();
        timeline.reset();
        let a = timeline.schedule(t, fast / 2.0, 2, t, 0.5).unwrap();
        let b = timeline.schedule(a.deadline - fast / 4.0, fast / 2.0, 2, t, 1.0).unwrap();
        assert!((b.deadline - a.deadline - fast / 2.0).abs() < 1e-9);
    }

    #[test]
    fn anchored_present_deadline_locks_to_capture_timestamp_and_balances_late_step0() {
        let dt = 1.0 / 70.0;
        let out_dt = dt * 0.5;
        let lead = 0.002;

        // Normal delivery (ready 0.8ms after t[n], well inside 2ms lead):
        // Step 0 hits t[n] + 2ms, Step 1 hits t[n] + 2ms + dt/2.
        let t0 = 10.0;
        let d0 = anchored_present_deadline(t0, lead, 0, 2, out_dt, Some(t0 - out_dt), t0 + 0.0008);
        assert!((d0 - (t0 + lead)).abs() < 1e-9);
        let d1 = anchored_present_deadline(t0, lead, 1, 2, out_dt, Some(d0), d0 + 0.0002);
        assert!((d1 - (t0 + lead + out_dt)).abs() < 1e-9);
        // Next frame at t0 + dt stays uniformly spaced by out_dt.
        let t1 = t0 + dt;
        let d2 = anchored_present_deadline(t1, lead, 0, 2, out_dt, Some(d1), t1 + 0.0010);
        assert!(((d2 - d1) - out_dt).abs() < 1e-9);

        // Late Step 0 delivery (+1.6ms past lead): Step 1 centers between actual Step 0 and next anchor.
        let late_now = t0 + lead + 0.0016;
        let d0_late =
            anchored_present_deadline(t0, lead, 0, 2, out_dt, Some(t0 - out_dt), late_now);
        assert!((d0_late - late_now).abs() < 1e-9);
        let d1_bal =
            anchored_present_deadline(t0, lead, 1, 2, out_dt, Some(d0_late), d0_late + 0.0002);
        assert!(((d1_bal - d0_late) - ((t1 + lead) - d1_bal)).abs() < 1e-9);
    }
}
