use std::{env, path::PathBuf, thread, time::Duration};

use frameflow_capture::{PhysicalRect, RegionSelection};
use frameflow_recorder::{AudioInputConfig, CaptureEngine, CaptureOutputSize};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    frameflow_windows::enable_per_monitor_v2_dpi()?;
    let output = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("frameflow-capture-engine-probe.mp4"));
    let temporary = output.with_extension("mp4.part");
    let display = frameflow_windows::list_displays()?
        .into_iter()
        .find(|display| display.primary)
        .ok_or("primary display is unavailable")?;
    let width = display.bounds.width & !1;
    let height = display.bounds.height & !1;
    let capture_rect = PhysicalRect {
        x: 0,
        y: 0,
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
    let engine = CaptureEngine::start(
        &selection,
        CaptureOutputSize::new(1920, 1080)?,
        &temporary,
        &output,
        60,
        20_000_000,
        AudioInputConfig::default(),
        |event| eprintln!("capture event: {event:?}"),
    )?;
    thread::sleep(Duration::from_secs(5));
    let stats = engine.stats()?;
    let finalized = engine.finalize()?;
    println!(
        "capture_engine duration_ms={} frames={} fps={:.1} dropped={} audio_dropped={} path={}",
        finalized.duration.as_millis(),
        stats.frame_count,
        stats.effective_fps,
        stats.dropped_frames,
        stats.audio.map_or(0, |audio| audio.dropped_packets),
        finalized.output_path.display(),
    );
    Ok(())
}
