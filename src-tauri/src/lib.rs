mod recording_index;

use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use frameflow_capture::{CaptureSession, DisplayInfo, PhysicalRect, RegionSelection};
use frameflow_core::{OutputConfig, RecordingAction, RecordingState, RecordingStateMachine};
use recording_index::{RecordingIndex, RecordingSummary};
use serde::{Deserialize, Serialize};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, PhysicalPosition, PhysicalSize, State, WindowEvent,
};

const TRAY_SHOW_ID: &str = "show";
const TRAY_QUIT_ID: &str = "quit";

struct AppState {
    recording: Mutex<RecordingStateMachine>,
    selection: Mutex<Option<ActiveSelection>>,
    capture: Mutex<Option<ActiveCapture>>,
    recording_index: Mutex<Option<RecordingIndex>>,
    native_selection: Mutex<Option<frameflow_windows::NativeSelectionSession>>,
    output_config: Mutex<OutputConfig>,
    audio_config: Mutex<AudioInputConfig>,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct AudioInputConfig {
    microphone: bool,
    system_audio: bool,
}

impl Default for AudioInputConfig {
    fn default() -> Self {
        Self {
            microphone: false,
            system_audio: true,
        }
    }
}

struct ActiveSelection {
    display: DisplayInfo,
    region: Option<RegionSelection>,
}

struct ActiveCapture {
    session: frameflow_windows::WgcCaptureSession,
    audio: Option<ActiveAudioCapture>,
    frame_count: Arc<AtomicU64>,
    last_error: Arc<Mutex<Option<String>>>,
    started_at: Instant,
    paused_at: Option<Instant>,
    paused_total: Duration,
    temporary_path: PathBuf,
    output_path: PathBuf,
}

struct ActiveAudioCapture {
    config: AudioInputConfig,
    sources: Vec<ActiveAudioSource>,
    mixer: Arc<Mutex<frameflow_windows::AudioMixer>>,
    output: frameflow_windows::WgcAudioInput,
    paused: Arc<AtomicBool>,
    pause_offset_100ns: Arc<AtomicU64>,
    warning: Arc<Mutex<Option<String>>>,
}

struct ActiveAudioSource {
    id: frameflow_windows::AudioSourceId,
    session: frameflow_windows::WasapiCaptureSession,
    normalizer: Arc<Mutex<Option<frameflow_windows::AudioNormalizer>>>,
    failed: Arc<AtomicBool>,
}

#[derive(Clone)]
struct AudioCallbackContext {
    mixer: Arc<Mutex<frameflow_windows::AudioMixer>>,
    output: frameflow_windows::WgcAudioInput,
    paused: Arc<AtomicBool>,
    pause_offset_100ns: Arc<AtomicU64>,
    warning: Arc<Mutex<Option<String>>>,
    app: tauri::AppHandle,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CaptureStats {
    frame_count: u64,
    elapsed_ms: u128,
    paused: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    audio: Option<CaptureAudioStats>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CaptureAudioStats {
    microphone: LiveInputSnapshot,
    system_audio: LiveInputSnapshot,
    #[serde(skip_serializing_if = "Option::is_none")]
    warning: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LiveInputSnapshot {
    enabled: bool,
    muted: bool,
    disconnected: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AudioWarningEvent {
    source: &'static str,
    message: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FinalizationEvent {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

struct FinalizedCapture {
    output_path: PathBuf,
    duration: Duration,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InputSnapshot {
    enabled: bool,
    available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    device_name: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AppSnapshot {
    recording_state: RecordingState,
    save_directory: String,
    microphone: InputSnapshot,
    system_audio: InputSnapshot,
    output_summary: String,
    capture_fps: u16,
    supported_capture_fps: [u16; 3],
    monitor_refresh_rate_hz: Option<u32>,
    recommended_capture_fps: u16,
    recent_recordings: Vec<RecordingSummary>,
}

const SUPPORTED_CAPTURE_FPS: [u16; 3] = [24, 30, 60];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExitDecision {
    Exit,
    CancelThenExit,
    Block,
}

fn exit_decision(state: RecordingState) -> ExitDecision {
    match state {
        RecordingState::Idle | RecordingState::Completed | RecordingState::Failed => {
            ExitDecision::Exit
        }
        RecordingState::Selecting | RecordingState::Ready | RecordingState::Countdown => {
            ExitDecision::CancelThenExit
        }
        RecordingState::Recording
        | RecordingState::Paused
        | RecordingState::Finalizing
        | RecordingState::Processing => ExitDecision::Block,
    }
}

fn recommended_capture_fps(refresh_rate_hz: Option<u32>) -> u16 {
    let Some(refresh_rate_hz) = refresh_rate_hz else {
        return 30;
    };
    SUPPORTED_CAPTURE_FPS
        .into_iter()
        .rev()
        .find(|fps| u32::from(*fps) <= refresh_rate_hz.saturating_add(1))
        .unwrap_or(SUPPORTED_CAPTURE_FPS[0])
}

fn active_display(app: &tauri::AppHandle) -> Result<DisplayInfo, String> {
    let main = app
        .get_webview_window("main")
        .ok_or_else(|| "main window is unavailable".to_owned())?;
    let monitor = main
        .current_monitor()
        .map_err(|error| error.to_string())?
        .or_else(|| main.primary_monitor().ok().flatten())
        .ok_or_else(|| "no active display is available".to_owned())?;
    let position = *monitor.position();
    let size = *monitor.size();

    frameflow_windows::list_displays()
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|display| {
            display.bounds.x == position.x
                && display.bounds.y == position.y
                && display.bounds.width == size.width
                && display.bounds.height == size.height
        })
        .ok_or_else(|| "active display geometry is unavailable".to_owned())
}

#[tauri::command]
fn get_app_snapshot(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<AppSnapshot, String> {
    let recording_state = state
        .recording
        .lock()
        .map_err(|_| "recording state is unavailable".to_owned())?
        .state();

    let save_directory = save_directory().to_string_lossy().into_owned();
    let recent_recordings = state
        .recording_index
        .lock()
        .map_err(|_| "recording index is unavailable".to_owned())?
        .as_ref()
        .ok_or_else(|| "recording index is not initialized".to_owned())?
        .recent(usize::MAX)?;
    let capture_fps = state
        .output_config
        .lock()
        .map_err(|_| "output settings are unavailable".to_owned())?
        .main
        .fps;
    let monitor_refresh_rate_hz = active_display(&app)
        .ok()
        .and_then(|display| display.refresh_rate_hz);
    let recommended_capture_fps = recommended_capture_fps(monitor_refresh_rate_hz);
    let audio_endpoints = frameflow_windows::default_audio_endpoints().unwrap_or_default();
    let audio_config = *state
        .audio_config
        .lock()
        .map_err(|_| "audio settings are unavailable".to_owned())?;
    let audio_label = match (audio_config.system_audio, audio_config.microphone) {
        (true, true) => "系统声 + 麦克风",
        (true, false) => "系统声",
        (false, true) => "麦克风",
        (false, false) => "无音频",
    };
    let codec_label = if audio_config.system_audio || audio_config.microphone {
        "H.264/AAC"
    } else {
        "H.264"
    };

    Ok(AppSnapshot {
        recording_state,
        save_directory,
        microphone: InputSnapshot {
            enabled: audio_config.microphone,
            available: audio_endpoints.microphone.is_some(),
            device_name: audio_endpoints.microphone.map(|endpoint| endpoint.name),
        },
        system_audio: InputSnapshot {
            enabled: audio_config.system_audio,
            available: audio_endpoints.system_output.is_some(),
            device_name: audio_endpoints.system_output.map(|endpoint| endpoint.name),
        },
        output_summary: format!("MP4 · {codec_label} · {capture_fps} FPS · {audio_label}"),
        capture_fps,
        supported_capture_fps: SUPPORTED_CAPTURE_FPS,
        monitor_refresh_rate_hz,
        recommended_capture_fps,
        recent_recordings,
    })
}

#[tauri::command]
fn set_audio_input(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    source: String,
    enabled: bool,
) -> Result<(), String> {
    if state
        .recording
        .lock()
        .map_err(|_| "recording state is unavailable".to_owned())?
        .state()
        != RecordingState::Idle
    {
        return Err("audio inputs can only be changed while idle".to_owned());
    }
    if enabled {
        let endpoints =
            frameflow_windows::default_audio_endpoints().map_err(|error| error.to_string())?;
        let available = match source.as_str() {
            "microphone" => endpoints.microphone.is_some(),
            "systemAudio" => endpoints.system_output.is_some(),
            _ => return Err("unknown audio input".to_owned()),
        };
        if !available {
            return Err("the selected audio input is unavailable".to_owned());
        }
    }
    let next = {
        let mut config = state
            .audio_config
            .lock()
            .map_err(|_| "audio settings are unavailable".to_owned())?;
        match source.as_str() {
            "microphone" => config.microphone = enabled,
            "systemAudio" => config.system_audio = enabled,
            _ => return Err("unknown audio input".to_owned()),
        }
        *config
    };
    save_audio_config(&app, next)
}

#[tauri::command]
fn open_recording_location(state: State<'_, AppState>, recording_id: String) -> Result<(), String> {
    let path = {
        let index = state
            .recording_index
            .lock()
            .map_err(|_| "recording index is unavailable".to_owned())?;
        let index = index
            .as_ref()
            .ok_or_else(|| "recording index is not initialized".to_owned())?;
        index.reconcile()?;
        index.path_for_id(&recording_id)?
    };
    Command::new("explorer.exe")
        .arg(format!("/select,{}", path.display()))
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("failed to open recording location: {error}"))
}

#[tauri::command]
fn set_capture_fps(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    fps: u16,
) -> Result<u16, String> {
    if !SUPPORTED_CAPTURE_FPS.contains(&fps) {
        return Err(format!("unsupported capture frame rate: {fps} FPS"));
    }
    if state
        .recording
        .lock()
        .map_err(|_| "recording state is unavailable".to_owned())?
        .state()
        != RecordingState::Idle
    {
        return Err("frame rate can only be changed while idle".to_owned());
    }

    let mut next = state
        .output_config
        .lock()
        .map_err(|_| "output settings are unavailable".to_owned())?
        .clone();
    next.main.fps = fps;
    next = next.normalized();
    save_output_config(&app, &next)?;
    *state
        .output_config
        .lock()
        .map_err(|_| "output settings are unavailable".to_owned())? = next;
    Ok(fps)
}

#[tauri::command]
fn begin_selection(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<RecordingState, String> {
    if state
        .recording
        .lock()
        .map_err(|_| "recording state is unavailable".to_owned())?
        .state()
        != RecordingState::Idle
    {
        return Err("a recording workflow is already active".to_owned());
    }
    let main = app
        .get_webview_window("main")
        .ok_or_else(|| "main window is unavailable".to_owned())?;
    let overlay = app
        .get_webview_window("selection")
        .ok_or_else(|| "selection window is unavailable".to_owned())?;
    let controls = app
        .get_webview_window("recording-controls")
        .ok_or_else(|| "recording controls window is unavailable".to_owned())?;
    let display = active_display(&app)?;

    overlay.hide().map_err(|error| error.to_string())?;
    controls.hide().map_err(|error| error.to_string())?;
    main.hide().map_err(|error| error.to_string())?;

    let next_state = state
        .recording
        .lock()
        .map_err(|_| "recording state is unavailable".to_owned())?
        .apply(RecordingAction::BeginSelection)
        .map_err(|error| error.to_string())?;

    *state
        .selection
        .lock()
        .map_err(|_| "selection state is unavailable".to_owned())? = Some(ActiveSelection {
        display: display.clone(),
        region: None,
    });

    let callback_app = app.clone();
    let native =
        match frameflow_windows::start_native_selection(display.bounds, move |relative_rect| {
            native_selection_finished(&callback_app, relative_rect)
        }) {
            Ok(native) => native,
            Err(error) => {
                if let Ok(mut recording) = state.recording.lock() {
                    let _ = recording.apply(RecordingAction::CancelSelection);
                }
                if let Ok(mut selection) = state.selection.lock() {
                    *selection = None;
                }
                let _ = restore_main_window(&app);
                return Err(error.to_string());
            }
        };
    *state
        .native_selection
        .lock()
        .map_err(|_| "native selection state is unavailable".to_owned())? = Some(native);

    app.emit("recording://state-changed", next_state)
        .map_err(|error| error.to_string())?;

    Ok(next_state)
}

#[tauri::command]
fn get_current_selection(state: State<'_, AppState>) -> Result<Option<RegionSelection>, String> {
    Ok(state
        .selection
        .lock()
        .map_err(|_| "selection state is unavailable".to_owned())?
        .as_ref()
        .and_then(|active| active.region.clone()))
}

#[tauri::command]
fn redraw_selection(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    if state
        .recording
        .lock()
        .map_err(|_| "recording state is unavailable".to_owned())?
        .state()
        != RecordingState::Ready
    {
        return Err("selection can only be redrawn while ready".to_owned());
    }
    if let Some(native) = state
        .native_selection
        .lock()
        .map_err(|_| "native selection state is unavailable".to_owned())?
        .take()
    {
        native.stop();
    }
    let bounds = {
        let mut selection = state
            .selection
            .lock()
            .map_err(|_| "selection state is unavailable".to_owned())?;
        let active = selection
            .as_mut()
            .ok_or_else(|| "selection has not started".to_owned())?;
        active.region = None;
        active.display.bounds
    };
    if let Some(toolbar) = app.get_webview_window("selection") {
        toolbar.hide().map_err(|error| error.to_string())?;
    }
    let callback_app = app.clone();
    let native = frameflow_windows::start_native_selection(bounds, move |relative_rect| {
        native_selection_finished(&callback_app, relative_rect)
    })
    .map_err(|error| error.to_string())?;
    *state
        .native_selection
        .lock()
        .map_err(|_| "native selection state is unavailable".to_owned())? = Some(native);
    Ok(())
}

fn native_selection_finished(app: &tauri::AppHandle, relative_rect: Option<PhysicalRect>) {
    let state = app.state::<AppState>();
    let Some(relative_rect) = relative_rect else {
        if let Ok(mut recording) = state.recording.lock() {
            let _ = recording.apply(RecordingAction::CancelSelection);
        }
        if let Ok(mut selection) = state.selection.lock() {
            *selection = None;
        }
        let _ = restore_main_window(app);
        let _ = app.emit("recording://state-changed", RecordingState::Idle);
        return;
    };
    let (region, first_confirmation) = {
        let mut selection = match state.selection.lock() {
            Ok(selection) => selection,
            Err(_) => return,
        };
        let active = match selection.as_mut() {
            Some(active) => active,
            None => return,
        };
        let mut capture_rect = relative_rect;
        capture_rect.width &= !1;
        capture_rect.height &= !1;
        let region = match RegionSelection::from_display_physical(&active.display, capture_rect) {
            Ok(region) => region,
            Err(_) => return,
        };
        let first_confirmation = active.region.is_none();
        active.region = Some(region.clone());
        (region, first_confirmation)
    };

    let ready = {
        let mut recording = match state.recording.lock() {
            Ok(recording) => recording,
            Err(_) => return,
        };
        match recording.state() {
            RecordingState::Selecting => match recording.apply(RecordingAction::ConfirmSelection) {
                Ok(ready) => ready,
                Err(_) => return,
            },
            RecordingState::Ready => RecordingState::Ready,
            _ => return,
        }
    };
    if let Some(toolbar) = app.get_webview_window("selection") {
        let _ = position_capture_toolbar(&toolbar, &region);
        if first_confirmation {
            let _ = toolbar.show();
            let _ = toolbar.set_focus();
        }
    }
    let _ = app.emit("selection://confirmed", region);
    let _ = app.emit("recording://state-changed", ready);
}

#[tauri::command]
fn cancel_selection(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<RecordingState, String> {
    let next_state = state
        .recording
        .lock()
        .map_err(|_| "recording state is unavailable".to_owned())?
        .apply(RecordingAction::CancelSelection)
        .map_err(|error| error.to_string())?;
    *state
        .selection
        .lock()
        .map_err(|_| "selection state is unavailable".to_owned())? = None;
    if let Some(native) = state
        .native_selection
        .lock()
        .map_err(|_| "native selection state is unavailable".to_owned())?
        .take()
    {
        native.stop();
    }
    restore_main_window(&app)?;
    app.emit("recording://state-changed", next_state)
        .map_err(|error| error.to_string())?;
    Ok(next_state)
}

#[tauri::command]
fn start_countdown(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<RecordingState, String> {
    let next_state = state
        .recording
        .lock()
        .map_err(|_| "recording state is unavailable".to_owned())?
        .apply(RecordingAction::StartCountdown)
        .map_err(|error| error.to_string())?;
    app.emit("recording://state-changed", next_state)
        .map_err(|error| error.to_string())?;
    Ok(next_state)
}

#[tauri::command]
fn cancel_countdown(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<RecordingState, String> {
    let next_state = state
        .recording
        .lock()
        .map_err(|_| "recording state is unavailable".to_owned())?
        .apply(RecordingAction::CancelCapture)
        .map_err(|error| error.to_string())?;
    *state
        .selection
        .lock()
        .map_err(|_| "selection state is unavailable".to_owned())? = None;
    if let Some(native) = state
        .native_selection
        .lock()
        .map_err(|_| "native selection state is unavailable".to_owned())?
        .take()
    {
        native.stop();
    }
    restore_main_window(&app)?;
    app.emit("recording://state-changed", next_state)
        .map_err(|error| error.to_string())?;
    Ok(next_state)
}

#[tauri::command]
fn start_capture(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<RecordingState, String> {
    let selection = state
        .selection
        .lock()
        .map_err(|_| "selection state is unavailable".to_owned())?
        .as_ref()
        .and_then(|selection| selection.region.clone())
        .ok_or_else(|| "capture region is unavailable".to_owned())?;
    if state
        .recording
        .lock()
        .map_err(|_| "recording state is unavailable".to_owned())?
        .state()
        != RecordingState::Countdown
    {
        return Err("capture is not waiting for countdown".to_owned());
    }

    let overlay = app
        .get_webview_window("selection")
        .ok_or_else(|| "selection window is unavailable".to_owned())?;
    let controls = app
        .get_webview_window("recording-controls")
        .ok_or_else(|| "recording controls window is unavailable".to_owned())?;
    position_capture_toolbar(&controls, &selection)?;
    #[cfg(target_os = "windows")]
    {
        let hwnd = controls.hwnd().map_err(|error| error.to_string())?;
        frameflow_windows::exclude_window_from_capture(hwnd.0 as isize)
            .map_err(|error| error.to_string())?;
    }

    if let Some(native) = state
        .native_selection
        .lock()
        .map_err(|_| "native selection state is unavailable".to_owned())?
        .as_ref()
    {
        native.lock().map_err(|error| error.to_string())?;
    }

    // Commit removal of the selection toolbar before WGC observes the desktop.
    // The native border remains visible but is click-through and excluded from capture.
    overlay.hide().map_err(|error| error.to_string())?;
    frameflow_windows::flush_desktop_compositor().map_err(|error| error.to_string())?;

    let (temporary_path, output_path) = allocate_recording_paths()?;
    let capture_fps = state
        .output_config
        .lock()
        .map_err(|_| "output settings are unavailable".to_owned())?
        .main
        .fps;
    let audio_config = *state
        .audio_config
        .lock()
        .map_err(|_| "audio settings are unavailable".to_owned())?;
    let audio_enabled = audio_config.system_audio || audio_config.microphone;
    let capture_options = frameflow_windows::Mp4CaptureOptions::new(u32::from(capture_fps))
        .map_err(|error| error.to_string())?
        .with_audio(audio_enabled);
    let frame_count = Arc::new(AtomicU64::new(0));
    let last_error = Arc::new(Mutex::new(None));
    let session = frameflow_windows::start_region_mp4_capture(
        &selection,
        &temporary_path,
        capture_options,
        {
            let frame_count = Arc::clone(&frame_count);
            let last_error = Arc::clone(&last_error);
            move |result| match result {
                Ok(frame) => frame_count.store(frame.sequence, Ordering::Release),
                Err(error) => {
                    if let Ok(mut last_error) = last_error.lock() {
                        *last_error = Some(error.to_string());
                    }
                }
            }
        },
    )
    .map_err(|error| error.to_string())?;
    let audio = if audio_enabled {
        let output = session
            .audio_input()
            .ok_or_else(|| "audio encoder input is unavailable".to_owned())?;
        Some(start_active_audio(
            app.clone(),
            audio_config,
            output,
            frameflow_windows::performance_time_100ns().ok(),
        ))
    } else {
        None
    };

    let next_state = state
        .recording
        .lock()
        .map_err(|_| "recording state is unavailable".to_owned())?
        .apply(RecordingAction::CountdownFinished)
        .map_err(|error| error.to_string())?;
    *state
        .capture
        .lock()
        .map_err(|_| "capture state is unavailable".to_owned())? = Some(ActiveCapture {
        session,
        audio,
        frame_count,
        last_error,
        started_at: Instant::now(),
        paused_at: None,
        paused_total: Duration::ZERO,
        temporary_path,
        output_path,
    });
    controls.show().map_err(|error| error.to_string())?;
    app.emit("recording://state-changed", next_state)
        .map_err(|error| error.to_string())?;
    Ok(next_state)
}

fn start_active_audio(
    app: tauri::AppHandle,
    config: AudioInputConfig,
    output: frameflow_windows::WgcAudioInput,
    timeline_origin_100ns: Option<u64>,
) -> ActiveAudioCapture {
    let mixer_config = frameflow_windows::AudioMixerConfig {
        system: frameflow_windows::AudioSourceControl {
            enabled: config.system_audio,
            ..frameflow_windows::AudioSourceControl::default()
        },
        microphone: frameflow_windows::AudioSourceControl {
            enabled: config.microphone,
            ..frameflow_windows::AudioSourceControl::default()
        },
        ..frameflow_windows::AudioMixerConfig::default()
    };
    let mixer = Arc::new(Mutex::new(match timeline_origin_100ns {
        Some(origin) => frameflow_windows::AudioMixer::with_origin(mixer_config, origin),
        None => frameflow_windows::AudioMixer::new(mixer_config),
    }));
    let paused = Arc::new(AtomicBool::new(false));
    let pause_offset_100ns = Arc::new(AtomicU64::new(0));
    let warning = Arc::new(Mutex::new(None));
    let callback_context = AudioCallbackContext {
        mixer: Arc::clone(&mixer),
        output: output.clone(),
        paused: Arc::clone(&paused),
        pause_offset_100ns: Arc::clone(&pause_offset_100ns),
        warning: Arc::clone(&warning),
        app: app.clone(),
    };
    let mut sources = Vec::new();
    for (enabled, capture_source, source_id) in [
        (
            config.system_audio,
            frameflow_windows::AudioCaptureSource::SystemOutput,
            frameflow_windows::AudioSourceId::System,
        ),
        (
            config.microphone,
            frameflow_windows::AudioCaptureSource::Microphone,
            frameflow_windows::AudioSourceId::Microphone,
        ),
    ] {
        if !enabled {
            continue;
        }
        match start_audio_source(capture_source, source_id, callback_context.clone()) {
            Ok(source) => sources.push(source),
            Err(error) => {
                record_audio_error(&app, &warning, source_id, error.to_string());
                if let Ok(mut mixer) = mixer.lock() {
                    let mut control = mixer.source_control(source_id);
                    control.enabled = false;
                    mixer.set_source_control(source_id, control);
                }
            }
        }
    }
    ActiveAudioCapture {
        config,
        sources,
        mixer,
        output,
        paused,
        pause_offset_100ns,
        warning,
    }
}

fn start_audio_source(
    capture_source: frameflow_windows::AudioCaptureSource,
    source_id: frameflow_windows::AudioSourceId,
    context: AudioCallbackContext,
) -> Result<ActiveAudioSource, frameflow_windows::PlatformError> {
    let normalizer = Arc::new(Mutex::new(None::<frameflow_windows::AudioNormalizer>));
    let callback_normalizer = Arc::clone(&normalizer);
    let failed = Arc::new(AtomicBool::new(false));
    let callback_failed = Arc::clone(&failed);
    let mut callback_stopped = false;
    let session = frameflow_windows::start_wasapi_capture(capture_source, move |result| {
        if callback_stopped || context.paused.load(Ordering::Acquire) {
            return;
        }
        let result = result.and_then(|mut packet| {
            packet.qpc_position_100ns = packet
                .qpc_position_100ns
                .saturating_sub(context.pause_offset_100ns.load(Ordering::Acquire));
            let chunk = {
                let mut normalizer = callback_normalizer.lock().map_err(|_| {
                    frameflow_windows::PlatformError::AudioCapture(
                        "audio normalizer is unavailable".to_owned(),
                    )
                })?;
                if normalizer.is_none() {
                    *normalizer = Some(frameflow_windows::AudioNormalizer::new(packet.format)?);
                }
                normalizer
                    .as_mut()
                    .expect("audio normalizer was initialized")
                    .push(&packet)?
            };
            let blocks = context
                .mixer
                .lock()
                .map_err(|_| {
                    frameflow_windows::PlatformError::AudioCapture(
                        "audio mixer is unavailable".to_owned(),
                    )
                })?
                .push(source_id, chunk);
            for block in blocks {
                context.output.submit(block)?;
            }
            Ok(())
        });
        if let Err(error) = result {
            record_audio_error(&context.app, &context.warning, source_id, error.to_string());
            if let Ok(mut mixer) = context.mixer.lock() {
                let mut control = mixer.source_control(source_id);
                control.enabled = false;
                mixer.set_source_control(source_id, control);
            }
            callback_failed.store(true, Ordering::Release);
            callback_stopped = true;
        }
    })?;
    Ok(ActiveAudioSource {
        id: source_id,
        session,
        normalizer,
        failed,
    })
}

fn audio_source_name(source: frameflow_windows::AudioSourceId) -> &'static str {
    match source {
        frameflow_windows::AudioSourceId::System => "systemAudio",
        frameflow_windows::AudioSourceId::Microphone => "microphone",
    }
}

fn record_audio_error(
    app: &tauri::AppHandle,
    warning: &Arc<Mutex<Option<String>>>,
    source: frameflow_windows::AudioSourceId,
    message: String,
) {
    let message = format!("音频已降级：{message}");
    if let Ok(mut warning) = warning.lock() {
        *warning = Some(message.clone());
    }
    let _ = app.emit(
        "recording://audio-warning",
        AudioWarningEvent {
            source: audio_source_name(source),
            message,
        },
    );
}

#[tauri::command]
fn pause_capture(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<RecordingState, String> {
    let mut capture = state
        .capture
        .lock()
        .map_err(|_| "capture state is unavailable".to_owned())?;
    let active = capture
        .as_mut()
        .ok_or_else(|| "capture session is unavailable".to_owned())?;
    if let Some(audio) = active.audio.as_ref() {
        audio.paused.store(true, Ordering::Release);
    }
    if let Err(error) = active.session.pause() {
        if let Some(audio) = active.audio.as_ref() {
            audio.paused.store(false, Ordering::Release);
        }
        return Err(error.to_string());
    }
    active.paused_at = Some(Instant::now());
    let next_state = state
        .recording
        .lock()
        .map_err(|_| "recording state is unavailable".to_owned())?
        .apply(RecordingAction::Pause)
        .map_err(|error| error.to_string())?;
    app.emit("recording://state-changed", next_state)
        .map_err(|error| error.to_string())?;
    Ok(next_state)
}

#[tauri::command]
fn resume_capture(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<RecordingState, String> {
    let mut capture = state
        .capture
        .lock()
        .map_err(|_| "capture state is unavailable".to_owned())?;
    let active = capture
        .as_mut()
        .ok_or_else(|| "capture session is unavailable".to_owned())?;
    active.session.resume().map_err(|error| error.to_string())?;
    if let Some(paused_at) = active.paused_at.take() {
        let paused_for = paused_at.elapsed();
        active.paused_total += paused_for;
        if let Some(audio) = active.audio.as_ref() {
            audio
                .pause_offset_100ns
                .fetch_add(duration_to_100ns(paused_for), Ordering::AcqRel);
            audio.paused.store(false, Ordering::Release);
        }
    }
    let next_state = state
        .recording
        .lock()
        .map_err(|_| "recording state is unavailable".to_owned())?
        .apply(RecordingAction::Resume)
        .map_err(|error| error.to_string())?;
    app.emit("recording://state-changed", next_state)
        .map_err(|error| error.to_string())?;
    Ok(next_state)
}

#[tauri::command]
fn get_capture_stats(state: State<'_, AppState>) -> Result<CaptureStats, String> {
    let capture = state
        .capture
        .lock()
        .map_err(|_| "capture state is unavailable".to_owned())?;
    let active = capture
        .as_ref()
        .ok_or_else(|| "capture session is unavailable".to_owned())?;
    let clock_end = active.paused_at.unwrap_or_else(Instant::now);
    let elapsed = clock_end
        .duration_since(active.started_at)
        .saturating_sub(active.paused_total);
    let last_error = active
        .last_error
        .lock()
        .map_err(|_| "capture error state is unavailable".to_owned())?
        .clone();
    let audio = active.audio.as_ref().map(capture_audio_stats).transpose()?;
    Ok(CaptureStats {
        frame_count: active.frame_count.load(Ordering::Acquire),
        elapsed_ms: elapsed.as_millis(),
        paused: active.paused_at.is_some(),
        last_error,
        audio,
    })
}

fn capture_audio_stats(audio: &ActiveAudioCapture) -> Result<CaptureAudioStats, String> {
    let mixer = audio
        .mixer
        .lock()
        .map_err(|_| "audio mixer is unavailable".to_owned())?;
    let snapshot = |source, configured| {
        let runtime = audio.sources.iter().find(|runtime| runtime.id == source);
        LiveInputSnapshot {
            enabled: configured,
            muted: mixer.source_control(source).muted,
            disconnected: configured
                && runtime.is_none_or(|runtime| runtime.failed.load(Ordering::Acquire)),
        }
    };
    Ok(CaptureAudioStats {
        microphone: snapshot(
            frameflow_windows::AudioSourceId::Microphone,
            audio.config.microphone,
        ),
        system_audio: snapshot(
            frameflow_windows::AudioSourceId::System,
            audio.config.system_audio,
        ),
        warning: audio
            .warning
            .lock()
            .map_err(|_| "audio warning state is unavailable".to_owned())?
            .clone(),
    })
}

#[tauri::command]
fn set_capture_audio_muted(
    state: State<'_, AppState>,
    source: String,
    muted: bool,
) -> Result<(), String> {
    let capture = state
        .capture
        .lock()
        .map_err(|_| "capture state is unavailable".to_owned())?;
    let active = capture
        .as_ref()
        .ok_or_else(|| "capture session is unavailable".to_owned())?;
    let audio = active
        .audio
        .as_ref()
        .ok_or_else(|| "audio is not enabled for this recording".to_owned())?;
    let source = match source.as_str() {
        "microphone" if audio.config.microphone => frameflow_windows::AudioSourceId::Microphone,
        "systemAudio" if audio.config.system_audio => frameflow_windows::AudioSourceId::System,
        "microphone" | "systemAudio" => {
            return Err("audio input is not enabled for this recording".to_owned());
        }
        _ => return Err("unknown audio input".to_owned()),
    };
    if audio
        .sources
        .iter()
        .find(|runtime| runtime.id == source)
        .is_none_or(|runtime| runtime.failed.load(Ordering::Acquire))
    {
        return Err("audio input is disconnected".to_owned());
    }
    let mut mixer = audio
        .mixer
        .lock()
        .map_err(|_| "audio mixer is unavailable".to_owned())?;
    let mut control = mixer.source_control(source);
    control.muted = muted;
    mixer.set_source_control(source, control);
    Ok(())
}

#[tauri::command]
fn request_stop_capture(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<RecordingState, String> {
    let active = state
        .capture
        .lock()
        .map_err(|_| "capture state is unavailable".to_owned())?
        .take()
        .ok_or_else(|| "capture session is unavailable".to_owned())?;
    let finalizing = state
        .recording
        .lock()
        .map_err(|_| "recording state is unavailable".to_owned())?
        .apply(RecordingAction::Stop)
        .map_err(|error| error.to_string())?;
    app.emit("recording://state-changed", finalizing)
        .map_err(|error| error.to_string())?;
    spawn_capture_finalizer(app, active);
    Ok(finalizing)
}

fn spawn_capture_finalizer(app: tauri::AppHandle, active: ActiveCapture) {
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("frameflow-mp4-finalize".to_owned())
        .spawn(move || {
            let _ = sender.send(finalize_active_capture(active));
        })
        .expect("failed to start MP4 finalizer");

    thread::Builder::new()
        .name("frameflow-finalize-coordinator".to_owned())
        .spawn(move || {
            let result = match receiver.recv_timeout(Duration::from_secs(20)) {
                Ok(result) => result,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    Err("MP4 收尾超过 20 秒，已解除界面锁定；未完成文件保留为 .part".to_owned())
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => Err("MP4 收尾线程意外退出".to_owned()),
            };
            complete_finalization(&app, result);
        })
        .expect("failed to start finalization coordinator");
}

fn finalize_active_capture(active: ActiveCapture) -> Result<FinalizedCapture, String> {
    let ActiveCapture {
        session,
        audio,
        started_at,
        paused_at,
        paused_total,
        temporary_path,
        output_path,
        ..
    } = active;
    let clock_end = paused_at.unwrap_or_else(Instant::now);
    let duration = clock_end
        .duration_since(started_at)
        .saturating_sub(paused_total);
    if let Some(audio) = audio {
        stop_active_audio(audio);
    }
    session.stop().map_err(|error| error.to_string())?;
    fs::rename(&temporary_path, &output_path)
        .map_err(|error| format!("failed to finalize recording file: {error}"))?;
    Ok(FinalizedCapture {
        output_path,
        duration,
    })
}

fn stop_active_audio(mut audio: ActiveAudioCapture) {
    audio.paused.store(true, Ordering::Release);
    for source in audio.sources.drain(..) {
        let _ = source.session.stop();
        let tail = source.normalizer.lock().ok().and_then(|mut normalizer| {
            normalizer
                .as_mut()
                .map(frameflow_windows::AudioNormalizer::finish)
        });
        if let Some(tail) = tail.filter(|chunk| chunk.frames() > 0) {
            if let Ok(mut mixer) = audio.mixer.lock() {
                let blocks = mixer.push(source.id, tail);
                submit_final_audio_blocks(&audio.output, blocks);
            }
        }
    }
    if let Ok(mut mixer) = audio.mixer.lock() {
        let final_blocks = mixer.finish();
        submit_final_audio_blocks(&audio.output, final_blocks);
    }
}

fn submit_final_audio_blocks(
    output: &frameflow_windows::WgcAudioInput,
    blocks: Vec<frameflow_windows::MixedAudioBlock>,
) {
    for block in blocks {
        let _ = output.submit_wait(block, Duration::from_millis(500));
    }
}

fn duration_to_100ns(duration: Duration) -> u64 {
    duration
        .as_nanos()
        .saturating_div(100)
        .min(u128::from(u64::MAX)) as u64
}

fn complete_finalization(app: &tauri::AppHandle, result: Result<FinalizedCapture, String>) {
    let state = app.state::<AppState>();
    let event = match result {
        Ok(finalized) => {
            let file_name = finalized
                .output_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("FrameFlow recording.mp4")
                .to_owned();
            if let Ok(index) = state.recording_index.lock() {
                if let Some(index) = index.as_ref() {
                    if let Err(error) =
                        index.record_completed(&finalized.output_path, finalized.duration)
                    {
                        eprintln!("failed to update recording index for {file_name}: {error}");
                    }
                }
            }
            if let Ok(mut recording) = state.recording.lock() {
                if let Ok(processing) = recording.apply(RecordingAction::Finalized) {
                    let _ = app.emit("recording://state-changed", processing);
                }
                if let Ok(completed) = recording.apply(RecordingAction::ProcessingCompleted) {
                    let _ = app.emit("recording://state-changed", completed);
                }
                let _ = recording.apply(RecordingAction::Reset);
            }
            FinalizationEvent {
                success: true,
                output_path: Some(finalized.output_path.to_string_lossy().into_owned()),
                error: None,
            }
        }
        Err(error) => {
            if let Ok(mut recording) = state.recording.lock() {
                if let Ok(failed) = recording.apply(RecordingAction::Fail) {
                    let _ = app.emit("recording://state-changed", failed);
                }
                let _ = recording.apply(RecordingAction::Reset);
            }
            FinalizationEvent {
                success: false,
                output_path: None,
                error: Some(error),
            }
        }
    };
    if let Ok(mut selection) = state.selection.lock() {
        *selection = None;
    }
    if let Ok(mut native) = state.native_selection.lock() {
        if let Some(session) = native.take() {
            session.stop();
        }
    }
    let _ = restore_main_window(app);
    let _ = app.emit("recording://finalized", event);
    let _ = app.emit("recording://state-changed", RecordingState::Idle);
}

fn restore_main_window(app: &tauri::AppHandle) -> Result<(), String> {
    let overlay = app
        .get_webview_window("selection")
        .ok_or_else(|| "selection window is unavailable".to_owned())?;
    let controls = app
        .get_webview_window("recording-controls")
        .ok_or_else(|| "recording controls window is unavailable".to_owned())?;
    let main = app
        .get_webview_window("main")
        .ok_or_else(|| "main window is unavailable".to_owned())?;
    overlay.hide().map_err(|error| error.to_string())?;
    controls.hide().map_err(|error| error.to_string())?;
    main.show().map_err(|error| error.to_string())?;
    main.set_focus().map_err(|error| error.to_string())?;
    Ok(())
}

fn show_active_window(app: &tauri::AppHandle) -> Result<(), String> {
    let state = app
        .state::<AppState>()
        .recording
        .lock()
        .map_err(|_| "recording state is unavailable".to_owned())?
        .state();
    let label = match state {
        RecordingState::Recording
        | RecordingState::Paused
        | RecordingState::Finalizing
        | RecordingState::Processing => "recording-controls",
        RecordingState::Selecting | RecordingState::Ready | RecordingState::Countdown => {
            "selection"
        }
        _ => "main",
    };
    let window = app
        .get_webview_window(label)
        .ok_or_else(|| format!("{label} window is unavailable"))?;
    window.show().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())
}

fn cancel_pending_workflow_for_exit(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    if let Ok(mut native) = state.native_selection.lock() {
        if let Some(session) = native.take() {
            session.stop();
        }
    }
    if let Ok(mut selection) = state.selection.lock() {
        *selection = None;
    }
    if let Ok(mut recording) = state.recording.lock() {
        let action = match recording.state() {
            RecordingState::Selecting | RecordingState::Ready => {
                Some(RecordingAction::CancelSelection)
            }
            RecordingState::Countdown => Some(RecordingAction::CancelCapture),
            _ => None,
        };
        if let Some(action) = action {
            let _ = recording.apply(action);
        }
    };
}

fn request_safe_exit(app: &tauri::AppHandle) {
    let app_state = app.state::<AppState>();
    let decision = app_state
        .recording
        .lock()
        .map(|recording| exit_decision(recording.state()))
        .map_err(|_| ());
    match decision {
        Ok(ExitDecision::Exit) => app.exit(0),
        Ok(ExitDecision::CancelThenExit) => {
            cancel_pending_workflow_for_exit(app);
            app.exit(0);
        }
        Ok(ExitDecision::Block) => {
            let message = "录制或保存尚未完成，请先点击“停止并保存”后再退出。";
            let _ = app.emit("recording://exit-blocked", message);
            let _ = show_active_window(app);
        }
        Err(_) => {
            let _ = app.emit(
                "recording://exit-blocked",
                "录制状态暂时不可用，请稍后再试。",
            );
        }
    }
}

fn setup_tray(app: &tauri::App) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, TRAY_SHOW_ID, "显示 FrameFlow", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, TRAY_QUIT_ID, "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;
    let mut builder = TrayIconBuilder::with_id("frameflow")
        .tooltip("FrameFlow")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            TRAY_SHOW_ID => {
                let _ = show_active_window(app);
            }
            TRAY_QUIT_ID => request_safe_exit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) {
                let _ = show_active_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

fn position_capture_toolbar(
    controls: &tauri::WebviewWindow,
    selection: &RegionSelection,
) -> Result<(), String> {
    const LOGICAL_WIDTH: u32 = 680;
    const LOGICAL_HEIGHT: u32 = 76;
    const LOGICAL_GAP: i32 = 12;

    let monitor = controls
        .monitor_from_point(
            f64::from(selection.desktop_rect.x),
            f64::from(selection.desktop_rect.y),
        )
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "selection display is unavailable".to_owned())?;
    let scale = monitor.scale_factor();
    let monitor_position = *monitor.position();
    let monitor_size = *monitor.size();
    let width = (f64::from(LOGICAL_WIDTH) * scale).round() as u32;
    let height = (f64::from(LOGICAL_HEIGHT) * scale).round() as u32;
    let gap = (f64::from(LOGICAL_GAP) * scale).round() as i32;

    let monitor_right = monitor_position.x + monitor_size.width as i32;
    let monitor_bottom = monitor_position.y + monitor_size.height as i32;
    let centered_x =
        selection.desktop_rect.x + (selection.desktop_rect.width as i32 - width as i32) / 2;
    let x = centered_x.clamp(monitor_position.x, monitor_right - width as i32);
    let above = selection.desktop_rect.y - height as i32 - gap;
    let below = selection.desktop_rect.y + selection.desktop_rect.height as i32 + gap;
    let y = if above >= monitor_position.y {
        above
    } else if below + height as i32 <= monitor_bottom {
        below
    } else {
        monitor_position.y.max(selection.desktop_rect.y + gap)
    };

    let current_size = controls.outer_size().map_err(|error| error.to_string())?;
    if current_size.width != width || current_size.height != height {
        controls
            .set_size(PhysicalSize::new(width, height))
            .map_err(|error| error.to_string())?;
    }
    controls
        .set_position(PhysicalPosition::new(x, y))
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn save_directory() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .map(|profile| profile.join("Videos").join("FrameFlow"))
        .unwrap_or_else(|| PathBuf::from(r"C:\Users\Public\Videos\FrameFlow"))
}

fn recording_id() -> Result<String, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().to_string())
        .map_err(|error| format!("system clock is unavailable: {error}"))
}

fn allocate_recording_paths() -> Result<(PathBuf, PathBuf), String> {
    let directory = save_directory();
    fs::create_dir_all(&directory)
        .map_err(|error| format!("failed to create recording directory: {error}"))?;
    let id = recording_id()?;
    let base = format!("FrameFlow-{id}");
    Ok((
        directory.join(format!("{base}.mp4.part")),
        directory.join(format!("{base}.mp4")),
    ))
}

fn output_config_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|directory| directory.join("output.json"))
        .map_err(|error| error.to_string())
}

fn load_output_config(app: &tauri::AppHandle) -> OutputConfig {
    output_config_path(app)
        .and_then(|path| fs::read(path).map_err(|error| error.to_string()))
        .and_then(|bytes| {
            serde_json::from_slice::<OutputConfig>(&bytes).map_err(|error| error.to_string())
        })
        .map(normalize_loaded_output_config)
        .unwrap_or_else(|_| OutputConfig::default().normalized())
}

fn normalize_loaded_output_config(config: OutputConfig) -> OutputConfig {
    let config = config.normalized();
    if SUPPORTED_CAPTURE_FPS.contains(&config.main.fps) {
        config
    } else {
        OutputConfig::default().normalized()
    }
}

fn save_output_config(app: &tauri::AppHandle, config: &OutputConfig) -> Result<(), String> {
    let path = output_config_path(app)?;
    let parent = path
        .parent()
        .ok_or_else(|| "output settings directory is unavailable".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let json = serde_json::to_vec_pretty(config).map_err(|error| error.to_string())?;
    fs::write(path, json).map_err(|error| error.to_string())
}

fn audio_config_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|directory| directory.join("audio.json"))
        .map_err(|error| error.to_string())
}

fn recording_index_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|directory| directory.join("recordings.sqlite3"))
        .map_err(|error| error.to_string())
}

fn load_audio_config(app: &tauri::AppHandle) -> AudioInputConfig {
    audio_config_path(app)
        .and_then(|path| fs::read(path).map_err(|error| error.to_string()))
        .and_then(|bytes| {
            serde_json::from_slice::<AudioInputConfig>(&bytes).map_err(|error| error.to_string())
        })
        .unwrap_or_default()
}

fn save_audio_config(app: &tauri::AppHandle, config: AudioInputConfig) -> Result<(), String> {
    let path = audio_config_path(app)?;
    let parent = path
        .parent()
        .ok_or_else(|| "audio settings directory is unavailable".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let json = serde_json::to_vec_pretty(&config).map_err(|error| error.to_string())?;
    fs::write(path, json).map_err(|error| error.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(target_os = "windows")]
    frameflow_windows::enable_per_monitor_v2_dpi()
        .expect("failed to enable Per-Monitor V2 DPI awareness");

    tauri::Builder::default()
        .manage(AppState {
            recording: Mutex::new(RecordingStateMachine::default()),
            selection: Mutex::new(None),
            capture: Mutex::new(None),
            recording_index: Mutex::new(None),
            native_selection: Mutex::new(None),
            output_config: Mutex::new(OutputConfig::default().normalized()),
            audio_config: Mutex::new(AudioInputConfig::default()),
        })
        .setup(|app| {
            let config = load_output_config(app.handle());
            *app.state::<AppState>()
                .output_config
                .lock()
                .expect("output settings are unavailable") = config;
            let audio_config = load_audio_config(app.handle());
            *app.state::<AppState>()
                .audio_config
                .lock()
                .expect("audio settings are unavailable") = audio_config;
            let recording_index = RecordingIndex::open(
                recording_index_path(app.handle()).map_err(std::io::Error::other)?,
                save_directory(),
            )
            .map_err(std::io::Error::other)?;
            recording_index.reconcile().map_err(std::io::Error::other)?;
            *app.state::<AppState>()
                .recording_index
                .lock()
                .expect("recording index is unavailable") = Some(recording_index);
            setup_tray(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                if window.label() == "main" {
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_app_snapshot,
            open_recording_location,
            set_capture_fps,
            set_audio_input,
            begin_selection,
            get_current_selection,
            redraw_selection,
            cancel_selection,
            start_countdown,
            cancel_countdown,
            start_capture,
            pause_capture,
            resume_capture,
            get_capture_stats,
            set_capture_audio_muted,
            request_stop_capture
        ])
        .run(tauri::generate_context!())
        .expect("failed to run FrameFlow");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_supported_persisted_frame_rate() {
        let mut config = OutputConfig::default();
        config.main.fps = 60;
        assert_eq!(normalize_loaded_output_config(config).main.fps, 60);
    }

    #[test]
    fn rejects_unsupported_persisted_frame_rate() {
        let mut config = OutputConfig::default();
        config.main.fps = 25;
        assert_eq!(normalize_loaded_output_config(config).main.fps, 30);
    }

    #[test]
    fn recommends_standard_rate_without_exceeding_nominal_refresh() {
        assert_eq!(recommended_capture_fps(None), 30);
        assert_eq!(recommended_capture_fps(Some(24)), 24);
        assert_eq!(recommended_capture_fps(Some(30)), 30);
        assert_eq!(recommended_capture_fps(Some(59)), 60);
        assert_eq!(recommended_capture_fps(Some(60)), 60);
        assert_eq!(recommended_capture_fps(Some(144)), 60);
    }

    #[test]
    fn exit_is_blocked_while_capture_or_finalization_owns_resources() {
        for state in [
            RecordingState::Recording,
            RecordingState::Paused,
            RecordingState::Finalizing,
            RecordingState::Processing,
        ] {
            assert_eq!(exit_decision(state), ExitDecision::Block);
        }
    }

    #[test]
    fn exit_cancels_workflows_that_have_not_started_capture() {
        for state in [
            RecordingState::Selecting,
            RecordingState::Ready,
            RecordingState::Countdown,
        ] {
            assert_eq!(exit_decision(state), ExitDecision::CancelThenExit);
        }
    }

    #[test]
    fn idle_and_terminal_states_can_exit_immediately() {
        for state in [
            RecordingState::Idle,
            RecordingState::Completed,
            RecordingState::Failed,
        ] {
            assert_eq!(exit_decision(state), ExitDecision::Exit);
        }
    }
}
