use flowscale_overlay::OverlayWindow;
use windows::Win32::{Foundation::RECT, UI::WindowsAndMessaging::*};

fn main() {
    let path = std::env::args_os().nth(1).expect("software driver path");
    let path = std::path::Path::new(&path);
    let window = OverlayWindow::create_software(
        RECT {
            left: 30,
            top: 30,
            right: 190,
            bottom: 126,
        },
        true,
    )
    .unwrap();
    window.set_exclude_from_capture(true).unwrap();
    let mut presenter =
        flowscale_vulkan::CpuPresenter::new(path, window.hwnd().0 as isize, 160, 96).unwrap();
    for i in 0..60u8 {
        let frame = [i * 3, 71, 199, 255].repeat(160 * 96);
        presenter.present(&frame).unwrap();
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(16));
    }
    let mut capture = flowscale_capture::GdiPixels::new(
        window.hwnd(),
        flowscale_capture::CaptureRegion {
            left: 1,
            top: 0,
            width: 159,
            height: 96,
        },
    )
    .unwrap();
    let pixels = capture.acquire(0).unwrap().expect("CPU window pixels");
    println!(
        "readback sample {:?}",
        &pixels[40 * 159 * 4..40 * 159 * 4 + 4]
    );
    assert!(
        pixels
            .chunks_exact(4)
            .filter(|p| p[1] == 71 && p[2] == 199)
            .count()
            > 100,
        "CPU presentation did not produce the expected window pixels"
    );
    println!("CPU Vulkan Win32 presentation, real window pixels and capture exclusion passed");
}
