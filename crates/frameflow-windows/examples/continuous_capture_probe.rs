use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

use frameflow_capture::{CaptureSession, PhysicalRect, RegionSelection};
use frameflow_windows::{enable_per_monitor_v2_dpi, list_displays, start_region_capture};
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
    let delivered = Arc::new(AtomicU64::new(0));
    let errors = Arc::new(Mutex::new(Vec::new()));
    let mut session = start_region_capture(&selection, {
        let delivered = Arc::clone(&delivered);
        let errors = Arc::clone(&errors);
        move |result| match result {
            Ok(frame) if frame.width == crop.width && frame.height == crop.height => {
                delivered.store(frame.sequence, Ordering::Release);
            }
            Ok(frame) => errors.lock().unwrap().push(format!(
                "unexpected frame size {}x{}",
                frame.width, frame.height
            )),
            Err(error) => errors.lock().unwrap().push(error.to_string()),
        }
    })?;

    thread::sleep(Duration::from_secs(2));
    session.pause()?;
    // pause() returns only after the capture worker has acknowledged the state
    // change, so frames completed before that acknowledgement are not counted
    // as paused delivery.
    let paused_at = delivered.load(Ordering::Acquire);
    thread::sleep(Duration::from_millis(750));
    let during_pause = delivered.load(Ordering::Acquire);
    session.resume()?;
    thread::sleep(Duration::from_secs(1));
    let after_resume = delivered.load(Ordering::Acquire);
    session.stop()?;

    println!(
        "continuous_frames={} paused_delta={} resumed_delta={} crop={}x{} errors={}",
        after_resume,
        during_pause.saturating_sub(paused_at),
        after_resume.saturating_sub(during_pause),
        crop.width,
        crop.height,
        errors.lock().unwrap().len()
    );
    for error in errors.lock().unwrap().iter() {
        println!("capture_error={error}");
    }
    Ok(())
}
