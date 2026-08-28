use std::time::Duration;

use frameflow_windows::{
    capture_primary_frame, enable_per_monitor_v2_dpi, graphics_capture_supported, list_displays,
};
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
    let displays = list_displays()?;

    println!("display_count={}", displays.len());
    for display in displays {
        println!(
            "{} primary={} bounds={},{},{}x{} dpi={}x{}",
            display.id,
            display.primary,
            display.bounds.x,
            display.bounds.y,
            display.bounds.width,
            display.bounds.height,
            display.dpi_x,
            display.dpi_y
        );
    }

    match graphics_capture_supported() {
        Ok(true) => match capture_primary_frame(Duration::from_secs(5)) {
            Ok(frame) => println!(
                "first_frame={}x{} timestamp_100ns={} arrival_ms={}",
                frame.width,
                frame.height,
                frame.system_relative_time_100ns,
                frame.arrival_latency_ms
            ),
            Err(error) => println!("first_frame_error={error}"),
        },
        Ok(false) => println!("wgc_supported=false"),
        Err(error) => println!("wgc_probe_error={error}"),
    }

    Ok(())
}
