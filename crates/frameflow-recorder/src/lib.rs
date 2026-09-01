#![cfg(target_os = "windows")]

use std::{
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, BufWriter, Read, Write},
    os::windows::io::{AsRawHandle, FromRawHandle},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use frameflow_capture::{CaptureSession, RegionSelection};
use serde::{Deserialize, Serialize};
use windows_sys::Win32::{
    Foundation::{ERROR_PIPE_CONNECTED, INVALID_HANDLE_VALUE},
    Storage::FileSystem::{
        FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_INBOUND, PIPE_ACCESS_OUTBOUND,
    },
    System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
        PIPE_TYPE_BYTE, PIPE_WAIT,
    },
};

pub const CAPTURE_WORKER_PROTOCOL_VERSION: u16 = 2;
const AUDIO_MIXER_TICK: Duration = Duration::from_millis(10);
const AUDIO_RENDER_DELAY: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioInputConfig {
    pub microphone: bool,
    pub system_audio: bool,
}

impl Default for AudioInputConfig {
    fn default() -> Self {
        Self {
            microphone: false,
            system_audio: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AudioInputSource {
    Microphone,
    SystemAudio,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CaptureEngineEvent {
    AudioWarning {
        source: AudioInputSource,
        message: String,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureStats {
    pub frame_count: u64,
    pub dropped_frames: u64,
    pub effective_fps: f64,
    pub elapsed_ms: u64,
    pub paused: bool,
    pub video_encoder: VideoEncoderSnapshot,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio: Option<CaptureAudioStats>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoEncoderSnapshot {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hardware_accelerated: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureAudioStats {
    pub microphone: LiveInputSnapshot,
    pub system_audio: LiveInputSnapshot,
    pub dropped_packets: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveInputSnapshot {
    pub enabled: bool,
    pub muted: bool,
    pub disconnected: bool,
}

pub struct FinalizedCapture {
    pub output_path: PathBuf,
    pub duration: Duration,
    pub output_size: CaptureOutputSize,
    pub frames_per_second: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureOutputSize {
    pub width: u32,
    pub height: u32,
}

impl CaptureOutputSize {
    pub fn new(width: u32, height: u32) -> Result<Self, String> {
        if width == 0 || height == 0 || !width.is_multiple_of(2) || !height.is_multiple_of(2) {
            return Err("capture output dimensions must be positive even numbers".to_owned());
        }
        Ok(Self { width, height })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureWorkerRequest {
    pub protocol_version: u16,
    pub request_id: u64,
    #[serde(flatten)]
    pub command: CaptureWorkerCommand,
}

impl CaptureWorkerRequest {
    pub fn new(request_id: u64, command: CaptureWorkerCommand) -> Self {
        Self {
            protocol_version: CAPTURE_WORKER_PROTOCOL_VERSION,
            request_id,
            command,
        }
    }

    pub fn validate_version(&self) -> Result<(), String> {
        if self.protocol_version == CAPTURE_WORKER_PROTOCOL_VERSION {
            Ok(())
        } else {
            Err(format!(
                "capture worker protocol {} is unsupported; expected {}",
                self.protocol_version, CAPTURE_WORKER_PROTOCOL_VERSION
            ))
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "command", rename_all = "camelCase")]
pub enum CaptureWorkerCommand {
    Start {
        selection: RegionSelection,
        output_size: CaptureOutputSize,
        temporary_path: PathBuf,
        output_path: PathBuf,
        frames_per_second: u32,
        audio: AudioInputConfig,
    },
    Pause,
    Resume,
    SetAudioMuted {
        source: AudioInputSource,
        muted: bool,
    },
    GetStats,
    Stop,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureWorkerResponse {
    pub protocol_version: u16,
    pub request_id: u64,
    #[serde(flatten)]
    pub result: CaptureWorkerResult,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "result", rename_all = "camelCase")]
pub enum CaptureWorkerResult {
    Accepted,
    Stats {
        stats: CaptureStats,
    },
    Finalized {
        output_path: PathBuf,
        duration_ms: u64,
    },
    Event {
        event: CaptureEngineEvent,
    },
    Error {
        message: String,
    },
}

pub struct CaptureWorkerClient {
    child: Child,
    input: Option<BufWriter<File>>,
    responses: mpsc::Receiver<Result<CaptureWorkerResponse, String>>,
    reader: Option<JoinHandle<()>>,
    reader_failure: Arc<Mutex<Option<String>>>,
    stderr: Arc<Mutex<String>>,
    stderr_reader: Option<JoinHandle<()>>,
    next_request_id: u64,
    finished: bool,
    output_size: CaptureOutputSize,
    frames_per_second: u32,
}

const WORKER_CONTROL_TIMEOUT: Duration = Duration::from_secs(5);
const WORKER_FINALIZE_TIMEOUT: Duration = Duration::from_secs(18);
const WORKER_PIPE_BUFFER_BYTES: u32 = 64 * 1024;

#[derive(Clone, Copy)]
enum WorkerPipeDirection {
    Commands,
    Responses,
}

fn create_worker_pipe(direction: WorkerPipeDirection) -> Result<(String, File), String> {
    let role = match direction {
        WorkerPipeDirection::Commands => "commands",
        WorkerPipeDirection::Responses => "responses",
    };
    let pipe_name = format!(
        r"\\.\pipe\frameflow-capture-{role}-{}",
        uuid::Uuid::new_v4().simple()
    );
    let wide_name = pipe_name
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let handle = unsafe {
        CreateNamedPipeW(
            wide_name.as_ptr(),
            match direction {
                WorkerPipeDirection::Commands => PIPE_ACCESS_OUTBOUND,
                WorkerPipeDirection::Responses => PIPE_ACCESS_INBOUND,
            } | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            WORKER_PIPE_BUFFER_BYTES,
            WORKER_PIPE_BUFFER_BYTES,
            0,
            std::ptr::null(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(format!(
            "failed to create capture worker pipe: {}",
            std::io::Error::last_os_error()
        ));
    }
    let pipe = unsafe { File::from_raw_handle(handle) };
    Ok((pipe_name, pipe))
}

fn connect_worker_pipe(pipe: &File) -> Result<(), String> {
    let connected = unsafe { ConnectNamedPipe(pipe.as_raw_handle(), std::ptr::null_mut()) };
    if connected != 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(ERROR_PIPE_CONNECTED as i32) {
        Ok(())
    } else {
        Err(format!("failed to connect capture worker pipe: {error}"))
    }
}

impl CaptureWorkerClient {
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        selection: &RegionSelection,
        output_size: CaptureOutputSize,
        temporary_path: impl AsRef<Path>,
        output_path: impl AsRef<Path>,
        frames_per_second: u32,
        audio: AudioInputConfig,
        event_handler: impl Fn(CaptureEngineEvent) + Send + Sync + 'static,
    ) -> Result<Self, String> {
        let executable = std::env::current_exe()
            .map_err(|error| format!("capture worker executable is unavailable: {error}"))?;
        let (command_pipe_name, command_pipe) = create_worker_pipe(WorkerPipeDirection::Commands)?;
        let (response_pipe_name, response_pipe) =
            create_worker_pipe(WorkerPipeDirection::Responses)?;
        let mut child = Command::new(executable)
            .arg("--capture-worker")
            .arg("--ipc-command-pipe")
            .arg(&command_pipe_name)
            .arg("--ipc-response-pipe")
            .arg(&response_pipe_name)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("failed to start capture worker: {error}"))?;
        connect_worker_pipe(&command_pipe)?;
        connect_worker_pipe(&response_pipe)?;
        let input = command_pipe;
        let output = response_pipe;
        let mut error_output = child
            .stderr
            .take()
            .ok_or_else(|| "capture worker stderr is unavailable".to_owned())?;
        let stderr = Arc::new(Mutex::new(String::new()));
        let stderr_reader = {
            let stderr = Arc::clone(&stderr);
            thread::Builder::new()
                .name("frameflow-worker-stderr-reader".to_owned())
                .spawn(move || {
                    let mut captured = String::new();
                    let _ = error_output.read_to_string(&mut captured);
                    if let Ok(mut output) = stderr.lock() {
                        *output = captured;
                    }
                })
                .map_err(|error| format!("failed to start capture worker stderr reader: {error}"))?
        };
        let event_handler: Arc<dyn Fn(CaptureEngineEvent) + Send + Sync> = Arc::new(event_handler);
        let (response_sender, responses) = mpsc::channel();
        let reader_failure = Arc::new(Mutex::new(None));
        let reader = thread::Builder::new()
            .name("frameflow-worker-ipc-reader".to_owned())
            .spawn({
                let reader_failure = Arc::clone(&reader_failure);
                move || {
                    let panic_sender = response_sender.clone();
                    let panic_failure = Arc::clone(&reader_failure);
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        read_worker_responses(
                            BufReader::new(output),
                            response_sender,
                            event_handler,
                            reader_failure,
                        )
                    }));
                    if let Err(payload) = result {
                        let message = format!(
                            "capture worker response reader panicked: {}",
                            panic_message(payload.as_ref())
                        );
                        store_reader_failure(&panic_failure, &message);
                        let _ = panic_sender.send(Err(message));
                    }
                }
            })
            .map_err(|error| format!("failed to start capture worker reader: {error}"))?;
        let mut client = Self {
            child,
            input: Some(BufWriter::new(input)),
            responses,
            reader: Some(reader),
            reader_failure,
            stderr,
            stderr_reader: Some(stderr_reader),
            next_request_id: 1,
            finished: false,
            output_size,
            frames_per_second,
        };
        match client.request(CaptureWorkerCommand::Start {
            selection: selection.clone(),
            output_size,
            temporary_path: temporary_path.as_ref().to_path_buf(),
            output_path: output_path.as_ref().to_path_buf(),
            frames_per_second,
            audio,
        })? {
            CaptureWorkerResult::Accepted => Ok(client),
            result => Err(unexpected_worker_result("start", result)),
        }
    }

    pub fn pause(&mut self) -> Result<(), String> {
        self.expect_accepted(CaptureWorkerCommand::Pause, "pause")
    }

    pub fn resume(&mut self) -> Result<(), String> {
        self.expect_accepted(CaptureWorkerCommand::Resume, "resume")
    }

    pub fn stats(&mut self) -> Result<CaptureStats, String> {
        match self.request(CaptureWorkerCommand::GetStats)? {
            CaptureWorkerResult::Stats { stats } => Ok(stats),
            result => Err(unexpected_worker_result("get stats", result)),
        }
    }

    pub fn set_audio_muted(&mut self, source: AudioInputSource, muted: bool) -> Result<(), String> {
        self.expect_accepted(
            CaptureWorkerCommand::SetAudioMuted { source, muted },
            "set audio mute",
        )
    }

    pub fn finalize(mut self) -> Result<FinalizedCapture, String> {
        let output_size = self.output_size;
        let frames_per_second = self.frames_per_second;
        let result = self.request(CaptureWorkerCommand::Stop)?;
        self.finished = true;
        self.input.take();
        let status = self
            .child
            .wait()
            .map_err(|error| format!("failed to wait for capture worker: {error}"))?;
        if !status.success() {
            self.join_stderr_reader();
            return Err(self.worker_exit_message(format!("capture worker exited with {status}")));
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        self.join_stderr_reader();
        match result {
            CaptureWorkerResult::Finalized {
                output_path,
                duration_ms,
            } => Ok(FinalizedCapture {
                output_path,
                duration: Duration::from_millis(duration_ms),
                output_size,
                frames_per_second,
            }),
            result => Err(unexpected_worker_result("stop", result)),
        }
    }

    fn expect_accepted(
        &mut self,
        command: CaptureWorkerCommand,
        operation: &'static str,
    ) -> Result<(), String> {
        match self.request(command)? {
            CaptureWorkerResult::Accepted => Ok(()),
            result => Err(unexpected_worker_result(operation, result)),
        }
    }

    fn request(&mut self, command: CaptureWorkerCommand) -> Result<CaptureWorkerResult, String> {
        let timeout = if matches!(&command, CaptureWorkerCommand::Stop) {
            WORKER_FINALIZE_TIMEOUT
        } else {
            WORKER_CONTROL_TIMEOUT
        };
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1);
        let request = CaptureWorkerRequest::new(request_id, command);
        let send_result = {
            let input = self
                .input
                .as_mut()
                .ok_or_else(|| "capture worker input is closed".to_owned())?;
            serde_json::to_writer(&mut *input, &request)
                .map_err(|error| format!("failed to encode capture worker request: {error}"))
                .and_then(|()| {
                    input
                        .write_all(b"\n")
                        .and_then(|()| input.flush())
                        .map_err(|error| format!("failed to send capture worker request: {error}"))
                })
        };
        if let Err(message) = send_result {
            return Err(self.worker_exit_context(message));
        }

        loop {
            let response = match self.responses.recv_timeout(timeout) {
                Ok(Ok(response)) => response,
                Ok(Err(message)) => return Err(self.worker_exit_context(message)),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    self.input.take();
                    return Err(format!(
                        "capture worker did not respond within {} seconds",
                        timeout.as_secs()
                    ));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let reader_failure = self
                        .reader_failure
                        .lock()
                        .ok()
                        .and_then(|failure| failure.clone())
                        .unwrap_or_else(|| {
                            "capture worker response reader stopped unexpectedly".to_owned()
                        });
                    return Err(self.worker_exit_context(format!(
                        "capture worker response channel is disconnected: {reader_failure}"
                    )));
                }
            };
            if response.protocol_version != CAPTURE_WORKER_PROTOCOL_VERSION {
                return Err(format!(
                    "capture worker protocol {} is unsupported; expected {}",
                    response.protocol_version, CAPTURE_WORKER_PROTOCOL_VERSION
                ));
            }
            match response.result {
                CaptureWorkerResult::Event { .. } => continue,
                CaptureWorkerResult::Error { message } if response.request_id == request_id => {
                    return Err(message);
                }
                result if response.request_id == request_id => return Ok(result),
                _ => {
                    return Err(format!(
                        "capture worker response ID {} does not match request {request_id}",
                        response.request_id
                    ));
                }
            }
        }
    }

    fn worker_exit_context(&mut self, message: String) -> String {
        for _ in 0..20 {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    self.join_stderr_reader();
                    return self
                        .worker_exit_message(format!("{message}; worker exited with {status}"));
                }
                Ok(None) => thread::sleep(Duration::from_millis(10)),
                Err(error) => return format!("{message}; failed to query worker status: {error}"),
            }
        }
        message
    }

    fn join_stderr_reader(&mut self) {
        if let Some(reader) = self.stderr_reader.take() {
            let _ = reader.join();
        }
    }

    fn worker_exit_message(&self, message: String) -> String {
        let stderr = self
            .stderr
            .lock()
            .ok()
            .map(|output| output.trim().to_owned())
            .filter(|output| !output.is_empty());
        match stderr {
            Some(stderr) => format!("{message}; worker stderr: {stderr}"),
            None => message,
        }
    }
}

impl Drop for CaptureWorkerClient {
    fn drop(&mut self) {
        self.input.take();
        if !self.finished {
            let _ = self.child.try_wait();
        }
    }
}

fn read_worker_responses(
    mut output: BufReader<impl std::io::Read>,
    sender: mpsc::Sender<Result<CaptureWorkerResponse, String>>,
    event_handler: Arc<dyn Fn(CaptureEngineEvent) + Send + Sync>,
    reader_failure: Arc<Mutex<Option<String>>>,
) {
    loop {
        let mut line = String::new();
        let bytes = match output.read_line(&mut line) {
            Ok(bytes) => bytes,
            Err(error) => {
                report_reader_failure(
                    &sender,
                    &reader_failure,
                    format!("failed to read capture worker response: {error}"),
                );
                return;
            }
        };
        if bytes == 0 {
            report_reader_failure(
                &sender,
                &reader_failure,
                "capture worker exited before responding".to_owned(),
            );
            return;
        }
        let response: CaptureWorkerResponse = match serde_json::from_str(&line) {
            Ok(response) => response,
            Err(error) => {
                let received = line.trim_end().chars().take(500).collect::<String>();
                report_reader_failure(
                    &sender,
                    &reader_failure,
                    format!(
                        "capture worker returned invalid JSON: {error}; received: {received:?}"
                    ),
                );
                return;
            }
        };
        if let CaptureWorkerResult::Event { event } = &response.result {
            if let Err(payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                event_handler(event.clone())
            })) {
                let message = format!(
                    "capture worker event handler panicked: {}",
                    panic_message(payload.as_ref())
                );
                store_reader_failure(&reader_failure, &message);
                let _ = sender.send(Err(message));
            }
        } else if sender.send(Ok(response)).is_err() {
            return;
        }
    }
}

fn report_reader_failure(
    sender: &mpsc::Sender<Result<CaptureWorkerResponse, String>>,
    reader_failure: &Mutex<Option<String>>,
    message: String,
) {
    store_reader_failure(reader_failure, &message);
    let _ = sender.send(Err(message));
}

fn store_reader_failure(reader_failure: &Mutex<Option<String>>, message: &str) {
    if let Ok(mut failure) = reader_failure.lock() {
        *failure = Some(message.to_owned());
    }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> &str {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("unknown panic payload")
}

fn unexpected_worker_result(operation: &'static str, result: CaptureWorkerResult) -> String {
    format!("capture worker returned an unexpected result for {operation}: {result:?}")
}

pub fn run_capture_worker_stdio() -> Result<(), String> {
    let output = Arc::new(Mutex::new(BufWriter::new(std::io::stdout())));
    let stdin = std::io::stdin();
    run_capture_worker_io(stdin.lock(), output)
}

pub fn run_capture_worker_named_pipes(
    command_pipe_name: &str,
    response_pipe_name: &str,
) -> Result<(), String> {
    if !command_pipe_name.starts_with(r"\\.\pipe\frameflow-capture-commands-")
        || !response_pipe_name.starts_with(r"\\.\pipe\frameflow-capture-responses-")
    {
        return Err("capture worker pipe names are invalid".to_owned());
    }
    let command_pipe = OpenOptions::new()
        .read(true)
        .open(command_pipe_name)
        .map_err(|error| format!("failed to open capture worker command pipe: {error}"))?;
    let response_pipe = OpenOptions::new()
        .write(true)
        .open(response_pipe_name)
        .map_err(|error| format!("failed to open capture worker response pipe: {error}"))?;
    run_capture_worker_io(
        BufReader::new(command_pipe),
        Arc::new(Mutex::new(BufWriter::new(response_pipe))),
    )
}

fn run_capture_worker_io<R, W>(mut input: R, output: Arc<Mutex<BufWriter<W>>>) -> Result<(), String>
where
    R: BufRead,
    W: Write + Send + 'static,
{
    let mut engine: Option<CaptureEngine> = None;

    loop {
        let mut line = String::new();
        let bytes = input
            .read_line(&mut line)
            .map_err(|error| format!("failed to read worker request: {error}"))?;
        if bytes == 0 {
            if let Some(active) = engine.take() {
                active.finalize()?;
            }
            return Ok(());
        }
        let request: CaptureWorkerRequest = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(error) => {
                write_worker_response(
                    &output,
                    CaptureWorkerResponse {
                        protocol_version: CAPTURE_WORKER_PROTOCOL_VERSION,
                        request_id: 0,
                        result: CaptureWorkerResult::Error {
                            message: format!("invalid capture worker request: {error}"),
                        },
                    },
                )?;
                continue;
            }
        };
        let request_id = request.request_id;
        let stop_requested = matches!(request.command, CaptureWorkerCommand::Stop);
        let result = match request.validate_version() {
            Ok(()) => execute_worker_command(&mut engine, request.command, Arc::clone(&output)),
            Err(message) => Err(message),
        };
        write_worker_response(
            &output,
            CaptureWorkerResponse {
                protocol_version: CAPTURE_WORKER_PROTOCOL_VERSION,
                request_id,
                result: result.unwrap_or_else(|message| CaptureWorkerResult::Error { message }),
            },
        )?;
        if stop_requested && engine.is_none() {
            return Ok(());
        }
    }
}

fn execute_worker_command<W>(
    engine: &mut Option<CaptureEngine>,
    command: CaptureWorkerCommand,
    output: Arc<Mutex<BufWriter<W>>>,
) -> Result<CaptureWorkerResult, String>
where
    W: Write + Send + 'static,
{
    match command {
        CaptureWorkerCommand::Start {
            selection,
            output_size,
            temporary_path,
            output_path,
            frames_per_second,
            audio,
        } => {
            if engine.is_some() {
                return Err("capture worker already owns a recording".to_owned());
            }
            let active = CaptureEngine::start(
                &selection,
                output_size,
                temporary_path,
                output_path,
                frames_per_second,
                audio,
                move |event| {
                    let _ = write_worker_response(
                        &output,
                        CaptureWorkerResponse {
                            protocol_version: CAPTURE_WORKER_PROTOCOL_VERSION,
                            request_id: 0,
                            result: CaptureWorkerResult::Event { event },
                        },
                    );
                },
            )?;
            *engine = Some(active);
            Ok(CaptureWorkerResult::Accepted)
        }
        CaptureWorkerCommand::Pause => {
            active_worker_engine(engine)?.pause()?;
            Ok(CaptureWorkerResult::Accepted)
        }
        CaptureWorkerCommand::Resume => {
            active_worker_engine(engine)?.resume()?;
            Ok(CaptureWorkerResult::Accepted)
        }
        CaptureWorkerCommand::SetAudioMuted { source, muted } => {
            active_worker_engine(engine)?.set_audio_muted(source, muted)?;
            Ok(CaptureWorkerResult::Accepted)
        }
        CaptureWorkerCommand::GetStats => Ok(CaptureWorkerResult::Stats {
            stats: active_worker_engine(engine)?.stats()?,
        }),
        CaptureWorkerCommand::Stop => {
            let finalized = engine
                .take()
                .ok_or_else(|| "capture worker has no active recording".to_owned())?
                .finalize()?;
            Ok(CaptureWorkerResult::Finalized {
                output_path: finalized.output_path,
                duration_ms: finalized.duration.as_millis().min(u128::from(u64::MAX)) as u64,
            })
        }
    }
}

fn active_worker_engine(engine: &mut Option<CaptureEngine>) -> Result<&mut CaptureEngine, String> {
    engine
        .as_mut()
        .ok_or_else(|| "capture worker has no active recording".to_owned())
}

fn write_worker_response<W: Write>(
    output: &Mutex<BufWriter<W>>,
    response: CaptureWorkerResponse,
) -> Result<(), String> {
    let mut output = output
        .lock()
        .map_err(|_| "capture worker output is unavailable".to_owned())?;
    serde_json::to_writer(&mut *output, &response)
        .map_err(|error| format!("failed to encode worker response: {error}"))?;
    output
        .write_all(b"\n")
        .and_then(|()| output.flush())
        .map_err(|error| format!("failed to write worker response: {error}"))
}

pub struct CaptureEngine {
    session: frameflow_windows::WgcCaptureSession,
    audio: Option<ActiveAudioCapture>,
    frame_count: Arc<AtomicU64>,
    timeline_frame: Arc<AtomicU64>,
    last_error: Arc<Mutex<Option<String>>>,
    started_at: Instant,
    paused_at: Option<Instant>,
    paused_total: Duration,
    temporary_path: PathBuf,
    output_path: PathBuf,
    output_size: CaptureOutputSize,
    frames_per_second: u32,
}

struct ActiveAudioCapture {
    config: AudioInputConfig,
    sources: Vec<ActiveAudioSource>,
    mixer: Arc<Mutex<frameflow_windows::AudioMixer>>,
    worker_input: mpsc::SyncSender<AudioWorkerMessage>,
    worker: Option<JoinHandle<()>>,
    failures: Arc<AudioFailureState>,
    dropped_packets: Arc<AtomicU64>,
    paused: Arc<AtomicBool>,
    pause_offset_100ns: Arc<AtomicU64>,
    warning: Arc<Mutex<Option<String>>>,
}

struct ActiveAudioSource {
    id: frameflow_windows::AudioSourceId,
    session: frameflow_windows::WasapiCaptureSession,
}

#[derive(Clone)]
struct AudioCallbackContext {
    worker_input: mpsc::SyncSender<AudioWorkerMessage>,
    failures: Arc<AudioFailureState>,
    paused: Arc<AtomicBool>,
    pause_offset_100ns: Arc<AtomicU64>,
    queue_overloaded: Arc<AtomicBool>,
    dropped_packets: Arc<AtomicU64>,
    warning: Arc<Mutex<Option<String>>>,
    event_handler: Arc<dyn Fn(CaptureEngineEvent) + Send + Sync>,
}

struct AudioMixerWorkerContext {
    mixer: Arc<Mutex<frameflow_windows::AudioMixer>>,
    output: frameflow_windows::WgcAudioInput,
    failures: Arc<AudioFailureState>,
    warning: Arc<Mutex<Option<String>>>,
    event_handler: Arc<dyn Fn(CaptureEngineEvent) + Send + Sync>,
    timeline_started_at: Instant,
    paused: Arc<AtomicBool>,
    pause_offset_100ns: Arc<AtomicU64>,
}

enum AudioWorkerMessage {
    Packet {
        source: frameflow_windows::AudioSourceId,
        packet: frameflow_windows::AudioPacket,
    },
    Stop {
        final_frame: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AudioCallbackKind {
    Packet,
    Failure,
}

fn should_handle_audio_callback(
    kind: AudioCallbackKind,
    callback_stopped: bool,
    paused: bool,
) -> bool {
    !callback_stopped && (kind == AudioCallbackKind::Failure || !paused)
}

#[derive(Default)]
struct AudioFailureState {
    system: AtomicBool,
    microphone: AtomicBool,
}

impl AudioFailureState {
    fn source(&self, source: frameflow_windows::AudioSourceId) -> &AtomicBool {
        match source {
            frameflow_windows::AudioSourceId::System => &self.system,
            frameflow_windows::AudioSourceId::Microphone => &self.microphone,
        }
    }
}

impl CaptureEngine {
    pub fn start(
        selection: &RegionSelection,
        output_size: CaptureOutputSize,
        temporary_path: impl AsRef<Path>,
        output_path: impl AsRef<Path>,
        frames_per_second: u32,
        audio_config: AudioInputConfig,
        event_handler: impl Fn(CaptureEngineEvent) + Send + Sync + 'static,
    ) -> Result<Self, String> {
        let audio_enabled = audio_config.system_audio || audio_config.microphone;
        let capture_options = frameflow_windows::Mp4CaptureOptions::new(frames_per_second)
            .map_err(|error| error.to_string())?
            .with_audio(audio_enabled)
            .with_output_size(output_size.width, output_size.height)
            .map_err(|error| error.to_string())?;
        let frame_count = Arc::new(AtomicU64::new(0));
        let timeline_frame = Arc::new(AtomicU64::new(0));
        let last_error = Arc::new(Mutex::new(None));
        let session = frameflow_windows::start_region_mp4_capture(
            selection,
            temporary_path.as_ref(),
            capture_options,
            {
                let frame_count = Arc::clone(&frame_count);
                let timeline_frame = Arc::clone(&timeline_frame);
                let last_error = Arc::clone(&last_error);
                move |result| match result {
                    Ok(frame) => {
                        frame_count.store(frame.sequence, Ordering::Release);
                        timeline_frame.store(frame.timeline_frame, Ordering::Release);
                    }
                    Err(error) => {
                        if let Ok(mut last_error) = last_error.lock() {
                            *last_error = Some(error.to_string());
                        }
                    }
                }
            },
        )
        .map_err(|error| error.to_string())?;
        let started_at = Instant::now();
        let audio = if audio_enabled {
            let output = session
                .audio_input()
                .ok_or_else(|| "audio encoder input is unavailable".to_owned())?;
            Some(start_active_audio(
                audio_config,
                output,
                frameflow_windows::performance_time_100ns().ok(),
                started_at,
                Arc::new(event_handler),
            )?)
        } else {
            None
        };
        Ok(Self {
            session,
            audio,
            frame_count,
            timeline_frame,
            last_error,
            started_at,
            paused_at: None,
            paused_total: Duration::ZERO,
            temporary_path: temporary_path.as_ref().to_path_buf(),
            output_path: output_path.as_ref().to_path_buf(),
            output_size,
            frames_per_second,
        })
    }

    pub fn pause(&mut self) -> Result<(), String> {
        if let Some(audio) = self.audio.as_ref() {
            audio.paused.store(true, Ordering::Release);
        }
        if let Err(error) = self.session.pause() {
            if let Some(audio) = self.audio.as_ref() {
                audio.paused.store(false, Ordering::Release);
            }
            return Err(error.to_string());
        }
        self.paused_at = Some(Instant::now());
        Ok(())
    }

    pub fn resume(&mut self) -> Result<(), String> {
        self.session.resume().map_err(|error| error.to_string())?;
        if let Some(paused_at) = self.paused_at.take() {
            let paused_for = paused_at.elapsed();
            self.paused_total += paused_for;
            if let Some(audio) = self.audio.as_ref() {
                audio
                    .pause_offset_100ns
                    .fetch_add(duration_to_100ns(paused_for), Ordering::AcqRel);
                audio.paused.store(false, Ordering::Release);
            }
        }
        Ok(())
    }

    pub fn stats(&self) -> Result<CaptureStats, String> {
        let clock_end = self.paused_at.unwrap_or_else(Instant::now);
        let elapsed = clock_end
            .duration_since(self.started_at)
            .saturating_sub(self.paused_total);
        let last_error = self
            .last_error
            .lock()
            .map_err(|_| "capture error state is unavailable".to_owned())?
            .clone();
        let audio = self.audio.as_ref().map(capture_audio_stats).transpose()?;
        let frame_count = self.frame_count.load(Ordering::Acquire);
        let timeline_frames = self
            .timeline_frame
            .load(Ordering::Acquire)
            .saturating_add(u64::from(frame_count > 0));
        let elapsed_ms = elapsed.as_millis().min(u128::from(u64::MAX)) as u64;
        let (dropped_frames, effective_fps) =
            capture_performance(frame_count, timeline_frames, elapsed_ms);
        let encoder = self.session.video_encoder_info().unwrap_or_else(|| {
            frameflow_windows::VideoEncoderInfo {
                name: "Media Foundation H.264".to_owned(),
                hardware_accelerated: None,
            }
        });
        Ok(CaptureStats {
            frame_count,
            dropped_frames,
            effective_fps,
            elapsed_ms,
            paused: self.paused_at.is_some(),
            video_encoder: VideoEncoderSnapshot {
                name: encoder.name,
                hardware_accelerated: encoder.hardware_accelerated,
            },
            last_error,
            audio,
        })
    }

    pub fn set_audio_muted(&self, source: AudioInputSource, muted: bool) -> Result<(), String> {
        let audio = self
            .audio
            .as_ref()
            .ok_or_else(|| "audio is not enabled for this recording".to_owned())?;
        let source = match source {
            AudioInputSource::Microphone if audio.config.microphone => {
                frameflow_windows::AudioSourceId::Microphone
            }
            AudioInputSource::SystemAudio if audio.config.system_audio => {
                frameflow_windows::AudioSourceId::System
            }
            _ => return Err("audio input is not enabled for this recording".to_owned()),
        };
        if audio
            .sources
            .iter()
            .find(|runtime| runtime.id == source)
            .is_none()
            || audio.failures.source(source).load(Ordering::Acquire)
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

    pub fn finalize(self) -> Result<FinalizedCapture, String> {
        let Self {
            session,
            audio,
            started_at,
            paused_at,
            paused_total,
            temporary_path,
            output_path,
            output_size,
            frames_per_second,
            ..
        } = self;
        let clock_end = paused_at.unwrap_or_else(Instant::now);
        let duration = clock_end
            .duration_since(started_at)
            .saturating_sub(paused_total);
        if let Some(audio) = audio {
            stop_active_audio(audio, duration);
        }
        session.stop().map_err(|error| error.to_string())?;
        fs::rename(&temporary_path, &output_path)
            .map_err(|error| format!("failed to finalize recording file: {error}"))?;
        Ok(FinalizedCapture {
            output_path,
            duration,
            output_size,
            frames_per_second,
        })
    }
}

fn start_active_audio(
    config: AudioInputConfig,
    output: frameflow_windows::WgcAudioInput,
    timeline_origin_100ns: Option<u64>,
    timeline_started_at: Instant,
    event_handler: Arc<dyn Fn(CaptureEngineEvent) + Send + Sync>,
) -> Result<ActiveAudioCapture, String> {
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
    let queue_overloaded = Arc::new(AtomicBool::new(false));
    let dropped_packets = Arc::new(AtomicU64::new(0));
    let warning = Arc::new(Mutex::new(None));
    let failures = Arc::new(AudioFailureState::default());
    let (worker_input, worker_receiver) = mpsc::sync_channel(128);
    let worker = {
        let mixer = Arc::clone(&mixer);
        let failures = Arc::clone(&failures);
        let warning = Arc::clone(&warning);
        let event_handler = Arc::clone(&event_handler);
        let paused = Arc::clone(&paused);
        let pause_offset_100ns = Arc::clone(&pause_offset_100ns);
        thread::Builder::new()
            .name("frameflow-audio-mixer".to_owned())
            .spawn(move || {
                run_audio_mixer_worker(
                    AudioMixerWorkerContext {
                        mixer,
                        output,
                        failures,
                        warning,
                        event_handler,
                        timeline_started_at,
                        paused,
                        pause_offset_100ns,
                    },
                    worker_receiver,
                );
            })
            .map_err(|error| format!("failed to start audio mixer worker: {error}"))?
    };
    let callback_context = AudioCallbackContext {
        worker_input: worker_input.clone(),
        failures: Arc::clone(&failures),
        paused: Arc::clone(&paused),
        pause_offset_100ns: Arc::clone(&pause_offset_100ns),
        queue_overloaded,
        dropped_packets: Arc::clone(&dropped_packets),
        warning: Arc::clone(&warning),
        event_handler: Arc::clone(&event_handler),
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
                record_audio_error(
                    &callback_context.event_handler,
                    &warning,
                    source_id,
                    error.to_string(),
                );
                failures.source(source_id).store(true, Ordering::Release);
                if let Ok(mut mixer) = mixer.lock() {
                    let mut control = mixer.source_control(source_id);
                    control.enabled = false;
                    mixer.set_source_control(source_id, control);
                }
            }
        }
    }
    Ok(ActiveAudioCapture {
        config,
        sources,
        mixer,
        worker_input,
        worker: Some(worker),
        failures,
        dropped_packets,
        paused,
        pause_offset_100ns,
        warning,
    })
}

fn start_audio_source(
    capture_source: frameflow_windows::AudioCaptureSource,
    source_id: frameflow_windows::AudioSourceId,
    context: AudioCallbackContext,
) -> Result<ActiveAudioSource, frameflow_windows::PlatformError> {
    let mut callback_stopped = false;
    let session = frameflow_windows::start_wasapi_capture(capture_source, move |result| {
        let kind = if result.is_ok() {
            AudioCallbackKind::Packet
        } else {
            AudioCallbackKind::Failure
        };
        if !should_handle_audio_callback(
            kind,
            callback_stopped,
            context.paused.load(Ordering::Acquire),
        ) {
            return;
        }
        match result {
            Ok(mut packet) => {
                packet.qpc_position_100ns = packet
                    .qpc_position_100ns
                    .saturating_sub(context.pause_offset_100ns.load(Ordering::Acquire));
                match context.worker_input.try_send(AudioWorkerMessage::Packet {
                    source: source_id,
                    packet,
                }) {
                    Ok(()) => {
                        context.queue_overloaded.store(false, Ordering::Release);
                    }
                    Err(mpsc::TrySendError::Full(_)) => {
                        context.dropped_packets.fetch_add(1, Ordering::Relaxed);
                        if !context.queue_overloaded.swap(true, Ordering::AcqRel) {
                            record_audio_error(
                                &context.event_handler,
                                &context.warning,
                                source_id,
                                "音频输入队列过载，已丢弃瞬时数据".to_owned(),
                            );
                        }
                    }
                    Err(mpsc::TrySendError::Disconnected(_)) => {
                        context
                            .failures
                            .source(source_id)
                            .store(true, Ordering::Release);
                        callback_stopped = true;
                    }
                }
            }
            Err(error) => {
                record_audio_error(
                    &context.event_handler,
                    &context.warning,
                    source_id,
                    error.to_string(),
                );
                context
                    .failures
                    .source(source_id)
                    .store(true, Ordering::Release);
                callback_stopped = true;
            }
        }
    })?;
    Ok(ActiveAudioSource {
        id: source_id,
        session,
    })
}

fn run_audio_mixer_worker(
    context: AudioMixerWorkerContext,
    input: mpsc::Receiver<AudioWorkerMessage>,
) {
    let mut system_normalizer = None;
    let mut microphone_normalizer = None;
    let mut disabled_failed_sources = [false; 2];
    let render_delay_frames = duration_to_audio_frames(AUDIO_RENDER_DELAY);
    loop {
        disable_failed_audio_sources(
            &context.mixer,
            &context.failures,
            &mut disabled_failed_sources,
        );
        match input.recv_timeout(AUDIO_MIXER_TICK) {
            Ok(AudioWorkerMessage::Packet { source, packet }) => {
                let normalizer = match source {
                    frameflow_windows::AudioSourceId::System => &mut system_normalizer,
                    frameflow_windows::AudioSourceId::Microphone => &mut microphone_normalizer,
                };
                let result = normalize_audio_packet(normalizer, &context.mixer, source, &packet);
                if let Err(error) = result {
                    context
                        .failures
                        .source(source)
                        .store(true, Ordering::Release);
                    record_audio_error(
                        &context.event_handler,
                        &context.warning,
                        source,
                        error.to_string(),
                    );
                    if let Ok(mut mixer) = context.mixer.lock() {
                        let mut control = mixer.source_control(source);
                        control.enabled = false;
                        mixer.set_source_control(source, control);
                    }
                }
            }
            Ok(AudioWorkerMessage::Stop { final_frame }) => {
                flush_audio_tail(
                    &context.mixer,
                    &context.output,
                    system_normalizer,
                    microphone_normalizer,
                    final_frame,
                );
                return;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let final_frame = audio_clock_frame(
                    context.timeline_started_at,
                    context.pause_offset_100ns.load(Ordering::Acquire),
                );
                flush_audio_tail(
                    &context.mixer,
                    &context.output,
                    system_normalizer,
                    microphone_normalizer,
                    final_frame,
                );
                return;
            }
        }
        if !context.paused.load(Ordering::Acquire) {
            let clock_frame = audio_clock_frame(
                context.timeline_started_at,
                context.pause_offset_100ns.load(Ordering::Acquire),
            );
            if render_audio_until(
                &context.mixer,
                &context.output,
                clock_frame.saturating_sub(render_delay_frames),
            )
            .is_err()
            {
                return;
            }
        }
    }
}

fn disable_failed_audio_sources(
    mixer: &Mutex<frameflow_windows::AudioMixer>,
    failures: &AudioFailureState,
    disabled: &mut [bool; 2],
) {
    if let Ok(mut mixer) = mixer.lock() {
        for (index, source) in [
            frameflow_windows::AudioSourceId::System,
            frameflow_windows::AudioSourceId::Microphone,
        ]
        .into_iter()
        .enumerate()
        {
            if !disabled[index] && failures.source(source).load(Ordering::Acquire) {
                let mut control = mixer.source_control(source);
                control.enabled = false;
                mixer.set_source_control(source, control);
                disabled[index] = true;
            }
        }
    }
}

fn normalize_audio_packet(
    normalizer: &mut Option<frameflow_windows::AudioNormalizer>,
    mixer: &Mutex<frameflow_windows::AudioMixer>,
    source: frameflow_windows::AudioSourceId,
    packet: &frameflow_windows::AudioPacket,
) -> Result<(), frameflow_windows::PlatformError> {
    if normalizer.is_none() {
        *normalizer = Some(frameflow_windows::AudioNormalizer::new(packet.format)?);
    }
    let chunk = normalizer
        .as_mut()
        .expect("audio normalizer was initialized")
        .push(packet)?;
    mixer
        .lock()
        .map_err(|_| {
            frameflow_windows::PlatformError::AudioCapture("audio mixer is unavailable".to_owned())
        })?
        .push(source, chunk);
    Ok(())
}

fn render_audio_until(
    mixer: &Mutex<frameflow_windows::AudioMixer>,
    output: &frameflow_windows::WgcAudioInput,
    target_frame: u64,
) -> Result<(), frameflow_windows::PlatformError> {
    let blocks = mixer
        .lock()
        .map_err(|_| {
            frameflow_windows::PlatformError::AudioCapture("audio mixer is unavailable".to_owned())
        })?
        .render_until(target_frame);
    for block in blocks {
        output.submit_blocking(block)?;
    }
    Ok(())
}

fn record_audio_error(
    event_handler: &Arc<dyn Fn(CaptureEngineEvent) + Send + Sync>,
    warning: &Arc<Mutex<Option<String>>>,
    source: frameflow_windows::AudioSourceId,
    message: String,
) {
    let message = format!("音频已降级：{message}");
    if let Ok(mut warning) = warning.lock() {
        *warning = Some(message.clone());
    }
    event_handler(CaptureEngineEvent::AudioWarning {
        source: match source {
            frameflow_windows::AudioSourceId::System => AudioInputSource::SystemAudio,
            frameflow_windows::AudioSourceId::Microphone => AudioInputSource::Microphone,
        },
        message,
    });
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
                && (runtime.is_none() || audio.failures.source(source).load(Ordering::Acquire)),
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
        dropped_packets: audio.dropped_packets.load(Ordering::Acquire),
        warning: audio
            .warning
            .lock()
            .map_err(|_| "audio warning state is unavailable".to_owned())?
            .clone(),
    })
}

fn stop_active_audio(mut audio: ActiveAudioCapture, final_duration: Duration) {
    audio.paused.store(true, Ordering::Release);
    for source in audio.sources.drain(..) {
        let _ = source.session.stop();
    }
    let _ = audio.worker_input.send(AudioWorkerMessage::Stop {
        final_frame: duration_to_audio_frames(final_duration),
    });
    if let Some(worker) = audio.worker.take() {
        let _ = worker.join();
    }
}

fn flush_audio_tail(
    mixer: &Mutex<frameflow_windows::AudioMixer>,
    output: &frameflow_windows::WgcAudioInput,
    mut system_normalizer: Option<frameflow_windows::AudioNormalizer>,
    mut microphone_normalizer: Option<frameflow_windows::AudioNormalizer>,
    final_frame: u64,
) {
    if let Ok(mut mixer) = mixer.lock() {
        for (source, tail) in [
            (
                frameflow_windows::AudioSourceId::System,
                system_normalizer
                    .as_mut()
                    .map(frameflow_windows::AudioNormalizer::finish),
            ),
            (
                frameflow_windows::AudioSourceId::Microphone,
                microphone_normalizer
                    .as_mut()
                    .map(frameflow_windows::AudioNormalizer::finish),
            ),
        ] {
            if let Some(tail) = tail.filter(|chunk| chunk.frames() > 0) {
                mixer.push(source, tail);
            }
        }
        let blocks = mixer.finish_at(final_frame);
        submit_final_audio_blocks(output, blocks);
    }
}

fn submit_final_audio_blocks(
    output: &frameflow_windows::WgcAudioInput,
    blocks: Vec<frameflow_windows::MixedAudioBlock>,
) {
    for block in blocks {
        let _ = output.submit_blocking(block);
    }
}

fn audio_clock_frame(started_at: Instant, pause_offset_100ns: u64) -> u64 {
    audio_frame_for_elapsed(started_at.elapsed(), pause_offset_100ns)
}

fn audio_frame_for_elapsed(elapsed: Duration, pause_offset_100ns: u64) -> u64 {
    let elapsed_100ns = duration_to_100ns(elapsed).saturating_sub(pause_offset_100ns);
    elapsed_100ns.saturating_mul(u64::from(frameflow_windows::NORMALIZED_AUDIO_SAMPLE_RATE))
        / 10_000_000
}

fn duration_to_audio_frames(duration: Duration) -> u64 {
    duration
        .as_nanos()
        .saturating_mul(u128::from(frameflow_windows::NORMALIZED_AUDIO_SAMPLE_RATE))
        .saturating_div(1_000_000_000)
        .min(u128::from(u64::MAX)) as u64
}

fn duration_to_100ns(duration: Duration) -> u64 {
    duration
        .as_nanos()
        .saturating_div(100)
        .min(u128::from(u64::MAX)) as u64
}

fn capture_performance(frame_count: u64, timeline_frames: u64, elapsed_ms: u64) -> (u64, f64) {
    let dropped_frames = timeline_frames.saturating_sub(frame_count);
    let effective_fps = if elapsed_ms == 0 {
        0.0
    } else {
        frame_count as f64 * 1_000.0 / elapsed_ms as f64
    };
    (dropped_frames, effective_fps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_defaults_to_system_output_only() {
        let config = AudioInputConfig::default();
        assert!(config.system_audio);
        assert!(!config.microphone);
    }

    #[test]
    fn audio_input_sources_have_stable_protocol_names() {
        assert_eq!(
            serde_json::to_string(&AudioInputSource::SystemAudio).unwrap(),
            "\"systemAudio\""
        );
        assert_eq!(
            serde_json::to_string(&AudioInputSource::Microphone).unwrap(),
            "\"microphone\""
        );
    }

    #[test]
    fn worker_requests_round_trip_with_a_version_and_request_id() {
        let request = CaptureWorkerRequest::new(
            42,
            CaptureWorkerCommand::SetAudioMuted {
                source: AudioInputSource::SystemAudio,
                muted: true,
            },
        );
        let json = serde_json::to_string(&request).unwrap();
        let decoded: CaptureWorkerRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(decoded.protocol_version, CAPTURE_WORKER_PROTOCOL_VERSION);
        assert_eq!(decoded.request_id, 42);
        assert!(matches!(
            decoded.command,
            CaptureWorkerCommand::SetAudioMuted {
                source: AudioInputSource::SystemAudio,
                muted: true
            }
        ));
    }

    #[test]
    fn capture_performance_reports_real_throughput_and_dropped_slots() {
        let (dropped_frames, effective_fps) = capture_performance(450, 600, 10_000);
        assert_eq!(dropped_frames, 150);
        assert!((effective_fps - 45.0).abs() < f64::EPSILON);
    }

    #[test]
    fn audio_clock_runs_at_48khz_and_excludes_paused_time() {
        assert_eq!(audio_frame_for_elapsed(Duration::from_secs(10), 0), 480_000);
        assert_eq!(
            audio_frame_for_elapsed(Duration::from_secs(10), 20_000_000),
            384_000
        );
    }

    #[test]
    fn audio_clock_saturates_when_pause_offset_exceeds_elapsed_time() {
        assert_eq!(
            audio_frame_for_elapsed(Duration::from_secs(1), 20_000_000),
            0
        );
    }

    #[test]
    fn encoder_snapshot_uses_stable_worker_protocol_fields() {
        let snapshot = VideoEncoderSnapshot {
            name: "Example H.264 Encoder".to_owned(),
            hardware_accelerated: Some(true),
        };
        assert_eq!(
            serde_json::to_value(snapshot).unwrap(),
            serde_json::json!({
                "name": "Example H.264 Encoder",
                "hardwareAccelerated": true
            })
        );
    }

    #[test]
    fn capture_stats_round_trip_through_json() {
        let response = CaptureWorkerResponse {
            protocol_version: CAPTURE_WORKER_PROTOCOL_VERSION,
            request_id: 2,
            result: CaptureWorkerResult::Stats {
                stats: CaptureStats {
                    frame_count: 1,
                    dropped_frames: 0,
                    effective_fps: 60.0,
                    elapsed_ms: 9,
                    paused: false,
                    video_encoder: VideoEncoderSnapshot {
                        name: "Example H.264 Encoder".to_owned(),
                        hardware_accelerated: Some(true),
                    },
                    last_error: None,
                    audio: None,
                },
            },
        };
        let json = serde_json::to_vec(&response).unwrap();
        let decoded: CaptureWorkerResponse = serde_json::from_slice(&json).unwrap();
        match decoded.result {
            CaptureWorkerResult::Stats { stats } => assert_eq!(stats.elapsed_ms, 9),
            result => panic!("unexpected worker result: {result:?}"),
        }
    }

    #[test]
    fn worker_protocol_rejects_incompatible_versions() {
        let mut request = CaptureWorkerRequest::new(1, CaptureWorkerCommand::GetStats);
        request.protocol_version += 1;
        assert!(request.validate_version().is_err());
    }

    #[test]
    fn worker_events_are_framed_as_unsolicited_protocol_messages() {
        let response = CaptureWorkerResponse {
            protocol_version: CAPTURE_WORKER_PROTOCOL_VERSION,
            request_id: 0,
            result: CaptureWorkerResult::Event {
                event: CaptureEngineEvent::AudioWarning {
                    source: AudioInputSource::SystemAudio,
                    message: "device disconnected".to_owned(),
                },
            },
        };
        let json = serde_json::to_value(response).unwrap();
        assert_eq!(json["requestId"], 0);
        assert_eq!(json["result"], "event");
        assert_eq!(json["event"]["kind"], "audioWarning");
        assert_eq!(json["event"]["source"], "systemAudio");
    }

    #[test]
    fn paused_audio_ignores_packets_but_still_handles_device_failure() {
        assert!(!should_handle_audio_callback(
            AudioCallbackKind::Packet,
            false,
            true
        ));
        assert!(should_handle_audio_callback(
            AudioCallbackKind::Failure,
            false,
            true
        ));
        assert!(!should_handle_audio_callback(
            AudioCallbackKind::Failure,
            true,
            false
        ));
    }

    #[test]
    fn output_size_requires_encoder_safe_dimensions() {
        assert_eq!(
            CaptureOutputSize::new(3840, 2160).unwrap(),
            CaptureOutputSize {
                width: 3840,
                height: 2160,
            }
        );
        assert!(CaptureOutputSize::new(1919, 1080).is_err());
    }
}
