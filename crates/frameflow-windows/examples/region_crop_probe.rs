use std::time::Duration;

use frameflow_capture::{PhysicalRect, RegionSelection};
use frameflow_windows::{capture_region_frame, enable_per_monitor_v2_dpi, list_displays};
use windows::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED};

struct WinRtApartment;

impl Drop for WinRtApartment {
    fn drop(&mut self) {
        unsafe { RoUninitialize() };
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    unsafe { RoInitialize(RO_INIT_MULTITHREADED)? };
    let _apartment = WinRtApartment;
    enable_per_monitor_v2_dpi()?;

    let display = list_displays()?
        .into_iter()
        .find(|display| display.primary)
        .ok_or("primary display is unavailable")?;
    let crop = PhysicalRect {
        x: (display.bounds.width / 4) as i32,
        y: (display.bounds.height / 4) as i32,
        width: display.bounds.width / 2,
        height: display.bounds.height / 2,
    };
    let selection = RegionSelection {
        display_id: display.id,
        desktop_rect: PhysicalRect {
            x: display.bounds.x + crop.x,
            y: display.bounds.y + crop.y,
            ..crop
        },
        capture_rect: crop,
    };

    match capture_region_frame(&selection, Duration::from_secs(5)) {
        Ok(frame) => println!(
            "gpu_crop={}x{} source={}x{} arrival_ms={}",
            frame.width,
            frame.height,
            display.bounds.width,
            display.bounds.height,
            frame.arrival_latency_ms
        ),
        Err(error) => println!("gpu_crop_error={error}"),
    }
    Ok(())
}
