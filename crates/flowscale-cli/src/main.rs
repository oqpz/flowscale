//! Clean-room CLI for `flowscale` diagnostics, GPU checks, pacing benchmarks, and scaling.

use std::env;
use std::process;
use std::thread;
use std::time::Duration;

use flowscale_abi::{CaptureApi, FrameGenerationType, ScalingProfile, ScalingType};
use flowscale_core::EngineRuntime;
use flowscale_gpu::D3d11Context;
use flowscale_overlay::{
    enumerate_adapters, enumerate_displays, get_dwm_refresh_rate, get_foreground_window,
    is_windows_build_at_least,
};
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};

fn print_usage() {
    eprintln!(
        "flowscale-cli\n\
         Usage:\n  \
           flowscale-cli adapters\n  \
           flowscale-cli displays\n  \
           flowscale-cli dwm-rate\n  \
           flowscale-cli check-gpu [gpu_id]\n  \
           flowscale-cli bench-pacing [frames | --frames <n>]\n  \
           flowscale-cli scale [--hwnd <id>] [--scale <factor>] [--algo <off|nearest|integer|bilinear|bicubic|fsr|nis|ls1|anime4k|sgsr|xbr>] [--capture <dxgi|wgc|gdi>] [--fg] [--duration-ms <ms>]"
    );
}

fn parse_algo(s: &str) -> Result<ScalingType, String> {
    match s.to_ascii_lowercase().as_str() {
        "off" => Ok(ScalingType::Off),
        "nearest" => Ok(ScalingType::Nearest),
        "integer" => Ok(ScalingType::Integer),
        "bilinear" | "sharp-bilinear" => Ok(ScalingType::SharpBilinear),
        "bicubic" | "bicubic-cas" => Ok(ScalingType::BicubicCas),
        "fsr" => Ok(ScalingType::Fsr),
        "nis" => Ok(ScalingType::Nis),
        "ls1" => Ok(ScalingType::Ls1),
        "anime4k" => Ok(ScalingType::Anime4k),
        "sgsr" => Ok(ScalingType::Sgsr),
        "xbr" => Ok(ScalingType::Xbr),
        other => Err(format!("Unknown scaling algorithm '{other}'")),
    }
}

fn parse_capture_api(s: &str) -> Result<CaptureApi, String> {
    match s.to_ascii_lowercase().as_str() {
        "wgc" => Ok(CaptureApi::Wgc),
        "dxgi" => Ok(CaptureApi::Dxgi),
        "gdi" => Ok(CaptureApi::Gdi),
        other => Err(format!("Unknown capture backend '{other}'")),
    }
}

fn parse_bench_frames(args: &[String]) -> Result<u32, String> {
    let raw = match args {
        [] => return Ok(60),
        [value] if !value.starts_with('-') => value,
        [flag, value] if flag == "--frames" => value,
        _ => return Err("Usage: flowscale-cli bench-pacing [frames | --frames <n>]".into()),
    };
    raw.parse::<u32>()
        .map_err(|_| format!("Invalid frame count '{raw}'; expected an unsigned integer"))
}

fn parse_gpu_id(args: &[String]) -> Result<i32, String> {
    match args {
        [] => Ok(0),
        [raw] => raw.parse::<i32>().map_err(|_| format!("Invalid adapter ID '{raw}'; expected a signed integer")),
        _ => Err("Usage: flowscale-cli check-gpu [gpu_id]".into()),
    }
}

fn parse_hwnd_arg(raw: &str) -> Result<isize, String> {
    let trimmed = raw.trim();
    let parsed = if let Some(hex) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        if hex.is_empty() {
            return Err(format!("Invalid --hwnd value '{raw}'"));
        }
        isize::from_str_radix(hex, 16)
            .map_err(|_| format!("Invalid --hwnd hex value '{raw}'"))?
    } else {
        trimmed
            .parse::<isize>()
            .map_err(|_| format!("Invalid --hwnd value '{raw}'; expected integer or 0x... hex"))?
    };
    if parsed <= 0 {
        return Err(format!(
            "Invalid --hwnd value '{raw}'; HWND must be a positive nonzero handle"
        ));
    }
    Ok(parsed)
}

fn main() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        print_usage();
        process::exit(1);
    }

    match args[1].as_str() {
        "adapters" => match enumerate_adapters(true) {
            Ok(list) => {
                for a in list {
                    println!(
                        "[{}] {} (vendor=0x{:04X}, device=0x{:04X}, sw={})",
                        a.index, a.description, a.vendor_id, a.device_id, a.is_software
                    );
                }
            }
            Err(e) => {
                eprintln!("Error enumerating adapters: {e}");
                process::exit(1);
            }
        },
        "displays" => match enumerate_displays() {
            Ok(list) => {
                for d in list {
                    println!(
                        "[{}] {} ({},{} -> {},{})",
                        d.index, d.device_name, d.left, d.top, d.right, d.bottom
                    );
                }
            }
            Err(e) => {
                eprintln!("Error enumerating displays: {e}");
                process::exit(1);
            }
        },
        "dwm-rate" => {
            let hz = get_dwm_refresh_rate();
            let w10_2004 = is_windows_build_at_least(19041);
            let w11 = is_windows_build_at_least(22000);
            println!("DWM refresh rate: {hz:.3} Hz");
            println!("Windows >= 19041: {w10_2004}, >= 22000 (Win11): {w11}");
        }
        "check-gpu" => {
            let gpu_id = match parse_gpu_id(args.get(2..).unwrap_or(&[])) {
                Ok(id) => id,
                Err(error) => { eprintln!("{error}"); process::exit(2); }
            };
            match D3d11Context::new(gpu_id) {
                Ok(ctx) => {
                    println!("Adapter: {}", ctx.caps.adapter_name);
                    println!("Feature level: 0x{:X}", ctx.caps.feature_level);
                    println!(
                        "TypedUAVLoadAdditionalFormats: {}",
                        ctx.caps.typed_uav_load_additional_formats
                    );
                }
                Err(e) => {
                    eprintln!("D3D11 init failed: {e}");
                    process::exit(1);
                }
            }
        }
        "bench-pacing" => {
            let frames = match parse_bench_frames(args.get(2..).unwrap_or(&[])) {
                Ok(frames) => frames,
                Err(error) => {
                    eprintln!("{error}");
                    process::exit(2);
                }
            };
            let rt = EngineRuntime::new();
            match rt.run_gpu_benchmark(frames) {
                Ok((adapter, elapsed)) => {
                    let fps = (frames as f64) / elapsed;
                    println!(
                        "Adapter: {adapter} | {frames} compute frames in {:.2} ms ({fps:.1} FPS)",
                        elapsed * 1000.0
                    );
                }
                Err(e) => {
                    eprintln!("Benchmark failed: {e}");
                    process::exit(1);
                }
            }
        }
        "scale" => {
            let mut profile = ScalingProfile::default();
            let mut hwnd: isize = 0;
            let mut duration_ms: u64 = 100;
            let mut i = 2;
            while i < args.len() {
                match args[i].as_str() {
                    "--hwnd" if i + 1 < args.len() => {
                        match parse_hwnd_arg(&args[i + 1]) {
                            Ok(parsed) => hwnd = parsed,
                            Err(e) => {
                                eprintln!("{e}");
                                process::exit(2);
                            }
                        }
                        i += 2;
                    }
                    "--scale" if i + 1 < args.len() => {
                        match args[i + 1].parse::<f32>() {
                            Ok(f) if f.is_finite() => profile.scale_factor = f.clamp(1.0, 10.0),
                            _ => {
                                eprintln!("Invalid --scale value '{}'", args[i + 1]);
                                process::exit(2);
                            }
                        }
                        i += 2;
                    }
                    "--algo" if i + 1 < args.len() => {
                        match parse_algo(&args[i + 1]) {
                            Ok(algo) => profile.scaling_type = algo,
                            Err(e) => {
                                eprintln!("{e}");
                                process::exit(2);
                            }
                        }
                        i += 2;
                    }
                    "--capture" if i + 1 < args.len() => {
                        match parse_capture_api(&args[i + 1]) {
                            Ok(api) => profile.capture_api = api,
                            Err(e) => {
                                eprintln!("{e}");
                                process::exit(2);
                            }
                        }
                        i += 2;
                    }
                    "--fg" => {
                        profile.frame_generation_type = FrameGenerationType::Lsfg2;
                        i += 1;
                    }
                    "--duration-ms" if i + 1 < args.len() => {
                        match args[i + 1].parse::<u64>() {
                            Ok(ms) => duration_ms = ms,
                            Err(_) => {
                                eprintln!("Invalid --duration-ms value '{}'", args[i + 1]);
                                process::exit(2);
                            }
                        }
                        i += 2;
                    }
                    unknown => {
                        eprintln!("Unknown or incomplete flag '{unknown}'");
                        print_usage();
                        process::exit(2);
                    }
                }
            }
            if hwnd == 0 {
                hwnd = get_foreground_window().0 as isize;
            }
            if hwnd == 0 {
                eprintln!("No target foreground window found; pass --hwnd <id>");
                process::exit(1);
            }
            let mut rt = EngineRuntime::new();
            if !rt.init(None) {
                eprintln!("Failed to initialize scaling engine");
                rt.uninit();
                process::exit(1);
            }
            rt.apply_settings(profile);
            let ok = rt.activate(hwnd);
            if !ok {
                eprintln!("Failed to activate scaling for hwnd=0x{hwnd:X}");
                rt.uninit();
                process::exit(1);
            }
            println!("Activated hwnd=0x{hwnd:X}: {ok}");
            thread::sleep(Duration::from_millis(duration_ms));
            let _ = rt.activate(hwnd);
            rt.uninit();
        }
        _ => {
            print_usage();
            process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_gpu_rejects_malformed_ids() {
        assert_eq!(parse_gpu_id(&[]).unwrap(), 0);
        for (raw, id) in [("0",0),("1",1),("-1",-1)] { assert_eq!(parse_gpu_id(&[raw.into()]).unwrap(), id); }
        for raw in ["bad", "1.5", "2147483648", "", "0x1"] { assert!(parse_gpu_id(&[raw.into()]).is_err()); }
        assert!(parse_gpu_id(&["0".into(), "1".into()]).is_err());
    }

    #[test]
    fn parse_algo_maps_expected_variants() {
        assert_eq!(parse_algo("off").unwrap(), ScalingType::Off);
        assert_eq!(parse_algo("nearest").unwrap(), ScalingType::Nearest);
        assert_eq!(parse_algo("bilinear").unwrap(), ScalingType::SharpBilinear);
        assert_eq!(parse_algo("bicubic").unwrap(), ScalingType::BicubicCas);
        assert_eq!(parse_algo("fsr").unwrap(), ScalingType::Fsr);
        assert_eq!(parse_algo("ls1").unwrap(), ScalingType::Ls1);
        assert_eq!(parse_algo("sgsr").unwrap(), ScalingType::Sgsr);
        assert_eq!(parse_algo("xbr").unwrap(), ScalingType::Xbr);
        assert!(parse_algo("unknown").is_err());
        assert_eq!(parse_capture_api("gdi").unwrap(), CaptureApi::Gdi);
        assert!(parse_capture_api("invalid").is_err());
    }

    #[test]
    fn bench_pacing_accepts_positional_and_flagged_frames() {
        assert_eq!(parse_bench_frames(&[]).unwrap(), 60);
        assert_eq!(parse_bench_frames(&["30".into()]).unwrap(), 30);
        assert_eq!(
            parse_bench_frames(&["--frames".into(), "45".into()]).unwrap(),
            45
        );
    }

    #[test]
    fn bench_pacing_rejects_invalid_syntax() {
        for args in [
            vec!["--frames".into()],
            vec!["--frames".into(), "bad".into()],
            vec!["30".into(), "40".into()],
            vec!["--other".into(), "30".into()],
        ] {
            assert!(parse_bench_frames(&args).is_err(), "accepted {args:?}");
        }
    }

    #[test]
    fn parse_hwnd_arg_accepts_valid_and_rejects_invalid() {
        assert_eq!(parse_hwnd_arg("12345").unwrap(), 12345);
        assert_eq!(parse_hwnd_arg("0x1A2B").unwrap(), 0x1A2B);
        assert_eq!(parse_hwnd_arg("0XFF").unwrap(), 0xFF);
        assert_eq!(parse_hwnd_arg("  0x10 ").unwrap(), 16);

        for bad in ["", "0", "0x0", "-1", "0x", "0xZZ", "abc", "12.34"] {
            assert!(parse_hwnd_arg(bad).is_err(), "accepted invalid hwnd '{bad}'");
        }
    }
}
