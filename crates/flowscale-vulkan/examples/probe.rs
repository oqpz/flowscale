fn main() {
    let path = std::env::args_os()
        .nth(1)
        .expect("absolute software Vulkan driver path");
    let width: u32 = std::env::args()
        .nth(2)
        .map(|s| s.parse().unwrap())
        .unwrap_or(80);
    let height: u32 = std::env::args()
        .nth(3)
        .map(|s| s.parse().unwrap())
        .unwrap_or(48);
    match flowscale_vulkan::probe_runtime(std::path::Path::new(&path)) {
        Ok(name) => {
            println!("CPU Vulkan device: {name}");
            let mut pipeline =
                flowscale_vulkan::CpuInterpolator::new(std::path::Path::new(&path), width, height)
                    .unwrap();
            let frame = [37, 93, 151, 255].repeat(width as usize * height as usize);
            for alpha in [0.0, 0.5, 1.0] {
                let start = std::time::Instant::now();
                let pixels = pipeline.interpolate(&frame, &frame, alpha).unwrap();
                assert!(
                    pixels.iter().zip(&frame).all(|(a, b)| a.abs_diff(*b) <= 1),
                    "static frame changed"
                );
                println!(
                    "static alpha {alpha}: {:.2} ms, pixels verified",
                    start.elapsed().as_secs_f64() * 1000.0
                );
                println!(
                    "upload/luma, coarse, fine/smooth, interpolation/readback ms {:?}",
                    pipeline.stage_ms
                );
            }
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}
