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
        }
        if self.base_frame != frame {
            return [0.0, 0.0];
        }
        let output = [
            self.samples.pop_front().unwrap_or(0.0),
            self.samples.pop_front().unwrap_or(0.0),
        ];
        self.base_frame += 1;
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

    pub fn push(
        &mut self,
        source: AudioSourceId,
        chunk: NormalizedAudioChunk,
    ) -> Vec<MixedAudioBlock> {
        if !chunk.samples.len().is_multiple_of(CHANNELS) {
            return Vec::new();
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
        self.drain_complete_blocks()
    }

    pub fn finish(&mut self) -> Vec<MixedAudioBlock> {
        let mut output = self.drain_complete_blocks();
        let end = self
            .tracks
            .iter()
            .map(|track| track.high_watermark)
            .max()
            .unwrap_or(self.next_output_frame);
        if end > self.next_output_frame {
            output.push(self.mix_block(
                (end - self.next_output_frame).min(AAC_AUDIO_BLOCK_FRAMES as u64) as usize,
            ));
            while self.next_output_frame < end {
                output.push(self.mix_block(
                    (end - self.next_output_frame).min(AAC_AUDIO_BLOCK_FRAMES as u64) as usize,
                ));
            }
        }
        output
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

    fn drain_complete_blocks(&mut self) -> Vec<MixedAudioBlock> {
        let Some(ready_end) = self.ready_end() else {
            return Vec::new();
        };
        let mut output = Vec::new();
        while ready_end.saturating_sub(self.next_output_frame) >= AAC_AUDIO_BLOCK_FRAMES as u64 {
            output.push(self.mix_block(AAC_AUDIO_BLOCK_FRAMES));
        }
        output
    }

    fn ready_end(&self) -> Option<u64> {
        AudioSourceId::ALL
            .into_iter()
            .filter(|source| self.control(*source).enabled)
            .map(|source| &self.tracks[source.index()])
            .try_fold(u64::MAX, |ready, track| {
                track.initialized.then_some(ready.min(track.high_watermark))
            })
            .map(|ready| {
                if ready == u64::MAX {
                    self.next_output_frame
                } else {
                    ready
                }
            })
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
        let blocks = mixer.push(AudioSourceId::System, chunk(1_000_000, 1_100, 0.25));
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].valid_frames, AAC_AUDIO_BLOCK_FRAMES);
        let tail = mixer.finish();
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
        assert!(mixer
            .push(AudioSourceId::System, chunk(100, 1_024, 0.5))
            .is_empty());
        let blocks = mixer.push(AudioSourceId::Microphone, chunk(100, 1_024, 0.9));
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].samples.iter().all(|sample| *sample == 0.8));
    }

    #[test]
    fn aligns_late_sources_on_the_qpc_timeline() {
        let mut mixer = AudioMixer::new(AudioMixerConfig {
            max_clock_correction_frames: 48_000,
            ..AudioMixerConfig::default()
        });
        assert!(mixer
            .push(AudioSourceId::System, chunk(1_000_000, 1_024, 0.25))
            .is_empty());
        let blocks = mixer.push(AudioSourceId::Microphone, chunk(1_100_000, 1_024, 0.5));
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
        let blocks = mixer.push(
            AudioSourceId::System,
            chunk(1_100_000, AAC_AUDIO_BLOCK_FRAMES, 0.25),
        );
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
        let first = mixer.push(AudioSourceId::System, chunk(0, 1_024, 0.2));
        assert_eq!(first.len(), 1);
        let second = mixer.push(AudioSourceId::System, chunk(10_000_000, 1_024, 0.4));
        assert_eq!(second.len(), 1);
        assert!(second[0].samples[..240 * CHANNELS]
            .iter()
            .all(|sample| *sample == 0.0));
        assert!(second[0].samples[240 * CHANNELS..]
            .iter()
            .all(|sample| *sample == 0.4));
        let tail = mixer.finish();
        assert_eq!(tail[0].valid_frames, 240);
        assert!(tail[0].samples[..240 * CHANNELS]
            .iter()
            .all(|sample| *sample == 0.4));
    }
}
