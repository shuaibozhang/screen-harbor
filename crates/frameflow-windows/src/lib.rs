#![cfg(target_os = "windows")]

mod displays;
mod media_foundation;
mod selection_overlay;
mod wgc;

pub use displays::{enable_per_monitor_v2_dpi, list_displays};
pub use selection_overlay::{start_native_selection, NativeSelectionSession};
pub use wgc::{
    capture_primary_frame, capture_region_frame, start_region_capture, start_region_mp4_capture,
    CapturedFrameInfo, Mp4CaptureOptions, WgcCaptureSession,
};

use thiserror::Error;
use windows::{
    core::Error as WindowsError,
    Graphics::Capture::GraphicsCaptureSession,
    Win32::{
        Foundation::HWND,
        Graphics::Dwm::DwmFlush,
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
    #[error("unsupported output frame rate: {0} FPS")]
    InvalidFrameRate(u32),
}

pub fn flush_desktop_compositor() -> Result<(), PlatformError> {
    unsafe { DwmFlush()? };
    Ok(())
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
