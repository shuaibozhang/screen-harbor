use std::{
    sync::mpsc,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM},
        System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, PeekMessageW,
            RegisterClassW, TranslateMessage, UnregisterClassW, MSG, PM_REMOVE, WM_DEVICECHANGE,
            WM_DISPLAYCHANGE, WM_DPICHANGED, WM_SETTINGCHANGE, WNDCLASSW, WS_EX_NOACTIVATE,
            WS_EX_TOOLWINDOW, WS_POPUP,
        },
    },
};

use crate::{displays::enable_current_thread_per_monitor_v2_dpi, PlatformError};

const DEBOUNCE: Duration = Duration::from_millis(200);

pub struct DisplayChangeSession {
    stop: mpsc::Sender<()>,
    worker: Option<JoinHandle<()>>,
}

impl DisplayChangeSession {
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        let _ = self.stop.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for DisplayChangeSession {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub fn watch_display_changes<F>(on_changed: F) -> Result<DisplayChangeSession, PlatformError>
where
    F: FnMut() + Send + 'static,
{
    let (stop_sender, stop_receiver) = mpsc::channel();
    let (startup_sender, startup_receiver) = mpsc::sync_channel(1);
    let worker = thread::Builder::new()
        .name("frameflow-display-watcher".to_owned())
        .spawn(move || run_watcher(stop_receiver, startup_sender, on_changed))
        .map_err(|error| PlatformError::WorkerUnavailable(error.to_string()))?;

    match startup_receiver.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(())) => Ok(DisplayChangeSession {
            stop: stop_sender,
            worker: Some(worker),
        }),
        Ok(Err(error)) => {
            let _ = worker.join();
            Err(PlatformError::WorkerUnavailable(error))
        }
        Err(_) => {
            let _ = stop_sender.send(());
            let _ = worker.join();
            Err(PlatformError::WorkerUnavailable(
                "display watcher did not start".to_owned(),
            ))
        }
    }
}

fn run_watcher<F>(
    stop: mpsc::Receiver<()>,
    startup: mpsc::SyncSender<Result<(), String>>,
    mut on_changed: F,
) where
    F: FnMut(),
{
    if let Err(error) = enable_current_thread_per_monitor_v2_dpi() {
        let _ = startup.send(Err(error.to_string()));
        return;
    }
    let (window, class_name, instance) = match unsafe { create_watcher_window() } {
        Ok(resources) => resources,
        Err(error) => {
            let _ = startup.send(Err(error.to_string()));
            return;
        }
    };
    let _ = startup.send(Ok(()));

    let mut pending_since = None;
    while stop.try_recv().is_err() {
        let mut message = MSG::default();
        while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
            if matches!(
                message.message,
                WM_DISPLAYCHANGE | WM_DEVICECHANGE | WM_DPICHANGED | WM_SETTINGCHANGE
            ) {
                pending_since = Some(Instant::now());
            }
            unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        if pending_since.is_some_and(|started| started.elapsed() >= DEBOUNCE) {
            pending_since = None;
            on_changed();
        }
        thread::sleep(Duration::from_millis(16));
    }

    unsafe {
        let _ = DestroyWindow(window);
        let _ = UnregisterClassW(PCWSTR(class_name.as_ptr()), Some(instance));
    }
}

unsafe fn create_watcher_window() -> Result<(HWND, Vec<u16>, HINSTANCE), PlatformError> {
    let module = unsafe { GetModuleHandleW(None)? };
    let instance = HINSTANCE(module.0);
    let class_name = wide(&format!("FrameFlowDisplayWatcher-{}", unsafe {
        GetCurrentThreadId()
    }));
    let class = WNDCLASSW {
        lpfnWndProc: Some(watcher_window_proc),
        hInstance: instance,
        lpszClassName: PCWSTR(class_name.as_ptr()),
        ..Default::default()
    };
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err(windows::core::Error::from_win32().into());
    }
    let window = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            PCWSTR(class_name.as_ptr()),
            PCWSTR(class_name.as_ptr()),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(instance),
            None,
        )?
    };
    Ok((window, class_name, instance))
}

unsafe extern "system" fn watcher_window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_display_watcher_starts_and_stops_cleanly() {
        let watcher = watch_display_changes(|| {}).unwrap();
        watcher.stop();
    }
}
