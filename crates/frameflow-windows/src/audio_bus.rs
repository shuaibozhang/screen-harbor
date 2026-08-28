use crate::{AudioPacket, AudioSampleFormat, AudioStreamFormat, PlatformError};

pub const NORMALIZED_AUDIO_SAMPLE_RATE: u32 = 48_000;
const NORMALIZED_CHANNELS: usize = 2;

#[derive(Debug, Clone, PartialEq)]
pub struct NormalizedAudioChunk {
    pub start_frame: u64,
    /// QPC-derived timestamp of the source packet's first frame, in 100 ns units.
    /// Missing or unreliable device timestamps are represented as `None`.
    pub capture_time_100ns: Option<u64>,
    pub discontinuous: bool,
    /// Interleaved stereo f32 samples at 48 kHz.
    pub samples: Vec<f32>,
}

impl NormalizedAudioChunk {
    pub fn frames(&self) -> usize {
        self.samples.len() / NORMALIZED_CHANNELS
    }
}

/// Stateful decoder, stereo mapper and sample-rate converter. Interpolation
/// state survives WASAPI packet boundaries to keep the output timeline gapless.
pub struct AudioNormalizer {
    source: AudioStreamFormat,
    pending_stereo: Vec<[f32; 2]>,
    source_position: f64,
    emitted_frames: u64,
}

impl AudioNormalizer {
    pub fn new(source: AudioStreamFormat) -> Result<Self, PlatformError> {
        validate_format(source)?;
        Ok(Self {
            source,
            pending_stereo: Vec::new(),
            source_position: 0.0,
            emitted_frames: 0,
        })
    }

    pub fn push(&mut self, packet: &AudioPacket) -> Result<NormalizedAudioChunk, PlatformError> {
        if packet.format != self.source {
            return Err(PlatformError::AudioCapture(
                "audio endpoint format changed during capture".to_owned(),
            ));
        }
        if packet.discontinuous {
            self.pending_stereo.clear();
            self.source_position = 0.0;
        }
        let decoded = decode_and_downmix(packet)?;
        let samples = if self.source.sample_rate == NORMALIZED_AUDIO_SAMPLE_RATE {
            decoded
                .into_iter()
                .flat_map(|frame| frame.into_iter())
                .collect()
        } else {
            self.pending_stereo.extend(decoded);
            self.resample_pending(false)
        };
        Ok(self.chunk(
            samples,
            (!packet.timestamp_error).then_some(packet.qpc_position_100ns),
            packet.discontinuous,
        ))
    }

    pub fn finish(&mut self) -> NormalizedAudioChunk {
        let samples = if self.source.sample_rate == NORMALIZED_AUDIO_SAMPLE_RATE
            || self.pending_stereo.is_empty()
        {
            Vec::new()
        } else {
            let last = *self
                .pending_stereo
                .last()
                .expect("pending audio is not empty");
            self.pending_stereo.push(last);
            self.resample_pending(true)
        };
        self.chunk(samples, None, false)
    }

    fn resample_pending(&mut self, finishing: bool) -> Vec<f32> {
        let step = f64::from(self.source.sample_rate) / f64::from(NORMALIZED_AUDIO_SAMPLE_RATE);
        let mut output = Vec::new();
        while self.source_position + 1.0 < self.pending_stereo.len() as f64 {
            let index = self.source_position.floor() as usize;
            let fraction = (self.source_position - index as f64) as f32;
            let first = self.pending_stereo[index];
            let second = self.pending_stereo[index + 1];
            output.push(first[0] + (second[0] - first[0]) * fraction);
            output.push(first[1] + (second[1] - first[1]) * fraction);
            self.source_position += step;
        }
        let consumed = self.source_position.floor() as usize;
        if consumed > 0 {
            self.pending_stereo.drain(..consumed);
            self.source_position -= consumed as f64;
        }
        if finishing {
            self.pending_stereo.clear();
            self.source_position = 0.0;
        }
        output
    }

    fn chunk(
        &mut self,
        samples: Vec<f32>,
        capture_time_100ns: Option<u64>,
        discontinuous: bool,
    ) -> NormalizedAudioChunk {
        let start_frame = self.emitted_frames;
        self.emitted_frames += (samples.len() / NORMALIZED_CHANNELS) as u64;
        NormalizedAudioChunk {
            start_frame,
            capture_time_100ns,
            discontinuous,
            samples,
        }
    }
}

fn validate_format(format: AudioStreamFormat) -> Result<(), PlatformError> {
    let bytes_per_sample = usize::from(format.bits_per_sample / 8);
    let expected_align = usize::from(format.channels)
        .checked_mul(bytes_per_sample)
        .ok_or_else(|| PlatformError::AudioCapture("audio block alignment overflow".to_owned()))?;
    if format.sample_rate == 0
        || format.channels == 0
        || bytes_per_sample == 0
        || expected_align != usize::from(format.block_align)
    {
        return Err(PlatformError::AudioCapture(
            "unsupported audio endpoint block layout".to_owned(),
        ));
    }
    Ok(())
}

fn decode_and_downmix(packet: &AudioPacket) -> Result<Vec<[f32; 2]>, PlatformError> {
    let frame_bytes = usize::from(packet.format.block_align);
    let expected = usize::try_from(packet.frames)
        .ok()
        .and_then(|frames| frames.checked_mul(frame_bytes))
        .ok_or_else(|| PlatformError::AudioCapture("audio packet size overflow".to_owned()))?;
    if packet.data.len() != expected {
        return Err(PlatformError::AudioCapture(format!(
            "audio packet has {} bytes, expected {expected}",
            packet.data.len()
        )));
    }
    packet
        .data
        .chunks_exact(frame_bytes)
        .map(|frame| decode_frame(frame, packet.format))
        .collect()
}

fn decode_frame(frame: &[u8], format: AudioStreamFormat) -> Result<[f32; 2], PlatformError> {
    let sample_bytes = usize::from(format.bits_per_sample / 8);
    let channels = (0..usize::from(format.channels))
        .map(|channel| {
            let offset = channel * sample_bytes;
            decode_sample(&frame[offset..offset + sample_bytes], format.sample_format)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(downmix(&channels, format.channel_mask))
}

fn decode_sample(bytes: &[u8], format: AudioSampleFormat) -> Result<f32, PlatformError> {
    let sample = match format {
        AudioSampleFormat::Float32 => {
            f32::from_le_bytes(bytes.try_into().map_err(|_| invalid_sample_width())?)
        }
        AudioSampleFormat::SignedPcm16 => {
            f32::from(i16::from_le_bytes(
                bytes.try_into().map_err(|_| invalid_sample_width())?,
            )) / 32_768.0
        }
        AudioSampleFormat::SignedPcm24 => {
            let value =
                i32::from(bytes[0]) | (i32::from(bytes[1]) << 8) | (i32::from(bytes[2]) << 16);
            let signed = if value & 0x80_0000 != 0 {
                value | !0xff_ffff
            } else {
                value
            };
            signed as f32 / 8_388_608.0
        }
        AudioSampleFormat::SignedPcm32 => {
            i32::from_le_bytes(bytes.try_into().map_err(|_| invalid_sample_width())?) as f32
                / 2_147_483_648.0
        }
    };
    Ok(if sample.is_finite() { sample } else { 0.0 })
}

fn invalid_sample_width() -> PlatformError {
    PlatformError::AudioCapture("audio sample width does not match its format".to_owned())
}

fn downmix(channels: &[f32], mask: u32) -> [f32; 2] {
    match channels {
        [] => [0.0, 0.0],
        [mono] => [*mono, *mono],
        [left, right] => [*left, *right],
        _ if mask != 0 => {
            let sample = |speaker: u32| channel_for_speaker(channels, mask, speaker);
            let center = sample(0x4);
            let lfe = sample(0x8);
            [
                sample(0x1)
                    + 0.707 * center
                    + 0.5 * lfe
                    + 0.707 * sample(0x10)
                    + 0.707 * sample(0x200),
                sample(0x2)
                    + 0.707 * center
                    + 0.5 * lfe
                    + 0.707 * sample(0x20)
                    + 0.707 * sample(0x400),
            ]
        }
        _ => {
            let mut left = 0.0;
            let mut right = 0.0;
            let mut left_count = 0;
            let mut right_count = 0;
            for (index, sample) in channels.iter().copied().enumerate() {
                if index % 2 == 0 {
                    left += sample;
                    left_count += 1;
                } else {
                    right += sample;
                    right_count += 1;
                }
            }
            [left / left_count as f32, right / right_count as f32]
        }
    }
}

fn channel_for_speaker(channels: &[f32], mask: u32, speaker: u32) -> f32 {
    if mask & speaker == 0 {
        return 0.0;
    }
    let index = (mask & (speaker - 1)).count_ones() as usize;
    channels.get(index).copied().unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn format(
        sample_rate: u32,
        channels: u16,
        sample_format: AudioSampleFormat,
    ) -> AudioStreamFormat {
        let bits = match sample_format {
            AudioSampleFormat::SignedPcm16 => 16,
            AudioSampleFormat::SignedPcm24 => 24,
            AudioSampleFormat::Float32 | AudioSampleFormat::SignedPcm32 => 32,
        };
        AudioStreamFormat {
            sample_rate,
            channels,
            bits_per_sample: bits,
            block_align: channels * (bits / 8),
            format_tag: 0,
            sample_format,
            channel_mask: if channels == 2 { 3 } else { 0 },
        }
    }

    fn packet(format: AudioStreamFormat, data: Vec<u8>, frames: u32) -> AudioPacket {
        AudioPacket {
            format,
            frames,
            device_position: 0,
            qpc_position_100ns: 0,
            discontinuous: false,
            timestamp_error: false,
            data,
        }
    }

    #[test]
    fn preserves_native_48khz_float_stereo() {
        let format = format(48_000, 2, AudioSampleFormat::Float32);
        let values = [0.25_f32, -0.25, 0.5, -0.5];
        let data = values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        let chunk = AudioNormalizer::new(format)
            .unwrap()
            .push(&packet(format, data, 2))
            .unwrap();
        assert_eq!(chunk.start_frame, 0);
        assert_eq!(chunk.samples, values);
    }

    #[test]
    fn duplicates_mono_pcm16_into_stereo() {
        let format = format(48_000, 1, AudioSampleFormat::SignedPcm16);
        let data = [16_384_i16, -16_384]
            .into_iter()
            .flat_map(i16::to_le_bytes)
            .collect();
        let chunk = AudioNormalizer::new(format)
            .unwrap()
            .push(&packet(format, data, 2))
            .unwrap();
        assert_eq!(chunk.samples, vec![0.5, 0.5, -0.5, -0.5]);
    }

    #[test]
    fn resampling_is_continuous_across_packet_boundaries() {
        let format = format(24_000, 1, AudioSampleFormat::Float32);
        let first = [0.0_f32, 1.0]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect();
        let second = [2.0_f32, 3.0]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect();
        let mut normalizer = AudioNormalizer::new(format).unwrap();
        let first = normalizer.push(&packet(format, first, 2)).unwrap();
        let second = normalizer.push(&packet(format, second, 2)).unwrap();
        let final_chunk = normalizer.finish();
        let samples = first
            .samples
            .into_iter()
            .chain(second.samples)
            .chain(final_chunk.samples)
            .step_by(2)
            .collect::<Vec<_>>();
        assert_eq!(samples, vec![0.0, 0.5, 1.0, 1.5, 2.0, 2.5, 3.0, 3.0]);
    }
}
