use std::{
    ffi::c_void,
    slice,
    sync::mpsc,
    thread::{self, JoinHandle},
    time::Duration,
};

use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{CloseHandle, E_ACCESSDENIED, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT},
        Media::Audio::{
            eCapture, eMultimedia, eRender, EDataFlow, IAudioCaptureClient, IAudioClient,
            IMMDeviceEnumerator, MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY,
            AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR,
            AUDCLNT_E_DEVICE_INVALIDATED, AUDCLNT_E_ENDPOINT_CREATE_FAILED,
            AUDCLNT_E_SERVICE_NOT_RUNNING, AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK, WAVEFORMATEX,
            WAVEFORMATEXTENSIBLE, WAVE_FORMAT_PCM,
        },
        Media::{
            KernelStreaming::KSDATAFORMAT_SUBTYPE_PCM,
            Multimedia::{KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, WAVE_FORMAT_IEEE_FLOAT},
        },
        System::{
            Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL},
            Threading::{CreateEventW, WaitForSingleObject},
        },
    },
};

use crate::{com::ComApartment, PlatformError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioSampleFormat {
    Float32,
    SignedPcm16,
    SignedPcm24,
    SignedPcm32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioStreamFormat {
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
    pub block_align: u16,
    pub format_tag: u16,
    pub sample_format: AudioSampleFormat,
    pub channel_mask: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioPacket {
    pub format: AudioStreamFormat,
    pub frames: u32,
    pub device_position: u64,
    pub qpc_position_100ns: u64,
    pub discontinuous: bool,
    pub timestamp_error: bool,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioCaptureSource {
    SystemOutput,
    Microphone,
}

impl AudioCaptureSource {
    fn data_flow(self) -> EDataFlow {
        match self {
            Self::SystemOutput => eRender,
            Self::Microphone => eCapture,
        }
    }

    fn stream_flags(self) -> u32 {
        AUDCLNT_STREAMFLAGS_EVENTCALLBACK
            | match self {
                Self::SystemOutput => AUDCLNT_STREAMFLAGS_LOOPBACK,
                Self::Microphone => 0,
            }
    }

    fn worker_name(self) -> &'static str {
        match self {
            Self::SystemOutput => "frameflow-wasapi-system",
            Self::Microphone => "frameflow-wasapi-microphone",
        }
    }
}

pub struct WasapiCaptureSession {
    control: mpsc::Sender<()>,
    worker: Option<JoinHandle<Result<(), PlatformError>>>,
    format: AudioStreamFormat,
    source: AudioCaptureSource,
}

impl WasapiCaptureSession {
    pub fn format(&self) -> AudioStreamFormat {
        self.format
    }

    pub fn source(&self) -> AudioCaptureSource {
        self.source
    }

    pub fn stop(mut self) -> Result<(), PlatformError> {
        self.shutdown()
    }

    fn shutdown(&mut self) -> Result<(), PlatformError> {
        if let Some(worker) = self.worker.take() {
            let _ = self.control.send(());
            worker.join().map_err(|_| {
                PlatformError::WorkerUnavailable("WASAPI audio worker panicked".to_owned())
            })??;
        }
        Ok(())
    }
}

impl Drop for WasapiCaptureSession {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

pub type WasapiLoopbackSession = WasapiCaptureSession;

/// Starts event-driven shared-mode loopback capture from the current default
/// multimedia output endpoint. Packets remain in the endpoint's native mix
/// format so resampling and mixing can happen once in the audio bus.
pub fn start_system_audio_loopback<F>(callback: F) -> Result<WasapiCaptureSession, PlatformError>
where
    F: FnMut(Result<AudioPacket, PlatformError>) + Send + 'static,
{
    start_wasapi_capture(AudioCaptureSource::SystemOutput, callback)
}

/// Starts event-driven shared-mode capture from the current default multimedia
/// microphone. Permission denial and device removal are returned as ordinary
/// capture errors so video recording can degrade to silent output.
pub fn start_microphone_capture<F>(callback: F) -> Result<WasapiCaptureSession, PlatformError>
where
    F: FnMut(Result<AudioPacket, PlatformError>) + Send + 'static,
{
    start_wasapi_capture(AudioCaptureSource::Microphone, callback)
}

pub fn start_wasapi_capture<F>(
    source: AudioCaptureSource,
    callback: F,
) -> Result<WasapiCaptureSession, PlatformError>
where
    F: FnMut(Result<AudioPacket, PlatformError>) + Send + 'static,
{
    start_wasapi_capture_from_device(source, None, callback)
}

pub fn start_wasapi_capture_from_device<F>(
    source: AudioCaptureSource,
    endpoint_id: Option<String>,
    callback: F,
) -> Result<WasapiCaptureSession, PlatformError>
where
    F: FnMut(Result<AudioPacket, PlatformError>) + Send + 'static,
{
    let (control_sender, control_receiver) = mpsc::channel();
    let (startup_sender, startup_receiver) = mpsc::sync_channel(1);
    let worker = thread::Builder::new()
        .name(source.worker_name().to_owned())
        .spawn(move || {
            run_capture_worker(
                source,
                endpoint_id,
                callback,
                control_receiver,
                startup_sender,
            )
        })
        .map_err(|error| PlatformError::WorkerUnavailable(error.to_string()))?;

    match startup_receiver.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(format)) => Ok(WasapiCaptureSession {
            control: control_sender,
            worker: Some(worker),
            format,
            source,
        }),
        Ok(Err(message)) => {
            let _ = worker.join();
            Err(PlatformError::AudioCapture(message))
        }
        Err(_) => {
            let _ = control_sender.send(());
            let _ = worker.join();
            Err(PlatformError::WorkerUnavailable(
                "WASAPI audio worker did not start".to_owned(),
            ))
        }
    }
}

fn run_capture_worker<F>(
    source: AudioCaptureSource,
    endpoint_id: Option<String>,
    mut callback: F,
    control: mpsc::Receiver<()>,
    startup: mpsc::SyncSender<Result<AudioStreamFormat, String>>,
) -> Result<(), PlatformError>
where
    F: FnMut(Result<AudioPacket, PlatformError>),
{
    let _apartment = match ComApartment::enter_multithreaded() {
        Ok(apartment) => apartment,
        Err(error) => {
            let _ = startup.send(Err(error.to_string()));
            return Err(error.into());
        }
    };
    let setup = setup_capture(source, endpoint_id.as_deref());
    let (audio_client, capture_client, format, event) = match setup {
        Ok(resources) => resources,
        Err(error) => {
            let _ = startup.send(Err(error.to_string()));
            return Err(error);
        }
    };
    if let Err(error) = unsafe { audio_client.Start() } {
        let error = audio_error("start stream", error);
        let _ = startup.send(Err(error.to_string()));
        return Err(error);
    }
    let _ = startup.send(Ok(format));

    let result = capture_packets(
        &audio_client,
        &capture_client,
        format,
        &event,
        &control,
        &mut callback,
    );
    if let Err(error) = &result {
        callback(Err(copy_audio_error(error)));
    }
    let stop_result =
        unsafe { audio_client.Stop() }.map_err(|error| audio_error("stop stream", error));
    result.and(stop_result)
}

fn setup_capture(
    source: AudioCaptureSource,
    endpoint_id: Option<&str>,
) -> Result<
    (
        IAudioClient,
        IAudioCaptureClient,
        AudioStreamFormat,
        OwnedEvent,
    ),
    PlatformError,
> {
    let enumerator: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
            .map_err(|error| audio_error("create endpoint enumerator", error))?;
    let endpoint = if let Some(endpoint_id) = endpoint_id {
        let endpoint_id: Vec<u16> = endpoint_id.encode_utf16().chain(Some(0)).collect();
        unsafe { enumerator.GetDevice(PCWSTR(endpoint_id.as_ptr())) }
            .map_err(|error| audio_error("open selected audio endpoint", error))?
    } else {
        unsafe { enumerator.GetDefaultAudioEndpoint(source.data_flow(), eMultimedia) }
            .map_err(|error| audio_error("open default audio endpoint", error))?
    };
    let audio_client: IAudioClient = unsafe { endpoint.Activate(CLSCTX_ALL, None) }
        .map_err(|error| audio_error("activate audio client", error))?;
    let mix_format = MixFormat::get(&audio_client)?;
    let format = AudioStreamFormat::from_wave_format(mix_format.as_ref())?;
    let event = OwnedEvent::create()?;
    unsafe {
        audio_client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                source.stream_flags(),
                0,
                0,
                mix_format.as_ptr(),
                None,
            )
            .map_err(|error| audio_error("initialize shared audio stream", error))?;
        audio_client
            .SetEventHandle(event.handle())
            .map_err(|error| audio_error("set audio event", error))?;
    }
    let capture_client: IAudioCaptureClient = unsafe { audio_client.GetService() }
        .map_err(|error| audio_error("get capture client", error))?;
    Ok((audio_client, capture_client, format, event))
}

fn capture_packets<F>(
    audio_client: &IAudioClient,
    capture_client: &IAudioCaptureClient,
    format: AudioStreamFormat,
    event: &OwnedEvent,
    control: &mpsc::Receiver<()>,
    callback: &mut F,
) -> Result<(), PlatformError>
where
    F: FnMut(Result<AudioPacket, PlatformError>),
{
    loop {
        if control.try_recv().is_ok() {
            return Ok(());
        }
        match unsafe { WaitForSingleObject(event.handle(), 100) } {
            WAIT_OBJECT_0 => drain_packets(capture_client, format, callback)?,
            WAIT_TIMEOUT => {
                // A removed or disabled endpoint does not always signal the sample
                // event. Querying the shared-mode client provides a bounded health
                // check so a silent device still degrades within one wait cycle.
                unsafe { audio_client.GetCurrentPadding() }
                    .map_err(|error| audio_error("check audio endpoint health", error))?;
            }
            result => {
                return Err(PlatformError::AudioCapture(format!(
                    "wait for audio samples returned {}",
                    result.0
                )))
            }
        }
    }
}

fn drain_packets<F>(
    capture_client: &IAudioCaptureClient,
    format: AudioStreamFormat,
    callback: &mut F,
) -> Result<(), PlatformError>
where
    F: FnMut(Result<AudioPacket, PlatformError>),
{
    loop {
        let packet_frames = unsafe { capture_client.GetNextPacketSize() }
            .map_err(|error| audio_error("query packet size", error))?;
        if packet_frames == 0 {
            return Ok(());
        }

        let mut pointer = std::ptr::null_mut();
        let mut frames = 0;
        let mut flags = 0;
        let mut device_position = 0;
        let mut qpc_position_100ns = 0;
        unsafe {
            capture_client
                .GetBuffer(
                    &mut pointer,
                    &mut frames,
                    &mut flags,
                    Some(&mut device_position),
                    Some(&mut qpc_position_100ns),
                )
                .map_err(|error| audio_error("acquire packet", error))?;
        }
        let packet = copy_packet(
            pointer,
            frames,
            flags,
            device_position,
            qpc_position_100ns,
            format,
        );
        let release = unsafe { capture_client.ReleaseBuffer(frames) }
            .map_err(|error| audio_error("release packet", error));
        match (packet, release) {
            (Ok(packet), Ok(())) => callback(Ok(packet)),
            (Err(error), _) | (_, Err(error)) => return Err(error),
        }
    }
}

fn copy_packet(
    pointer: *mut u8,
    frames: u32,
    flags: u32,
    device_position: u64,
    qpc_position_100ns: u64,
    format: AudioStreamFormat,
) -> Result<AudioPacket, PlatformError> {
    let byte_length = usize::try_from(frames)
        .ok()
        .and_then(|frames| frames.checked_mul(usize::from(format.block_align)))
        .ok_or_else(|| PlatformError::AudioCapture("audio packet size overflow".to_owned()))?;
    let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
    let discontinuous = flags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32 != 0;
    let timestamp_error = flags & AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR.0 as u32 != 0;
    let data = if silent {
        vec![0; byte_length]
    } else if pointer.is_null() && byte_length != 0 {
        return Err(PlatformError::AudioCapture(
            "WASAPI returned a null packet buffer".to_owned(),
        ));
    } else {
        unsafe { slice::from_raw_parts(pointer, byte_length) }.to_vec()
    };
    Ok(AudioPacket {
        format,
        frames,
        device_position,
        qpc_position_100ns,
        discontinuous,
        timestamp_error,
        data,
    })
}

impl AudioStreamFormat {
    fn from_wave_format(format: &WAVEFORMATEX) -> Result<Self, PlatformError> {
        if format.nSamplesPerSec == 0
            || format.nChannels == 0
            || format.nBlockAlign == 0
            || format.wBitsPerSample == 0
        {
            return Err(PlatformError::AudioCapture(
                "default output endpoint reported an invalid mix format".to_owned(),
            ));
        }
        let (sample_format, channel_mask) = sample_format(format)?;
        Ok(Self {
            sample_rate: format.nSamplesPerSec,
            channels: format.nChannels,
            bits_per_sample: format.wBitsPerSample,
            block_align: format.nBlockAlign,
            format_tag: format.wFormatTag,
            sample_format,
            channel_mask,
        })
    }
}

fn sample_format(format: &WAVEFORMATEX) -> Result<(AudioSampleFormat, u32), PlatformError> {
    let tag = u32::from(format.wFormatTag);
    let bits_per_sample = format.wBitsPerSample;
    let (subtype, channel_mask) = if tag == 0xfffe && usize::from(format.cbSize) >= 22 {
        let extensible =
            unsafe { &*(format as *const WAVEFORMATEX).cast::<WAVEFORMATEXTENSIBLE>() };
        (Some(extensible.SubFormat), extensible.dwChannelMask)
    } else {
        (None, 0)
    };
    let is_float =
        tag == WAVE_FORMAT_IEEE_FLOAT || subtype == Some(KSDATAFORMAT_SUBTYPE_IEEE_FLOAT);
    let is_pcm = tag == WAVE_FORMAT_PCM || subtype == Some(KSDATAFORMAT_SUBTYPE_PCM);
    let sample_format = match (is_float, is_pcm, bits_per_sample) {
        (true, _, 32) => AudioSampleFormat::Float32,
        (_, true, 16) => AudioSampleFormat::SignedPcm16,
        (_, true, 24) => AudioSampleFormat::SignedPcm24,
        (_, true, 32) => AudioSampleFormat::SignedPcm32,
        _ => {
            return Err(PlatformError::AudioCapture(format!(
                "unsupported endpoint sample format: tag={} bits={}",
                tag, bits_per_sample
            )))
        }
    };
    Ok((sample_format, channel_mask))
}

struct MixFormat(*mut WAVEFORMATEX);

impl MixFormat {
    fn get(audio_client: &IAudioClient) -> Result<Self, PlatformError> {
        let pointer = unsafe { audio_client.GetMixFormat() }
            .map_err(|error| audio_error("read endpoint mix format", error))?;
        if pointer.is_null() {
            return Err(PlatformError::AudioCapture(
                "default output endpoint returned no mix format".to_owned(),
            ));
        }
        Ok(Self(pointer))
    }

    fn as_ptr(&self) -> *const WAVEFORMATEX {
        self.0
    }

    fn as_ref(&self) -> &WAVEFORMATEX {
        unsafe { &*self.0 }
    }
}

impl Drop for MixFormat {
    fn drop(&mut self) {
        unsafe { CoTaskMemFree(Some(self.0.cast::<c_void>())) };
    }
}

struct OwnedEvent(HANDLE);

impl OwnedEvent {
    fn create() -> Result<Self, PlatformError> {
        let handle = unsafe { CreateEventW(None, false, false, PCWSTR::null()) }
            .map_err(|error| audio_error("create sample event", error))?;
        Ok(Self(handle))
    }

    fn handle(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedEvent {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

fn audio_error(stage: &'static str, error: windows::core::Error) -> PlatformError {
    match error.code() {
        E_ACCESSDENIED => PlatformError::AudioPermissionDenied,
        AUDCLNT_E_DEVICE_INVALIDATED
        | AUDCLNT_E_ENDPOINT_CREATE_FAILED
        | AUDCLNT_E_SERVICE_NOT_RUNNING => {
            PlatformError::AudioDeviceUnavailable(format!("{stage}: {error}"))
        }
        _ => PlatformError::AudioCapture(format!("{stage}: {error}")),
    }
}

fn copy_audio_error(error: &PlatformError) -> PlatformError {
    match error {
        PlatformError::AudioPermissionDenied => PlatformError::AudioPermissionDenied,
        PlatformError::AudioDeviceUnavailable(message) => {
            PlatformError::AudioDeviceUnavailable(message.clone())
        }
        _ => PlatformError::AudioCapture(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stereo_float_format() -> AudioStreamFormat {
        AudioStreamFormat {
            sample_rate: 48_000,
            channels: 2,
            bits_per_sample: 32,
            block_align: 8,
            format_tag: 0xfffe,
            sample_format: AudioSampleFormat::Float32,
            channel_mask: 3,
        }
    }

    #[test]
    fn copies_interleaved_packet_bytes() {
        let mut source = vec![7_u8; 80];
        let packet = copy_packet(
            source.as_mut_ptr(),
            10,
            0,
            480,
            100_000,
            stereo_float_format(),
        )
        .unwrap();
        assert_eq!(packet.data, source);
        assert_eq!(packet.frames, 10);
    }

    #[test]
    fn materializes_silent_packets_as_zeroes() {
        let packet = copy_packet(
            std::ptr::null_mut(),
            10,
            AUDCLNT_BUFFERFLAGS_SILENT.0 as u32,
            0,
            0,
            stereo_float_format(),
        )
        .unwrap();
        assert_eq!(packet.data, vec![0; 80]);
    }

    #[test]
    fn only_system_output_uses_loopback_flag() {
        assert_ne!(
            AudioCaptureSource::SystemOutput.stream_flags() & AUDCLNT_STREAMFLAGS_LOOPBACK,
            0
        );
        assert_eq!(
            AudioCaptureSource::Microphone.stream_flags() & AUDCLNT_STREAMFLAGS_LOOPBACK,
            0
        );
        assert_ne!(
            AudioCaptureSource::Microphone.stream_flags() & AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
            0
        );
    }

    #[test]
    fn classifies_privacy_and_device_failures() {
        assert!(matches!(
            audio_error(
                "activate microphone",
                windows::core::Error::from_hresult(E_ACCESSDENIED)
            ),
            PlatformError::AudioPermissionDenied
        ));
        assert!(matches!(
            audio_error(
                "read microphone",
                windows::core::Error::from_hresult(AUDCLNT_E_DEVICE_INVALIDATED)
            ),
            PlatformError::AudioDeviceUnavailable(_)
        ));
    }
}
