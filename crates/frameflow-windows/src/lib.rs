#![cfg(target_os = "windows")]

mod audio_bus;
mod audio_devices;
mod audio_mixer;
mod com;
mod display_watcher;
mod displays;
mod media_foundation;
mod selection_overlay;
mod shell;
mod smart_selection;
mod wasapi_loopback;
mod wgc;

pub use audio_bus::{AudioNormalizer, NormalizedAudioChunk, NORMALIZED_AUDIO_SAMPLE_RATE};
pub use audio_mixer::{
    AudioMixer, AudioMixerConfig, AudioSourceControl, AudioSourceId, MixedAudioBlock,
    AAC_AUDIO_BLOCK_FRAMES,
};
pub use display_watcher::{watch_display_changes, DisplayChangeSession};
pub use displays::{enable_per_monitor_v2_dpi, list_displays};
pub use media_foundation::VideoEncoderInfo;
pub use selection_overlay::{
    start_native_display_selection, start_native_selection, NativeDisplaySelectionSession,
    NativeSelectionSession,
};
pub use shell::{choose_recording_directory, move_paths_to_recycle_bin};
pub use wasapi_loopback::{
    start_microphone_capture, start_system_audio_loopback, start_wasapi_capture,
    AudioCaptureSource, AudioPacket, AudioSampleFormat, AudioStreamFormat, WasapiCaptureSession,
    WasapiLoopbackSession,
};
pub use wgc::{
    capture_primary_frame, capture_region_frame, start_region_capture, start_region_mp4_capture,
    CapturedFrameInfo, Mp4CaptureOptions, WgcAudioInput, WgcCaptureSession,
};

use thiserror::Error;
use windows::{
    core::Error as WindowsError,
    Graphics::Capture::GraphicsCaptureSession,
    Win32::{
        Foundation::HWND,
        Graphics::Dwm::DwmFlush,
        System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency},
        UI::WindowsAndMessaging::{SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE},
    },
};

#[derive(Debug, Error)]
pub enum PlatformError {
    #[error("Windows API call failed: {0}")]
    Windows(#[from] WindowsError),
    #[error("screen capture is not supported on this Windows installation")]
    CaptureUnsupported,
    #[error("no frame arrived within {0} ms")]
    FrameTimeout(u64),
    #[error("capture frame has an invalid size: {width}x{height}")]
    InvalidFrameSize { width: i32, height: i32 },
    #[error("display does not exist: {0}")]
    DisplayNotFound(String),
    #[error("crop region {x},{y},{width}x{height} is outside frame {frame_width}x{frame_height}")]
    InvalidCrop {
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        frame_width: u32,
        frame_height: u32,
    },
    #[error("capture worker is unavailable: {0}")]
    WorkerUnavailable(String),
    #[error("capture worker did not acknowledge {0}")]
    WorkerControlTimeout(&'static str),
    #[error("Media Foundation pipeline failed: {0}")]
    MediaFoundation(String),
    #[error("WASAPI capture failed: {0}")]
    AudioCapture(String),
    #[error("microphone access is denied by Windows privacy settings")]
    AudioPermissionDenied,
    #[error("audio device is unavailable: {0}")]
    AudioDeviceUnavailable(String),
    #[error("audio is not enabled for this recording")]
    AudioNotEnabled,
    #[error("audio encoder queue is full")]
    AudioQueueFull,
    #[error("unsupported output frame rate: {0} FPS")]
    InvalidFrameRate(u32),
    #[error("output dimensions must be positive even numbers: {width}x{height}")]
    InvalidOutputSize { width: u32, height: u32 },
}

pub fn flush_desktop_compositor() -> Result<(), PlatformError> {
    unsafe { DwmFlush()? };
    Ok(())
}

pub fn performance_time_100ns() -> Result<u64, PlatformError> {
    let mut counter = 0_i64;
    let mut frequency = 0_i64;
    unsafe {
        QueryPerformanceCounter(&mut counter)?;
        QueryPerformanceFrequency(&mut frequency)?;
    }
    if counter < 0 || frequency <= 0 {
        return Err(PlatformError::AudioCapture(
            "performance counter returned an invalid value".to_owned(),
        ));
    }
    Ok((counter as u128)
        .saturating_mul(10_000_000)
        .saturating_div(frequency as u128)
        .min(u128::from(u64::MAX)) as u64)
}

pub fn graphics_capture_supported() -> Result<bool, PlatformError> {
    Ok(GraphicsCaptureSession::IsSupported()?)
}

/// Excludes a FrameFlow-owned window from supported Windows capture paths.
/// This is a UX measure, not a content-protection boundary.
pub fn exclude_window_from_capture(hwnd: isize) -> Result<(), PlatformError> {
    let hwnd = HWND(hwnd as *mut core::ffi::c_void);
    unsafe { SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE)? };
    Ok(())
}
pub use audio_devices::{default_audio_endpoints, AudioEndpointInfo, DefaultAudioEndpoints};
