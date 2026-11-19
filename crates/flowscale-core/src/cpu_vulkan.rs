use super::*;
use flowscale_abi::ScalingType;
use flowscale_capture::GdiPixels;
use flowscale_vulkan::{CpuInterpolator, CpuPresenter};
use std::{path::PathBuf, time::Instant};

struct Job {
    id: u64,
    width: u32,
    height: u32,
    previous: Arc<Vec<u8>>,
    current: Arc<Vec<u8>>,
}

#[derive(Default)]
struct Work {
    pending: Option<Job>,
    completed: Option<CompletedFrame>,
    stop: bool,
}

type CompletedFrame = (u64, Result<(Vec<u8>, f32), String>);

struct Worker {
    state: Arc<(Mutex<Work>, Condvar)>,
    handle: Option<JoinHandle<()>>,
}

impl Worker {
    fn new(runtime: PathBuf) -> Result<Self, String> {
        let state = Arc::new((Mutex::new(Work::default()), Condvar::new()));
        let shared = state.clone();
        let handle = thread::Builder::new()
            .name("FlowScale CPU Vulkan".into())
            .spawn(move || {
                let mut pipeline = None;
                let mut dimensions = (0, 0);
                loop {
                    let (lock, cv) = &*shared;
                    let job = {
                        let mut work = lock.lock().unwrap();
                        while work.pending.is_none() && !work.stop {
                            work = cv.wait(work).unwrap();
                        }
                        if work.stop {
                            break;
                        }
                        work.pending.take().unwrap()
                    };
                    let start = Instant::now();
                    let result = (|| {
                        if dimensions != (job.width, job.height) {
                            pipeline = Some(CpuInterpolator::new(&runtime, job.width, job.height)?);
                            dimensions = (job.width, job.height);
                        }
                        let pixels = pipeline.as_mut().unwrap().interpolate(
                            &job.previous,
                            &job.current,
                            0.5,
                        )?;
                        Ok((pixels, start.elapsed().as_secs_f32() * 1000.0))
                    })();
                    let mut work = lock.lock().unwrap();
                    if work.stop {
                        break;
                    }
                    work.completed = Some((job.id, result));
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            state,
            handle: Some(handle),
        })
    }

    fn submit(&self, job: Job) {
        let mut state = self.state.0.lock().unwrap();
        state.pending = Some(job);
        self.state.1.notify_one();
    }

    fn take(&self) -> Option<CompletedFrame> {
        self.state.0.lock().unwrap().completed.take()
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        {
            let mut state = self.state.0.lock().unwrap();
            state.stop = true;
            state.pending = None;
            self.state.1.notify_one();
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

pub(super) fn runtime_path() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let path = exe
        .parent()
        .ok_or("Executable folder unavailable")?
        .join("runtime/cpu-vulkan/vk_swiftshader.dll");
    if !path.is_file() {
        return Err("CPU Vulkan runtime is missing. Install the complete FlowScale package, including runtime/cpu-vulkan.".into());
    }
    Ok(path)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run(
    hwnd_raw: isize,
    profile: ScalingProfile,
    config: EngineConfig,
    live: &Arc<Mutex<(u64, ScalingProfile, EngineConfig)>>,
    stop: &AtomicBool,
    reporter: &SessionReporter,
    resized: bool,
    init: Option<mpsc::Sender<Result<(), String>>>,
) -> Result<SessionExit, String> {
    let mut init = init;
    let result = run_inner(
        hwnd_raw, &profile, &config, live, stop, reporter, resized, &mut init,
    );
    if let Err(error) = &result {
        let mut telemetry = reporter.telemetry.lock().unwrap();
        telemetry.processing_backend = "CPU Vulkan".into();
        telemetry.last_output_drop_reason = error.clone();
    }
    if let (Some(sender), Err(error)) = (init.take(), &result) {
        let _ = sender.send(Err(error.clone()));
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn run_inner(
    hwnd_raw: isize,
    profile: &ScalingProfile,
    config: &EngineConfig,
    live: &Arc<Mutex<(u64, ScalingProfile, EngineConfig)>>,
    stop: &AtomicBool,
    reporter: &SessionReporter,
    resized: bool,
    init: &mut Option<mpsc::Sender<Result<(), String>>>,
) -> Result<SessionExit, String> {
    if profile.hdr_support
        || profile.scaling_type != ScalingType::Off
        || profile.frame_generation_type != FrameGenerationType::Lsfg3
        || profile.frame_generation_mode != Lsfg3Mode::Fixed as i32
        || (profile.frame_generation_multiplier - 2.0).abs() > 0.001
        || profile.capture_offset_left != 0
        || profile.capture_offset_top != 0
        || profile.capture_offset_right != 0
        || profile.capture_offset_bottom != 0
    {
        return Err(
            "CPU Vulkan currently requires SDR, scaling Off, LSFG 3 Fixed 2x and cropping Off."
                .into(),
        );
    }
    let runtime = runtime_path()?;
    let name = flowscale_vulkan::probe_runtime(&runtime)?;
    eprintln!("FlowScale capture backend: CpuVulkan");
    eprintln!("CPU processing device: {name}; capture=GDI CPU");
    let hwnd = HWND(hwnd_raw as *mut std::ffi::c_void);
    let mut client = window_client_screen_rect(hwnd).ok_or("Target window is unavailable")?;
    let mut size = rect_size(client);
    let region = |size: (u32, u32)| CaptureRegion {
        left: 0,
        top: 0,
        width: size.0,
        height: size.1,
    };
    let mut capture = GdiPixels::new(hwnd, region(size)).map_err(|e| e.to_string())?;
    let overlay = OverlayWindow::create_software(client, false).map_err(|e| e.to_string())?;
    overlay
        .set_exclude_from_capture(true)
        .map_err(|e| format!("CPU overlay capture exclusion failed: {e}"))?;
    let mut presenter = CpuPresenter::new(&runtime, overlay.hwnd().0 as isize, size.0, size.1)?;
    let worker = Worker::new(runtime.clone())?;
    let watcher = WindowWatcher::new(hwnd, Some(overlay.hwnd()), None);
    let pacer = HybridPacer::new().map_err(|e| e.to_string())?;
    let version = live.lock().unwrap().0;
    let mut tracker = FrametimeTracker::new(
        config.capture.frametime_buffer_size,
        config.capture.frametime_buffer_reset_multiplier,
    );
    let mut previous: Option<Arc<Vec<u8>>> = None;
    let mut generation = 0u64;
    let mut midpoint: Option<(u64, f64, Option<Vec<u8>>)> = None;
    let mut next_capture = 0.0;
    let mut next_work = 0.0;
    let mut budget_misses = 0u8;
    let mut last_stats = pacer.now_sec();
    let mut captured = 0u32;
    let mut presented = 0u32;
    let mut paused = false;
    let mut clip = CursorClipGuard::new();
    let mut trace = OutputTrace::from_env();
    if let Some(trace) = trace.as_mut() {
        trace.event(
            "session",
            pacer.now_sec(),
            format_args!("backend=CpuVulkan driver={name:?}"),
        );
    }
    {
        let mut t = reporter.telemetry.lock().unwrap();
        t.processing_backend = format!("CPU Vulkan ({name})");
        t.requested_capture_api = "GDI CPU".into();
        t.actual_capture_api = "GDI CPU".into();
    }
    reporter.emit_state(
        StatusCode::Active,
        hwnd_raw,
        size.0 as i32,
        size.1 as i32,
        size.0 as i32,
        size.1 as i32,
        resized,
        1.0,
        ErrorCode::NoCode,
    );
    if let Some(sender) = init.take() {
        let _ = sender.send(Ok(()));
    }
    while !stop.load(Ordering::Acquire) {
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        if live.lock().unwrap().0 != version {
            return Ok(SessionExit::Restart);
        }
        let state = watcher.snapshot();
        if state.destroyed {
            break;
        }
        if !state.focused || state.minimized {
            overlay.set_visible(false);
            clip.release();
            previous = None;
            midpoint = None;
            tracker.break_capture_sequence();
            paused = true;
            thread::sleep(Duration::from_millis(12));
            continue;
        }
        let bounds = window_client_screen_rect(hwnd).ok_or("Target client bounds unavailable")?;
        if bounds != client || paused {
            let new_size = rect_size(bounds);
            overlay.set_visible(false);
            if new_size != size {
                // Drop the old presenter before destroying its native window.
                drop(presenter);
                overlay.set_bounds(bounds).map_err(|e| e.to_string())?;
                presenter =
                    CpuPresenter::new(&runtime, overlay.hwnd().0 as isize, new_size.0, new_size.1)?;
                capture = GdiPixels::new(hwnd, region(new_size)).map_err(|e| e.to_string())?;
                size = new_size;
                previous = None;
            } else {
                overlay.set_bounds(bounds).map_err(|e| e.to_string())?;
            }
            client = bounds;
            midpoint = None;
            tracker.break_capture_sequence();
            paused = false;
        }
        if profile.clip_cursor {
            clip.apply(client);
        }
        let now = pacer.now_sec();
        if let Some((id, result)) = worker.take() {
            let (pixels, ms) = result?;
            if let Some(trace) = trace.as_mut() {
                trace.event("cpu_work", now, format_args!("source={id} ms={ms:.3}"));
            }
            reporter.telemetry.lock().unwrap().cpu_ms = ms;
            if ms as f64 * 0.001 > tracker.average_frametime_sec().clamp(1.0 / 120.0, 0.1) * 0.5 {
                budget_misses = budget_misses.saturating_add(1);
                if budget_misses >= 3 {
                    next_work = now + 1.0;
                    worker.state.0.lock().unwrap().pending = None;
                    budget_misses = 0;
                }
            } else {
                budget_misses = 0;
                reporter
                    .telemetry
                    .lock()
                    .unwrap()
                    .last_output_drop_reason
                    .clear();
            }
            if let Some((expected, _, output)) = midpoint.as_mut() {
                if *expected == id {
                    *output = Some(pixels);
                }
            }
        }
        if midpoint
            .as_ref()
            .is_some_and(|(_, target, _)| now >= *target)
        {
            let (id, target, output) = midpoint.take().unwrap();
            if let Some(pixels) = output.filter(|_| now <= target + 0.002) {
                presenter.present(&pixels)?;
                presented += 1;
                if let Some(trace) = trace.as_mut() {
                    trace.event(
                        "cpu_present",
                        pacer.now_sec(),
                        format_args!("source={id} alpha=0.5 target={target:.9}"),
                    );
                }
            } else {
                let mut t = reporter.telemetry.lock().unwrap();
                t.dropped_outputs += 1;
                t.last_output_drop_reason = "CPU interpolation missed deadline".into();
            }
        }
        if now >= next_capture
            && midpoint
                .as_ref()
                .is_none_or(|(_, target, _)| *target - now > 0.006)
        {
            next_capture = now + 0.008;
            if let Some(pixels) = capture.acquire_window(4).map_err(|e| e.to_string())? {
                if previous.as_ref().is_none_or(|p| p.as_slice() != pixels) {
                    let frame = Arc::new(pixels.to_vec());
                    let captured_at = pacer.now_sec();
                    tracker.record_capture_timestamp(captured_at);
                    captured += 1;
                    generation += 1;
                    if let Some(old) = previous.replace(frame.clone()) {
                        let try_generation = captured_at >= next_work;
                        presenter.present(if try_generation { &old } else { &frame })?;
                        if let Some(trace) = trace.as_mut() {
                            trace.event(
                                "cpu_present",
                                pacer.now_sec(),
                                format_args!(
                                    "source={} alpha=1",
                                    if try_generation {
                                        generation - 1
                                    } else {
                                        generation
                                    }
                                ),
                            );
                        }
                        overlay.set_visible(true);
                        presented += 1;
                        let dt = tracker.average_frametime_sec().clamp(1.0 / 120.0, 0.1);
                        if try_generation {
                            midpoint = Some((generation, captured_at + dt * 0.5, None));
                            worker.submit(Job {
                                id: generation,
                                width: size.0,
                                height: size.1,
                                previous: old,
                                current: frame,
                            });
                        } else {
                            midpoint = None;
                        }
                    } else {
                        presenter.present(&frame)?;
                        if let Some(trace) = trace.as_mut() {
                            trace.event(
                                "cpu_present",
                                pacer.now_sec(),
                                format_args!("source={generation} alpha=1"),
                            );
                        }
                        overlay.set_visible(true);
                        presented += 1;
                    }
                    if let Some(trace) = trace.as_mut() {
                        trace.event(
                            "cpu_capture",
                            captured_at,
                            format_args!("source={generation} width={} height={}", size.0, size.1),
                        );
                    }
                }
            }
        }
        let now = pacer.now_sec();
        if now - last_stats >= 1.0 {
            let mut t = reporter.telemetry.lock().unwrap();
            t.captured_fps = captured as f32 / (now - last_stats) as f32;
            t.presented_fps = presented as f32 / (now - last_stats) as f32;
            t.input_width = size.0;
            t.input_height = size.1;
            t.output_width = size.0;
            t.output_height = size.1;
            t.submitted_outputs += u64::from(presented);
            t.source_id = generation as i64;
            captured = 0;
            presented = 0;
            last_stats = now;
        }
        thread::sleep(Duration::from_millis(1));
    }
    Ok(SessionExit::Stopped)
}
