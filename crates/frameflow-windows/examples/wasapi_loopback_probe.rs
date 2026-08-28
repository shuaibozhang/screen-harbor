use std::{
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use frameflow_windows::{
    start_system_audio_loopback, AudioNormalizer, AudioStreamFormat, NORMALIZED_AUDIO_SAMPLE_RATE,
};

struct ProbeStats {
    packets: u64,
    source_frames: u64,
    normalized_frames: u64,
    packet_format: Option<AudioStreamFormat>,
    normalizer: Option<AudioNormalizer>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let stats = Arc::new(Mutex::new(ProbeStats {
        packets: 0,
        source_frames: 0,
        normalized_frames: 0,
        packet_format: None,
        normalizer: None,
    }));
    let callback_stats = Arc::clone(&stats);
    let session = start_system_audio_loopback(move |packet| match packet {
        Ok(packet) => {
            let mut stats = callback_stats.lock().expect("probe stats are unavailable");
            stats.packets += 1;
            stats.source_frames += u64::from(packet.frames);
            stats.packet_format = Some(packet.format);
            if stats.normalizer.is_none() {
                stats.normalizer = Some(
                    AudioNormalizer::new(packet.format).expect("endpoint format is unsupported"),
                );
            }
            let chunk = stats
                .normalizer
                .as_mut()
                .expect("normalizer is initialized")
                .push(&packet)
                .expect("audio packet could not be normalized");
            stats.normalized_frames += chunk.frames() as u64;
        }
        Err(error) => eprintln!("loopback error: {error}"),
    })?;
    let startup_format = session.format();

    thread::sleep(Duration::from_secs(2));
    session.stop()?;
    let mut stats = stats.lock().expect("probe stats are unavailable");
    if let Some(normalizer) = stats.normalizer.as_mut() {
        stats.normalized_frames += normalizer.finish().frames() as u64;
    }
    println!(
        "packets={} source_frames={} normalized_frames={} normalized_rate={} startup_format={startup_format:?} packet_format={:?}",
        stats.packets,
        stats.source_frames,
        stats.normalized_frames,
        NORMALIZED_AUDIO_SAMPLE_RATE,
        stats.packet_format
    );
    Ok(())
}
