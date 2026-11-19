use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-env-changed=DXC_PATH");
    println!("cargo:rerun-if-env-changed=VULKAN_SDK");
    let dxc = env::var_os("DXC_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            env::var_os("VULKAN_SDK")
                .map(PathBuf::from)
                .map(|p| p.join("Bin/dxc.exe"))
                .unwrap_or_else(|| PathBuf::from("dxc.exe"))
        });
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    for (shader, entry, name) in [
        ("motion_luma", "main", "luma"),
        ("motion_est", "coarse", "coarse"),
        ("motion_est", "main", "motion"),
        ("motion_smooth", "main", "smooth"),
        ("frame_interp", "cpu", "interpolate"),
    ] {
        let source = PathBuf::from("../flowscale-gpu/shaders").join(format!("{shader}.hlsl"));
        println!("cargo:rerun-if-changed={}", source.display());
        let result = Command::new(&dxc)
            .args([
                "-spirv",
                "-T",
                "cs_6_0",
                "-E",
                entry,
                "-O3",
                "-WX",
                "-fspv-target-env=vulkan1.1",
                "-fspv-entrypoint-name=main",
                "-fvk-t-shift",
                "0",
                "0",
                "-fvk-u-shift",
                "8",
                "0",
                "-fvk-b-shift",
                "16",
                "0",
                "-fvk-s-shift",
                "24",
                "0",
                "-Fo",
            ])
            .arg(output.join(format!("{name}.spv")))
            .arg(&source)
            .output()
            .expect("DXC with SPIR-V support is required for CPU Vulkan shaders");
        assert!(
            result.status.success(),
            "DXC failed for {shader}: {}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
}
