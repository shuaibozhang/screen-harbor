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

use capture_display::{choose_reference_display, ordered_displays};
use frameflow_capture::{DisplayInfo, PhysicalRect, RegionSelection};
use frameflow_core::{
    OutputConfig, OutputFormat, OutputResolution, RecordingAction, RecordingState,
    RecordingStateMachine, Resolution, VideoQuality,
};
use frameflow_media::{
    CoverJob, CoverOutput, MainWebmJob, MediaJob, MediaWorkerClient, PreviewFormat, PreviewJob,
};
use frameflow_recorder::{
    AudioInputConfig, AudioInputSource, CaptureEngineEvent, CaptureOutputSize, CaptureStats,
    CaptureWorkerClient, FinalizedCapture,
};
use recording_index::{
    CoverJobSpec, PreviewJobSpec, PreviewProfile, RecordingDetails, RecordingIndex,
    RecordingSummary,
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
    native_selection: Mutex<Option<frameflow_windows::NativeDisplaySelectionSession>>,
    output_config: Mutex<OutputConfig>,
    audio_config: Mutex<AudioInputConfig>,
    recording_preferences: Mutex<RecordingPreferences>,
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
struct AudioDeviceSnapshot {
    id: String,
    name: String,
    is_default: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InputSnapshot {
    enabled: bool,
    available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    device_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    selected_device_id: Option<String>,
    devices: Vec<AudioDeviceSnapshot>,
}

fn audio_input_snapshot(
    enabled: bool,
    selected_device_id: Option<&str>,
    endpoints: &[frameflow_windows::AudioEndpointInfo],
    default_device_id: Option<&str>,
) -> InputSnapshot {
    let active_id = selected_device_id.or(default_device_id);
    let active = active_id.and_then(|id| endpoints.iter().find(|endpoint| endpoint.id == id));
    InputSnapshot {
        enabled,
        available: active.is_some(),
        device_name: active.map(|endpoint| endpoint.name.clone()),
        selected_device_id: selected_device_id.map(ToOwned::to_owned),
        devices: endpoints
            .iter()
            .map(|endpoint| AudioDeviceSnapshot {
                id: endpoint.id.clone(),
                name: endpoint.name.clone(),
                is_default: default_device_id == Some(endpoint.id.as_str()),
            })
            .collect(),
    }
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
    main_format: OutputFormat,
    main_quality: VideoQuality,
    preview_enabled: bool,
    preview_format: OutputFormat,
    preview_resolution: OutputResolution,
    preview_fps: u16,
    supported_capture_fps: [u16; 3],
    supported_preview_fps: [u16; 4],
    monitor_refresh_rate_hz: Option<u32>,
    recommended_capture_fps: u16,
    countdown_enabled: bool,
    countdown_seconds: u8,
    recent_recordings: Vec<RecordingSummary>,
    interrupted_recordings: Vec<InterruptedRecording>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
struct RecordingPreferences {
    countdown_enabled: bool,
    countdown_seconds: u8,
}

impl Default for RecordingPreferences {
    fn default() -> Self {
        Self {
            countdown_enabled: false,
            countdown_seconds: 3,
        }
    }
}

impl RecordingPreferences {
    fn normalized(self) -> Self {
        Self {
            countdown_enabled: self.countdown_enabled,
            countdown_seconds: self.countdown_seconds.clamp(1, 10),
        }
    }
}

#[tauri::command]
fn get_recording_preferences(state: State<'_, AppState>) -> Result<RecordingPreferences, String> {
    state
        .recording_preferences
        .lock()
        .map(|preferences| *preferences)
        .map_err(|_| "recording preferences are unavailable".to_owned())
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
        let legacy_directory = legacy_default_save_directory();
        Self {
            library_directories: vec![current_directory.clone(), legacy_directory],
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
const SUPPORTED_PREVIEW_FPS: [u16; 4] = [10, 15, 24, 30];

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

fn capture_displays() -> Result<Vec<DisplayInfo>, String> {
    let displays =
        ordered_displays(frameflow_windows::list_displays().map_err(|error| error.to_string())?);
    if displays.is_empty() {
        return Err("no capture display is available".to_owned());
    }
    Ok(displays)
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
    let index = recording_index(&state, &app)?;
    // The user may remove a recording group outside ScreenHarbor. Reconcile on
    // every library snapshot so stale database rows never outlive the files
    // they describe. Missing library roots are still preserved by reconcile(),
    // which avoids treating a disconnected drive as a mass deletion.
    index.reconcile()?;
    let recent_recordings = index.recent(usize::MAX)?;
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
    let preview_enabled = output_config.preview.is_some();
    let preview = output_config.preview.clone().unwrap_or_else(|| {
        OutputConfig::default()
            .preview
            .expect("default preview profile")
    });
    let displays = capture_displays()?;
    let reference_display = choose_reference_display(&displays, main_window_display_bounds(&app))
        .ok_or_else(|| "no capture display is available".to_owned())?;
    let monitor_refresh_rate_hz = reference_display.refresh_rate_hz;
    let recommended_capture_fps = recommended_capture_fps(monitor_refresh_rate_hz);
    let recording_preferences = *state
        .recording_preferences
        .lock()
        .map_err(|_| "recording preferences are unavailable".to_owned())?;
    let audio_endpoints = frameflow_windows::audio_endpoints().unwrap_or_default();
    let audio_config = state
        .audio_config
        .lock()
        .map_err(|_| "audio settings are unavailable".to_owned())?
        .clone();
    let audio_label = match (audio_config.system_audio, audio_config.microphone) {
        (true, true) => "系统声 + 麦克风",
        (true, false) => "系统声",
        (false, true) => "麦克风",
        (false, false) => "无音频",
    };
    let main_format = output_config.main.format;
    let main_quality = output_config.main.quality;
    let main_format_label = output_format_label(main_format);
    let quality_label = match main_quality {
        VideoQuality::Low => "低画质",
        VideoQuality::Balanced => "平衡",
        VideoQuality::High => "高画质",
    };
    let codec_label = match (
        main_format,
        audio_config.system_audio || audio_config.microphone,
    ) {
        (OutputFormat::Webm, true) => "VP9/Opus",
        (OutputFormat::Webm, false) => "VP9",
        (_, true) => "H.264/AAC",
        (_, false) => "H.264",
    };

    Ok(AppSnapshot {
        recording_state,
        save_directory: display_path(&save_directory),
        save_directory_available_bytes,
        global_shortcut_available: state.global_shortcut_available.load(Ordering::Relaxed),
        microphone: audio_input_snapshot(
            audio_config.microphone,
            audio_config.microphone_device_id.as_deref(),
            &audio_endpoints.microphones,
            audio_endpoints.default_microphone_id.as_deref(),
        ),
        system_audio: audio_input_snapshot(
            audio_config.system_audio,
            audio_config.system_audio_device_id.as_deref(),
            &audio_endpoints.system_outputs,
            audio_endpoints.default_system_output_id.as_deref(),
        ),
        output_summary: if preview_enabled {
            format!(
                "母版 {main_format_label} · {codec_label} · {quality_label} · {} · {capture_fps} FPS · {audio_label} / 预览 {} · {} · {} FPS",
                output_resolution_label(output_resolution),
                output_format_label(preview.format),
                output_resolution_label(preview.resolution),
                preview.fps,
            )
        } else {
            format!(
                "母版 {main_format_label} · {codec_label} · {quality_label} · {} · {capture_fps} FPS · {audio_label} / 不生成轻量预览和封面",
                output_resolution_label(output_resolution),
            )
        },
        capture_fps,
        output_resolution,
        main_format,
        main_quality,
        preview_enabled,
        preview_format: preview.format,
        preview_resolution: preview.resolution,
        preview_fps: preview.fps,
        supported_capture_fps: SUPPORTED_CAPTURE_FPS,
        supported_preview_fps: SUPPORTED_PREVIEW_FPS,
        monitor_refresh_rate_hz,
        recommended_capture_fps,
        countdown_enabled: recording_preferences.countdown_enabled,
        countdown_seconds: recording_preferences.countdown_seconds,
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
    Ok(Some(display_path(&selected)))
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
    let selection_cancelled = recording_state == RecordingState::Selecting
        || matches!(
            recording_state,
            RecordingState::Ready | RecordingState::Countdown
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
    device_id: Option<String>,
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
    let endpoints = frameflow_windows::audio_endpoints().map_err(|error| error.to_string())?;
    let (devices, default_id) = match source.as_str() {
        "microphone" => (
            &endpoints.microphones,
            endpoints.default_microphone_id.as_deref(),
        ),
        "systemAudio" => (
            &endpoints.system_outputs,
            endpoints.default_system_output_id.as_deref(),
        ),
        _ => return Err("unknown audio input".to_owned()),
    };
    if let Some(device_id) = device_id.as_deref() {
        if enabled && !devices.iter().any(|device| device.id == device_id) {
            return Err("the selected audio device is unavailable".to_owned());
        }
    } else if enabled && default_id.is_none() {
        return Err("the default audio device is unavailable".to_owned());
    }
    if enabled {
        let available = match source.as_str() {
            "microphone" | "systemAudio" => {
                device_id.as_deref().map_or(default_id.is_some(), |id| {
                    devices.iter().any(|device| device.id == id)
                })
            }
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
            "microphone" => {
                config.microphone = enabled;
                config.microphone_device_id = device_id;
            }
            "systemAudio" => {
                config.system_audio = enabled;
                config.system_audio_device_id = device_id;
            }
            _ => return Err("unknown audio input".to_owned()),
        }
        config.clone()
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

fn output_format_label(format: OutputFormat) -> &'static str {
    match format {
        OutputFormat::Mp4 => "MP4",
        OutputFormat::Webm => "WebM",
        OutputFormat::Mov => "MOV",
        OutputFormat::AnimatedWebp => "WebP",
        OutputFormat::AnimatedAvif => "AVIF",
    }
}

fn preview_profile(config: &OutputConfig) -> Option<PreviewProfile> {
    let preview = config.preview.as_ref()?;
    let (maximum_width, maximum_height) = match preview.resolution {
        OutputResolution::FitWithin { width, height } => (width, height),
        OutputResolution::Native => (854, 480),
    };
    Some(PreviewProfile {
        format: match preview.format {
            OutputFormat::Webm => PreviewFormat::Webm,
            OutputFormat::AnimatedWebp => PreviewFormat::AnimatedWebp,
            _ => PreviewFormat::Mp4,
        },
        maximum_width,
        maximum_height,
        frames_per_second: preview.fps,
        include_audio: preview.include_audio,
    })
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
fn set_output_settings(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    fps: u16,
    resolution: OutputResolution,
    main_format: OutputFormat,
    main_quality: VideoQuality,
    preview_format: OutputFormat,
    preview_resolution: OutputResolution,
    preview_fps: u16,
) -> Result<(), String> {
    if !SUPPORTED_CAPTURE_FPS.contains(&fps) {
        return Err(format!("unsupported capture frame rate: {fps} FPS"));
    }
    if !resolution.is_valid() {
        return Err("output resolution must be at least 2×2".to_owned());
    }
    if !matches!(
        preview_format,
        OutputFormat::Mp4 | OutputFormat::Webm | OutputFormat::AnimatedWebp
    ) {
        return Err("preview format must be MP4, WebM, or animated WebP".to_owned());
    }
    if !SUPPORTED_PREVIEW_FPS.contains(&preview_fps) {
        return Err(format!("unsupported preview frame rate: {preview_fps} FPS"));
    }
    if preview_fps > fps {
        return Err("preview frame rate cannot exceed the master frame rate".to_owned());
    }
    let OutputResolution::FitWithin {
        width: preview_width,
        height: preview_height,
    } = preview_resolution
    else {
        return Err("preview resolution must use a maximum width and height".to_owned());
    };
    if preview_width < 2 || preview_height < 2 || preview_width > 3840 || preview_height > 2160 {
        return Err("preview resolution must be between 2×2 and 3840×2160".to_owned());
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
    if !matches!(main_format, OutputFormat::Mp4 | OutputFormat::Webm) {
        return Err("main format must be MP4 or WebM".to_owned());
    }
    next.main.format = main_format;
    next.main.quality = main_quality;
    next.main.fps = fps;
    next.main.resolution = resolution;
    if let Some(preview) = &mut next.preview {
        preview.format = preview_format;
        preview.resolution = preview_resolution;
        preview.fps = preview_fps;
        preview.include_audio = preview_format != OutputFormat::AnimatedWebp;
    }
    next = next.normalized();
    save_output_config(&app, &next)?;
    *state
        .output_config
        .lock()
        .map_err(|_| "output settings are unavailable".to_owned())? = next;
    Ok(())
}

#[tauri::command]
fn set_preview_enabled(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<(), String> {
    if state
        .recording
        .lock()
        .map_err(|_| "recording state is unavailable".to_owned())?
        .state()
        != RecordingState::Idle
    {
        return Err("preview generation can only be changed while idle".to_owned());
    }

    let mut next = state
        .output_config
        .lock()
        .map_err(|_| "output settings are unavailable".to_owned())?
        .clone();
    if enabled {
        if next.preview.is_none() {
            next.preview = OutputConfig::default().preview;
        }
        next.cover_enabled = true;
    } else {
        next.preview = None;
        next.cover_enabled = false;
    }
    next = next.normalized();
    save_output_config(&app, &next)?;
    *state
        .output_config
        .lock()
        .map_err(|_| "output settings are unavailable".to_owned())? = next;
    Ok(())
}

#[tauri::command]
fn set_recording_preferences(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    countdown_enabled: bool,
    countdown_seconds: u8,
) -> Result<(), String> {
    if !(1..=10).contains(&countdown_seconds) {
        return Err("countdown duration must be between 1 and 10 seconds".to_owned());
    }
    if state
        .recording
        .lock()
        .map_err(|_| "recording state is unavailable".to_owned())?
        .state()
        != RecordingState::Idle
    {
        return Err("recording preferences can only be changed while idle".to_owned());
    }

    let next = RecordingPreferences {
        countdown_enabled,
        countdown_seconds,
    };
    save_recording_preferences(&app, next)?;
    *state
        .recording_preferences
        .lock()
        .map_err(|_| "recording preferences are unavailable".to_owned())? = next;
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
    let displays = capture_displays()?;

    if let Some(native) = state
        .native_selection
        .lock()
        .map_err(|_| "native selection state is unavailable".to_owned())?
        .take()
    {
        native.stop();
    }

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
        .map_err(|_| "selection state is unavailable".to_owned())? = None;

    let callback_app = app.clone();
    let native = match frameflow_windows::start_native_display_selection(
        &displays,
        move |display, relative_rect| {
            let event_app = callback_app.clone();
            let dispatch_app = event_app.clone();
            let _ = dispatch_app.run_on_main_thread(move || {
                native_selection_finished(&event_app, display, relative_rect);
            });
        },
    ) {
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
    let _ = app.emit("selection://started", ());

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
    let display = {
        let mut selection = state
            .selection
            .lock()
            .map_err(|_| "selection state is unavailable".to_owned())?;
        let active = selection
            .as_mut()
            .ok_or_else(|| "selection has not started".to_owned())?;
        active.region = None;
        active.display.clone()
    };
    if let Some(toolbar) = app.get_webview_window("selection") {
        toolbar.hide().map_err(|error| error.to_string())?;
    }
    let callback_app = app.clone();
    let native = frameflow_windows::start_native_display_selection(
        std::slice::from_ref(&display),
        move |display, relative_rect| {
            let event_app = callback_app.clone();
            let dispatch_app = event_app.clone();
            let _ = dispatch_app.run_on_main_thread(move || {
                native_selection_finished(&event_app, display, relative_rect);
            });
        },
    )
    .map_err(|error| error.to_string())?;
    *state
        .native_selection
        .lock()
        .map_err(|_| "native selection state is unavailable".to_owned())? = Some(native);
    let _ = app.emit("selection://started", ());
    Ok(())
}

fn native_selection_finished(
    app: &tauri::AppHandle,
    display: DisplayInfo,
    relative_rect: Option<PhysicalRect>,
) {
    let state = app.state::<AppState>();
    let event_is_current = state
        .native_selection
        .lock()
        .ok()
        .and_then(|mut native| {
            native
                .as_mut()
                .map(|session| session.select_display(&display.id))
        })
        .unwrap_or(false);
    if !event_is_current {
        return;
    }

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
        let active = selection.get_or_insert_with(|| ActiveSelection {
            display: display.clone(),
            region: None,
        });
        if active.display.id != display.id {
            return;
        }
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
    let audio_config = state
        .audio_config
        .lock()
        .map_err(|_| "audio settings are unavailable".to_owned())?
        .clone();
    let event_app = app.clone();
    let capture = CaptureWorkerClient::start(
        &selection,
        output_size,
        &temporary_path,
        &output_path,
        u32::from(capture_fps),
        // Keep the intermediate high quality to avoid compounding compression for WebM.
        if output_config.main.format == OutputFormat::Webm {
            VideoQuality::High
        } else {
            output_config.main.quality
        }
        .target_bitrate(
            output_size.width,
            output_size.height,
            u32::from(capture_fps),
        ),
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
                message: Some("磁盘保护无法启动，ScreenHarbor 正在安全停止录制。".to_owned()),
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
                    "保存磁盘仅剩 {}，ScreenHarbor 正在自动停止并保存。",
                    format_storage_bytes(available_bytes.unwrap_or_default())
                )),
                StorageHealth::Unavailable => {
                    Some("保存位置已断开或不可访问，ScreenHarbor 正在尝试停止并保存。".to_owned())
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
    let main_settings = app
        .state::<AppState>()
        .output_config
        .lock()
        .map(|config| config.main.clone())
        .map_err(|_| "output settings are unavailable".to_owned());
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
            let result = result.and_then(|mut finalized| {
                let settings = main_settings?;
                if settings.format == OutputFormat::Webm {
                    // This conversion runs after capture has stopped, outside the MP4 timeout.
                    let source = finalized.output_path.clone();
                    MediaWorkerClient::run(MediaJob::MainWebm(MainWebmJob {
                        input_path: source.clone(),
                        width: finalized.output_size.width,
                        height: finalized.output_size.height,
                        frames_per_second: finalized.frames_per_second as u16,
                        video_bitrate: settings.quality.target_bitrate(
                            finalized.output_size.width,
                            finalized.output_size.height,
                            finalized.frames_per_second,
                        ),
                    }))
                    .map_err(|error| {
                        format!(
                            "WebM 转换失败，原始 MP4 已保留在 {}：{error}",
                            source.display()
                        )
                    })?;
                    finalized.output_path = source.with_file_name("main.webm");
                    // Only remove our intermediate after the WebM file is published.
                    if let Err(error) = fs::remove_file(&source) {
                        eprintln!(
                            "failed to remove WebM intermediate {}: {error}",
                            source.display()
                        );
                    }
                }
                Ok(finalized)
            });
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
                .unwrap_or("ScreenHarbor recording.mp4")
                .to_owned();
            let (frozen_preview_profile, cover_enabled) = state
                .output_config
                .lock()
                .map(|config| (preview_profile(&config), config.cover_enabled))
                .unwrap_or((None, false));
            let should_generate_preview = frozen_preview_profile.is_some();
            let recording_id = match recording_index(&state, app).and_then(|index| {
                index.record_completed(
                    &finalized.output_path,
                    finalized.duration,
                    Resolution {
                        width: finalized.output_size.width,
                        height: finalized.output_size.height,
                    },
                    finalized.frames_per_second.min(u32::from(u16::MAX)) as u16,
                    frozen_preview_profile,
                    cover_enabled,
                )
            }) {
                Ok(recording_id) => {
                    if should_generate_preview {
                        match recording_index(&state, app).and_then(|index| {
                            index.prepare_preview_for_path(&finalized.output_path)
                        }) {
                            Ok(job) => {
                                if let Err(error) =
                                    launch_media_job(app.clone(), MediaJobSpec::Preview(job))
                                {
                                    eprintln!(
                                        "failed to schedule preview for {file_name}: {error}"
                                    );
                                }
                            }
                            Err(error) => {
                                eprintln!("failed to prepare preview for {file_name}: {error}");
                            }
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
                format: job.profile.format,
                maximum_width: job.profile.maximum_width,
                maximum_height: job.profile.maximum_height,
                frames_per_second: job.profile.frames_per_second,
                include_audio: job.profile.include_audio,
            }),
            MediaJobSpec::Cover(job) => MediaJob::Cover(CoverJob {
                input_path: job.input_path.clone(),
                preview: CoverOutput {
                    temporary_path: job.preview_temporary_path.clone(),
                    output_path: job.preview_output_path.clone(),
                    maximum_width: 320,
                    maximum_height: 180,
                },
                high_resolution: CoverOutput {
                    temporary_path: job.high_temporary_path.clone(),
                    output_path: job.high_output_path.clone(),
                    maximum_width: 1920,
                    maximum_height: 1080,
                },
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
    let show = MenuItem::with_id(app, TRAY_SHOW_ID, "显示 ScreenHarbor", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, TRAY_QUIT_ID, "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;
    let mut builder = TrayIconBuilder::with_id("screenharbor")
        .tooltip("ScreenHarbor")
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
        .map(|profile| profile.join("Videos").join("ScreenHarbor"))
        .unwrap_or_else(|| PathBuf::from(r"C:\Users\Public\Videos\ScreenHarbor"))
}

fn legacy_default_save_directory() -> PathBuf {
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

fn display_path(path: &std::path::Path) -> String {
    let path = path.to_string_lossy();
    if let Some(path) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{path}")
    } else if let Some(path) = path.strip_prefix(r"\\?\") {
        path.to_owned()
    } else {
        path.into_owned()
    }
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
    let group_directory = directory.join(format!("ScreenHarbor-{id}"));
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
    let mut config = config;
    if !matches!(config.main.format, OutputFormat::Mp4 | OutputFormat::Webm) {
        config.main.format = OutputFormat::Mp4;
    }
    if config.preview.is_none() && config.cover_enabled {
        config.preview = OutputConfig::default().preview;
    }
    config.cover_enabled = config.preview.is_some();
    let mut config = config.normalized();
    if let Some(preview) = &mut config.preview {
        if !matches!(
            preview.format,
            OutputFormat::Mp4 | OutputFormat::Webm | OutputFormat::AnimatedWebp
        ) {
            preview.format = OutputFormat::Mp4;
        }
        if !matches!(preview.resolution, OutputResolution::FitWithin { width, height } if width >= 2 && height >= 2 && width <= 3840 && height <= 2160)
        {
            preview.resolution = OutputResolution::FitWithin {
                width: 854,
                height: 480,
            };
        }
        if !SUPPORTED_PREVIEW_FPS.contains(&preview.fps) {
            preview.fps = 15;
        }
        preview.fps = preview.fps.min(config.main.fps);
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

fn recording_preferences_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|directory| directory.join("recording.json"))
        .map_err(|error| error.to_string())
}

fn load_recording_preferences(app: &tauri::AppHandle) -> RecordingPreferences {
    recording_preferences_path(app)
        .and_then(|path| fs::read(path).map_err(|error| error.to_string()))
        .and_then(|bytes| {
            serde_json::from_slice::<RecordingPreferences>(&bytes)
                .map_err(|error| error.to_string())
        })
        .map(RecordingPreferences::normalized)
        .unwrap_or_default()
}

fn save_recording_preferences(
    app: &tauri::AppHandle,
    preferences: RecordingPreferences,
) -> Result<(), String> {
    let path = recording_preferences_path(app)?;
    let parent = path
        .parent()
        .ok_or_else(|| "recording preferences directory is unavailable".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let json = serde_json::to_vec_pretty(&preferences).map_err(|error| error.to_string())?;
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
        .write_all(b"ScreenHarbor storage probe")
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
    let directories = state
        .storage_config
        .lock()
        .map_err(|_| "storage settings are unavailable".to_owned())?
        .library_directories
        .clone();
    let index = state
        .recording_index
        .get_or_init(|| {
            let index = RecordingIndex::open(recording_index_path(app)?, directories.clone())?;
            index.reconcile()?;
            Ok(index)
        })
        .as_ref()
        .map_err(Clone::clone)?;
    index.synchronize_directories(directories)?;
    Ok(index)
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
            recording_preferences: Mutex::new(RecordingPreferences::default()),
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
            let recording_preferences = load_recording_preferences(app.handle());
            *app.state::<AppState>()
                .recording_preferences
                .lock()
                .expect("recording preferences are unavailable") = recording_preferences;
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
            get_recording_preferences,
            choose_save_directory,
            open_recording_location,
            get_recording_details,
            delete_recordings,
            open_interrupted_recording_location,
            delete_interrupted_recording,
            retry_preview,
            retry_cover,
            set_output_settings,
            set_preview_enabled,
            set_recording_preferences,
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
        .expect("failed to run ScreenHarbor");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn audio_snapshot_resolves_defaults_and_explicit_devices() {
        let endpoints = vec![
            frameflow_windows::AudioEndpointInfo {
                id: "default".to_owned(),
                name: "Default microphone".to_owned(),
            },
            frameflow_windows::AudioEndpointInfo {
                id: "usb".to_owned(),
                name: "USB microphone".to_owned(),
            },
        ];
        let following_default = audio_input_snapshot(true, None, &endpoints, Some("default"));
        assert!(following_default.available);
        assert_eq!(
            following_default.device_name.as_deref(),
            Some("Default microphone")
        );
        assert!(following_default.selected_device_id.is_none());

        let explicit = audio_input_snapshot(true, Some("usb"), &endpoints, Some("default"));
        assert!(explicit.available);
        assert_eq!(explicit.device_name.as_deref(), Some("USB microphone"));
        assert_eq!(explicit.selected_device_id.as_deref(), Some("usb"));

        let disconnected = audio_input_snapshot(true, Some("missing"), &endpoints, Some("default"));
        assert!(!disconnected.available);
        assert!(disconnected.device_name.is_none());
    }

    #[test]
    fn recording_countdown_is_disabled_by_default() {
        let preferences = RecordingPreferences::default();
        assert!(!preferences.countdown_enabled);
        assert_eq!(preferences.countdown_seconds, 3);
    }

    #[test]
    fn persisted_countdown_duration_is_kept_within_supported_bounds() {
        let too_short = RecordingPreferences {
            countdown_enabled: true,
            countdown_seconds: 0,
        }
        .normalized();
        let too_long = RecordingPreferences {
            countdown_enabled: true,
            countdown_seconds: 30,
        }
        .normalized();
        assert_eq!(too_short.countdown_seconds, 1);
        assert_eq!(too_long.countdown_seconds, 10);
    }

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
    fn main_quality_and_webm_settings_survive_persistence_and_legacy_files() {
        let mut config = OutputConfig::default();
        config.main.format = OutputFormat::Webm;
        config.main.quality = VideoQuality::Low;
        let saved = serde_json::to_vec(&config).unwrap();
        let loaded = normalize_loaded_output_config(serde_json::from_slice(&saved).unwrap());
        assert_eq!(loaded.main.format, OutputFormat::Webm);
        assert_eq!(loaded.main.quality, VideoQuality::Low);
        let mut legacy = serde_json::to_value(&config).unwrap();
        legacy["main"].as_object_mut().unwrap().remove("quality");
        let loaded: OutputConfig = serde_json::from_value(legacy).unwrap();
        assert_eq!(loaded.main.quality, VideoQuality::Balanced);
    }

    #[test]
    fn preserves_supported_preview_format_and_repairs_invalid_resolution() {
        let mut config = OutputConfig::default();
        let preview = config.preview.as_mut().unwrap();
        preview.format = OutputFormat::Webm;
        preview.fps = 60;
        preview.resolution = OutputResolution::Native;

        let normalized = normalize_loaded_output_config(config);
        let preview = normalized.preview.unwrap();
        assert_eq!(preview.format, OutputFormat::Webm);
        assert_eq!(preview.fps, 30);
        assert_eq!(
            preview.resolution,
            OutputResolution::FitWithin {
                width: 854,
                height: 480,
            }
        );
    }

    #[test]
    fn migrates_unsupported_preview_formats_to_mp4() {
        let mut config = OutputConfig::default();
        config.preview.as_mut().unwrap().format = OutputFormat::Mov;

        assert_eq!(
            normalize_loaded_output_config(config)
                .preview
                .unwrap()
                .format,
            OutputFormat::Mp4
        );
    }

    #[test]
    fn preserves_a_supported_preview_frame_rate() {
        let mut config = OutputConfig::default();
        config.preview.as_mut().unwrap().fps = 24;

        assert_eq!(
            normalize_loaded_output_config(config).preview.unwrap().fps,
            24
        );
    }

    #[test]
    fn preserves_animated_webp_and_disables_its_audio_track() {
        let mut config = OutputConfig::default();
        let preview = config.preview.as_mut().unwrap();
        preview.format = OutputFormat::AnimatedWebp;
        preview.include_audio = true;

        let preview = normalize_loaded_output_config(config).preview.unwrap();
        assert_eq!(preview.format, OutputFormat::AnimatedWebp);
        assert!(!preview.include_audio);
    }

    #[test]
    fn restores_a_missing_preview_profile() {
        let config = OutputConfig {
            preview: None,
            ..OutputConfig::default()
        };

        assert!(normalize_loaded_output_config(config).preview.is_some());
    }

    #[test]
    fn preserves_an_explicitly_disabled_preview_and_cover_pair() {
        let config = OutputConfig {
            preview: None,
            cover_enabled: false,
            ..OutputConfig::default()
        };

        let normalized = normalize_loaded_output_config(config);
        assert!(normalized.preview.is_none());
        assert!(!normalized.cover_enabled);
    }

    #[test]
    fn allocates_each_new_recording_inside_its_own_group_directory() {
        let directory = std::env::temp_dir().join(format!(
            "frameflow-recording-layout-{}",
            recording_id().unwrap()
        ));
        fs::create_dir_all(&directory).unwrap();

        let (temporary, output) = allocate_recording_paths_in(&directory, "1700000000009").unwrap();
        let group = directory.join("ScreenHarbor-1700000000009");
        assert_eq!(temporary, group.join("main.mp4.part"));
        assert_eq!(output, group.join("main.mp4"));
        assert!(group.is_dir());

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn hides_windows_verbatim_prefixes_in_display_paths() {
        assert_eq!(
            display_path(Path::new(r"\\?\E:\Work\Record")),
            r"E:\Work\Record"
        );
        assert_eq!(
            display_path(Path::new(r"\\?\UNC\server\share\Record")),
            r"\\server\share\Record"
        );
        assert_eq!(
            display_path(Path::new(r"E:\Work\Record")),
            r"E:\Work\Record"
        );
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
