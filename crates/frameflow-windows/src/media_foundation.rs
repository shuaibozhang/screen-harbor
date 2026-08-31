use std::{mem::ManuallyDrop, path::Path, ptr, slice};

use windows::{
    core::{Interface, GUID, HSTRING},
    Win32::{
        Graphics::{
            Direct3D11::{
                ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, ID3D11VideoContext,
                ID3D11VideoDevice, ID3D11VideoProcessor, ID3D11VideoProcessorEnumerator,
                D3D11_BIND_RENDER_TARGET, D3D11_BIND_VIDEO_ENCODER, D3D11_TEX2D_VPIV,
                D3D11_TEX2D_VPOV, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
                D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE, D3D11_VIDEO_PROCESSOR_CONTENT_DESC,
                D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0,
                D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0,
                D3D11_VIDEO_PROCESSOR_STREAM, D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
                D3D11_VPIV_DIMENSION_TEXTURE2D, D3D11_VPOV_DIMENSION_TEXTURE2D,
            },
            Dxgi::Common::{DXGI_FORMAT_NV12, DXGI_RATIONAL, DXGI_SAMPLE_DESC},
        },
        Media::MediaFoundation::{
            IMF2DBuffer, IMFActivate, IMFAttributes, IMFByteStream, IMFDXGIDeviceManager,
            IMFSinkWriter, IMFSinkWriterEx, IMFTransform, MFAudioFormat_AAC, MFAudioFormat_PCM,
            MFCreateAttributes, MFCreateDXGIDeviceManager, MFCreateDXGISurfaceBuffer,
            MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample, MFCreateSinkWriterFromURL,
            MFMediaType_Audio, MFMediaType_Video, MFShutdown, MFStartup, MFTEnumEx,
            MFT_ENUM_HARDWARE_URL_Attribute, MFT_FRIENDLY_NAME_Attribute,
            MFT_TRANSFORM_CLSID_Attribute, MFTranscodeContainerType_MPEG4, MFVideoFormat_H264,
            MFVideoFormat_NV12, MFVideoInterlace_Progressive, MFSTARTUP_FULL,
            MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_FLAG_HARDWARE, MFT_ENUM_FLAG_SORTANDFILTER,
            MFT_REGISTER_TYPE_INFO, MF_MT_AAC_AUDIO_PROFILE_LEVEL_INDICATION,
            MF_MT_AAC_PAYLOAD_TYPE, MF_MT_ALL_SAMPLES_INDEPENDENT,
            MF_MT_AUDIO_AVG_BYTES_PER_SECOND, MF_MT_AUDIO_BITS_PER_SAMPLE,
            MF_MT_AUDIO_BLOCK_ALIGNMENT, MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND,
            MF_MT_AVG_BITRATE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE,
            MF_MT_MAJOR_TYPE, MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE,
            MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, MF_SINK_WRITER_D3D_MANAGER,
            MF_SINK_WRITER_DISABLE_THROTTLING, MF_TRANSCODE_CONTAINERTYPE, MF_VERSION,
        },
        System::Com::CoTaskMemFree,
    },
};

use crate::{MixedAudioBlock, PlatformError, NORMALIZED_AUDIO_SAMPLE_RATE};

const AAC_BIT_RATE: u32 = 128_000;
const AAC_CHANNELS: u32 = 2;
const PCM_BITS_PER_SAMPLE: u32 = 16;
const PCM_BLOCK_ALIGNMENT: u32 = AAC_CHANNELS * (PCM_BITS_PER_SAMPLE / 8);
const AAC_LC_PROFILE_LEVEL_2: u32 = 0x29;

pub(crate) struct Mp4Writer {
    sink: IMFSinkWriter,
    video_stream: u32,
    audio_stream: Option<u32>,
    _device_manager: IMFDXGIDeviceManager,
    converter: GpuBgraToNv12,
    timeline: SampleTimeline,
    encoder: VideoEncoderInfo,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoEncoderInfo {
    pub name: String,
    pub hardware_accelerated: Option<bool>,
}

#[derive(Clone, Copy)]
pub(crate) struct Mp4VideoConfig {
    pub(crate) source_width: u32,
    pub(crate) source_height: u32,
    pub(crate) output_width: u32,
    pub(crate) output_height: u32,
    pub(crate) frame_rate: u32,
}

struct SampleTimeline {
    frame_rate: u64,
}

impl SampleTimeline {
    fn new(frame_rate: u32) -> Self {
        Self {
            frame_rate: u64::from(frame_rate),
        }
    }

    fn sample_for_frame(&self, frame: u64) -> (i64, i64) {
        let timestamp = self.time_for_frame(frame);
        let next_timestamp = self.time_for_frame(frame.saturating_add(1));
        (timestamp, next_timestamp - timestamp)
    }

    fn time_for_frame(&self, frame: u64) -> i64 {
        ((u128::from(frame) * 10_000_000) / u128::from(self.frame_rate)) as i64
    }
}

impl Mp4Writer {
    pub(crate) fn new(
        path: &Path,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        video: Mp4VideoConfig,
    ) -> Result<Self, PlatformError> {
        unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL)? };
        let result = Self::create(path, device, context, video, false);
        if result.is_err() {
            let _ = unsafe { MFShutdown() };
        }
        result
    }

    pub(crate) fn new_with_audio(
        path: &Path,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        video: Mp4VideoConfig,
    ) -> Result<Self, PlatformError> {
        unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL)? };
        let result = Self::create(path, device, context, video, true);
        if result.is_err() {
            let _ = unsafe { MFShutdown() };
        }
        result
    }

    fn create(
        path: &Path,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        video: Mp4VideoConfig,
        include_audio: bool,
    ) -> Result<Self, PlatformError> {
        let mut manager = None;
        let mut reset_token = 0;
        unsafe { MFCreateDXGIDeviceManager(&mut reset_token, &mut manager)? };
        let manager = manager.ok_or_else(windows::core::Error::from_win32)?;
        unsafe { manager.ResetDevice(device, reset_token)? };

        let attributes = create_attributes(4)?;
        unsafe {
            attributes.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;
            attributes.SetUnknown(&MF_SINK_WRITER_D3D_MANAGER, &manager)?;
            attributes.SetUINT32(&MF_SINK_WRITER_DISABLE_THROTTLING, 1)?;
            attributes.SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4)?;
        }

        let path = HSTRING::from(path.as_os_str().to_string_lossy().as_ref());
        let sink =
            unsafe { MFCreateSinkWriterFromURL(&path, None::<&IMFByteStream>, Some(&attributes))? };

        let output = unsafe { MFCreateMediaType()? };
        set_video_type(
            &output,
            MFVideoFormat_H264,
            video.output_width,
            video.output_height,
            video.frame_rate,
        )?;
        unsafe {
            output.SetUINT32(
                &MF_MT_AVG_BITRATE,
                target_bitrate(video.output_width, video.output_height, video.frame_rate),
            )?
        };
        let video_stream = unsafe { sink.AddStream(&output)? };

        let input = unsafe { MFCreateMediaType()? };
        set_video_type(
            &input,
            MFVideoFormat_NV12,
            video.output_width,
            video.output_height,
            video.frame_rate,
        )?;
        unsafe {
            sink.SetInputMediaType(video_stream, &input, None::<&IMFAttributes>)?;
        }
        let audio_stream = if include_audio {
            Some(add_audio_stream(&sink)?)
        } else {
            None
        };
        unsafe {
            sink.BeginWriting()?;
        }
        let encoder = inspect_video_encoder(&sink, video_stream);

        Ok(Self {
            sink,
            video_stream,
            audio_stream,
            _device_manager: manager,
            converter: GpuBgraToNv12::new(device, context, video)?,
            timeline: SampleTimeline::new(video.frame_rate),
            encoder,
        })
    }

    pub(crate) fn encoder_info(&self) -> VideoEncoderInfo {
        self.encoder.clone()
    }

    pub(crate) fn write_texture(
        &mut self,
        texture: &ID3D11Texture2D,
        timeline_frame: u64,
    ) -> Result<(), PlatformError> {
        let (timestamp, duration) = self.timeline.sample_for_frame(timeline_frame);
        let nv12_texture = self.converter.convert(texture)?;
        let buffer =
            unsafe { MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, &nv12_texture, 0, false) }
                .map_err(|error| media_error("wrap NV12 DXGI surface", error))?;
        let buffer_2d: IMF2DBuffer = buffer
            .cast()
            .map_err(|error| media_error("query NV12 2D buffer", error))?;
        let valid_length = unsafe { buffer_2d.GetContiguousLength() }
            .map_err(|error| media_error("get NV12 contiguous length", error))?;
        unsafe { buffer.SetCurrentLength(valid_length) }
            .map_err(|error| media_error("set NV12 buffer length", error))?;
        let sample = unsafe { MFCreateSample()? };
        unsafe {
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(timestamp)?;
            sample.SetSampleDuration(duration)?;
            self.sink
                .WriteSample(self.video_stream, &sample)
                .map_err(|error| media_error("write NV12 sample", error))?;
        }
        Ok(())
    }

    pub(crate) fn write_audio_block(
        &mut self,
        block: &MixedAudioBlock,
    ) -> Result<(), PlatformError> {
        let stream = self.audio_stream.ok_or_else(|| {
            PlatformError::MediaFoundation("MP4 writer has no audio stream".to_owned())
        })?;
        if block.valid_frames == 0
            || block.valid_frames > block.samples.len() / AAC_CHANNELS as usize
        {
            return Err(PlatformError::MediaFoundation(
                "mixed audio block has an invalid frame count".to_owned(),
            ));
        }

        let byte_length = block
            .valid_frames
            .checked_mul(PCM_BLOCK_ALIGNMENT as usize)
            .and_then(|length| u32::try_from(length).ok())
            .ok_or_else(|| {
                PlatformError::MediaFoundation("PCM audio block is too large".to_owned())
            })?;
        let buffer = unsafe { MFCreateMemoryBuffer(byte_length)? };
        let mut destination = std::ptr::null_mut();
        unsafe { buffer.Lock(&mut destination, None, None)? };
        let destination =
            unsafe { std::slice::from_raw_parts_mut(destination, byte_length as usize) };
        let (destination, remainder) = destination.as_chunks_mut::<2>();
        debug_assert!(remainder.is_empty());
        for (sample, bytes) in block.samples[..block.valid_frames * AAC_CHANNELS as usize]
            .iter()
            .zip(destination)
        {
            let pcm = float_to_pcm16(*sample).to_le_bytes();
            bytes.copy_from_slice(&pcm);
        }
        unsafe {
            buffer.Unlock()?;
            buffer.SetCurrentLength(byte_length)?;
        }

        let sample = unsafe { MFCreateSample()? };
        let timestamp = audio_time_for_frame(block.start_frame);
        let duration =
            audio_time_for_frame(block.start_frame + block.valid_frames as u64) - timestamp;
        unsafe {
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(timestamp)?;
            sample.SetSampleDuration(duration)?;
            self.sink
                .WriteSample(stream, &sample)
                .map_err(|error| media_error("write PCM audio sample", error))?;
        }
        Ok(())
    }

    pub(crate) fn finalize(self) -> Result<(), PlatformError> {
        unsafe { self.sink.Finalize()? };
        drop(self);
        Ok(())
    }
}

struct GpuBgraToNv12 {
    device: ID3D11Device,
    video_device: ID3D11VideoDevice,
    video_context: ID3D11VideoContext,
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    output_width: u32,
    output_height: u32,
}

impl GpuBgraToNv12 {
    fn new(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        video: Mp4VideoConfig,
    ) -> Result<Self, PlatformError> {
        let video_device: ID3D11VideoDevice = device.cast()?;
        let video_context: ID3D11VideoContext = context.cast()?;
        let description = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: DXGI_RATIONAL {
                Numerator: video.frame_rate,
                Denominator: 1,
            },
            InputWidth: video.source_width,
            InputHeight: video.source_height,
            OutputFrameRate: DXGI_RATIONAL {
                Numerator: video.frame_rate,
                Denominator: 1,
            },
            OutputWidth: video.output_width,
            OutputHeight: video.output_height,
            Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
        };
        let enumerator = unsafe { video_device.CreateVideoProcessorEnumerator(&description)? };
        let processor = unsafe { video_device.CreateVideoProcessor(&enumerator, 0)? };
        Ok(Self {
            device: device.clone(),
            video_device,
            video_context,
            enumerator,
            processor,
            output_width: video.output_width,
            output_height: video.output_height,
        })
    }

    fn convert(&self, source: &ID3D11Texture2D) -> Result<ID3D11Texture2D, PlatformError> {
        let description = D3D11_TEXTURE2D_DESC {
            Width: self.output_width,
            Height: self.output_height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_NV12,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_VIDEO_ENCODER.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut destination = None;
        unsafe {
            self.device
                .CreateTexture2D(&description, None, Some(&mut destination))
                .map_err(|error| media_error("create NV12 texture", error))?;
        }
        let destination = destination.ok_or_else(windows::core::Error::from_win32)?;

        let input_description = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
            FourCC: 0,
            ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPIV {
                    MipSlice: 0,
                    ArraySlice: 0,
                },
            },
        };
        let mut input_view = None;
        unsafe {
            self.video_device
                .CreateVideoProcessorInputView(
                    source,
                    &self.enumerator,
                    &input_description,
                    Some(&mut input_view),
                )
                .map_err(|error| media_error("create BGRA video-processor input view", error))?;
        }
        let input_view = input_view.ok_or_else(windows::core::Error::from_win32)?;

        let output_description = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
            ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
            },
        };
        let mut output_view = None;
        unsafe {
            self.video_device
                .CreateVideoProcessorOutputView(
                    &destination,
                    &self.enumerator,
                    &output_description,
                    Some(&mut output_view),
                )
                .map_err(|error| media_error("create NV12 video-processor output view", error))?;
        }
        let output_view = output_view.ok_or_else(windows::core::Error::from_win32)?;

        let stream = D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: true.into(),
            pInputSurface: ManuallyDrop::new(Some(input_view)),
            ..Default::default()
        };
        let mut streams = [stream];
        let result = unsafe {
            self.video_context
                .VideoProcessorBlt(&self.processor, &output_view, 0, &streams)
        };
        unsafe { ManuallyDrop::drop(&mut streams[0].pInputSurface) };
        result.map_err(|error| media_error("convert BGRA texture to NV12", error))?;
        Ok(destination)
    }
}

fn media_error(stage: &'static str, error: windows::core::Error) -> PlatformError {
    PlatformError::MediaFoundation(format!("{stage}: {error}"))
}

impl Drop for Mp4Writer {
    fn drop(&mut self) {
        let _ = unsafe { MFShutdown() };
    }
}

fn create_attributes(capacity: u32) -> Result<IMFAttributes, PlatformError> {
    let mut attributes = None;
    unsafe { MFCreateAttributes(&mut attributes, capacity)? };
    attributes.ok_or_else(|| windows::core::Error::from_win32().into())
}

fn inspect_video_encoder(sink: &IMFSinkWriter, stream: u32) -> VideoEncoderInfo {
    inspect_video_encoder_inner(sink, stream).unwrap_or_else(|| VideoEncoderInfo {
        name: "Media Foundation H.264".to_owned(),
        hardware_accelerated: None,
    })
}

fn inspect_video_encoder_inner(sink: &IMFSinkWriter, stream: u32) -> Option<VideoEncoderInfo> {
    let extended: IMFSinkWriterEx = sink.cast().ok()?;
    for index in 0..16 {
        let mut category = GUID::zeroed();
        let mut transform: Option<IMFTransform> = None;
        if unsafe {
            extended.GetTransformForStream(stream, index, Some(&mut category), &mut transform)
        }
        .is_err()
        {
            break;
        }
        if category != MFT_CATEGORY_VIDEO_ENCODER {
            continue;
        }
        let attributes = unsafe { transform?.GetAttributes() }.ok()?;
        let class_id = unsafe { attributes.GetGUID(&MFT_TRANSFORM_CLSID_Attribute) }.ok();
        let direct_hardware_marker =
            unsafe { attributes.GetStringLength(&MFT_ENUM_HARDWARE_URL_Attribute) }.is_ok();
        let registered_hardware = class_id.and_then(find_hardware_encoder);
        let name = attribute_string(&attributes, &MFT_FRIENDLY_NAME_Attribute)
            .or_else(|| registered_hardware.as_ref().map(|entry| entry.name.clone()))
            .unwrap_or_else(|| "Media Foundation H.264".to_owned());
        let hardware_accelerated = if direct_hardware_marker {
            Some(true)
        } else {
            class_id.map(|_| registered_hardware.is_some())
        };
        return Some(VideoEncoderInfo {
            name,
            hardware_accelerated,
        });
    }
    None
}

struct RegisteredEncoder {
    class_id: GUID,
    name: String,
}

fn find_hardware_encoder(class_id: GUID) -> Option<RegisteredEncoder> {
    enumerate_hardware_encoders()
        .into_iter()
        .find(|encoder| encoder.class_id == class_id)
}

fn enumerate_hardware_encoders() -> Vec<RegisteredEncoder> {
    let input = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_NV12,
    };
    let output = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_H264,
    };
    let mut activations: *mut Option<IMFActivate> = ptr::null_mut();
    let mut count = 0;
    if unsafe {
        MFTEnumEx(
            MFT_CATEGORY_VIDEO_ENCODER,
            MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
            Some(&input),
            Some(&output),
            &mut activations,
            &mut count,
        )
    }
    .is_err()
        || activations.is_null()
    {
        return Vec::new();
    }

    let mut encoders = Vec::with_capacity(count as usize);
    for activation in unsafe { slice::from_raw_parts_mut(activations, count as usize) } {
        let Some(activation) = activation.take() else {
            continue;
        };
        let Ok(class_id) = (unsafe { activation.GetGUID(&MFT_TRANSFORM_CLSID_Attribute) }) else {
            continue;
        };
        let name = attribute_string(&activation, &MFT_FRIENDLY_NAME_Attribute)
            .unwrap_or_else(|| "Hardware H.264".to_owned());
        encoders.push(RegisteredEncoder { class_id, name });
    }
    unsafe { CoTaskMemFree(Some(activations.cast())) };
    encoders
}

fn attribute_string(attributes: &IMFAttributes, key: &GUID) -> Option<String> {
    let length = unsafe { attributes.GetStringLength(key) }.ok()?;
    let mut buffer = vec![0_u16; length.saturating_add(1) as usize];
    unsafe { attributes.GetString(key, &mut buffer, None) }.ok()?;
    Some(String::from_utf16_lossy(&buffer[..length as usize]))
}

fn set_video_type(
    media_type: &windows::Win32::Media::MediaFoundation::IMFMediaType,
    subtype: windows::core::GUID,
    width: u32,
    height: u32,
    frame_rate: u32,
) -> Result<(), PlatformError> {
    unsafe {
        media_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        media_type.SetGUID(&MF_MT_SUBTYPE, &subtype)?;
        media_type.SetUINT64(&MF_MT_FRAME_SIZE, pack_pair(width, height))?;
        media_type.SetUINT64(&MF_MT_FRAME_RATE, pack_pair(frame_rate, 1))?;
        media_type.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack_pair(1, 1))?;
        media_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
    }
    Ok(())
}

fn add_audio_stream(sink: &IMFSinkWriter) -> Result<u32, PlatformError> {
    let output = unsafe { MFCreateMediaType()? };
    set_audio_type(&output, MFAudioFormat_AAC, 16, 1, AAC_BIT_RATE / 8)?;
    unsafe {
        output.SetUINT32(&MF_MT_AAC_PAYLOAD_TYPE, 0)?;
        output.SetUINT32(
            &MF_MT_AAC_AUDIO_PROFILE_LEVEL_INDICATION,
            AAC_LC_PROFILE_LEVEL_2,
        )?;
        output.SetUINT32(&MF_MT_AVG_BITRATE, AAC_BIT_RATE)?;
    }
    let stream = unsafe { sink.AddStream(&output)? };

    let input = unsafe { MFCreateMediaType()? };
    set_audio_type(
        &input,
        MFAudioFormat_PCM,
        PCM_BITS_PER_SAMPLE,
        PCM_BLOCK_ALIGNMENT,
        NORMALIZED_AUDIO_SAMPLE_RATE * PCM_BLOCK_ALIGNMENT,
    )?;
    unsafe {
        input.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;
        sink.SetInputMediaType(stream, &input, None::<&IMFAttributes>)?;
    }
    Ok(stream)
}

fn set_audio_type(
    media_type: &windows::Win32::Media::MediaFoundation::IMFMediaType,
    subtype: windows::core::GUID,
    bits_per_sample: u32,
    block_alignment: u32,
    average_bytes_per_second: u32,
) -> Result<(), PlatformError> {
    unsafe {
        media_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        media_type.SetGUID(&MF_MT_SUBTYPE, &subtype)?;
        media_type.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, AAC_CHANNELS)?;
        media_type.SetUINT32(
            &MF_MT_AUDIO_SAMPLES_PER_SECOND,
            NORMALIZED_AUDIO_SAMPLE_RATE,
        )?;
        media_type.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, bits_per_sample)?;
        media_type.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, block_alignment)?;
        media_type.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, average_bytes_per_second)?;
    }
    Ok(())
}

fn audio_time_for_frame(frame: u64) -> i64 {
    ((u128::from(frame) * 10_000_000) / u128::from(NORMALIZED_AUDIO_SAMPLE_RATE)) as i64
}

fn float_to_pcm16(sample: f32) -> i16 {
    let sample = if sample.is_finite() { sample } else { 0.0 };
    (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16
}

fn pack_pair(high: u32, low: u32) -> u64 {
    (u64::from(high) << 32) | u64::from(low)
}

fn target_bitrate(width: u32, height: u32, frame_rate: u32) -> u32 {
    let pixels_per_second = u64::from(width) * u64::from(height) * u64::from(frame_rate);
    (pixels_per_second / 6).clamp(2_000_000, 24_000_000) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_media_foundation_ratio() {
        assert_eq!(pack_pair(1920, 1080), 0x0000_0780_0000_0438);
    }

    #[test]
    fn bitrate_is_bounded() {
        assert_eq!(target_bitrate(320, 240, 15), 2_000_000);
        assert_eq!(target_bitrate(7680, 4320, 60), 24_000_000);
    }

    #[test]
    fn sample_timeline_is_exactly_constant_rate() {
        let timeline = SampleTimeline::new(30);
        let samples = (0..30)
            .map(|frame| timeline.sample_for_frame(frame))
            .collect::<Vec<_>>();
        assert_eq!(samples[0], (0, 333_333));
        assert_eq!(samples[1], (333_333, 333_333));
        assert_eq!(samples[2], (666_666, 333_334));
        assert_eq!(
            samples.iter().map(|(_, duration)| duration).sum::<i64>(),
            10_000_000
        );
    }

    #[test]
    fn configured_frame_rates_each_land_exactly_on_one_second() {
        for frame_rate in [24, 30, 60] {
            let timeline = SampleTimeline::new(frame_rate);
            assert_eq!(timeline.time_for_frame(u64::from(frame_rate)), 10_000_000);
        }
    }

    #[test]
    fn audio_timeline_lands_exactly_on_one_second() {
        assert_eq!(audio_time_for_frame(0), 0);
        assert_eq!(audio_time_for_frame(1_024), 213_333);
        assert_eq!(audio_time_for_frame(48_000), 10_000_000);
    }

    #[test]
    fn pcm_conversion_clamps_and_sanitizes_samples() {
        assert_eq!(float_to_pcm16(0.0), 0);
        assert_eq!(float_to_pcm16(1.5), i16::MAX);
        assert_eq!(float_to_pcm16(-1.5), -i16::MAX);
        assert_eq!(float_to_pcm16(f32::NAN), 0);
    }
}
