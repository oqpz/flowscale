use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

const SHADERS: &[&str] = &[
    "nearest.hlsl",
    "bilinear_sharp.hlsl",
    "bicubic_cas.hlsl",
    "spatial_easu.hlsl",
    "nis_sharpen.hlsl",
    "ls1_detail.hlsl",
    "anime4k_restore.hlsl",
    "sgsr_mobile.hlsl",
    "xbr_pixel.hlsl",
    "motion_est.hlsl",
    "motion_luma.hlsl",
    "motion_smooth.hlsl",
    "frame_interp.hlsl",
    "overlay_blend.hlsl",
    "sdr_to_scrgb.hlsl",
];

fn find_fxc() -> PathBuf {
    if let Ok(p) = env::var("FXC_PATH") {
        let path = PathBuf::from(p);
        if path.is_file() {
            return path;
        }
    }

    let preferred = Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64\fxc.exe");
    if preferred.is_file() {
        return preferred.to_path_buf();
    }

    let kits_bin = Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    if let Ok(entries) = std::fs::read_dir(kits_bin) {
        let mut candidates: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path().join("x64").join("fxc.exe"))
            .filter(|p| p.is_file())
            .collect();
        candidates.sort();
        if let Some(latest) = candidates.pop() {
            return latest;
        }
    }

    PathBuf::from("fxc.exe")
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=FXC_PATH");

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set"));
    let shader_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("shaders");
    let fxc = find_fxc();

    for shader in SHADERS {
        let src = shader_dir.join(shader);
        println!("cargo:rerun-if-changed={}", src.display());

        let stem = Path::new(shader).file_stem().unwrap().to_str().unwrap();
        let mut entries = vec![(stem, "main")];
        if stem == "motion_est" {
            entries.push(("motion_est_coarse", "coarse"));
        }
        for (stem, entry) in entries {
            let dst = out_dir.join(format!("{stem}.cso"));
            let output = Command::new(&fxc)
                .args([
                    "/nologo",
                    "/WX",
                    "/T",
                    "cs_5_0",
                    "/E",
                    entry,
                    "/O3",
                    "/Fo",
                    dst.to_str().unwrap(),
                    src.to_str().unwrap(),
                ])
                .output()
                .unwrap_or_else(|e| panic!("failed to invoke fxc ({}): {e}", fxc.display()));

            if !output.status.success() {
                panic!(
                    "fxc failed for {}:\n{}\n{}",
                    src.display(),
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }
    }
}
