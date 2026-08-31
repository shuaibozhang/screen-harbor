use std::collections::VecDeque;

use crate::{NormalizedAudioChunk, NORMALIZED_AUDIO_SAMPLE_RATE};

pub const AAC_AUDIO_BLOCK_FRAMES: usize = 1_024;
const CHANNELS: usize = 2;
const HUNDRED_NS_PER_SECOND: u64 = 10_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioSourceId {
    System,
    Microphone,
}

impl AudioSourceId {
    const ALL: [Self; 2] = [Self::System, Self::Microphone];

    fn index(self) -> usize {
        match self {
            Self::System => 0,
            Self::Microphone => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioSourceControl {
    pub enabled: bool,
    pub muted: bool,
    pub gain: f32,
}

impl Default for AudioSourceControl {
    fn default() -> Self {
        Self {
            enabled: true,
            muted: false,
            gain: 1.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioMixerConfig {
    pub system: AudioSourceControl,
    pub microphone: AudioSourceControl,
    /// Maximum timestamp correction applied to one chunk. Five milliseconds
    /// absorbs ordinary callback jitter without permitting a bad timestamp to
    /// tear the output timeline apart.
    pub max_clock_correction_frames: u32,
    pub limiter_ceiling: f32,
}

impl Default for AudioMixerConfig {
    fn default() -> Self {
        Self {
            system: AudioSourceControl::default(),
            microphone: AudioSourceControl::default(),
            max_clock_correction_frames: NORMALIZED_AUDIO_SAMPLE_RATE / 200,
            limiter_ceiling: 0.98,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MixedAudioBlock {
    pub start_frame: u64,
    /// The final block may contain padding; only this many frames belong to the
    /// recording timeline.
    pub valid_frames: usize,
    /// Always 1024 interleaved stereo frames, ready for the AAC input stage.
    pub samples: Vec<f32>,
}

#[derive(Default)]
struct SourceTrack {
    initialized: bool,
    base_frame: u64,
    high_watermark: u64,
    expected_end: u64,
    samples: VecDeque<f32>,
}

impl SourceTrack {
    fn shift_forward(&mut self, frames: u64) {
        if !self.initialized || frames == 0 {
            return;
        }
        for _ in 0..frames.saturating_mul(CHANNELS as u64) {
            self.samples.push_front(0.0);
        }
        self.high_watermark = self.high_watermark.saturating_add(frames);
        self.expected_end = self.expected_end.saturating_add(frames);
    }

    fn reset_at(&mut self, frame: u64) {
        self.initialized = false;
        self.base_frame = frame;
        self.high_watermark = frame;
        self.expected_end = frame;
        self.samples.clear();
    }

    fn append(&mut self, mut start: u64, mut samples: &[f32]) {
        if start < self.high_watermark {
            let overlap = usize::try_from(self.high_watermark - start).unwrap_or(usize::MAX);
            let skipped_samples = overlap.saturating_mul(CHANNELS).min(samples.len());
            samples = &samples[skipped_samples..];
            start = self.high_watermark;
        }
        let gap = start.saturating_sub(self.high_watermark);
        self.samples.extend(std::iter::repeat_n(
            0.0,
            usize::try_from(gap)
                .unwrap_or(usize::MAX)
                .saturating_mul(CHANNELS),
        ));
        self.samples.extend(samples.iter().copied());
        self.high_watermark = start.saturating_add((samples.len() / CHANNELS) as u64);
        self.initialized = true;
    }

    fn take_frame(&mut self, frame: u64) -> [f32; 2] {
        while self.base_frame < frame {
            let _ = self.samples.pop_front();
            let _ = self.samples.pop_front();
            self.base_frame += 1;
            self.high_watermark = self.high_watermark.max(self.base_frame);
        }
        if self.base_frame != frame {
            return [0.0, 0.0];
        }
        let output = [
            self.samples.pop_front().unwrap_or(0.0),
            self.samples.pop_front().unwrap_or(0.0),
        ];
        self.base_frame += 1;
        self.high_watermark = self.high_watermark.max(self.base_frame);
        output
    }
}

/// Places normalized system and microphone audio on one QPC-backed 48 kHz
/// timeline. Gaps become silence, overlaps are trimmed, and timestamp movement
/// is bounded per chunk to correct drift without reacting to callback jitter.
pub struct AudioMixer {
    config: AudioMixerConfig,
    origin_100ns: Option<u64>,
    origin_locked: bool,
    tracks: [SourceTrack; 2],
    next_output_frame: u64,
}

impl AudioMixer {
    pub fn new(config: AudioMixerConfig) -> Self {
        Self::create(config, None, false)
    }

    pub fn with_origin(config: AudioMixerConfig, origin_100ns: u64) -> Self {
        Self::create(config, Some(origin_100ns), true)
    }

    fn create(config: AudioMixerConfig, origin_100ns: Option<u64>, origin_locked: bool) -> Self {
        Self {
            config: sanitize_config(config),
            origin_100ns,
            origin_locked,
            tracks: [SourceTrack::default(), SourceTrack::default()],
            next_output_frame: 0,
        }
    }

    pub fn source_control(&self, source: AudioSourceId) -> AudioSourceControl {
        self.control(source)
    }

    pub fn set_source_control(&mut self, source: AudioSourceId, control: AudioSourceControl) {
        let control = sanitize_control(control);
        let was_enabled = self.control(source).enabled;
        match source {
            AudioSourceId::System => self.config.system = control,
            AudioSourceId::Microphone => self.config.microphone = control,
        }
        if control.enabled && !was_enabled {
            self.tracks[source.index()].reset_at(self.next_output_frame);
        }
    }

    pub fn push(&mut self, source: AudioSourceId, chunk: NormalizedAudioChunk) {
        if !chunk.samples.len().is_multiple_of(CHANNELS) {
            return;
        }
        let timestamp_frame = chunk
            .capture_time_100ns
            .map(|timestamp| self.timeline_frame(timestamp));
        let track = &mut self.tracks[source.index()];
        let expected = track.expected_end.max(self.next_output_frame);
        let placement = match timestamp_frame {
            Some(frame) if !track.initialized || chunk.discontinuous => {
                frame.max(self.next_output_frame)
            }
            Some(frame) => {
                let correction = u64::from(self.config.max_clock_correction_frames);
                frame.clamp(
                    expected.saturating_sub(correction),
                    expected.saturating_add(correction),
                )
            }
            None => expected,
        };
        let frames = (chunk.samples.len() / CHANNELS) as u64;
        track.append(placement, &chunk.samples);
        track.expected_end = placement.saturating_add(frames);
    }

    /// Renders every complete AAC block up to the recording clock. Input tracks
    /// that have no samples for a frame contribute silence, so the encoded
    /// timeline remains continuous even when WASAPI loopback is idle.
    pub fn render_until(&mut self, target_frame: u64) -> Vec<MixedAudioBlock> {
        let mut output = Vec::new();
        while target_frame.saturating_sub(self.next_output_frame) >= AAC_AUDIO_BLOCK_FRAMES as u64 {
            output.push(self.mix_block(AAC_AUDIO_BLOCK_FRAMES));
        }
        output
    }

    /// Completes the audio timeline at an exact recording frame. This is used
    /// only after capture sources have stopped and all queued input was read.
    pub fn finish_at(&mut self, target_frame: u64) -> Vec<MixedAudioBlock> {
        let mut output = self.render_until(target_frame);
        if target_frame > self.next_output_frame {
            output.push(self.mix_block((target_frame - self.next_output_frame) as usize));
        }
        output
    }

    pub fn next_output_frame(&self) -> u64 {
        self.next_output_frame
    }

    fn timeline_frame(&mut self, timestamp_100ns: u64) -> u64 {
        match self.origin_100ns {
            None => {
                self.origin_100ns = Some(timestamp_100ns);
                0
            }
            Some(origin)
                if timestamp_100ns < origin
                    && self.next_output_frame == 0
                    && !self.origin_locked =>
            {
                let shift = time_to_frames(origin - timestamp_100ns);
                for track in &mut self.tracks {
                    track.shift_forward(shift);
                }
                self.origin_100ns = Some(timestamp_100ns);
                0
            }
            Some(origin) => time_to_frames(timestamp_100ns.saturating_sub(origin)),
        }
    }

    fn mix_block(&mut self, valid_frames: usize) -> MixedAudioBlock {
        let start_frame = self.next_output_frame;
        let mut samples = Vec::with_capacity(AAC_AUDIO_BLOCK_FRAMES * CHANNELS);
        for offset in 0..AAC_AUDIO_BLOCK_FRAMES {
            let frame = start_frame + offset as u64;
            let mut mixed = [0.0_f32; 2];
            for source in AudioSourceId::ALL {
                let input = self.tracks[source.index()].take_frame(frame);
                let control = self.control(source);
                if offset < valid_frames && control.enabled && !control.muted {
                    mixed[0] += input[0] * control.gain;
                    mixed[1] += input[1] * control.gain;
                }
            }
            let ceiling = self.config.limiter_ceiling;
            samples.extend(mixed.map(|sample| sample.clamp(-ceiling, ceiling)));
        }
        self.next_output_frame += valid_frames as u64;
        MixedAudioBlock {
            start_frame,
            valid_frames,
            samples,
        }
    }

    fn control(&self, source: AudioSourceId) -> AudioSourceControl {
        match source {
            AudioSourceId::System => self.config.system,
            AudioSourceId::Microphone => self.config.microphone,
        }
    }
}

fn sanitize_config(mut config: AudioMixerConfig) -> AudioMixerConfig {
    config.system = sanitize_control(config.system);
    config.microphone = sanitize_control(config.microphone);
    config.limiter_ceiling = if config.limiter_ceiling.is_finite() {
        config.limiter_ceiling.clamp(0.01, 1.0)
    } else {
        0.98
    };
    config
}

fn sanitize_control(mut control: AudioSourceControl) -> AudioSourceControl {
    control.gain = if control.gain.is_finite() {
        control.gain.clamp(0.0, 4.0)
    } else {
        1.0
    };
    control
}

fn time_to_frames(duration_100ns: u64) -> u64 {
    duration_100ns
        .saturating_mul(u64::from(NORMALIZED_AUDIO_SAMPLE_RATE))
        .saturating_add(HUNDRED_NS_PER_SECOND / 2)
        / HUNDRED_NS_PER_SECOND
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(timestamp: u64, frames: usize, value: f32) -> NormalizedAudioChunk {
        NormalizedAudioChunk {
            start_frame: 0,
            capture_time_100ns: Some(timestamp),
            discontinuous: false,
            samples: vec![value; frames * CHANNELS],
        }
    }

    fn system_only() -> AudioMixerConfig {
        AudioMixerConfig {
            microphone: AudioSourceControl {
                enabled: false,
                ..AudioSourceControl::default()
            },
            ..AudioMixerConfig::default()
        }
    }

    #[test]
    fn emits_aac_sized_blocks_and_pads_the_tail() {
        let mut mixer = AudioMixer::new(system_only());
        mixer.push(AudioSourceId::System, chunk(1_000_000, 1_100, 0.25));
        let blocks = mixer.render_until(1_100);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].valid_frames, AAC_AUDIO_BLOCK_FRAMES);
        let tail = mixer.finish_at(1_100);
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].valid_frames, 76);
        assert_eq!(tail[0].samples.len(), AAC_AUDIO_BLOCK_FRAMES * CHANNELS);
        assert!(tail[0].samples[76 * CHANNELS..]
            .iter()
            .all(|sample| *sample == 0.0));
    }

    #[test]
    fn mixes_gain_mute_and_limits_peaks() {
        let mut mixer = AudioMixer::new(AudioMixerConfig {
            system: AudioSourceControl {
                gain: 2.0,
                ..AudioSourceControl::default()
            },
            microphone: AudioSourceControl {
                muted: true,
                ..AudioSourceControl::default()
            },
            limiter_ceiling: 0.8,
            ..AudioMixerConfig::default()
        });
        mixer.push(AudioSourceId::System, chunk(100, 1_024, 0.5));
        mixer.push(AudioSourceId::Microphone, chunk(100, 1_024, 0.9));
        let blocks = mixer.render_until(1_024);
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].samples.iter().all(|sample| *sample == 0.8));
    }

    #[test]
    fn live_mute_keeps_the_source_timeline_running() {
        let mut mixer = AudioMixer::new(system_only());
        mixer.push(AudioSourceId::System, chunk(0, 1_024, 0.3));
        let first = mixer.render_until(1_024);
        assert!(first[0].samples.iter().all(|sample| *sample == 0.3));

        let mut control = mixer.source_control(AudioSourceId::System);
        control.muted = true;
        mixer.set_source_control(AudioSourceId::System, control);
        mixer.push(AudioSourceId::System, chunk(213_333, 1_024, 0.3));
        let muted = mixer.render_until(2_048);
        assert!(muted[0].samples.iter().all(|sample| *sample == 0.0));

        control.muted = false;
        mixer.set_source_control(AudioSourceId::System, control);
        mixer.push(AudioSourceId::System, chunk(426_666, 1_024, 0.3));
        let restored = mixer.render_until(3_072);
        assert!(restored[0].samples.iter().all(|sample| *sample == 0.3));
    }

    #[test]
    fn aligns_late_sources_on_the_qpc_timeline() {
        let mut mixer = AudioMixer::new(AudioMixerConfig {
            max_clock_correction_frames: 48_000,
            ..AudioMixerConfig::default()
        });
        mixer.push(AudioSourceId::System, chunk(1_000_000, 1_024, 0.25));
        mixer.push(AudioSourceId::Microphone, chunk(1_100_000, 1_024, 0.5));
        let blocks = mixer.render_until(1_024);
        assert_eq!(blocks.len(), 1);
        let left = blocks[0]
            .samples
            .iter()
            .step_by(2)
            .copied()
            .collect::<Vec<_>>();
        assert!(left[..480].iter().all(|sample| *sample == 0.25));
        assert!(left[480..].iter().all(|sample| *sample == 0.75));
    }

    #[test]
    fn fixed_recording_origin_preserves_audio_startup_delay() {
        let mut mixer = AudioMixer::with_origin(system_only(), 1_000_000);
        mixer.push(
            AudioSourceId::System,
            chunk(1_100_000, AAC_AUDIO_BLOCK_FRAMES, 0.25),
        );
        let blocks = mixer.render_until(AAC_AUDIO_BLOCK_FRAMES as u64);
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].samples[..480 * CHANNELS]
            .iter()
            .all(|sample| *sample == 0.0));
        assert!(blocks[0].samples[480 * CHANNELS..]
            .iter()
            .all(|sample| *sample == 0.25));
    }

    #[test]
    fn bounds_clock_correction_and_inserts_a_small_gap() {
        let mut mixer = AudioMixer::new(system_only());
        mixer.push(AudioSourceId::System, chunk(0, 1_024, 0.2));
        let first = mixer.render_until(1_024);
        assert_eq!(first.len(), 1);
        mixer.push(AudioSourceId::System, chunk(10_000_000, 1_024, 0.4));
        let second = mixer.render_until(2_048);
        assert_eq!(second.len(), 1);
        assert!(second[0].samples[..240 * CHANNELS]
            .iter()
            .all(|sample| *sample == 0.0));
        assert!(second[0].samples[240 * CHANNELS..]
            .iter()
            .all(|sample| *sample == 0.4));
        let tail = mixer.finish_at(2_288);
        assert_eq!(tail[0].valid_frames, 240);
        assert!(tail[0].samples[..240 * CHANNELS]
            .iter()
            .all(|sample| *sample == 0.4));
    }

    #[test]
    fn recording_clock_emits_continuous_silence_without_input_callbacks() {
        let mut mixer = AudioMixer::with_origin(system_only(), 0);
        let blocks = mixer.render_until((AAC_AUDIO_BLOCK_FRAMES * 3) as u64);
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].start_frame, 0);
        assert_eq!(blocks[1].start_frame, 1_024);
        assert_eq!(blocks[2].start_frame, 2_048);
        assert!(blocks
            .iter()
            .flat_map(|block| &block.samples)
            .all(|sample| *sample == 0.0));
    }

    #[test]
    fn audio_resumes_on_the_continuous_clock_after_a_silent_period() {
        let mut mixer = AudioMixer::with_origin(system_only(), 0);
        let silent = mixer.render_until(3_072);
        assert_eq!(silent.len(), 3);

        mixer.push(AudioSourceId::System, chunk(640_000, 1_024, 0.4));
        let resumed = mixer.render_until(4_096);
        assert_eq!(resumed.len(), 1);
        assert_eq!(resumed[0].start_frame, 3_072);
        assert!(resumed[0].samples.iter().all(|sample| *sample == 0.4));
    }

    #[test]
    fn finish_at_uses_an_exact_contiguous_tail() {
        let mut mixer = AudioMixer::with_origin(system_only(), 0);
        let blocks = mixer.finish_at(2_500);
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].start_frame, 0);
        assert_eq!(blocks[1].start_frame, 1_024);
        assert_eq!(blocks[2].start_frame, 2_048);
        assert_eq!(blocks[2].valid_frames, 452);
        assert_eq!(mixer.next_output_frame(), 2_500);
    }
}
