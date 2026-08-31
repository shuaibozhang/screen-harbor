use std::{
    env,
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use frameflow_capture::{CaptureSession, PhysicalRect, RegionSelection};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    frameflow_windows::enable_per_monitor_v2_dpi()?;
    let mut arguments = env::args_os().skip(1);
    let output = arguments
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("frameflow-h264-probe.mp4"));
    let requested_output_size = match (arguments.next(), arguments.next()) {
        (Some(width), Some(height)) => Some((
            width.to_string_lossy().parse::<u32>()?,
            height.to_string_lossy().parse::<u32>()?,
        )),
        (None, None) => None,
        _ => return Err("output width and height must be provided together".into()),
    };
    let display = frameflow_windows::list_displays()?
        .into_iter()
        .find(|display| display.primary)
        .ok_or("primary display is unavailable")?;
    let width = (display.bounds.width / 2) & !1;
    let height = (display.bounds.height / 2) & !1;
    let capture_rect = PhysicalRect {
        x: ((display.bounds.width - width) / 2) as i32,
        y: ((display.bounds.height - height) / 2) as i32,
        width,
        height,
    };
    let selection = RegionSelection {
        display_id: display.id,
        desktop_rect: PhysicalRect {
            x: display.bounds.x + capture_rect.x,
            y: display.bounds.y + capture_rect.y,
            width,
            height,
        },
        capture_rect,
    };

    let first_error = Arc::new(Mutex::new(None));
    let options = match requested_output_size {
        Some((width, height)) => {
            frameflow_windows::Mp4CaptureOptions::default().with_output_size(width, height)?
        }
        None => frameflow_windows::Mp4CaptureOptions::default(),
    };
    let session = frameflow_windows::start_region_mp4_capture(&selection, &output, options, {
        let first_error = Arc::clone(&first_error);
        move |result| {
            if let Err(error) = result {
                if let Ok(mut slot) = first_error.lock() {
                    slot.get_or_insert_with(|| error.to_string());
                }
            }
        }
    })?;
    let encoder = session.video_encoder_info();
    thread::sleep(Duration::from_secs(5));
    let stop_result = session.stop();
    if let Some(error) = first_error.lock().ok().and_then(|slot| slot.clone()) {
        return Err(error.into());
    }
    stop_result?;
    let bytes = std::fs::metadata(&output)?.len();
    println!(
        "h264_mp4_source={}x{} output={:?} duration_s=5 encoder={:?} bytes={} path={}",
        width,
        height,
        requested_output_size.unwrap_or((width, height)),
        encoder,
        bytes,
        output.display()
    );
    Ok(())
}
