use std::{
    path::{Path, PathBuf},
    sync::{mpsc, Arc, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use windows::{
    core::{factory, IInspectable, Interface},
    Foundation::TypedEventHandler,
    Graphics::{
        Capture::{
            Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem,
            GraphicsCaptureSession,
        },
        DirectX::{Direct3D11::IDirect3DDevice, DirectXPixelFormat},
    },
    Win32::{
        Foundation::{HMODULE, POINT},
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_HARDWARE,
            Direct3D10::ID3D10Multithread,
            Direct3D11::{
                D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
                D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_BOX,
                D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
            },
            Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC},
            Dxgi::IDXGIDevice,
            Gdi::{MonitorFromPoint, MONITOR_DEFAULTTOPRIMARY},
        },
        System::WinRT::{
            Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess},
            Graphics::Capture::IGraphicsCaptureItemInterop,
            RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED,
        },
    },
};

use frameflow_capture::{CaptureSession, PhysicalRect, RegionSelection};

use crate::{
    displays::monitor_for_display_id,
    graphics_capture_supported,
    media_foundation::{Mp4VideoConfig, Mp4Writer, VideoEncoderInfo},
    MixedAudioBlock, PlatformError,
};

const MAX_AUDIO_BLOCKS_PER_VIDEO_TICK: usize = 4;
const MEDIA_CONTROL_POLL_INTERVAL: Duration = Duration::from_millis(10);
const CROP_TEXTURE_POOL_SIZE: usize = 4;

type SharedCaptureCallback =
    Arc<Mutex<Box<dyn FnMut(Result<CapturedFrameInfo, PlatformError>) + Send>>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapturedFrameInfo {
    pub sequence: u64,
    pub timeline_frame: u64,
    pub width: u32,
    pub height: u32,
    pub system_relative_time_100ns: i64,
    pub arrival_latency_ms: u128,
}

pub struct WgcCaptureSession {
    control: mpsc::Sender<WorkerControl>,
    audio: Option<WgcAudioInput>,
    encoder: Option<VideoEncoderInfo>,
    worker: Option<JoinHandle<Result<(), PlatformError>>>,
}

#[derive(Clone)]
pub struct WgcAudioInput {
    sender: mpsc::SyncSender<MixedAudioBlock>,
}

impl WgcAudioInput {
    pub fn submit(&self, block: MixedAudioBlock) -> Result<(), PlatformError> {
        self.sender.try_send(block).map_err(|error| match error {
            mpsc::TrySendError::Full(_) => PlatformError::AudioQueueFull,
            mpsc::TrySendError::Disconnected(_) => {
                PlatformError::WorkerUnavailable("audio encoder queue is disconnected".to_owned())
            }
        })
    }

    pub fn submit_wait(
        &self,
        mut block: MixedAudioBlock,
        timeout: Duration,
    ) -> Result<(), PlatformError> {
        let deadline = Instant::now() + timeout;
        loop {
            match self.sender.try_send(block) {
                Ok(()) => return Ok(()),
                Err(mpsc::TrySendError::Full(returned)) if Instant::now() < deadline => {
                    block = returned;
                    thread::sleep(Duration::from_millis(2));
                }
                Err(mpsc::TrySendError::Full(_)) => return Err(PlatformError::AudioQueueFull),
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    return Err(PlatformError::WorkerUnavailable(
                        "audio encoder queue is disconnected".to_owned(),
                    ));
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mp4CaptureOptions {
    frames_per_second: u32,
    audio_enabled: bool,
    output_size: Option<(u32, u32)>,
}

impl Mp4CaptureOptions {
    pub fn new(frames_per_second: u32) -> Result<Self, PlatformError> {
        if !(1..=240).contains(&frames_per_second) {
            return Err(PlatformError::InvalidFrameRate(frames_per_second));
        }
        Ok(Self {
            frames_per_second,
            audio_enabled: false,
            output_size: None,
        })
    }

    pub fn frames_per_second(self) -> u32 {
        self.frames_per_second
    }

    pub fn with_audio(mut self, enabled: bool) -> Self {
        self.audio_enabled = enabled;
        self
    }

    pub fn audio_enabled(self) -> bool {
        self.audio_enabled
    }

    pub fn with_output_size(mut self, width: u32, height: u32) -> Result<Self, PlatformError> {
        if width == 0 || height == 0 || !width.is_multiple_of(2) || !height.is_multiple_of(2) {
            return Err(PlatformError::InvalidOutputSize { width, height });
        }
        self.output_size = Some((width, height));
        Ok(self)
    }

    pub fn output_size(self, source_width: u32, source_height: u32) -> (u32, u32) {
        self.output_size.unwrap_or((source_width, source_height))
    }
}

impl Default for Mp4CaptureOptions {
    fn default() -> Self {
        Self {
            frames_per_second: 30,
            audio_enabled: false,
            output_size: None,
        }
    }
}

struct Mp4Output {
    path: PathBuf,
    options: Mp4CaptureOptions,
}

impl WgcCaptureSession {
    /// Non-blocking delivery keeps WASAPI callback threads isolated from video
    /// conversion and disk I/O. A full queue is explicit so the coordinator can
    /// report degraded audio instead of stalling capture.
    pub fn submit_audio_block(&self, block: MixedAudioBlock) -> Result<(), PlatformError> {
        let audio = self.audio.as_ref().ok_or(PlatformError::AudioNotEnabled)?;
        audio.submit(block)
    }

    pub fn audio_input(&self) -> Option<WgcAudioInput> {
        self.audio.clone()
    }

    pub fn video_encoder_info(&self) -> Option<VideoEncoderInfo> {
        self.encoder.clone()
    }

    fn control(&self, action: WorkerAction) -> Result<(), PlatformError> {
        let (acknowledge, receiver) = mpsc::sync_channel(1);
        self.control
            .send(WorkerControl::Action(action, acknowledge))
            .map_err(|error| PlatformError::WorkerUnavailable(error.to_string()))?;
        receiver
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| PlatformError::WorkerControlTimeout(action.label()))
    }

    fn shutdown(&mut self) -> Result<(), PlatformError> {
        if let Some(worker) = self.worker.take() {
            let _ = self.control.send(WorkerControl::Stop);
            worker.join().map_err(|_| {
                PlatformError::WorkerUnavailable("capture worker panicked".to_owned())
            })??;
        }
        Ok(())
    }
}

impl CaptureSession for WgcCaptureSession {
    type Error = PlatformError;

    fn pause(&mut self) -> Result<(), Self::Error> {
        self.control(WorkerAction::Pause)
    }

    fn resume(&mut self) -> Result<(), Self::Error> {
        self.control(WorkerAction::Resume)
    }

    fn stop(mut self) -> Result<(), Self::Error> {
        self.shutdown()
    }
}

impl Drop for WgcCaptureSession {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

#[derive(Clone, Copy)]
enum WorkerAction {
    Pause,
    Resume,
}

impl WorkerAction {
    fn label(self) -> &'static str {
        match self {
            Self::Pause => "pause",
            Self::Resume => "resume",
        }
    }
}

enum WorkerControl {
    Action(WorkerAction, mpsc::SyncSender<()>),
    Stop,
}

struct WinRtApartment;

impl Drop for WinRtApartment {
    fn drop(&mut self) {
        unsafe { RoUninitialize() };
    }
}

struct OutputClock {
    frame_interval: Duration,
    next_deadline: Option<Instant>,
    next_timeline_frame: u64,
}

impl OutputClock {
    fn new(frames_per_second: u32) -> Self {
        Self {
            frame_interval: Duration::from_secs_f64(1.0 / f64::from(frames_per_second)),
            next_deadline: None,
            next_timeline_frame: 0,
        }
    }

    fn start(&mut self, now: Instant) {
        self.next_deadline = Some(now);
    }

    fn pause(&mut self) {
        self.next_deadline = None;
    }

    fn resume(&mut self, now: Instant) {
        self.next_deadline = Some(now);
    }

    fn wait_duration(&self, now: Instant, maximum: Duration) -> Duration {
        self.next_deadline
            .map(|deadline| deadline.saturating_duration_since(now).min(maximum))
            .unwrap_or(maximum)
    }

    #[cfg(test)]
    fn is_due(&self, now: Instant) -> bool {
        self.next_deadline.is_some_and(|deadline| now >= deadline)
    }

    fn take_due_frame(&mut self, now: Instant) -> Option<u64> {
        let deadline = self.next_deadline?;
        if now < deadline {
            return None;
        }
        let missed =
            now.duration_since(deadline).as_nanos() / self.frame_interval.as_nanos().max(1);
        let missed = u64::try_from(missed).unwrap_or(u64::MAX);
        let timeline_frame = self.next_timeline_frame.saturating_add(missed);
        self.next_timeline_frame = timeline_frame.saturating_add(1);
        let intervals = u32::try_from(missed.saturating_add(1)).unwrap_or(u32::MAX);
        self.next_deadline = Some(deadline + self.frame_interval.saturating_mul(intervals));
        Some(timeline_frame)
    }
}

#[derive(Clone)]
struct PreparedFrame {
    texture: Arc<ID3D11Texture2D>,
    width: u32,
    height: u32,
    system_relative_time_100ns: i64,
}

struct CropTexturePool {
    textures: Vec<Arc<ID3D11Texture2D>>,
    crop: PhysicalRect,
}

impl CropTexturePool {
    fn new(device: &D3dDevice, crop: PhysicalRect) -> Result<Self, PlatformError> {
        let textures = (0..CROP_TEXTURE_POOL_SIZE)
            .map(|_| create_crop_texture(device, crop).map(Arc::new))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { textures, crop })
    }

    fn copy_latest(
        &self,
        device: &D3dDevice,
        source: &ID3D11Texture2D,
    ) -> Option<Arc<ID3D11Texture2D>> {
        let destination = self
            .textures
            .iter()
            .find(|texture| Arc::strong_count(texture) == 1)?
            .clone();
        copy_crop(device, source, destination.as_ref(), self.crop);
        Some(destination)
    }
}

struct LatestValueState<T> {
    generation: u64,
    value: Option<T>,
}

struct LatestValue<T> {
    state: Mutex<LatestValueState<T>>,
}

impl<T> Default for LatestValue<T> {
    fn default() -> Self {
        Self {
            state: Mutex::new(LatestValueState {
                generation: 0,
                value: None,
            }),
        }
    }
}

impl<T: Clone> LatestValue<T> {
    fn publish(&self, value: T) -> Result<(), PlatformError> {
        let mut state = self.state.lock().map_err(|_| {
            PlatformError::WorkerUnavailable("latest video frame is unavailable".to_owned())
        })?;
        state.generation = state.generation.saturating_add(1);
        state.value = Some(value);
        Ok(())
    }

    fn latest_after(&self, generation: u64) -> Result<Option<(u64, T)>, PlatformError> {
        let state = self.state.lock().map_err(|_| {
            PlatformError::WorkerUnavailable("latest video frame is unavailable".to_owned())
        })?;
        if state.generation == generation {
            Ok(None)
        } else {
            Ok(state
                .value
                .as_ref()
                .map(|value| (state.generation, value.clone())))
        }
    }

    fn has_value(&self) -> Result<bool, PlatformError> {
        self.state
            .lock()
            .map(|state| state.value.is_some())
            .map_err(|_| {
                PlatformError::WorkerUnavailable("latest video frame is unavailable".to_owned())
            })
    }
}

struct MediaPipeline {
    latest_frame: Arc<LatestValue<PreparedFrame>>,
    control: mpsc::Sender<WorkerControl>,
    encoder: VideoEncoderInfo,
    worker: Option<JoinHandle<Result<(), PlatformError>>>,
}

impl MediaPipeline {
    fn start(
        output: Mp4Output,
        selection: &RegionSelection,
        device: &D3dDevice,
        audio: Option<mpsc::Receiver<MixedAudioBlock>>,
        callback: SharedCaptureCallback,
        started: Instant,
    ) -> Result<Self, PlatformError> {
        let source_width = selection.capture_rect.width;
        let source_height = selection.capture_rect.height;
        let (output_width, output_height) = output.options.output_size(source_width, source_height);
        let video = Mp4VideoConfig {
            source_width,
            source_height,
            output_width,
            output_height,
            frame_rate: output.options.frames_per_second(),
        };
        let latest_frame = Arc::new(LatestValue::default());
        let worker_latest_frame = Arc::clone(&latest_frame);
        let (control, control_receiver) = mpsc::channel();
        let (startup, startup_receiver) = mpsc::sync_channel(1);
        let d3d11 = device.d3d11.clone();
        let context = device.context.clone();
        let worker = thread::Builder::new()
            .name("frameflow-media".to_owned())
            .spawn(move || {
                run_media_worker(
                    output,
                    video,
                    d3d11,
                    context,
                    worker_latest_frame,
                    control_receiver,
                    audio,
                    callback,
                    started,
                    startup,
                )
            })
            .map_err(|error| PlatformError::WorkerUnavailable(error.to_string()))?;
        match startup_receiver.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(encoder)) => Ok(Self {
                latest_frame,
                control,
                encoder,
                worker: Some(worker),
            }),
            Ok(Err(message)) => {
                let _ = worker.join();
                Err(PlatformError::WorkerUnavailable(message))
            }
            Err(_) => {
                let _ = control.send(WorkerControl::Stop);
                let _ = worker.join();
                Err(PlatformError::WorkerUnavailable(
                    "media worker did not start".to_owned(),
                ))
            }
        }
    }

    fn publish(&self, frame: PreparedFrame) -> Result<(), PlatformError> {
        self.latest_frame.publish(frame)
    }

    fn control(&self, action: WorkerAction) -> Result<(), PlatformError> {
        let (acknowledge, receiver) = mpsc::sync_channel(1);
        self.control
            .send(WorkerControl::Action(action, acknowledge))
            .map_err(|error| PlatformError::WorkerUnavailable(error.to_string()))?;
        receiver
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| PlatformError::WorkerControlTimeout(action.label()))
    }

    fn is_finished(&self) -> bool {
        self.worker.as_ref().is_none_or(JoinHandle::is_finished)
    }

    fn shutdown(&mut self) -> Result<(), PlatformError> {
        if let Some(worker) = self.worker.take() {
            let _ = self.control.send(WorkerControl::Stop);
            worker.join().map_err(|_| {
                PlatformError::WorkerUnavailable("media worker panicked".to_owned())
            })??;
        }
        Ok(())
    }
}

impl Drop for MediaPipeline {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

struct D3dDevice {
    winrt: IDirect3DDevice,
    d3d11: ID3D11Device,
    context: ID3D11DeviceContext,
}

/// Captures one frame from the primary monitor and confirms that its backing
/// surface can be accessed as an ID3D11Texture2D without a CPU readback.
/// The caller must have initialized a WinRT apartment on this thread.
pub fn capture_primary_frame(timeout: Duration) -> Result<CapturedFrameInfo, PlatformError> {
    if !graphics_capture_supported()? {
        return Err(PlatformError::CaptureUnsupported);
    }

    let item = create_primary_monitor_item()?;
    capture_item_frame(item, None, timeout)
}

/// Captures one display frame and copies the selected subregion into a new
/// GPU-resident BGRA texture without mapping either texture to CPU memory.
pub fn capture_region_frame(
    selection: &RegionSelection,
    timeout: Duration,
) -> Result<CapturedFrameInfo, PlatformError> {
    if !graphics_capture_supported()? {
        return Err(PlatformError::CaptureUnsupported);
    }
    let monitor = monitor_for_display_id(&selection.display_id)?;
    let item = create_monitor_item(monitor)?;
    capture_item_frame(item, Some(selection.capture_rect), timeout)
}

/// Starts a continuous GPU-only region capture on a dedicated WinRT/D3D thread.
pub fn start_region_capture<F>(
    selection: &RegionSelection,
    callback: F,
) -> Result<WgcCaptureSession, PlatformError>
where
    F: FnMut(Result<CapturedFrameInfo, PlatformError>) + Send + 'static,
{
    start_region_capture_internal(selection, None, callback)
}

/// Starts continuous region capture and writes the cropped GPU textures to an
/// H.264 MP4 sink on the same D3D11 worker thread.
pub fn start_region_mp4_capture<F>(
    selection: &RegionSelection,
    output_path: impl AsRef<Path>,
    options: Mp4CaptureOptions,
    callback: F,
) -> Result<WgcCaptureSession, PlatformError>
where
    F: FnMut(Result<CapturedFrameInfo, PlatformError>) + Send + 'static,
{
    start_region_capture_internal(
        selection,
        Some(Mp4Output {
            path: output_path.as_ref().to_path_buf(),
            options,
        }),
        callback,
    )
}

fn start_region_capture_internal<F>(
    selection: &RegionSelection,
    output: Option<Mp4Output>,
    callback: F,
) -> Result<WgcCaptureSession, PlatformError>
where
    F: FnMut(Result<CapturedFrameInfo, PlatformError>) + Send + 'static,
{
    let selection = selection.clone();
    let (control_sender, control_receiver) = mpsc::channel();
    let (startup_sender, startup_receiver) = mpsc::sync_channel(1);
    let (audio_sender, audio_receiver) = if output
        .as_ref()
        .is_some_and(|output| output.options.audio_enabled())
    {
        let (sender, receiver) = mpsc::sync_channel(64);
        (Some(WgcAudioInput { sender }), Some(receiver))
    } else {
        (None, None)
    };
    let worker = thread::Builder::new()
        .name("frameflow-wgc".to_owned())
        .spawn(move || {
            run_capture_worker(
                selection,
                output,
                callback,
                control_receiver,
                audio_receiver,
                startup_sender,
            )
        })
        .map_err(|error| PlatformError::WorkerUnavailable(error.to_string()))?;

    match startup_receiver.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(encoder)) => Ok(WgcCaptureSession {
            control: control_sender,
            audio: audio_sender,
            encoder,
            worker: Some(worker),
        }),
        Ok(Err(message)) => {
            let _ = worker.join();
            Err(PlatformError::WorkerUnavailable(message))
        }
        Err(_) => {
            let _ = control_sender.send(WorkerControl::Stop);
            let _ = worker.join();
            Err(PlatformError::WorkerUnavailable(
                "capture worker did not start".to_owned(),
            ))
        }
    }
}

fn run_capture_worker<F>(
    selection: RegionSelection,
    output: Option<Mp4Output>,
    callback: F,
    control: mpsc::Receiver<WorkerControl>,
    audio: Option<mpsc::Receiver<MixedAudioBlock>>,
    startup: mpsc::SyncSender<Result<Option<VideoEncoderInfo>, String>>,
) -> Result<(), PlatformError>
where
    F: FnMut(Result<CapturedFrameInfo, PlatformError>) + Send + 'static,
{
    if let Err(error) = unsafe { RoInitialize(RO_INIT_MULTITHREADED) } {
        let _ = startup.send(Err(error.to_string()));
        return Err(error.into());
    }
    let _apartment = WinRtApartment;

    let setup = (|| -> Result<_, PlatformError> {
        if !graphics_capture_supported()? {
            return Err(PlatformError::CaptureUnsupported);
        }
        let monitor = monitor_for_display_id(&selection.display_id)?;
        let item = create_monitor_item(monitor)?;
        let size = item.Size()?;
        if size.Width <= 0 || size.Height <= 0 {
            return Err(PlatformError::InvalidFrameSize {
                width: size.Width,
                height: size.Height,
            });
        }
        validate_crop(
            selection.capture_rect,
            size.Width as u32,
            size.Height as u32,
        )?;
        let device = create_direct3d_device()?;
        let crop_pool = CropTexturePool::new(&device, selection.capture_rect)?;
        let frame_pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &device.winrt,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            2,
            size,
        )?;
        let session = frame_pool.CreateCaptureSession(&item)?;
        Ok((device, frame_pool, session, size, crop_pool))
    })();
    let (device, frame_pool, session, mut expected_size, crop_pool) = match setup {
        Ok(resources) => resources,
        Err(error) => {
            let _ = startup.send(Err(error.to_string()));
            return Err(error);
        }
    };

    let started = Instant::now();
    let callback: SharedCaptureCallback = Arc::new(Mutex::new(Box::new(callback)));
    let (mut media, encoder) = match output {
        Some(output) => match MediaPipeline::start(
            output,
            &selection,
            &device,
            audio,
            Arc::clone(&callback),
            started,
        ) {
            Ok(media) => {
                let encoder = media.encoder.clone();
                (Some(media), Some(encoder))
            }
            Err(error) => {
                let _ = startup.send(Err(error.to_string()));
                return Err(error);
            }
        },
        None => (None, None),
    };

    let (frame_sender, frame_receiver) = mpsc::sync_channel(2);
    let handler = TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new(
        move |_sender, _args| {
            let _ = frame_sender.try_send(());
            Ok(())
        },
    );
    let token = frame_pool.FrameArrived(&handler)?;
    if let Err(error) = session.StartCapture() {
        let _ = startup.send(Err(error.to_string()));
        return Err(error.into());
    }
    let _ = startup.send(Ok(encoder));

    let mut paused = false;
    let mut sequence = 0;
    let mut stopping = false;
    while !stopping {
        while let Ok(command) = control.try_recv() {
            match command {
                WorkerControl::Action(action, acknowledge) => {
                    if let Some(media) = media.as_ref() {
                        media.control(action)?;
                    }
                    paused = matches!(action, WorkerAction::Pause);
                    let _ = acknowledge.send(());
                }
                WorkerControl::Stop => stopping = true,
            }
        }
        if stopping {
            break;
        }
        if media.as_ref().is_some_and(MediaPipeline::is_finished) {
            break;
        }

        match frame_receiver.recv_timeout(Duration::from_millis(20)) {
            Ok(()) => {
                let frame = match frame_pool.TryGetNextFrame() {
                    Ok(frame) => frame,
                    Err(error) => {
                        emit_capture_callback(&callback, Err(error.into()));
                        continue;
                    }
                };
                let content_size = frame.ContentSize()?;
                if content_size.Width <= 0 || content_size.Height <= 0 {
                    emit_capture_callback(
                        &callback,
                        Err(PlatformError::InvalidFrameSize {
                            width: content_size.Width,
                            height: content_size.Height,
                        }),
                    );
                    continue;
                }
                if content_size != expected_size {
                    drop(frame);
                    if let Err(error) = validate_crop(
                        selection.capture_rect,
                        content_size.Width as u32,
                        content_size.Height as u32,
                    ) {
                        emit_capture_callback(&callback, Err(error));
                        break;
                    }
                    frame_pool.Recreate(
                        &device.winrt,
                        DirectXPixelFormat::B8G8R8A8UIntNormalized,
                        2,
                        content_size,
                    )?;
                    expected_size = content_size;
                    continue;
                }
                let Some(prepared) = prepare_frame(
                    &frame,
                    &device,
                    Some(selection.capture_rect),
                    Some(&crop_pool),
                )?
                else {
                    continue;
                };
                if let Some(media) = media.as_ref() {
                    media.publish(prepared)?;
                } else if !paused {
                    sequence += 1;
                    emit_capture_callback(
                        &callback,
                        Ok(frame_info(
                            &prepared,
                            started,
                            sequence,
                            sequence.saturating_sub(1),
                        )),
                    );
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    frame_pool.RemoveFrameArrived(token)?;
    session.Close()?;
    frame_pool.Close()?;
    if let Some(media) = media.as_mut() {
        media.shutdown()?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_media_worker(
    output: Mp4Output,
    video: Mp4VideoConfig,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    latest_frame: Arc<LatestValue<PreparedFrame>>,
    control: mpsc::Receiver<WorkerControl>,
    audio: Option<mpsc::Receiver<MixedAudioBlock>>,
    callback: SharedCaptureCallback,
    started: Instant,
    startup: mpsc::SyncSender<Result<VideoEncoderInfo, String>>,
) -> Result<(), PlatformError> {
    if let Err(error) = unsafe { RoInitialize(RO_INIT_MULTITHREADED) } {
        let _ = startup.send(Err(error.to_string()));
        return Err(error.into());
    }
    let _apartment = WinRtApartment;
    let writer = if output.options.audio_enabled() {
        Mp4Writer::new_with_audio(&output.path, &device, &context, video)
    } else {
        Mp4Writer::new(&output.path, &device, &context, video)
    };
    let mut writer = match writer {
        Ok(writer) => writer,
        Err(error) => {
            let _ = startup.send(Err(error.to_string()));
            return Err(error);
        }
    };
    let _ = startup.send(Ok(writer.encoder_info()));

    let recording_result = run_media_loop(
        &mut writer,
        video.frame_rate,
        &latest_frame,
        &control,
        audio.as_ref(),
        &callback,
        started,
    );
    if let Err(error) = &recording_result {
        emit_capture_callback(
            &callback,
            Err(PlatformError::MediaFoundation(error.to_string())),
        );
    }
    let finalize_result = writer.finalize();
    recording_result.and(finalize_result)
}

fn run_media_loop(
    writer: &mut Mp4Writer,
    frame_rate: u32,
    latest_frame: &LatestValue<PreparedFrame>,
    control: &mpsc::Receiver<WorkerControl>,
    audio: Option<&mpsc::Receiver<MixedAudioBlock>>,
    callback: &SharedCaptureCallback,
    started: Instant,
) -> Result<(), PlatformError> {
    let mut output_clock = OutputClock::new(frame_rate);
    let mut paused = false;
    let mut stopping = false;
    let mut generation = 0;
    let mut current_frame = None;
    let mut sequence = 0;

    while !stopping {
        let wait = if !paused && current_frame.is_some() {
            output_clock.wait_duration(Instant::now(), MEDIA_CONTROL_POLL_INTERVAL)
        } else {
            MEDIA_CONTROL_POLL_INTERVAL
        };
        match control.recv_timeout(wait) {
            Ok(WorkerControl::Action(action, acknowledge)) => {
                paused = matches!(action, WorkerAction::Pause);
                match action {
                    WorkerAction::Pause => output_clock.pause(),
                    WorkerAction::Resume if latest_frame.has_value()? => {
                        output_clock.resume(Instant::now())
                    }
                    WorkerAction::Resume => {}
                }
                let _ = acknowledge.send(());
            }
            Ok(WorkerControl::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                stopping = true;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if stopping {
            break;
        }

        if let Some((next_generation, frame)) = latest_frame.latest_after(generation)? {
            generation = next_generation;
            current_frame = Some(frame);
            if !paused && output_clock.next_deadline.is_none() {
                output_clock.start(Instant::now());
            }
        }
        if let Some(audio) = audio {
            drain_audio_blocks(writer, audio, Some(MAX_AUDIO_BLOCKS_PER_VIDEO_TICK))?;
        }
        if !paused {
            if let (Some(frame), Some(timeline_frame)) = (
                current_frame.as_ref(),
                output_clock.take_due_frame(Instant::now()),
            ) {
                sequence += 1;
                writer.write_texture(frame.texture.as_ref(), timeline_frame)?;
                emit_capture_callback(
                    callback,
                    Ok(frame_info(frame, started, sequence, timeline_frame)),
                );
            }
        }
    }

    if !paused {
        if let (Some(frame), Some(timeline_frame)) = (
            current_frame.as_ref(),
            output_clock.take_due_frame(Instant::now()),
        ) {
            sequence += 1;
            writer.write_texture(frame.texture.as_ref(), timeline_frame)?;
            emit_capture_callback(
                callback,
                Ok(frame_info(frame, started, sequence, timeline_frame)),
            );
        }
    }
    if let Some(audio) = audio {
        drain_audio_blocks(writer, audio, None)?;
    }
    Ok(())
}

fn emit_capture_callback(
    callback: &SharedCaptureCallback,
    result: Result<CapturedFrameInfo, PlatformError>,
) {
    if let Ok(mut callback) = callback.lock() {
        callback(result);
    }
}

fn drain_audio_blocks(
    writer: &mut Mp4Writer,
    receiver: &mpsc::Receiver<MixedAudioBlock>,
    maximum_blocks: Option<usize>,
) -> Result<(), PlatformError> {
    match maximum_blocks {
        Some(maximum_blocks) => {
            for _ in 0..maximum_blocks {
                let Ok(block) = receiver.try_recv() else {
                    break;
                };
                writer.write_audio_block(&block)?;
            }
        }
        None => {
            while let Ok(block) = receiver.try_recv() {
                writer.write_audio_block(&block)?;
            }
        }
    }
    Ok(())
}

fn capture_item_frame(
    item: GraphicsCaptureItem,
    crop: Option<PhysicalRect>,
    timeout: Duration,
) -> Result<CapturedFrameInfo, PlatformError> {
    let size = item.Size()?;
    if size.Width <= 0 || size.Height <= 0 {
        return Err(PlatformError::InvalidFrameSize {
            width: size.Width,
            height: size.Height,
        });
    }

    if let Some(crop) = crop {
        validate_crop(crop, size.Width as u32, size.Height as u32)?;
    }

    let device = create_direct3d_device()?;
    let frame_pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
        &device.winrt,
        DirectXPixelFormat::B8G8R8A8UIntNormalized,
        2,
        size,
    )?;
    let session = frame_pool.CreateCaptureSession(&item)?;

    let (sender, receiver) = mpsc::sync_channel(1);
    let handler =
        TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new(move |_pool, _args| {
            let _ = sender.try_send(());
            Ok(())
        });
    let token = frame_pool.FrameArrived(&handler)?;
    let started = Instant::now();
    session.StartCapture()?;

    let result = match receiver.recv_timeout(timeout) {
        Ok(()) => inspect_next_frame(&frame_pool, &device, crop, started),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            Err(PlatformError::FrameTimeout(timeout.as_millis() as u64))
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err(PlatformError::FrameTimeout(timeout.as_millis() as u64))
        }
    };

    let _ = frame_pool.RemoveFrameArrived(token);
    close_capture(&session, &frame_pool);
    result
}

fn create_primary_monitor_item() -> Result<GraphicsCaptureItem, PlatformError> {
    let monitor = unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) };
    create_monitor_item(monitor)
}

fn create_monitor_item(
    monitor: windows::Win32::Graphics::Gdi::HMONITOR,
) -> Result<GraphicsCaptureItem, PlatformError> {
    let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
    Ok(unsafe { interop.CreateForMonitor(monitor)? })
}

fn create_direct3d_device() -> Result<D3dDevice, PlatformError> {
    let mut d3d11_device: Option<ID3D11Device> = None;
    let mut context: Option<ID3D11DeviceContext> = None;
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut d3d11_device),
            None,
            Some(&mut context),
        )?;
    }

    let d3d11_device = d3d11_device.ok_or_else(windows::core::Error::from_win32)?;
    let context = context.ok_or_else(windows::core::Error::from_win32)?;
    let multithread: ID3D10Multithread = d3d11_device.cast()?;
    let _ = unsafe { multithread.SetMultithreadProtected(true) };
    let dxgi_device: IDXGIDevice = d3d11_device.cast()?;
    let inspectable = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi_device)? };
    Ok(D3dDevice {
        winrt: inspectable.cast()?,
        d3d11: d3d11_device,
        context,
    })
}

fn inspect_next_frame(
    frame_pool: &Direct3D11CaptureFramePool,
    device: &D3dDevice,
    crop: Option<PhysicalRect>,
    started: Instant,
) -> Result<CapturedFrameInfo, PlatformError> {
    let frame = frame_pool.TryGetNextFrame()?;
    inspect_frame(&frame, device, crop, started, 1, None)
}

fn inspect_frame(
    frame: &Direct3D11CaptureFrame,
    device: &D3dDevice,
    crop: Option<PhysicalRect>,
    started: Instant,
    sequence: u64,
    writer: Option<&mut Mp4Writer>,
) -> Result<CapturedFrameInfo, PlatformError> {
    let prepared = prepare_frame(frame, device, crop, None)?.ok_or_else(|| {
        PlatformError::WorkerUnavailable("capture texture pool is exhausted".to_owned())
    })?;
    if let Some(writer) = writer {
        writer.write_texture(prepared.texture.as_ref(), sequence.saturating_sub(1))?;
    }
    Ok(frame_info(
        &prepared,
        started,
        sequence,
        sequence.saturating_sub(1),
    ))
}

fn prepare_frame(
    frame: &Direct3D11CaptureFrame,
    device: &D3dDevice,
    crop: Option<PhysicalRect>,
    crop_pool: Option<&CropTexturePool>,
) -> Result<Option<PreparedFrame>, PlatformError> {
    let size = frame.ContentSize()?;
    if size.Width <= 0 || size.Height <= 0 {
        return Err(PlatformError::InvalidFrameSize {
            width: size.Width,
            height: size.Height,
        });
    }

    let surface = frame.Surface()?;
    let access: IDirect3DDxgiInterfaceAccess = surface.cast()?;
    let source_texture: ID3D11Texture2D = unsafe { access.GetInterface()? };
    let (width, height, texture) = if let Some(crop) = crop {
        validate_crop(crop, size.Width as u32, size.Height as u32)?;
        let cropped = match crop_pool {
            Some(pool) => pool.copy_latest(device, &source_texture),
            None => Some(Arc::new(crop_texture(device, &source_texture, crop)?)),
        };
        let Some(cropped) = cropped else {
            return Ok(None);
        };
        (crop.width, crop.height, cropped)
    } else {
        (
            size.Width as u32,
            size.Height as u32,
            Arc::new(source_texture),
        )
    };
    Ok(Some(PreparedFrame {
        texture,
        width,
        height,
        system_relative_time_100ns: frame.SystemRelativeTime()?.Duration,
    }))
}

fn frame_info(
    frame: &PreparedFrame,
    started: Instant,
    sequence: u64,
    timeline_frame: u64,
) -> CapturedFrameInfo {
    CapturedFrameInfo {
        sequence,
        timeline_frame,
        width: frame.width,
        height: frame.height,
        system_relative_time_100ns: frame.system_relative_time_100ns,
        arrival_latency_ms: started.elapsed().as_millis(),
    }
}

fn validate_crop(
    crop: PhysicalRect,
    frame_width: u32,
    frame_height: u32,
) -> Result<(), PlatformError> {
    let right = u32::try_from(crop.x)
        .ok()
        .and_then(|x| x.checked_add(crop.width));
    let bottom = u32::try_from(crop.y)
        .ok()
        .and_then(|y| y.checked_add(crop.height));
    if !crop.is_valid()
        || right.is_none_or(|right| right > frame_width)
        || bottom.is_none_or(|bottom| bottom > frame_height)
    {
        return Err(PlatformError::InvalidCrop {
            x: crop.x,
            y: crop.y,
            width: crop.width,
            height: crop.height,
            frame_width,
            frame_height,
        });
    }
    Ok(())
}

fn crop_texture(
    device: &D3dDevice,
    source: &ID3D11Texture2D,
    crop: PhysicalRect,
) -> Result<ID3D11Texture2D, PlatformError> {
    let destination = create_crop_texture(device, crop)?;
    copy_crop(device, source, &destination, crop);
    Ok(destination)
}

fn create_crop_texture(
    device: &D3dDevice,
    crop: PhysicalRect,
) -> Result<ID3D11Texture2D, PlatformError> {
    let description = D3D11_TEXTURE2D_DESC {
        Width: crop.width,
        Height: crop.height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let mut destination = None;
    unsafe {
        device
            .d3d11
            .CreateTexture2D(&description, None, Some(&mut destination))?;
    }
    destination.ok_or_else(|| windows::core::Error::from_win32().into())
}

fn copy_crop(
    device: &D3dDevice,
    source: &ID3D11Texture2D,
    destination: &ID3D11Texture2D,
    crop: PhysicalRect,
) {
    let source_box = D3D11_BOX {
        left: crop.x as u32,
        top: crop.y as u32,
        front: 0,
        right: crop.x as u32 + crop.width,
        bottom: crop.y as u32 + crop.height,
        back: 1,
    };
    unsafe {
        device
            .context
            .CopySubresourceRegion(destination, 0, 0, 0, 0, source, 0, Some(&source_box));
        device.context.Flush();
    }
}

fn close_capture(session: &GraphicsCaptureSession, frame_pool: &Direct3D11CaptureFramePool) {
    let _ = session.Close();
    let _ = frame_pool.Close();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_crop_inside_frame() {
        validate_crop(
            PhysicalRect {
                x: 100,
                y: 50,
                width: 1280,
                height: 720,
            },
            3840,
            2160,
        )
        .unwrap();
    }

    #[test]
    fn rejects_negative_or_overflowing_crop() {
        for crop in [
            PhysicalRect {
                x: -1,
                y: 0,
                width: 10,
                height: 10,
            },
            PhysicalRect {
                x: 3800,
                y: 0,
                width: 100,
                height: 100,
            },
        ] {
            assert!(matches!(
                validate_crop(crop, 3840, 2160),
                Err(PlatformError::InvalidCrop { .. })
            ));
        }
    }

    #[test]
    fn output_clock_is_independent_of_capture_timestamp_rounding() {
        let started = Instant::now();
        let mut clock = OutputClock::new(30);
        clock.start(started);
        assert_eq!(clock.take_due_frame(started), Some(0));
        assert!(!clock.is_due(started + Duration::from_micros(33_332)));
        assert!(clock.is_due(started + Duration::from_micros(33_334)));
    }

    #[test]
    fn output_clock_skips_timeline_frames_when_encoding_falls_behind() {
        let started = Instant::now();
        let mut clock = OutputClock::new(60);
        clock.start(started);

        assert_eq!(clock.take_due_frame(started), Some(0));
        assert_eq!(
            clock.take_due_frame(started + Duration::from_millis(105)),
            Some(6)
        );
        assert_eq!(
            clock.take_due_frame(started + Duration::from_millis(205)),
            Some(12)
        );
    }

    #[test]
    fn latest_value_mailbox_overwrites_backlog_instead_of_blocking_capture() {
        let mailbox = LatestValue::default();
        mailbox.publish(10_u32).unwrap();
        mailbox.publish(20_u32).unwrap();
        mailbox.publish(30_u32).unwrap();

        assert_eq!(mailbox.latest_after(0).unwrap(), Some((3, 30)));
        assert_eq!(mailbox.latest_after(3).unwrap(), None);
    }

    #[test]
    fn output_clock_restarts_immediately_after_pause() {
        let started = Instant::now();
        let mut clock = OutputClock::new(30);
        clock.start(started);
        clock.pause();
        assert!(!clock.is_due(started + Duration::from_secs(10)));
        let resumed = started + Duration::from_secs(10);
        clock.resume(resumed);
        assert!(clock.is_due(resumed));
    }

    #[test]
    fn mp4_options_validate_the_public_frame_rate_boundary() {
        for fps in [24, 30, 60] {
            let options = Mp4CaptureOptions::new(fps).unwrap();
            assert_eq!(options.frames_per_second(), fps);
            assert!(!options.audio_enabled());
            assert!(options.with_audio(true).audio_enabled());
        }
        assert!(matches!(
            Mp4CaptureOptions::new(0),
            Err(PlatformError::InvalidFrameRate(0))
        ));
        assert!(matches!(
            Mp4CaptureOptions::new(241),
            Err(PlatformError::InvalidFrameRate(241))
        ));
    }

    #[test]
    fn mp4_options_require_encoder_safe_output_dimensions() {
        let options = Mp4CaptureOptions::new(60)
            .unwrap()
            .with_output_size(2194, 1234)
            .unwrap();
        assert_eq!(options.output_size(3840, 2160), (2194, 1234));
        assert!(matches!(
            Mp4CaptureOptions::new(60)
                .unwrap()
                .with_output_size(2195, 1234),
            Err(PlatformError::InvalidOutputSize { .. })
        ));
    }

    #[test]
    fn output_clock_uses_the_configured_frame_rate() {
        let started = Instant::now();
        let mut clock = OutputClock::new(60);
        clock.start(started);
        assert_eq!(clock.take_due_frame(started), Some(0));
        assert!(!clock.is_due(started + Duration::from_micros(16_665)));
        assert!(clock.is_due(started + Duration::from_micros(16_667)));
    }
}
