mod capture_display;
mod recording_index;
mod recording_recovery;
mod window_layout;

use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::Write,
    path::PathBuf,
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex, OnceLock,
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use capture_display::{choose_display, display_options, ordered_displays, CaptureDisplayOption};
use frameflow_capture::{DisplayInfo, PhysicalRect, RegionSelection};
use frameflow_core::{
    OutputConfig, OutputFormat, OutputResolution, RecordingAction, RecordingState,
    RecordingStateMachine, Resolution,
};
use frameflow_media::{CoverJob, MediaJob, MediaWorkerClient, PreviewJob};
use frameflow_recorder::{
    AudioInputConfig, AudioInputSource, CaptureEngineEvent, CaptureOutputSize, CaptureStats,
    CaptureWorkerClient, FinalizedCapture,
};
use recording_index::{
    CoverJobSpec, PreviewJobSpec, RecordingDetails, RecordingIndex, RecordingSummary,
};
use recording_recovery::{
    interrupted_group_for_id, interrupted_recordings, recover_publishable_recordings,
    InterruptedRecording,
};
use serde::{Deserialize, Serialize};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, PhysicalPosition, PhysicalSize, State, WindowEvent,
};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use window_layout::{capture_toolbar_layout, CaptureToolbar, MonitorGeometry};

const TRAY_SHOW_ID: &str = "show";
const TRAY_QUIT_ID: &str = "quit";
const START_RECORDING_SHORTCUT: &str = "Alt+Shift+R";
const STORAGE_WARNING_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const STORAGE_CRITICAL_BYTES: u64 = 512 * 1024 * 1024;
const STORAGE_CHECK_INTERVAL: Duration = Duration::from_secs(2);

struct AppState {
    recording: Mutex<RecordingStateMachine>,
    selection: Mutex<Option<ActiveSelection>>,
    capture: Mutex<Option<CaptureWorkerClient>>,
    recording_index: OnceLock<Result<RecordingIndex, String>>,
    media_jobs: Mutex<HashSet<String>>,
    media_queue: OnceLock<mpsc::SyncSender<MediaJobSpec>>,
    native_selection: Mutex<Option<frameflow_windows::NativeSelectionSession>>,
    output_config: Mutex<OutputConfig>,
    audio_config: Mutex<AudioInputConfig>,
    capture_display_id: Mutex<Option<String>>,
    display_watcher: Mutex<Option<frameflow_windows::DisplayChangeSession>>,
    storage_config: Mutex<StorageConfig>,
    global_shortcut_available: AtomicBool,
    storage_watchdog: Mutex<Option<StorageWatchdog>>,
}

#[derive(Clone, Debug)]
enum MediaJobSpec {
    Preview(PreviewJobSpec),
    Cover(CoverJobSpec),
}

impl MediaJobSpec {
    fn key(&self) -> String {
        match self {
            Self::Preview(job) => format!("preview:{}", job.id),
            Self::Cover(job) => format!("cover:{}", job.id),
        }
    }
}

struct ActiveSelection {
    display: DisplayInfo,
    region: Option<RegionSelection>,
}

struct StorageWatchdog {
    cancelled: Arc<AtomicBool>,
}

impl StorageWatchdog {
    fn cancel(self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
enum StorageHealth {
    Healthy,
    Low,
    Critical,
    Unavailable,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StorageWarningEvent {
    health: StorageHealth,
    #[serde(skip_serializing_if = "Option::is_none")]
    available_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    auto_stopping: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FinalizationEvent {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    recording_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
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
    save_directory_available_bytes: Option<u64>,
    global_shortcut_available: bool,
    microphone: InputSnapshot,
    system_audio: InputSnapshot,
    output_summary: String,
    capture_fps: u16,
    output_resolution: OutputResolution,
    supported_capture_fps: [u16; 3],
    monitor_refresh_rate_hz: Option<u32>,
    recommended_capture_fps: u16,
    capture_displays: Vec<CaptureDisplayOption>,
    selected_capture_display_id: String,
    recent_recordings: Vec<RecordingSummary>,
    interrupted_recordings: Vec<InterruptedRecording>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CaptureSourceConfig {
    #[serde(default)]
    display_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StorageConfig {
    current_directory: PathBuf,
    #[serde(default)]
    library_directories: Vec<PathBuf>,
}

impl Default for StorageConfig {
    fn default() -> Self {
        let current_directory = default_save_directory();
        Self {
            library_directories: vec![current_directory.clone()],
            current_directory,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DisplayChangeEvent {
    selection_cancelled: bool,
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

fn should_begin_selection_from_shortcut(
    recording_state: RecordingState,
    shortcut_state: ShortcutState,
) -> bool {
    recording_state == RecordingState::Idle && shortcut_state == ShortcutState::Pressed
}

fn main_window_display_bounds(app: &tauri::AppHandle) -> Option<PhysicalRect> {
    let main = app.get_webview_window("main")?;
    let monitor = main
        .current_monitor()
        .ok()
        .flatten()
        .or_else(|| main.primary_monitor().ok().flatten())?;
    let position = *monitor.position();
    let size = *monitor.size();
    Some(PhysicalRect {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
    })
}

fn capture_displays(
    app: &tauri::AppHandle,
    state: &AppState,
) -> Result<(Vec<DisplayInfo>, DisplayInfo), String> {
    let displays =
        ordered_displays(frameflow_windows::list_displays().map_err(|error| error.to_string())?);
    let preferred_id = state
        .capture_display_id
        .lock()
        .map_err(|_| "capture display setting is unavailable".to_owned())?
        .clone();
    let selected = choose_display(
        &displays,
        preferred_id.as_deref(),
        main_window_display_bounds(app),
    )
    .cloned()
    .ok_or_else(|| "no capture display is available".to_owned())?;
    Ok((displays, selected))
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
    let save_directory = current_save_directory(&state)?;
    let save_directory_available_bytes = available_space(&save_directory).ok();
    let recent_recordings = recording_index(&state, &app)?.recent(usize::MAX)?;
    let interrupted_recordings = if recording_state == RecordingState::Idle {
        interrupted_recordings(&storage_directories(&state)?)?
    } else {
        Vec::new()
    };
    let output_config = state
        .output_config
        .lock()
        .map_err(|_| "output settings are unavailable".to_owned())?
        .clone();
    let capture_fps = output_config.main.fps;
    let output_resolution = output_config.main.resolution;
    let (displays, selected_display) = capture_displays(&app, &state)?;
    let monitor_refresh_rate_hz = selected_display.refresh_rate_hz;
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
        save_directory: save_directory.to_string_lossy().into_owned(),
        save_directory_available_bytes,
        global_shortcut_available: state.global_shortcut_available.load(Ordering::Relaxed),
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
        output_summary: format!(
            "MP4 · {codec_label} · {} · {capture_fps} FPS · {audio_label}",
            output_resolution_label(output_resolution)
        ),
        capture_fps,
        output_resolution,
        supported_capture_fps: SUPPORTED_CAPTURE_FPS,
        monitor_refresh_rate_hz,
        recommended_capture_fps,
        capture_displays: display_options(&displays),
        selected_capture_display_id: selected_display.id,
        recent_recordings,
        interrupted_recordings,
    })
}

#[tauri::command]
async fn choose_save_directory(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    ensure_storage_change_allowed(&state)?;
    let selected =
        tauri::async_runtime::spawn_blocking(frameflow_windows::choose_recording_directory)
            .await
            .map_err(|error| format!("folder picker failed: {error}"))??;
    let Some(selected) = selected else {
        return Ok(None);
    };
    ensure_storage_change_allowed(&state)?;
    let selected = validate_recording_directory(&selected)?;
    let mut next = state
        .storage_config
        .lock()
        .map_err(|_| "storage settings are unavailable".to_owned())?
        .clone();
    next.current_directory = selected.clone();
    if !next
        .library_directories
        .iter()
        .any(|directory| directory == &selected)
    {
        next.library_directories.push(selected.clone());
    }
    let index = recording_index(&state, &app)?;
    index.register_directory(selected.clone())?;
    index.reconcile()?;
    app.asset_protocol_scope()
        .allow_directory(&selected, true)
        .map_err(|error| format!("failed to allow media playback from this directory: {error}"))?;
    save_storage_config(&app, &next)?;
    *state
        .storage_config
        .lock()
        .map_err(|_| "storage settings are unavailable".to_owned())? = next;
    app.emit("storage://changed", ())
        .map_err(|error| error.to_string())?;
    Ok(Some(selected.to_string_lossy().into_owned()))
}

fn ensure_storage_change_allowed(state: &AppState) -> Result<(), String> {
    if state
        .recording
        .lock()
        .map_err(|_| "recording state is unavailable".to_owned())?
        .state()
        != RecordingState::Idle
    {
        return Err("save location can only be changed while idle".to_owned());
    }
    Ok(())
}

#[tauri::command]
fn set_capture_display(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    display_id: String,
) -> Result<(), String> {
    if state
        .recording
        .lock()
        .map_err(|_| "recording state is unavailable".to_owned())?
        .state()
        != RecordingState::Idle
    {
        return Err("capture display can only be changed while idle".to_owned());
    }
    let displays = frameflow_windows::list_displays().map_err(|error| error.to_string())?;
    if !displays.iter().any(|display| display.id == display_id) {
        return Err("the selected capture display is unavailable".to_owned());
    }
    let config = CaptureSourceConfig {
        display_id: Some(display_id.clone()),
    };
    save_capture_source_config(&app, &config)?;
    *state
        .capture_display_id
        .lock()
        .map_err(|_| "capture display setting is unavailable".to_owned())? = Some(display_id);
    Ok(())
}

fn handle_display_change(app: &tauri::AppHandle) {
    let available_ids = match frameflow_windows::list_displays() {
        Ok(displays) => displays
            .into_iter()
            .map(|display| display.id)
            .collect::<HashSet<_>>(),
        Err(error) => {
            eprintln!("failed to refresh display topology: {error}");
            let _ = app.emit(
                "display://changed",
                DisplayChangeEvent {
                    selection_cancelled: false,
                },
            );
            return;
        }
    };
    let state = app.state::<AppState>();
    let selected_display_id = state
        .selection
        .lock()
        .ok()
        .and_then(|selection| selection.as_ref().map(|active| active.display.id.clone()));
    let recording_state = state
        .recording
        .lock()
        .map(|recording| recording.state())
        .unwrap_or(RecordingState::Idle);
    let selection_cancelled = matches!(
        recording_state,
        RecordingState::Selecting | RecordingState::Ready | RecordingState::Countdown
    ) && selected_display_id
        .as_ref()
        .is_some_and(|display_id| !available_ids.contains(display_id));

    if selection_cancelled {
        if let Ok(mut native) = state.native_selection.lock() {
            if let Some(session) = native.take() {
                session.stop();
            }
        }
        if let Ok(mut selection) = state.selection.lock() {
            *selection = None;
        }
        let next_state = state.recording.lock().ok().and_then(|mut recording| {
            let action = match recording.state() {
                RecordingState::Selecting | RecordingState::Ready => {
                    Some(RecordingAction::CancelSelection)
                }
                RecordingState::Countdown => Some(RecordingAction::CancelCapture),
                _ => None,
            }?;
            recording.apply(action).ok()
        });
        if let Some(next_state) = next_state {
            let _ = restore_main_window(app);
            let _ = app.emit("recording://state-changed", next_state);
        }
    }

    let _ = app.emit(
        "display://changed",
        DisplayChangeEvent {
            selection_cancelled,
        },
    );
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
fn open_recording_location(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    recording_id: String,
) -> Result<(), String> {
    let path = {
        let index = recording_index(&state, &app)?;
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
fn get_recording_details(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    recording_id: String,
) -> Result<RecordingDetails, String> {
    let index = recording_index(&state, &app)?;
    index.reconcile()?;
    index.details(&recording_id)
}

#[tauri::command]
fn delete_recordings(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    recording_ids: Vec<String>,
) -> Result<(), String> {
    {
        let jobs = state
            .media_jobs
            .lock()
            .map_err(|_| "media scheduler is unavailable".to_owned())?;
        if recording_ids.iter().any(|id| {
            jobs.contains(&format!("preview:{id}")) || jobs.contains(&format!("cover:{id}"))
        }) {
            return Err(
                "wait for preview and cover generation to finish before deleting".to_owned(),
            );
        }
    }
    let index = recording_index(&state, &app)?;
    let targets = index.deletion_targets(&recording_ids)?;
    let paths = targets
        .iter()
        .map(|target| target.path.clone())
        .collect::<Vec<_>>();
    let deleted_ids = targets
        .iter()
        .map(|target| target.id.clone())
        .collect::<Vec<_>>();
    frameflow_windows::move_paths_to_recycle_bin(&paths)?;
    if let Err(error) = index.forget_recordings(&deleted_ids) {
        let _ = index.reconcile();
        return Err(format!(
            "recordings were moved to the Recycle Bin, but the library index refresh failed: {error}"
        ));
    }
    let _ = app.emit("recording://library-changed", ());
    Ok(())
}

#[tauri::command]
fn open_interrupted_recording_location(
    state: State<'_, AppState>,
    recording_id: String,
) -> Result<(), String> {
    let group = interrupted_group_for_id(&storage_directories(&state)?, &recording_id)?;
    let part = group.join("main.mp4.part");
    Command::new("explorer.exe")
        .arg(format!("/select,{}", part.display()))
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("failed to open interrupted recording location: {error}"))
}

#[tauri::command]
fn delete_interrupted_recording(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    recording_id: String,
) -> Result<(), String> {
    if state
        .recording
        .lock()
        .map_err(|_| "recording state is unavailable".to_owned())?
        .state()
        != RecordingState::Idle
    {
        return Err("interrupted recordings can only be removed while idle".to_owned());
    }
    let group = interrupted_group_for_id(&storage_directories(&state)?, &recording_id)?;
    frameflow_windows::move_paths_to_recycle_bin(&[group])?;
    app.emit("recording://library-changed", ())
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn retry_preview(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    recording_id: String,
) -> Result<(), String> {
    let job = recording_index(&state, &app)?.prepare_preview_for_id(&recording_id)?;
    launch_media_job(app, MediaJobSpec::Preview(job))
}

#[tauri::command]
fn retry_cover(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    recording_id: String,
) -> Result<(), String> {
    let job = recording_index(&state, &app)?.prepare_cover_for_id(&recording_id)?;
    launch_media_job(app, MediaJobSpec::Cover(job))
}

fn output_resolution_label(resolution: OutputResolution) -> String {
    match resolution {
        OutputResolution::Native => "原始分辨率".to_owned(),
        OutputResolution::FitWithin { width, height } => format!("最高 {width}×{height}"),
    }
}

#[tauri::command]
fn set_output_settings(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    fps: u16,
    resolution: OutputResolution,
) -> Result<(), String> {
    if !SUPPORTED_CAPTURE_FPS.contains(&fps) {
        return Err(format!("unsupported capture frame rate: {fps} FPS"));
    }
    if !resolution.is_valid() {
        return Err("output resolution must be at least 2×2".to_owned());
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
    next.main.resolution = resolution;
    next = next.normalized();
    save_output_config(&app, &next)?;
    *state
        .output_config
        .lock()
        .map_err(|_| "output settings are unavailable".to_owned())? = next;
    Ok(())
}

#[tauri::command]
fn begin_selection(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<RecordingState, String> {
    begin_selection_impl(&app, &state)
}

fn begin_selection_impl(
    app: &tauri::AppHandle,
    state: &AppState,
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
    let (_, display) = capture_displays(app, state)?;

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
                let _ = restore_main_window(app);
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
        let _ = position_capture_toolbar(&toolbar, &region, CaptureToolbar::Selection);
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
    position_capture_toolbar(&controls, &selection, CaptureToolbar::Recording)?;
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

    let save_directory = current_save_directory(&state)?;
    let (temporary_path, output_path) = allocate_recording_paths(&save_directory)?;
    let output_config = state
        .output_config
        .lock()
        .map_err(|_| "output settings are unavailable".to_owned())?
        .clone();
    let capture_fps = output_config.main.fps;
    let resolved_output = output_config.main.resolution.resolve(Resolution {
        width: selection.capture_rect.width,
        height: selection.capture_rect.height,
    });
    let output_size = CaptureOutputSize::new(resolved_output.width, resolved_output.height)?;
    let audio_config = *state
        .audio_config
        .lock()
        .map_err(|_| "audio settings are unavailable".to_owned())?;
    let event_app = app.clone();
    let capture = CaptureWorkerClient::start(
        &selection,
        output_size,
        &temporary_path,
        &output_path,
        u32::from(capture_fps),
        audio_config,
        move |event| match event {
            CaptureEngineEvent::AudioWarning { .. } => {
                let _ = event_app.emit("recording://audio-warning", event);
            }
        },
    )
    .map_err(|error| error.to_string())?;

    let next_state = state
        .recording
        .lock()
        .map_err(|_| "recording state is unavailable".to_owned())?
        .apply(RecordingAction::CountdownFinished)
        .map_err(|error| error.to_string())?;
    *state
        .capture
        .lock()
        .map_err(|_| "capture state is unavailable".to_owned())? = Some(capture);
    controls.show().map_err(|error| error.to_string())?;
    app.emit("recording://state-changed", next_state)
        .map_err(|error| error.to_string())?;
    if let Err(error) = start_storage_watchdog(app.clone(), save_directory) {
        let _ = app.emit(
            "recording://storage-warning",
            StorageWarningEvent {
                health: StorageHealth::Unavailable,
                available_bytes: None,
                message: Some("磁盘保护无法启动，FrameFlow 正在安全停止录制。".to_owned()),
                auto_stopping: true,
            },
        );
        let _ = request_stop_capture_impl(&app, &state);
        return Err(error);
    }
    Ok(next_state)
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
    active.pause()?;
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
    active.resume()?;
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
    let mut capture = state
        .capture
        .lock()
        .map_err(|_| "capture state is unavailable".to_owned())?;
    let active = capture
        .as_mut()
        .ok_or_else(|| "capture session is unavailable".to_owned())?;
    active.stats()
}

#[tauri::command]
fn get_recording_state(state: State<'_, AppState>) -> Result<RecordingState, String> {
    state
        .recording
        .lock()
        .map_err(|_| "recording state is unavailable".to_owned())
        .map(|recording| recording.state())
}

#[tauri::command]
fn set_capture_audio_muted(
    state: State<'_, AppState>,
    source: String,
    muted: bool,
) -> Result<(), String> {
    let mut capture = state
        .capture
        .lock()
        .map_err(|_| "capture state is unavailable".to_owned())?;
    let active = capture
        .as_mut()
        .ok_or_else(|| "capture session is unavailable".to_owned())?;
    let source = match source.as_str() {
        "microphone" => AudioInputSource::Microphone,
        "systemAudio" => AudioInputSource::SystemAudio,
        _ => return Err("unknown audio input".to_owned()),
    };
    active.set_audio_muted(source, muted)
}

#[tauri::command]
fn request_stop_capture(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<RecordingState, String> {
    request_stop_capture_impl(&app, &state)
}

fn request_stop_capture_impl(
    app: &tauri::AppHandle,
    state: &AppState,
) -> Result<RecordingState, String> {
    cancel_storage_watchdog(state);
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
    spawn_capture_finalizer(app.clone(), active);
    Ok(finalizing)
}

fn start_storage_watchdog(app: tauri::AppHandle, directory: PathBuf) -> Result<(), String> {
    let cancelled = Arc::new(AtomicBool::new(false));
    let state = app.state::<AppState>();
    if let Some(previous) = state
        .storage_watchdog
        .lock()
        .map_err(|_| "storage watchdog is unavailable".to_owned())?
        .replace(StorageWatchdog {
            cancelled: cancelled.clone(),
        })
    {
        previous.cancel();
    }
    thread::Builder::new()
        .name("frameflow-storage-watchdog".to_owned())
        .spawn(move || run_storage_watchdog(app, directory, cancelled))
        .map(|_| ())
        .map_err(|error| format!("failed to start storage watchdog: {error}"))
}

fn run_storage_watchdog(app: tauri::AppHandle, directory: PathBuf, cancelled: Arc<AtomicBool>) {
    let mut previous = None;
    loop {
        thread::park_timeout(STORAGE_CHECK_INTERVAL);
        if cancelled.load(Ordering::Acquire) {
            return;
        }
        let (health, available_bytes) = match available_space(&directory) {
            Ok(available) => (storage_health(available), Some(available)),
            Err(_) => (StorageHealth::Unavailable, None),
        };
        if previous != Some(health) {
            let auto_stopping =
                matches!(health, StorageHealth::Critical | StorageHealth::Unavailable);
            let message = match health {
                StorageHealth::Healthy => None,
                StorageHealth::Low => Some(format!(
                    "保存磁盘空间不足，仅剩 {}，建议尽快停止录制。",
                    format_storage_bytes(available_bytes.unwrap_or_default())
                )),
                StorageHealth::Critical => Some(format!(
                    "保存磁盘仅剩 {}，FrameFlow 正在自动停止并保存。",
                    format_storage_bytes(available_bytes.unwrap_or_default())
                )),
                StorageHealth::Unavailable => {
                    Some("保存位置已断开或不可访问，FrameFlow 正在尝试停止并保存。".to_owned())
                }
            };
            let _ = app.emit(
                "recording://storage-warning",
                StorageWarningEvent {
                    health,
                    available_bytes,
                    message,
                    auto_stopping,
                },
            );
            previous = Some(health);
        }
        if matches!(health, StorageHealth::Critical | StorageHealth::Unavailable) {
            let state = app.state::<AppState>();
            if let Err(error) = request_stop_capture_impl(&app, &state) {
                let recording_state = state
                    .recording
                    .lock()
                    .map(|recording| recording.state())
                    .unwrap_or(RecordingState::Failed);
                if !matches!(
                    recording_state,
                    RecordingState::Finalizing | RecordingState::Processing
                ) {
                    let _ = app.emit(
                        "recording://storage-warning",
                        StorageWarningEvent {
                            health,
                            available_bytes,
                            message: Some(format!("自动停止录制失败：{error}")),
                            auto_stopping: false,
                        },
                    );
                }
            }
            return;
        }
    }
}

fn cancel_storage_watchdog(state: &AppState) {
    if let Ok(mut watchdog) = state.storage_watchdog.lock() {
        if let Some(watchdog) = watchdog.take() {
            watchdog.cancel();
        }
    }
}

fn spawn_capture_finalizer(app: tauri::AppHandle, active: CaptureWorkerClient) {
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("frameflow-mp4-finalize".to_owned())
        .spawn(move || {
            let _ = sender.send(active.finalize());
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

fn complete_finalization(app: &tauri::AppHandle, result: Result<FinalizedCapture, String>) {
    let state = app.state::<AppState>();
    cancel_storage_watchdog(&state);
    let event = match result {
        Ok(finalized) => {
            let file_name = finalized
                .output_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("FrameFlow recording.mp4")
                .to_owned();
            let recording_id = match recording_index(&state, app).and_then(|index| {
                index.record_completed(
                    &finalized.output_path,
                    finalized.duration,
                    Resolution {
                        width: finalized.output_size.width,
                        height: finalized.output_size.height,
                    },
                    finalized.frames_per_second.min(u32::from(u16::MAX)) as u16,
                )
            }) {
                Ok(recording_id) => {
                    match recording_index(&state, app)
                        .and_then(|index| index.prepare_preview_for_path(&finalized.output_path))
                    {
                        Ok(job) => {
                            if let Err(error) =
                                launch_media_job(app.clone(), MediaJobSpec::Preview(job))
                            {
                                eprintln!("failed to schedule preview for {file_name}: {error}");
                            }
                        }
                        Err(error) => {
                            eprintln!("failed to prepare preview for {file_name}: {error}");
                        }
                    }
                    Some(recording_id)
                }
                Err(error) => {
                    eprintln!("failed to update recording index for {file_name}: {error}");
                    None
                }
            };
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
                recording_id,
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
                recording_id: None,
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

fn launch_media_job(app: tauri::AppHandle, job: MediaJobSpec) -> Result<(), String> {
    let key = job.key();
    let sender = app
        .state::<AppState>()
        .media_queue
        .get()
        .cloned()
        .ok_or_else(|| "media scheduler is unavailable".to_owned())?;
    {
        let state = app.state::<AppState>();
        let mut jobs = state
            .media_jobs
            .lock()
            .map_err(|_| "media scheduler is unavailable".to_owned())?;
        if !jobs.insert(key.clone()) {
            return Err("media artifact is already processing".to_owned());
        }
    }
    let _ = app.emit("recording://library-changed", ());
    if let Err(error) = sender.try_send(job.clone()) {
        let state = app.state::<AppState>();
        if let Ok(mut jobs) = state.media_jobs.lock() {
            jobs.remove(&key);
        }
        let message = format!("media queue is unavailable: {error}");
        let _ = recording_index(&state, &app).and_then(|index| match &job {
            MediaJobSpec::Preview(job) => index.finish_preview(job, Err(message.clone())),
            MediaJobSpec::Cover(job) => index.finish_cover(job, Err(message.clone())),
        });
        return Err(message);
    }
    Ok(())
}

fn run_media_scheduler(app: tauri::AppHandle, receiver: mpsc::Receiver<MediaJobSpec>) {
    while let Ok(job) = receiver.recv() {
        let key = job.key();
        let worker_job = match &job {
            MediaJobSpec::Preview(job) => MediaJob::Preview(PreviewJob {
                input_path: job.input_path.clone(),
                temporary_path: job.temporary_path.clone(),
                output_path: job.output_path.clone(),
                maximum_width: 854,
                maximum_height: 480,
                frames_per_second: 15,
                include_audio: true,
            }),
            MediaJobSpec::Cover(job) => MediaJob::Cover(CoverJob {
                input_path: job.input_path.clone(),
                temporary_path: job.temporary_path.clone(),
                output_path: job.output_path.clone(),
                maximum_width: 480,
                maximum_height: 270,
                position_ms: job.position_ms,
            }),
        };
        let result = MediaWorkerClient::run(worker_job);
        let worker_error = result.as_ref().err().cloned();
        let state = app.state::<AppState>();
        let persist_result = recording_index(&state, &app).and_then(|index| match &job {
            MediaJobSpec::Preview(job) => index.finish_preview(job, result),
            MediaJobSpec::Cover(job) => index.finish_cover(job, result),
        });
        if let Err(error) = persist_result {
            eprintln!("failed to persist media result for {key}: {error}");
        }
        if let Ok(mut jobs) = state.media_jobs.lock() {
            jobs.remove(&key);
        }
        if let Some(error) = worker_error {
            eprintln!("media generation failed for {key}: {error}");
        }
        if let MediaJobSpec::Preview(preview) = &job {
            match recording_index(&state, &app)
                .and_then(|index| index.prepare_cover_for_id(&preview.id))
            {
                Ok(cover) => {
                    if let Err(error) = launch_media_job(app.clone(), MediaJobSpec::Cover(cover)) {
                        eprintln!("failed to schedule cover for {}: {error}", preview.id);
                    }
                }
                Err(error) => eprintln!("failed to prepare cover for {}: {error}", preview.id),
            }
        }
        let _ = app.emit("recording://library-changed", ());
    }
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
    toolbar: CaptureToolbar,
) -> Result<(), String> {
    let monitor = controls
        .monitor_from_point(
            f64::from(selection.desktop_rect.x),
            f64::from(selection.desktop_rect.y),
        )
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "selection display is unavailable".to_owned())?;
    let monitor_position = *monitor.position();
    let monitor_size = *monitor.size();
    let layout = capture_toolbar_layout(
        toolbar,
        selection.desktop_rect,
        MonitorGeometry {
            x: monitor_position.x,
            y: monitor_position.y,
            width: monitor_size.width,
            height: monitor_size.height,
        },
        monitor.scale_factor(),
    );

    let current_size = controls.outer_size().map_err(|error| error.to_string())?;
    if current_size.width != layout.width || current_size.height != layout.height {
        controls
            .set_size(PhysicalSize::new(layout.width, layout.height))
            .map_err(|error| error.to_string())?;
    }
    controls
        .set_position(PhysicalPosition::new(layout.x, layout.y))
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn default_save_directory() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .map(|profile| profile.join("Videos").join("FrameFlow"))
        .unwrap_or_else(|| PathBuf::from(r"C:\Users\Public\Videos\FrameFlow"))
}

fn current_save_directory(state: &AppState) -> Result<PathBuf, String> {
    state
        .storage_config
        .lock()
        .map(|config| config.current_directory.clone())
        .map_err(|_| "storage settings are unavailable".to_owned())
}

fn storage_directories(state: &AppState) -> Result<Vec<PathBuf>, String> {
    state
        .storage_config
        .lock()
        .map(|config| config.library_directories.clone())
        .map_err(|_| "storage settings are unavailable".to_owned())
}

fn recording_id() -> Result<String, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().to_string())
        .map_err(|error| format!("system clock is unavailable: {error}"))
}

fn allocate_recording_paths(directory: &std::path::Path) -> Result<(PathBuf, PathBuf), String> {
    validate_recording_directory(directory)?;
    let id = recording_id()?;
    allocate_recording_paths_in(directory, &id)
}

fn allocate_recording_paths_in(
    directory: &std::path::Path,
    id: &str,
) -> Result<(PathBuf, PathBuf), String> {
    let group_directory = directory.join(format!("FrameFlow-{id}"));
    fs::create_dir(&group_directory)
        .map_err(|error| format!("failed to create recording group: {error}"))?;
    Ok((
        group_directory.join("main.mp4.part"),
        group_directory.join("main.mp4"),
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
    let mut config = config.normalized();
    if let Some(preview) = &mut config.preview {
        preview.format = OutputFormat::Mp4;
        preview.resolution = OutputResolution::FitWithin {
            width: 854,
            height: 480,
        };
        preview.fps = 15.min(config.main.fps);
    }
    if SUPPORTED_CAPTURE_FPS.contains(&config.main.fps) && config.main.resolution.is_valid() {
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

fn capture_source_config_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|directory| directory.join("capture-source.json"))
        .map_err(|error| error.to_string())
}

fn load_capture_source_config(app: &tauri::AppHandle) -> CaptureSourceConfig {
    capture_source_config_path(app)
        .and_then(|path| fs::read(path).map_err(|error| error.to_string()))
        .and_then(|bytes| {
            serde_json::from_slice::<CaptureSourceConfig>(&bytes).map_err(|error| error.to_string())
        })
        .unwrap_or_default()
}

fn save_capture_source_config(
    app: &tauri::AppHandle,
    config: &CaptureSourceConfig,
) -> Result<(), String> {
    let path = capture_source_config_path(app)?;
    let parent = path
        .parent()
        .ok_or_else(|| "capture source settings directory is unavailable".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let json = serde_json::to_vec_pretty(config).map_err(|error| error.to_string())?;
    fs::write(path, json).map_err(|error| error.to_string())
}

fn storage_config_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|directory| directory.join("storage.json"))
        .map_err(|error| error.to_string())
}

fn load_storage_config(app: &tauri::AppHandle) -> StorageConfig {
    let default = StorageConfig::default();
    let mut config = storage_config_path(app)
        .and_then(|path| fs::read(path).map_err(|error| error.to_string()))
        .and_then(|bytes| {
            serde_json::from_slice::<StorageConfig>(&bytes).map_err(|error| error.to_string())
        })
        .unwrap_or_else(|_| default.clone());
    if !config.current_directory.is_absolute()
        || fs::create_dir_all(&config.current_directory).is_err()
    {
        config.current_directory = default.current_directory.clone();
    }
    if !config
        .library_directories
        .iter()
        .any(|directory| directory == &config.current_directory)
    {
        config
            .library_directories
            .push(config.current_directory.clone());
    }
    if !config
        .library_directories
        .iter()
        .any(|directory| directory == &default.current_directory)
    {
        config.library_directories.push(default.current_directory);
    }
    config
        .library_directories
        .retain(|directory| directory.is_absolute());
    config.library_directories.dedup();
    config
}

fn save_storage_config(app: &tauri::AppHandle, config: &StorageConfig) -> Result<(), String> {
    let path = storage_config_path(app)?;
    let parent = path
        .parent()
        .ok_or_else(|| "storage settings directory is unavailable".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary = path.with_extension("json.part");
    let bytes = serde_json::to_vec_pretty(config).map_err(|error| error.to_string())?;
    let mut file = File::create(&temporary).map_err(|error| error.to_string())?;
    file.write_all(&bytes).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    drop(file);
    replace_file_atomically(&temporary, &path)
}

fn validate_recording_directory(directory: &std::path::Path) -> Result<PathBuf, String> {
    if !directory.is_absolute() {
        return Err("recording directory must be an absolute path".to_owned());
    }
    fs::create_dir_all(directory)
        .map_err(|error| format!("failed to create recording directory: {error}"))?;
    let directory = directory
        .canonicalize()
        .map_err(|error| format!("failed to resolve recording directory: {error}"))?;
    let probe = directory.join(format!(".frameflow-write-test-{}.tmp", recording_id()?));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&probe)
        .map_err(|error| format!("recording directory is not writable: {error}"))?;
    let probe_result = file
        .write_all(b"FrameFlow storage probe")
        .and_then(|_| file.sync_all());
    drop(file);
    let cleanup_result = fs::remove_file(&probe);
    probe_result.map_err(|error| format!("recording directory write test failed: {error}"))?;
    cleanup_result.map_err(|error| format!("failed to clean up directory write test: {error}"))?;
    let free = available_space(&directory)?;
    if free < STORAGE_CRITICAL_BYTES {
        return Err("recording directory has less than 512 MB available".to_owned());
    }
    Ok(directory)
}

fn available_space(directory: &std::path::Path) -> Result<u64, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let path = directory
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let mut available = 0_u64;
    let result = unsafe {
        GetDiskFreeSpaceExW(
            path.as_ptr(),
            &mut available,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if result == 0 {
        Err(std::io::Error::last_os_error().to_string())
    } else {
        Ok(available)
    }
}

fn storage_health(available_bytes: u64) -> StorageHealth {
    if available_bytes < STORAGE_CRITICAL_BYTES {
        StorageHealth::Critical
    } else if available_bytes < STORAGE_WARNING_BYTES {
        StorageHealth::Low
    } else {
        StorageHealth::Healthy
    }
}

fn format_storage_bytes(bytes: u64) -> String {
    const GIB: f64 = (1024_u64 * 1024 * 1024) as f64;
    format!("{:.1} GB", bytes as f64 / GIB)
}

fn replace_file_atomically(
    source: &std::path::Path,
    destination: &std::path::Path,
) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(std::io::Error::last_os_error().to_string())
    } else {
        Ok(())
    }
}

fn recording_index_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|directory| directory.join("recordings.sqlite3"))
        .map_err(|error| error.to_string())
}

fn recording_index<'a>(
    state: &'a AppState,
    app: &tauri::AppHandle,
) -> Result<&'a RecordingIndex, String> {
    state
        .recording_index
        .get_or_init(|| {
            let directories = state
                .storage_config
                .lock()
                .map_err(|_| "storage settings are unavailable".to_owned())?
                .library_directories
                .clone();
            let index = RecordingIndex::open(recording_index_path(app)?, directories)?;
            index.reconcile()?;
            Ok(index)
        })
        .as_ref()
        .map_err(Clone::clone)
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
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    let state = app.state::<AppState>();
                    let recording_state = state
                        .recording
                        .lock()
                        .map(|recording| recording.state())
                        .unwrap_or(RecordingState::Failed);
                    if !should_begin_selection_from_shortcut(recording_state, event.state) {
                        return;
                    }
                    if let Err(error) = begin_selection_impl(app, &state) {
                        let _ = restore_main_window(app);
                        let _ = app.emit("recording://shortcut-error", error);
                    }
                })
                .build(),
        )
        .manage(AppState {
            recording: Mutex::new(RecordingStateMachine::default()),
            selection: Mutex::new(None),
            capture: Mutex::new(None),
            recording_index: OnceLock::new(),
            media_jobs: Mutex::new(HashSet::new()),
            media_queue: OnceLock::new(),
            native_selection: Mutex::new(None),
            output_config: Mutex::new(OutputConfig::default().normalized()),
            audio_config: Mutex::new(AudioInputConfig::default()),
            capture_display_id: Mutex::new(None),
            display_watcher: Mutex::new(None),
            storage_config: Mutex::new(StorageConfig::default()),
            global_shortcut_available: AtomicBool::new(false),
            storage_watchdog: Mutex::new(None),
        })
        .setup(|app| {
            match app.global_shortcut().register(START_RECORDING_SHORTCUT) {
                Ok(()) => app
                    .state::<AppState>()
                    .global_shortcut_available
                    .store(true, Ordering::Relaxed),
                Err(error) => {
                    eprintln!("global shortcut {START_RECORDING_SHORTCUT} is unavailable: {error}")
                }
            }
            let (media_sender, media_receiver) = mpsc::sync_channel(32);
            app.state::<AppState>()
                .media_queue
                .set(media_sender)
                .map_err(|_| std::io::Error::other("media scheduler was already initialized"))?;
            let media_app = app.handle().clone();
            thread::Builder::new()
                .name("frameflow-media-scheduler".to_owned())
                .spawn(move || run_media_scheduler(media_app, media_receiver))
                .map_err(std::io::Error::other)?;
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
            let capture_source_config = load_capture_source_config(app.handle());
            *app.state::<AppState>()
                .capture_display_id
                .lock()
                .expect("capture display setting is unavailable") =
                capture_source_config.display_id;
            let storage_config = load_storage_config(app.handle());
            for directory in &storage_config.library_directories {
                if directory.is_dir() {
                    app.asset_protocol_scope()
                        .allow_directory(directory, true)?;
                }
            }
            *app.state::<AppState>()
                .storage_config
                .lock()
                .expect("storage settings are unavailable") = storage_config;
            let recovery_roots =
                storage_directories(&app.state::<AppState>()).map_err(std::io::Error::other)?;
            let recovered =
                recover_publishable_recordings(&recovery_roots).map_err(std::io::Error::other)?;
            if !recovered.is_empty() {
                eprintln!(
                    "published {} recording(s) finalized before the previous shutdown",
                    recovered.len()
                );
            }
            let recoverable_previews = recording_index(&app.state::<AppState>(), app.handle())
                .and_then(RecordingIndex::recoverable_preview_paths)
                .map_err(std::io::Error::other)?;
            setup_tray(app)?;
            let display_app = app.handle().clone();
            let display_watcher = frameflow_windows::watch_display_changes(move || {
                handle_display_change(&display_app)
            })
            .map_err(std::io::Error::other)?;
            *app.state::<AppState>()
                .display_watcher
                .lock()
                .expect("display watcher is unavailable") = Some(display_watcher);
            for main_path in recoverable_previews {
                match recording_index(&app.state::<AppState>(), app.handle())
                    .and_then(|index| index.prepare_preview_for_path(&main_path))
                {
                    Ok(job) => {
                        if let Err(error) =
                            launch_media_job(app.handle().clone(), MediaJobSpec::Preview(job))
                        {
                            eprintln!("failed to resume preview task: {error}");
                        }
                    }
                    Err(error) => eprintln!("failed to recover preview task: {error}"),
                }
            }
            let recoverable_covers = recording_index(&app.state::<AppState>(), app.handle())
                .and_then(RecordingIndex::recoverable_cover_ids)
                .map_err(std::io::Error::other)?;
            for recording_id in recoverable_covers {
                match recording_index(&app.state::<AppState>(), app.handle())
                    .and_then(|index| index.prepare_cover_for_id(&recording_id))
                {
                    Ok(job) => {
                        if let Err(error) =
                            launch_media_job(app.handle().clone(), MediaJobSpec::Cover(job))
                        {
                            eprintln!("failed to resume cover task: {error}");
                        }
                    }
                    Err(error) => eprintln!("failed to recover cover task: {error}"),
                }
            }
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
            choose_save_directory,
            set_capture_display,
            open_recording_location,
            get_recording_details,
            delete_recordings,
            open_interrupted_recording_location,
            delete_interrupted_recording,
            retry_preview,
            retry_cover,
            set_output_settings,
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
            get_recording_state,
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
    fn rejects_invalid_persisted_output_resolution() {
        let mut config = OutputConfig::default();
        config.main.resolution = OutputResolution::FitWithin {
            width: 0,
            height: 1080,
        };
        assert_eq!(
            normalize_loaded_output_config(config).main.resolution,
            OutputResolution::Native
        );
    }

    #[test]
    fn migrates_unimplemented_preview_formats_to_the_first_mp4_profile() {
        let mut config = OutputConfig::default();
        let preview = config.preview.as_mut().unwrap();
        preview.format = OutputFormat::Webm;
        preview.fps = 60;
        preview.resolution = OutputResolution::Native;

        let normalized = normalize_loaded_output_config(config);
        let preview = normalized.preview.unwrap();
        assert_eq!(preview.format, OutputFormat::Mp4);
        assert_eq!(preview.fps, 15);
        assert_eq!(
            preview.resolution,
            OutputResolution::FitWithin {
                width: 854,
                height: 480,
            }
        );
    }

    #[test]
    fn allocates_each_new_recording_inside_its_own_group_directory() {
        let directory = std::env::temp_dir().join(format!(
            "frameflow-recording-layout-{}",
            recording_id().unwrap()
        ));
        fs::create_dir_all(&directory).unwrap();

        let (temporary, output) = allocate_recording_paths_in(&directory, "1700000000009").unwrap();
        let group = directory.join("FrameFlow-1700000000009");
        assert_eq!(temporary, group.join("main.mp4.part"));
        assert_eq!(output, group.join("main.mp4"));
        assert!(group.is_dir());

        fs::remove_dir_all(directory).unwrap();
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
    fn global_shortcut_only_starts_an_idle_workflow_on_key_press() {
        assert!(should_begin_selection_from_shortcut(
            RecordingState::Idle,
            ShortcutState::Pressed
        ));
        assert!(!should_begin_selection_from_shortcut(
            RecordingState::Idle,
            ShortcutState::Released
        ));
        assert!(!should_begin_selection_from_shortcut(
            RecordingState::Recording,
            ShortcutState::Pressed
        ));
        assert!(!should_begin_selection_from_shortcut(
            RecordingState::Finalizing,
            ShortcutState::Pressed
        ));
    }

    #[test]
    fn storage_health_preserves_a_finalize_reserve() {
        assert_eq!(
            storage_health(STORAGE_WARNING_BYTES),
            StorageHealth::Healthy
        );
        assert_eq!(
            storage_health(STORAGE_WARNING_BYTES - 1),
            StorageHealth::Low
        );
        assert_eq!(storage_health(STORAGE_CRITICAL_BYTES), StorageHealth::Low);
        assert_eq!(
            storage_health(STORAGE_CRITICAL_BYTES - 1),
            StorageHealth::Critical
        );
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
