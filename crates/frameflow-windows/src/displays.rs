use frameflow_capture::{DisplayInfo, PhysicalRect};
use windows::{
    core::{Error as WindowsError, BOOL, PCWSTR},
    Win32::{
        Foundation::{LPARAM, RECT},
        Graphics::Gdi::{
            EnumDisplayMonitors, EnumDisplaySettingsW, GetMonitorInfoW, DEVMODEW,
            ENUM_CURRENT_SETTINGS, HDC, HMONITOR, MONITORINFOEXW,
        },
        UI::{
            HiDpi::{
                AreDpiAwarenessContextsEqual, GetDpiForMonitor, GetThreadDpiAwarenessContext,
                SetProcessDpiAwarenessContext, SetThreadDpiAwarenessContext,
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, MDT_EFFECTIVE_DPI,
            },
            WindowsAndMessaging::MONITORINFOF_PRIMARY,
        },
    },
};

use crate::PlatformError;

pub fn enable_per_monitor_v2_dpi() -> Result<(), PlatformError> {
    let current = unsafe { GetThreadDpiAwarenessContext() };
    if unsafe {
        AreDpiAwarenessContextsEqual(current, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).as_bool()
    } {
        return Ok(());
    }

    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)? };
    Ok(())
}

pub(crate) fn enable_current_thread_per_monitor_v2_dpi() -> Result<(), PlatformError> {
    let previous =
        unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    if previous.0.is_null() {
        return Err(WindowsError::from_win32().into());
    }
    Ok(())
}

struct MonitorCollector {
    displays: Vec<DisplayInfo>,
    error: Option<windows::core::Error>,
}

pub fn list_displays() -> Result<Vec<DisplayInfo>, PlatformError> {
    let mut collector = MonitorCollector {
        displays: Vec::new(),
        error: None,
    };

    let succeeded = unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(collect_monitor),
            LPARAM(&mut collector as *mut MonitorCollector as isize),
        )
    };
    if !succeeded.as_bool() {
        return Err(WindowsError::from_win32().into());
    }

    if let Some(error) = collector.error {
        return Err(error.into());
    }

    Ok(collector.displays)
}

pub(crate) fn monitor_for_display_id(display_id: &str) -> Result<HMONITOR, PlatformError> {
    let mut monitors = Vec::new();
    let succeeded = unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(collect_monitor_handle),
            LPARAM(&mut monitors as *mut Vec<HMONITOR> as isize),
        )
    };
    if !succeeded.as_bool() {
        return Err(WindowsError::from_win32().into());
    }
    for monitor in monitors {
        let info = monitor_info(monitor)?;
        if display_device_id(&info) == display_id {
            return Ok(monitor);
        }
    }
    Err(PlatformError::DisplayNotFound(display_id.to_owned()))
}

unsafe extern "system" fn collect_monitor_handle(
    monitor: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    context: LPARAM,
) -> BOOL {
    let monitors = unsafe { &mut *(context.0 as *mut Vec<HMONITOR>) };
    monitors.push(monitor);
    true.into()
}

unsafe extern "system" fn collect_monitor(
    monitor: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    context: LPARAM,
) -> BOOL {
    let collector = unsafe { &mut *(context.0 as *mut MonitorCollector) };
    let info = match monitor_info(monitor) {
        Ok(info) => info,
        Err(error) => {
            collector.error = Some(error);
            return false.into();
        }
    };

    let bounds = info.monitorInfo.rcMonitor;
    let mut dpi_x = 0;
    let mut dpi_y = 0;
    if let Err(error) =
        unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) }
    {
        collector.error = Some(error);
        return false.into();
    }
    collector.displays.push(DisplayInfo {
        id: display_device_id(&info),
        bounds: PhysicalRect {
            x: bounds.left,
            y: bounds.top,
            width: (bounds.right - bounds.left).max(0) as u32,
            height: (bounds.bottom - bounds.top).max(0) as u32,
        },
        primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY == MONITORINFOF_PRIMARY,
        dpi_x,
        dpi_y,
        refresh_rate_hz: nominal_refresh_rate(&info),
    });

    true.into()
}

fn monitor_info(monitor: HMONITOR) -> Result<MONITORINFOEXW, windows::core::Error> {
    let mut info = MONITORINFOEXW {
        monitorInfo: windows::Win32::Graphics::Gdi::MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFOEXW>() as u32,
            ..Default::default()
        },
        ..Default::default()
    };

    if unsafe { GetMonitorInfoW(monitor, &mut info.monitorInfo) }.as_bool() {
        Ok(info)
    } else {
        Err(WindowsError::from_win32())
    }
}

fn display_device_id(info: &MONITORINFOEXW) -> String {
    let end = info
        .szDevice
        .iter()
        .position(|character| *character == 0)
        .unwrap_or(info.szDevice.len());
    String::from_utf16_lossy(&info.szDevice[..end])
}

fn nominal_refresh_rate(info: &MONITORINFOEXW) -> Option<u32> {
    let mut mode = DEVMODEW {
        dmSize: std::mem::size_of::<DEVMODEW>() as u16,
        ..Default::default()
    };
    let found = unsafe {
        EnumDisplaySettingsW(
            PCWSTR(info.szDevice.as_ptr()),
            ENUM_CURRENT_SETTINGS,
            &mut mode,
        )
    };
    found
        .as_bool()
        .then_some(mode.dmDisplayFrequency)
        .filter(|rate| *rate > 1)
}
