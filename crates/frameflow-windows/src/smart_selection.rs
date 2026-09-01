use std::{ffi::c_void, mem::size_of};

use frameflow_capture::PhysicalRect;
use windows::{
    core::BOOL,
    Win32::{
        Foundation::{HWND, LPARAM, POINT, RECT},
        Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS},
        System::Threading::GetCurrentProcessId,
        UI::WindowsAndMessaging::{
            EnumChildWindows, GetClassNameW, GetTopWindow, GetWindow, GetWindowLongPtrW,
            GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindowVisible, GWL_EXSTYLE,
            GW_HWNDNEXT, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
        },
    },
};

const BROWSER_VIEWPORT_CLASSES: &[&str] =
    &["Chrome_RenderWidgetHostHWND", "Internet Explorer_Server"];
const IGNORED_TOP_LEVEL_CLASSES: &[&str] = &["Progman", "WorkerW", "Shell_TrayWnd"];

pub(crate) fn candidate_at_point(
    display: PhysicalRect,
    display_point: (i32, i32),
) -> Option<PhysicalRect> {
    let desktop_point = POINT {
        x: display.x + display_point.0,
        y: display.y + display_point.1,
    };
    let process_id = unsafe { GetCurrentProcessId() };
    let window = unsafe { top_level_window_at_point(desktop_point, process_id)? };
    let desktop_rect = unsafe { browser_viewport_at_point(window, desktop_point) }
        .or_else(|| unsafe { visible_window_rect(window) })?;
    desktop_rect_to_display(display, desktop_rect)
}

unsafe fn top_level_window_at_point(point: POINT, excluded_process_id: u32) -> Option<HWND> {
    let mut window = GetTopWindow(None).ok();
    while let Some(hwnd) = window {
        if is_selectable_window(hwnd, point, excluded_process_id) {
            return Some(hwnd);
        }
        window = GetWindow(hwnd, GW_HWNDNEXT).ok();
    }
    None
}

unsafe fn is_selectable_window(hwnd: HWND, point: POINT, excluded_process_id: u32) -> bool {
    if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
        return false;
    }
    let mut process_id = 0;
    GetWindowThreadProcessId(hwnd, Some(&mut process_id));
    if process_id == excluded_process_id {
        return false;
    }
    let extended_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
    if extended_style & (WS_EX_TOOLWINDOW.0 | WS_EX_TRANSPARENT.0) != 0 {
        return false;
    }
    let class_name = window_class_name(hwnd);
    if IGNORED_TOP_LEVEL_CLASSES.contains(&class_name.as_str()) || is_cloaked(hwnd) {
        return false;
    }
    visible_window_rect(hwnd).is_some_and(|rect| rect_contains(rect, point))
}

unsafe fn visible_window_rect(hwnd: HWND) -> Option<RECT> {
    let mut rect = RECT::default();
    if DwmGetWindowAttribute(
        hwnd,
        DWMWA_EXTENDED_FRAME_BOUNDS,
        (&mut rect as *mut RECT).cast::<c_void>(),
        size_of::<RECT>() as u32,
    )
    .is_err()
        && GetWindowRect(hwnd, &mut rect).is_err()
    {
        return None;
    }
    (rect.right > rect.left && rect.bottom > rect.top).then_some(rect)
}

unsafe fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked = 0u32;
    DwmGetWindowAttribute(
        hwnd,
        DWMWA_CLOAKED,
        (&mut cloaked as *mut u32).cast::<c_void>(),
        size_of::<u32>() as u32,
    )
    .is_ok()
        && cloaked != 0
}

struct BrowserViewportSearch {
    point: POINT,
    best: Option<RECT>,
}

unsafe fn browser_viewport_at_point(window: HWND, point: POINT) -> Option<RECT> {
    let mut search = BrowserViewportSearch { point, best: None };
    let _ = EnumChildWindows(
        Some(window),
        Some(collect_browser_viewport),
        LPARAM((&mut search as *mut BrowserViewportSearch) as isize),
    );
    search.best
}

unsafe extern "system" fn collect_browser_viewport(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let search = &mut *(lparam.0 as *mut BrowserViewportSearch);
    if !IsWindowVisible(hwnd).as_bool() {
        return true.into();
    }
    let class_name = window_class_name(hwnd);
    if !BROWSER_VIEWPORT_CLASSES.contains(&class_name.as_str()) {
        return true.into();
    }
    let mut rect = RECT::default();
    if GetWindowRect(hwnd, &mut rect).is_err() || !rect_contains(rect, search.point) {
        return true.into();
    }
    if search
        .best
        .is_none_or(|current| rect_area(rect) < rect_area(current))
    {
        search.best = Some(rect);
    }
    true.into()
}

unsafe fn window_class_name(hwnd: HWND) -> String {
    let mut buffer = [0u16; 256];
    let length = GetClassNameW(hwnd, &mut buffer);
    String::from_utf16_lossy(&buffer[..length.max(0) as usize])
}

fn desktop_rect_to_display(display: PhysicalRect, rect: RECT) -> Option<PhysicalRect> {
    let display_right = display.x.checked_add_unsigned(display.width)?;
    let display_bottom = display.y.checked_add_unsigned(display.height)?;
    let left = rect.left.max(display.x);
    let top = rect.top.max(display.y);
    let right = rect.right.min(display_right);
    let bottom = rect.bottom.min(display_bottom);
    (right > left && bottom > top).then_some(PhysicalRect {
        x: left - display.x,
        y: top - display.y,
        width: (right - left) as u32,
        height: (bottom - top) as u32,
    })
}

fn rect_contains(rect: RECT, point: POINT) -> bool {
    point.x >= rect.left && point.x < rect.right && point.y >= rect.top && point.y < rect.bottom
}

fn rect_area(rect: RECT) -> i64 {
    i64::from(rect.right - rect.left) * i64::from(rect.bottom - rect.top)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_window_is_converted_to_display_relative_pixels() {
        let display = PhysicalRect {
            x: -1920,
            y: 120,
            width: 1920,
            height: 1080,
        };
        let rect = RECT {
            left: -1700,
            top: 260,
            right: -400,
            bottom: 980,
        };

        assert_eq!(
            desktop_rect_to_display(display, rect),
            Some(PhysicalRect {
                x: 220,
                y: 140,
                width: 1300,
                height: 720,
            })
        );
    }

    #[test]
    fn cross_display_window_is_clipped_to_the_capture_display() {
        let display = PhysicalRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        let rect = RECT {
            left: -300,
            top: -100,
            right: 800,
            bottom: 700,
        };

        assert_eq!(
            desktop_rect_to_display(display, rect),
            Some(PhysicalRect {
                x: 0,
                y: 0,
                width: 800,
                height: 700,
            })
        );
    }

    #[test]
    fn rectangles_use_half_open_point_containment() {
        let rect = RECT {
            left: 10,
            top: 20,
            right: 30,
            bottom: 40,
        };

        assert!(rect_contains(rect, POINT { x: 10, y: 20 }));
        assert!(rect_contains(rect, POINT { x: 29, y: 39 }));
        assert!(!rect_contains(rect, POINT { x: 30, y: 39 }));
        assert!(!rect_contains(rect, POINT { x: 29, y: 40 }));
    }
}
