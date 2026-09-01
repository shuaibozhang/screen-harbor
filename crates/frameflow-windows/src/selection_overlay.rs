use std::{
    ffi::c_void,
    sync::mpsc,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use frameflow_capture::PhysicalRect;
use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        Graphics::Gdi::{
            BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, GetStockObject,
            ScreenToClient, BLACK_BRUSH, HBRUSH, HGDIOBJ, PAINTSTRUCT,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Input::KeyboardAndMouse::{ReleaseCapture, SetCapture},
            WindowsAndMessaging::{
                BeginDeferWindowPos, CreateWindowExW, DefWindowProcW, DeferWindowPos,
                DestroyWindow, DispatchMessageW, EndDeferWindowPos, GetCursorPos,
                GetWindowLongPtrW, LoadCursorW, PeekMessageW, RegisterClassW, SetCursor,
                SetForegroundWindow, SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos,
                ShowWindow, TranslateMessage, CREATESTRUCTW, GWLP_USERDATA, GWL_EXSTYLE, HTCLIENT,
                HTTRANSPARENT, IDC_CROSS, IDC_SIZEALL, IDC_SIZENESW, IDC_SIZENS, IDC_SIZENWSE,
                IDC_SIZEWE, LWA_ALPHA, MSG, PM_REMOVE, SWP_HIDEWINDOW, SWP_NOACTIVATE,
                SWP_NOZORDER, SWP_SHOWWINDOW, SW_HIDE, SW_SHOW, SW_SHOWNOACTIVATE, WM_ERASEBKGND,
                WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCCREATE, WM_NCHITTEST,
                WM_PAINT, WM_SETCURSOR, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
                WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
            },
        },
    },
};

use crate::{
    displays::enable_current_thread_per_monitor_v2_dpi, exclude_window_from_capture,
    smart_selection::candidate_at_point, PlatformError,
};

const MIN_SELECTION_SIZE: i32 = 32;
const BORDER_THICKNESS: i32 = 5;
const CORNER_HIT_SIZE: i32 = 18;
const MOVE_HANDLE_WIDTH: i32 = 28;
const MOVE_HANDLE_HEIGHT: i32 = 16;
const SELECTION_MASK_ALPHA: u8 = 78;
const FREE_SELECTION_DRAG_THRESHOLD: i32 = 5;
const SMART_HOVER_SCAN_INTERVAL: Duration = Duration::from_millis(16);

enum SessionCommand {
    Lock(mpsc::SyncSender<()>),
    Stop,
}

pub struct NativeSelectionSession {
    control: mpsc::Sender<SessionCommand>,
    worker: Option<JoinHandle<()>>,
}

impl NativeSelectionSession {
    pub fn lock(&self) -> Result<(), PlatformError> {
        let (acknowledge, receiver) = mpsc::sync_channel(1);
        self.control
            .send(SessionCommand::Lock(acknowledge))
            .map_err(|error| PlatformError::WorkerUnavailable(error.to_string()))?;
        receiver
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| PlatformError::WorkerControlTimeout("lock selection border"))
    }

    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        let _ = self.control.send(SessionCommand::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for NativeSelectionSession {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct OverlayState {
    display: PhysicalRect,
    origin: Option<(i32, i32)>,
    current: Option<(i32, i32)>,
    hovered: Option<PhysicalRect>,
    last_hover_scan: Option<Instant>,
    dragging: bool,
    completed: Option<PhysicalRect>,
    cancelled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BorderRole {
    Top,
    Bottom,
    Left,
    Right,
    Move,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DragMode {
    Move,
    Top,
    Bottom,
    Left,
    Right,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

struct DragState {
    mode: DragMode,
    cursor: POINT,
    selection: PhysicalRect,
}

struct BorderState {
    display: PhysicalRect,
    selection: PhysicalRect,
    windows: Vec<(HWND, BorderRole)>,
    drag: Option<DragState>,
    pending_update: Option<PhysicalRect>,
    locked: bool,
}

struct BorderWindowBinding {
    state: *mut BorderState,
    role: BorderRole,
}

struct BorderResources {
    windows: Vec<HWND>,
    _state: Box<BorderState>,
    // Each allocation must remain stable because its address is stored in
    // GWLP_USERDATA before the vector has finished growing.
    #[allow(clippy::vec_box)]
    _bindings: Vec<Box<BorderWindowBinding>>,
    brush: windows::Win32::Graphics::Gdi::HBRUSH,
    class_name: Vec<u16>,
}

struct PreviewResources {
    display: PhysicalRect,
    windows: Vec<(HWND, BorderRole)>,
    brush: HBRUSH,
    class_name: Vec<u16>,
}

struct MaskResources {
    display: PhysicalRect,
    windows: Vec<HWND>,
    brush: HBRUSH,
    class_name: Vec<u16>,
}

#[derive(Clone, Copy)]
enum SelectionVisualLayout {
    FullMask,
    Selection(PhysicalRect),
    Hidden,
}

impl OverlayState {
    fn selection(&self) -> Option<PhysicalRect> {
        if self.dragging {
            Some(rect_from_points(self.origin?, self.current?))
        } else {
            self.hovered
        }
    }

    fn update_hover(&mut self, point: (i32, i32)) {
        self.hovered = candidate_at_point(self.display, point);
        self.last_hover_scan = Some(Instant::now());
    }

    fn update_hover_if_due(&mut self, point: (i32, i32)) {
        if self
            .last_hover_scan
            .is_none_or(|last_scan| last_scan.elapsed() >= SMART_HOVER_SCAN_INTERVAL)
        {
            self.update_hover(point);
        }
    }

    fn update_drag(&mut self, point: (i32, i32)) {
        self.current = Some(point);
        let Some(origin) = self.origin else {
            return;
        };
        if (point.0 - origin.0).abs() >= FREE_SELECTION_DRAG_THRESHOLD
            || (point.1 - origin.1).abs() >= FREE_SELECTION_DRAG_THRESHOLD
        {
            self.dragging = true;
        }
    }

    fn finish_pointer_action(&mut self, point: (i32, i32)) {
        self.update_drag(point);
        let selection = self.selection().filter(|selection| {
            selection.width >= MIN_SELECTION_SIZE as u32
                && selection.height >= MIN_SELECTION_SIZE as u32
        });
        self.origin = None;
        self.current = None;
        self.dragging = false;
        if let Some(selection) = selection {
            self.completed = Some(selection);
        } else {
            self.update_hover(point);
        }
    }
}

fn rect_from_points(origin: (i32, i32), current: (i32, i32)) -> PhysicalRect {
    let left = origin.0.min(current.0);
    let top = origin.1.min(current.1);
    let right = origin.0.max(current.0);
    let bottom = origin.1.max(current.1);
    PhysicalRect {
        x: left,
        y: top,
        width: (right - left) as u32,
        height: (bottom - top) as u32,
    }
}

/// Starts a native, translucent Win32 drag surface. Once drawing completes the
/// full-screen surface is destroyed and replaced by editable native border
/// windows. The session can be locked before recording to make them click-through.
pub fn start_native_selection<F>(
    display_bounds: PhysicalRect,
    on_selected: F,
) -> Result<NativeSelectionSession, PlatformError>
where
    F: FnMut(Option<PhysicalRect>) + Send + 'static,
{
    let (control_sender, control_receiver) = mpsc::channel();
    let (startup_sender, startup_receiver) = mpsc::sync_channel(1);
    let worker = thread::Builder::new()
        .name("frameflow-native-selection".to_owned())
        .spawn(move || {
            run_overlay(
                display_bounds,
                on_selected,
                control_receiver,
                startup_sender,
            )
        })
        .map_err(|error| PlatformError::WorkerUnavailable(error.to_string()))?;

    match startup_receiver.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(())) => Ok(NativeSelectionSession {
            control: control_sender,
            worker: Some(worker),
        }),
        Ok(Err(error)) => {
            let _ = worker.join();
            Err(PlatformError::WorkerUnavailable(error))
        }
        Err(_) => {
            let _ = control_sender.send(SessionCommand::Stop);
            let _ = worker.join();
            Err(PlatformError::WorkerUnavailable(
                "native selection window did not start".to_owned(),
            ))
        }
    }
}

fn run_overlay<F>(
    display_bounds: PhysicalRect,
    mut on_selected: F,
    control: mpsc::Receiver<SessionCommand>,
    startup: mpsc::SyncSender<Result<(), String>>,
) where
    F: FnMut(Option<PhysicalRect>),
{
    if let Err(error) = enable_current_thread_per_monitor_v2_dpi() {
        let _ = startup.send(Err(format!(
            "failed to enable physical-pixel selection coordinates: {error}"
        )));
        return;
    }
    let result = unsafe { create_overlay_window(display_bounds) };
    let (overlay, mut overlay_state, overlay_brush, overlay_class) = match result {
        Ok(value) => value,
        Err(error) => {
            let _ = startup.send(Err(error.to_string()));
            return;
        }
    };
    // Create the dimming mask before the border preview so the border remains
    // the top-most visual while both sets of windows are visible.
    let mask = match unsafe { create_mask_windows(display_bounds) } {
        Ok(mask) => mask,
        Err(error) => {
            unsafe {
                let _ = DestroyWindow(overlay);
                let _ = DeleteObject(HGDIOBJ(overlay_brush.0));
                let _ = windows::Win32::UI::WindowsAndMessaging::UnregisterClassW(
                    PCWSTR(overlay_class.as_ptr()),
                    Some(module_instance()),
                );
            }
            let _ = startup.send(Err(error.to_string()));
            return;
        }
    };
    let preview = match unsafe { create_preview_windows(display_bounds) } {
        Ok(preview) => preview,
        Err(error) => {
            unsafe {
                let _ = DestroyWindow(overlay);
                for window in mask.windows {
                    let _ = DestroyWindow(window);
                }
                let _ = DeleteObject(HGDIOBJ(mask.brush.0));
                let _ = windows::Win32::UI::WindowsAndMessaging::UnregisterClassW(
                    PCWSTR(mask.class_name.as_ptr()),
                    Some(module_instance()),
                );
                let _ = DeleteObject(HGDIOBJ(overlay_brush.0));
                let _ = windows::Win32::UI::WindowsAndMessaging::UnregisterClassW(
                    PCWSTR(overlay_class.as_ptr()),
                    Some(module_instance()),
                );
            }
            let _ = startup.send(Err(error.to_string()));
            return;
        }
    };
    let mut preview_selection = overlay_state
        .selection()
        .filter(|selection| selection.width > 0 && selection.height > 0);
    unsafe {
        let layout = preview_selection.map_or(
            SelectionVisualLayout::FullMask,
            SelectionVisualLayout::Selection,
        );
        let _ = layout_selection_visuals(&mask, &preview, layout);
    }
    let _ = startup.send(Ok(()));

    let mut borders: Option<BorderResources> = None;
    let mut running = true;
    while running {
        unsafe { pump_messages() };

        if overlay_state.cancelled {
            on_selected(None);
            running = false;
            continue;
        }

        if borders.is_none() {
            if let Some(selection) = overlay_state.completed.take() {
                unsafe {
                    let _ = DestroyWindow(overlay);
                    let _ =
                        layout_selection_visuals(&mask, &preview, SelectionVisualLayout::Hidden);
                }
                match unsafe { create_border_windows(display_bounds, selection) } {
                    Ok(resources) => {
                        borders = Some(resources);
                        on_selected(Some(selection));
                    }
                    Err(_) => {
                        on_selected(None);
                        running = false;
                    }
                }
            } else {
                let next_preview = overlay_state
                    .selection()
                    .filter(|selection| selection.width > 0 && selection.height > 0);
                if next_preview != preview_selection {
                    unsafe {
                        let layout = next_preview.map_or(
                            SelectionVisualLayout::FullMask,
                            SelectionVisualLayout::Selection,
                        );
                        let _ = layout_selection_visuals(&mask, &preview, layout);
                    }
                    preview_selection = next_preview;
                }
            }
        } else if let Some(selection) = borders
            .as_mut()
            .and_then(|resources| resources._state.pending_update.take())
        {
            on_selected(Some(selection));
        }

        match control.recv_timeout(Duration::from_millis(8)) {
            Ok(SessionCommand::Lock(acknowledge)) => {
                if let Some(resources) = borders.as_mut() {
                    unsafe { lock_border_windows(resources) };
                }
                let _ = acknowledge.send(());
            }
            Ok(SessionCommand::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => running = false,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }

    unsafe {
        if borders.is_none() {
            let _ = DestroyWindow(overlay);
        }
        let _ = DeleteObject(HGDIOBJ(overlay_brush.0));
        let instance = module_instance();
        let _ = windows::Win32::UI::WindowsAndMessaging::UnregisterClassW(
            PCWSTR(overlay_class.as_ptr()),
            Some(instance),
        );
        for (window, _) in preview.windows {
            let _ = DestroyWindow(window);
        }
        let _ = DeleteObject(HGDIOBJ(preview.brush.0));
        let _ = windows::Win32::UI::WindowsAndMessaging::UnregisterClassW(
            PCWSTR(preview.class_name.as_ptr()),
            Some(instance),
        );
        for window in mask.windows {
            let _ = DestroyWindow(window);
        }
        let _ = DeleteObject(HGDIOBJ(mask.brush.0));
        let _ = windows::Win32::UI::WindowsAndMessaging::UnregisterClassW(
            PCWSTR(mask.class_name.as_ptr()),
            Some(instance),
        );
        if let Some(resources) = borders {
            for window in resources.windows {
                let _ = DestroyWindow(window);
            }
            let _ = DeleteObject(HGDIOBJ(resources.brush.0));
            let _ = windows::Win32::UI::WindowsAndMessaging::UnregisterClassW(
                PCWSTR(resources.class_name.as_ptr()),
                Some(instance),
            );
        }
    }
}

unsafe fn create_overlay_window(
    bounds: PhysicalRect,
) -> Result<
    (
        HWND,
        Box<OverlayState>,
        windows::Win32::Graphics::Gdi::HBRUSH,
        Vec<u16>,
    ),
    PlatformError,
> {
    let instance = module_instance();
    let class_name = wide(&format!("FrameFlowNativeSelection-{}", thread_id()));
    let brush = CreateSolidBrush(COLORREF(0));
    let class = WNDCLASSW {
        lpfnWndProc: Some(overlay_window_proc),
        hInstance: instance,
        hCursor: LoadCursorW(None, IDC_CROSS)?,
        hbrBackground: brush,
        lpszClassName: PCWSTR(class_name.as_ptr()),
        ..Default::default()
    };
    if RegisterClassW(&class) == 0 {
        return Err(windows::core::Error::from_win32().into());
    }

    let mut state = Box::new(OverlayState {
        display: bounds,
        origin: None,
        current: None,
        hovered: None,
        last_hover_scan: None,
        dragging: false,
        completed: None,
        cancelled: false,
    });
    let hwnd = CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
        PCWSTR(class_name.as_ptr()),
        w!("FrameFlow 区域选择"),
        WS_POPUP,
        bounds.x,
        bounds.y,
        bounds.width as i32,
        bounds.height as i32,
        None,
        None,
        Some(instance),
        Some((&mut *state as *mut OverlayState).cast::<c_void>()),
    )?;
    // A non-zero uniform alpha keeps the whole monitor mouse-active. At 1/255
    // it is visually imperceptible; four small native windows draw the preview.
    SetLayeredWindowAttributes(hwnd, COLORREF(0), 1, LWA_ALPHA)?;
    exclude_window_from_capture(hwnd.0 as isize)?;
    let _ = ShowWindow(hwnd, SW_SHOW);
    let _ = SetForegroundWindow(hwnd);
    let mut cursor = POINT::default();
    if GetCursorPos(&mut cursor).is_ok() {
        state.update_hover((cursor.x - bounds.x, cursor.y - bounds.y));
    }
    Ok((hwnd, state, brush, class_name))
}

unsafe fn create_preview_windows(display: PhysicalRect) -> Result<PreviewResources, PlatformError> {
    let instance = module_instance();
    let class_name = wide(&format!("FrameFlowSelectionPreview-{}", thread_id()));
    let brush = CreateSolidBrush(COLORREF(0x00ff_956f));
    let class = WNDCLASSW {
        lpfnWndProc: Some(preview_window_proc),
        hInstance: instance,
        hbrBackground: brush,
        lpszClassName: PCWSTR(class_name.as_ptr()),
        ..Default::default()
    };
    if RegisterClassW(&class) == 0 {
        return Err(windows::core::Error::from_win32().into());
    }

    let roles = [
        BorderRole::Top,
        BorderRole::Bottom,
        BorderRole::Left,
        BorderRole::Right,
    ];
    let mut windows = Vec::with_capacity(roles.len());
    for role in roles {
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
            PCWSTR(class_name.as_ptr()),
            w!("FrameFlow 选区预览"),
            WS_POPUP,
            display.x,
            display.y,
            1,
            1,
            None,
            None,
            Some(instance),
            None,
        )?;
        exclude_window_from_capture(hwnd.0 as isize)?;
        windows.push((hwnd, role));
    }
    Ok(PreviewResources {
        display,
        windows,
        brush,
        class_name,
    })
}

unsafe fn create_mask_windows(display: PhysicalRect) -> Result<MaskResources, PlatformError> {
    let instance = module_instance();
    let class_name = wide(&format!("FrameFlowSelectionMask-{}", thread_id()));
    let brush = CreateSolidBrush(COLORREF(0));
    let class = WNDCLASSW {
        lpfnWndProc: Some(mask_window_proc),
        hInstance: instance,
        hbrBackground: brush,
        lpszClassName: PCWSTR(class_name.as_ptr()),
        ..Default::default()
    };
    if RegisterClassW(&class) == 0 {
        return Err(windows::core::Error::from_win32().into());
    }

    let mut windows = Vec::with_capacity(4);
    for _ in 0..4 {
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
            PCWSTR(class_name.as_ptr()),
            w!("FrameFlow 框选遮罩"),
            WS_POPUP,
            display.x,
            display.y,
            1,
            1,
            None,
            None,
            Some(instance),
            None,
        )?;
        SetLayeredWindowAttributes(hwnd, COLORREF(0), SELECTION_MASK_ALPHA, LWA_ALPHA)?;
        exclude_window_from_capture(hwnd.0 as isize)?;
        windows.push(hwnd);
    }
    Ok(MaskResources {
        display,
        windows,
        brush,
        class_name,
    })
}

unsafe extern "system" fn mask_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCHITTEST {
        return LRESULT(HTTRANSPARENT as isize);
    }
    DefWindowProcW(hwnd, message, wparam, lparam)
}

fn mask_rectangles(display: PhysicalRect, selection: PhysicalRect) -> [(i32, i32, i32, i32); 4] {
    let selection_left = display.x + selection.x;
    let selection_top = display.y + selection.y;
    let selection_right = selection_left + selection.width as i32;
    let selection_bottom = selection_top + selection.height as i32;
    let display_right = display.x + display.width as i32;
    let display_bottom = display.y + display.height as i32;
    [
        (display.x, display.y, display.width as i32, selection.y),
        (
            display.x,
            selection_bottom,
            display.width as i32,
            display_bottom - selection_bottom,
        ),
        (
            display.x,
            selection_top,
            selection.x,
            selection.height as i32,
        ),
        (
            selection_right,
            selection_top,
            display_right - selection_right,
            selection.height as i32,
        ),
    ]
}

unsafe extern "system" fn preview_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCHITTEST {
        return LRESULT(HTTRANSPARENT as isize);
    }
    DefWindowProcW(hwnd, message, wparam, lparam)
}

unsafe fn layout_selection_visuals(
    mask: &MaskResources,
    preview: &PreviewResources,
    layout: SelectionVisualLayout,
) -> bool {
    let Ok(mut batch) = BeginDeferWindowPos((mask.windows.len() + preview.windows.len()) as i32)
    else {
        return false;
    };
    let mask_rects = match layout {
        SelectionVisualLayout::FullMask | SelectionVisualLayout::Hidden => [
            (
                mask.display.x,
                mask.display.y,
                mask.display.width as i32,
                mask.display.height as i32,
            ),
            (mask.display.x, mask.display.y, 1, 1),
            (mask.display.x, mask.display.y, 1, 1),
            (mask.display.x, mask.display.y, 1, 1),
        ],
        SelectionVisualLayout::Selection(selection) => mask_rectangles(mask.display, selection),
    };
    for (index, (window, (x, y, width, height))) in mask.windows.iter().zip(mask_rects).enumerate()
    {
        let visible = match layout {
            SelectionVisualLayout::FullMask => index == 0,
            SelectionVisualLayout::Selection(_) => width > 0 && height > 0,
            SelectionVisualLayout::Hidden => false,
        };
        let Ok(next) = DeferWindowPos(
            batch,
            *window,
            None,
            x,
            y,
            width.max(1),
            height.max(1),
            SWP_NOACTIVATE
                | SWP_NOZORDER
                | if visible {
                    SWP_SHOWWINDOW
                } else {
                    SWP_HIDEWINDOW
                },
        ) else {
            return false;
        };
        batch = next;
    }
    for (window, role) in &preview.windows {
        let (x, y, width, height, visible) = match layout {
            SelectionVisualLayout::Selection(selection) => {
                let (x, y, width, height) = window_rect(preview.display, selection, *role);
                (x, y, width, height, true)
            }
            SelectionVisualLayout::FullMask | SelectionVisualLayout::Hidden => {
                (preview.display.x, preview.display.y, 1, 1, false)
            }
        };
        let Ok(next) = DeferWindowPos(
            batch,
            *window,
            None,
            x,
            y,
            width,
            height,
            SWP_NOACTIVATE
                | SWP_NOZORDER
                | if visible {
                    SWP_SHOWWINDOW
                } else {
                    SWP_HIDEWINDOW
                },
        ) else {
            return false;
        };
        batch = next;
    }
    EndDeferWindowPos(batch).is_ok()
}

unsafe fn create_border_windows(
    display: PhysicalRect,
    selection: PhysicalRect,
) -> Result<BorderResources, PlatformError> {
    let instance = module_instance();
    let class_name = wide(&format!("FrameFlowSelectionBorder-{}", thread_id()));
    let brush = CreateSolidBrush(COLORREF(0x00ff_956f));
    let class = WNDCLASSW {
        lpfnWndProc: Some(border_window_proc),
        hInstance: instance,
        hbrBackground: brush,
        lpszClassName: PCWSTR(class_name.as_ptr()),
        ..Default::default()
    };
    if RegisterClassW(&class) == 0 {
        return Err(windows::core::Error::from_win32().into());
    }

    let mut state = Box::new(BorderState {
        display,
        selection,
        windows: Vec::new(),
        drag: None,
        pending_update: None,
        locked: false,
    });
    let roles = [
        BorderRole::Top,
        BorderRole::Bottom,
        BorderRole::Left,
        BorderRole::Right,
        BorderRole::Move,
    ];
    let mut windows = Vec::with_capacity(roles.len());
    let mut bindings = Vec::with_capacity(roles.len());
    for role in roles {
        let (left, top, window_width, window_height) = window_rect(display, selection, role);
        let mut binding = Box::new(BorderWindowBinding {
            state: &mut *state,
            role,
        });
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            PCWSTR(class_name.as_ptr()),
            w!("FrameFlow 录制边框"),
            WS_POPUP,
            left,
            top,
            window_width,
            window_height,
            None,
            None,
            Some(instance),
            Some((&mut *binding as *mut BorderWindowBinding).cast::<c_void>()),
        )?;
        exclude_window_from_capture(hwnd.0 as isize)?;
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        windows.push(hwnd);
        bindings.push(binding);
    }
    state.windows = windows.iter().copied().zip(roles).collect();
    Ok(BorderResources {
        windows,
        _state: state,
        _bindings: bindings,
        brush,
        class_name,
    })
}

fn border_rectangles(display: PhysicalRect, selection: PhysicalRect) -> [(i32, i32, i32, i32); 4] {
    let x = display.x + selection.x;
    let y = display.y + selection.y;
    let width = selection.width as i32;
    let height = selection.height as i32;
    [
        (x, y, width, BORDER_THICKNESS),
        (x, y + height - BORDER_THICKNESS, width, BORDER_THICKNESS),
        (x, y, BORDER_THICKNESS, height),
        (x + width - BORDER_THICKNESS, y, BORDER_THICKNESS, height),
    ]
}

unsafe extern "system" fn border_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = &*(lparam.0 as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
    }
    let binding = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut BorderWindowBinding;
    if binding.is_null() {
        return DefWindowProcW(hwnd, message, wparam, lparam);
    }
    let binding = &mut *binding;
    let state = &mut *binding.state;
    let mode = if message == WM_SETCURSOR {
        cursor_drag_mode(hwnd, binding.role, state.selection)
    } else {
        drag_mode(
            binding.role,
            signed_low_word(lparam.0),
            signed_high_word(lparam.0),
            state.selection,
        )
    };
    match message {
        WM_NCHITTEST if state.locked => LRESULT(HTTRANSPARENT as isize),
        WM_SETCURSOR if !state.locked => {
            let cursor = match mode {
                DragMode::Move => IDC_SIZEALL,
                DragMode::Top | DragMode::Bottom => IDC_SIZENS,
                DragMode::Left | DragMode::Right => IDC_SIZEWE,
                DragMode::TopLeft | DragMode::BottomRight => IDC_SIZENWSE,
                DragMode::TopRight | DragMode::BottomLeft => IDC_SIZENESW,
            };
            if let Ok(cursor) = LoadCursorW(None, cursor) {
                SetCursor(Some(cursor));
            }
            LRESULT(1)
        }
        WM_LBUTTONDOWN if !state.locked => {
            let mut cursor = POINT::default();
            if GetCursorPos(&mut cursor).is_ok() {
                state.drag = Some(DragState {
                    mode,
                    cursor,
                    selection: state.selection,
                });
                SetCapture(hwnd);
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE if state.drag.is_some() && !state.locked => {
            let mut cursor = POINT::default();
            if GetCursorPos(&mut cursor).is_ok() {
                let drag = state.drag.as_ref().expect("drag state must exist");
                let next = resize_selection(
                    drag.selection,
                    drag.mode,
                    cursor.x - drag.cursor.x,
                    cursor.y - drag.cursor.y,
                    state.display,
                );
                if next != state.selection {
                    state.selection = next;
                    layout_border_windows(state);
                    state.pending_update = Some(next);
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONUP if state.drag.is_some() && !state.locked => {
            state.drag = None;
            state.pending_update = Some(state.selection);
            let _ = ReleaseCapture();
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

unsafe fn cursor_drag_mode(window: HWND, role: BorderRole, selection: PhysicalRect) -> DragMode {
    let mut cursor = POINT::default();
    if GetCursorPos(&mut cursor).is_ok() {
        let _ = ScreenToClient(window, &mut cursor);
    }
    drag_mode(role, cursor.x, cursor.y, selection)
}

unsafe fn lock_border_windows(resources: &mut BorderResources) {
    resources._state.locked = true;
    resources._state.drag = None;
    let _ = ReleaseCapture();
    for (window, role) in &resources._state.windows {
        if *role == BorderRole::Move {
            let _ = ShowWindow(*window, SW_HIDE);
        } else {
            let style = GetWindowLongPtrW(*window, GWL_EXSTYLE);
            SetWindowLongPtrW(
                *window,
                GWL_EXSTYLE,
                style | WS_EX_TRANSPARENT.0 as isize | WS_EX_NOACTIVATE.0 as isize,
            );
        }
    }
}

fn drag_mode(role: BorderRole, x: i32, y: i32, selection: PhysicalRect) -> DragMode {
    match role {
        BorderRole::Move => DragMode::Move,
        BorderRole::Top if x <= CORNER_HIT_SIZE => DragMode::TopLeft,
        BorderRole::Top if x >= selection.width as i32 - CORNER_HIT_SIZE => DragMode::TopRight,
        BorderRole::Bottom if x <= CORNER_HIT_SIZE => DragMode::BottomLeft,
        BorderRole::Bottom if x >= selection.width as i32 - CORNER_HIT_SIZE => {
            DragMode::BottomRight
        }
        BorderRole::Top => DragMode::Top,
        BorderRole::Bottom => DragMode::Bottom,
        BorderRole::Left if y <= CORNER_HIT_SIZE => DragMode::TopLeft,
        BorderRole::Left if y >= selection.height as i32 - CORNER_HIT_SIZE => DragMode::BottomLeft,
        BorderRole::Right if y <= CORNER_HIT_SIZE => DragMode::TopRight,
        BorderRole::Right if y >= selection.height as i32 - CORNER_HIT_SIZE => {
            DragMode::BottomRight
        }
        BorderRole::Left => DragMode::Left,
        BorderRole::Right => DragMode::Right,
    }
}

fn resize_selection(
    initial: PhysicalRect,
    mode: DragMode,
    delta_x: i32,
    delta_y: i32,
    display: PhysicalRect,
) -> PhysicalRect {
    let mut left = initial.x;
    let mut top = initial.y;
    let mut right = initial.x + initial.width as i32;
    let mut bottom = initial.y + initial.height as i32;
    let max_x = display.width as i32;
    let max_y = display.height as i32;

    if mode == DragMode::Move {
        let width = initial.width as i32;
        let height = initial.height as i32;
        left = (initial.x + delta_x).clamp(0, max_x - width);
        top = (initial.y + delta_y).clamp(0, max_y - height);
        right = left + width;
        bottom = top + height;
    } else {
        if matches!(
            mode,
            DragMode::Left | DragMode::TopLeft | DragMode::BottomLeft
        ) {
            left = (initial.x + delta_x).clamp(0, right - MIN_SELECTION_SIZE);
        }
        if matches!(
            mode,
            DragMode::Right | DragMode::TopRight | DragMode::BottomRight
        ) {
            right = (initial.x + initial.width as i32 + delta_x)
                .clamp(left + MIN_SELECTION_SIZE, max_x);
        }
        if matches!(mode, DragMode::Top | DragMode::TopLeft | DragMode::TopRight) {
            top = (initial.y + delta_y).clamp(0, bottom - MIN_SELECTION_SIZE);
        }
        if matches!(
            mode,
            DragMode::Bottom | DragMode::BottomLeft | DragMode::BottomRight
        ) {
            bottom = (initial.y + initial.height as i32 + delta_y)
                .clamp(top + MIN_SELECTION_SIZE, max_y);
        }
    }

    PhysicalRect {
        x: left,
        y: top,
        width: (right - left) as u32,
        height: (bottom - top) as u32,
    }
}

unsafe fn layout_border_windows(state: &BorderState) {
    if let Ok(mut batch) = BeginDeferWindowPos(state.windows.len() as i32) {
        let mut complete = true;
        for (window, role) in &state.windows {
            let (x, y, width, height) = window_rect(state.display, state.selection, *role);
            match DeferWindowPos(
                batch,
                *window,
                None,
                x,
                y,
                width,
                height,
                SWP_NOACTIVATE | SWP_NOZORDER,
            ) {
                Ok(next) => batch = next,
                Err(_) => {
                    complete = false;
                    break;
                }
            }
        }
        if complete {
            let _ = EndDeferWindowPos(batch);
            return;
        }
    }

    // Keep a conservative fallback for systems that cannot allocate a deferred
    // positioning batch.
    for (window, role) in &state.windows {
        let (x, y, width, height) = window_rect(state.display, state.selection, *role);
        let _ = SetWindowPos(
            *window,
            None,
            x,
            y,
            width,
            height,
            SWP_NOACTIVATE | SWP_NOZORDER,
        );
    }
}

fn window_rect(
    display: PhysicalRect,
    selection: PhysicalRect,
    role: BorderRole,
) -> (i32, i32, i32, i32) {
    let borders = border_rectangles(display, selection);
    match role {
        BorderRole::Top => borders[0],
        BorderRole::Bottom => borders[1],
        BorderRole::Left => borders[2],
        BorderRole::Right => borders[3],
        BorderRole::Move => {
            let x = display.x + selection.x + (selection.width as i32 - MOVE_HANDLE_WIDTH) / 2;
            let y = display.y + selection.y + BORDER_THICKNESS;
            (x, y, MOVE_HANDLE_WIDTH, MOVE_HANDLE_HEIGHT)
        }
    }
}

unsafe extern "system" fn overlay_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = &*(lparam.0 as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
    }
    let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut OverlayState;
    if state_ptr.is_null() {
        return DefWindowProcW(hwnd, message, wparam, lparam);
    }
    let state = &mut *state_ptr;
    let point = || (signed_low_word(lparam.0), signed_high_word(lparam.0));
    match message {
        // The full monitor remains an explicit input surface even though its
        // uniform alpha is visually imperceptible.
        WM_NCHITTEST => LRESULT(HTCLIENT as isize),
        WM_KEYDOWN
            if wparam.0 as u32
                == windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE.0 as u32 =>
        {
            state.cancelled = true;
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let point = point();
            state.update_hover(point);
            state.origin = Some(point);
            state.current = Some(point);
            state.dragging = false;
            SetCapture(hwnd);
            LRESULT(0)
        }
        WM_MOUSEMOVE if state.origin.is_some() => {
            state.update_drag(point());
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            state.update_hover_if_due(point());
            LRESULT(0)
        }
        WM_LBUTTONUP if state.origin.is_some() => {
            state.finish_pointer_action(point());
            let _ = ReleaseCapture();
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut paint);
            let mut client = RECT::default();
            let _ = windows::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut client);
            FillRect(dc, &client, HBRUSH(GetStockObject(BLACK_BRUSH).0));
            let _ = EndPaint(hwnd, &paint);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

unsafe fn pump_messages() {
    let mut message = MSG::default();
    while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
        let _ = TranslateMessage(&message);
        DispatchMessageW(&message);
    }
}

unsafe fn module_instance() -> HINSTANCE {
    HINSTANCE(
        GetModuleHandleW(None)
            .expect("current module handle is unavailable")
            .0,
    )
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

fn thread_id() -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    thread::current().id().hash(&mut hasher);
    hasher.finish()
}

fn signed_low_word(value: isize) -> i32 {
    (value as u16 as i16) as i32
}

fn signed_high_word(value: isize) -> i32 {
    ((value >> 16) as u16 as i16) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_rectangle_is_direction_independent() {
        let expected = PhysicalRect {
            x: 120,
            y: 80,
            width: 300,
            height: 200,
        };
        assert_eq!(rect_from_points((120, 80), (420, 280)), expected);
        assert_eq!(rect_from_points((420, 280), (120, 80)), expected);
    }

    #[test]
    fn selection_mask_surrounds_the_clear_capture_region() {
        let display = PhysicalRect {
            x: -1920,
            y: 120,
            width: 1920,
            height: 1080,
        };
        let selection = PhysicalRect {
            x: 320,
            y: 180,
            width: 1280,
            height: 720,
        };

        assert_eq!(
            mask_rectangles(display, selection),
            [
                (-1920, 120, 1920, 180),
                (-1920, 1020, 1920, 180),
                (-1920, 300, 320, 720),
                (-320, 300, 320, 720),
            ]
        );
    }

    #[test]
    fn click_keeps_the_hovered_smart_candidate() {
        let candidate = PhysicalRect {
            x: 100,
            y: 80,
            width: 900,
            height: 600,
        };
        let mut state = OverlayState {
            display: PhysicalRect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
            origin: Some((300, 300)),
            current: Some((300, 300)),
            hovered: Some(candidate),
            last_hover_scan: None,
            dragging: false,
            completed: None,
            cancelled: false,
        };

        state.update_drag((304, 303));
        assert_eq!(state.selection(), Some(candidate));
        state.finish_pointer_action((304, 303));
        assert_eq!(state.completed, Some(candidate));
    }

    #[test]
    fn pointer_motion_switches_from_smart_candidate_to_free_selection() {
        let mut state = OverlayState {
            display: PhysicalRect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
            origin: Some((300, 260)),
            current: Some((300, 260)),
            hovered: Some(PhysicalRect {
                x: 100,
                y: 80,
                width: 900,
                height: 600,
            }),
            last_hover_scan: None,
            dragging: false,
            completed: None,
            cancelled: false,
        };

        state.update_drag((305, 420));

        assert!(state.dragging);
        assert_eq!(
            state.selection(),
            Some(PhysicalRect {
                x: 300,
                y: 260,
                width: 5,
                height: 160,
            })
        );
    }

    #[test]
    fn border_windows_follow_virtual_desktop_coordinates() {
        let display = PhysicalRect {
            x: -2560,
            y: 0,
            width: 2560,
            height: 1440,
        };
        let selection = PhysicalRect {
            x: 100,
            y: 50,
            width: 640,
            height: 360,
        };
        assert_eq!(
            border_rectangles(display, selection),
            [
                (-2460, 50, 640, 5),
                (-2460, 405, 640, 5),
                (-2460, 50, 5, 360),
                (-1825, 50, 5, 360),
            ]
        );
    }

    #[test]
    fn move_and_resize_stay_inside_display() {
        let display = PhysicalRect {
            x: -2560,
            y: 0,
            width: 1000,
            height: 800,
        };
        let selection = PhysicalRect {
            x: 100,
            y: 100,
            width: 300,
            height: 200,
        };
        assert_eq!(
            resize_selection(selection, DragMode::Move, -500, 900, display),
            PhysicalRect {
                x: 0,
                y: 600,
                ..selection
            }
        );
        assert_eq!(
            resize_selection(selection, DragMode::TopLeft, 350, 250, display),
            PhysicalRect {
                x: 368,
                y: 268,
                width: 32,
                height: 32,
            }
        );
        assert_eq!(
            resize_selection(selection, DragMode::BottomRight, 900, 900, display),
            PhysicalRect {
                x: 100,
                y: 100,
                width: 900,
                height: 700,
            }
        );
    }

    #[test]
    fn border_hit_testing_exposes_eight_resize_directions() {
        let selection = PhysicalRect {
            x: 0,
            y: 0,
            width: 640,
            height: 360,
        };
        assert_eq!(
            drag_mode(BorderRole::Top, 0, 0, selection),
            DragMode::TopLeft
        );
        assert_eq!(drag_mode(BorderRole::Top, 320, 0, selection), DragMode::Top);
        assert_eq!(
            drag_mode(BorderRole::Bottom, 639, 0, selection),
            DragMode::BottomRight
        );
        assert_eq!(
            drag_mode(BorderRole::Left, 0, 180, selection),
            DragMode::Left
        );
        assert_eq!(drag_mode(BorderRole::Move, 0, 0, selection), DragMode::Move);
    }
}
