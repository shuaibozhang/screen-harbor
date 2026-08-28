use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

use frameflow_windows::start_microphone_capture;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let frames = Arc::new(AtomicU64::new(0));
    let callback_frames = Arc::clone(&frames);
    let session = start_microphone_capture(move |packet| match packet {
        Ok(packet) => {
            callback_frames.fetch_add(u64::from(packet.frames), Ordering::Relaxed);
        }
        Err(error) => eprintln!("microphone error: {error}"),
    })?;
    let format = session.format();
    thread::sleep(Duration::from_secs(2));
    session.stop()?;
    println!(
        "frames={} format={format:?}",
        frames.load(Ordering::Relaxed)
    );
    Ok(())
}
