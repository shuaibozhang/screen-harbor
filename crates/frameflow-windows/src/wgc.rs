use std::{
    path::{Path, PathBuf},
    sync::mpsc,
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
    displays::monitor_for_display_id, graphics_capture_supported, media_foundation::Mp4Writer,
    PlatformError,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapturedFrameInfo {
    pub sequence: u64,
    pub width: u32,
    pub height: u32,
    pub system_relative_time_100ns: i64,
    pub arrival_latency_ms: u128,
}

pub struct WgcCaptureSession {
    control: mpsc::Sender<WorkerControl>,
    worker: Option<JoinHandle<Result<(), PlatformError>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mp4CaptureOptions {
    frames_per_second: u32,
}

impl Mp4CaptureOptions {
    pub fn new(frames_per_second: u32) -> Result<Self, PlatformError> {
        if !(1..=240).contains(&frames_per_second) {
            return Err(PlatformError::InvalidFrameRate(frames_per_second));
        }
        Ok(Self { frames_per_second })
    }

    pub fn frames_per_second(self) -> u32 {
        self.frames_per_second
    }
}

impl Default for Mp4CaptureOptions {
    fn default() -> Self {
        Self {
            frames_per_second: 30,
        }
    }
}

struct Mp4Output {
    path: PathBuf,
    options: Mp4CaptureOptions,
}

impl WgcCaptureSession {
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
}

impl OutputClock {
    fn new(frames_per_second: u32) -> Self {
        Self {
            frame_interval: Duration::from_secs_f64(1.0 / f64::from(frames_per_second)),
            next_deadline: None,
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

    fn is_due(&self, now: Instant) -> bool {
        self.next_deadline.is_some_and(|deadline| now >= deadline)
    }

    fn advance(&mut self) {
        if let Some(deadline) = self.next_deadline {
            self.next_deadline = Some(deadline + self.frame_interval);
        }
    }
}

struct PreparedFrame {
    texture: ID3D11Texture2D,
    width: u32,
    height: u32,
    system_relative_time_100ns: i64,
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
    let worker = thread::Builder::new()
        .name("frameflow-wgc".to_owned())
        .spawn(move || {
            run_capture_worker(
                selection,
                output,
                callback,
                control_receiver,
                startup_sender,
            )
        })
        .map_err(|error| PlatformError::WorkerUnavailable(error.to_string()))?;

    match startup_receiver.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(())) => Ok(WgcCaptureSession {
            control: control_sender,
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
    mut callback: F,
    control: mpsc::Receiver<WorkerControl>,
    startup: mpsc::SyncSender<Result<(), String>>,
) -> Result<(), PlatformError>
where
    F: FnMut(Result<CapturedFrameInfo, PlatformError>),
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
        let frame_pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &device.winrt,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            2,
            size,
        )?;
        let session = frame_pool.CreateCaptureSession(&item)?;
        let writer = output
            .as_ref()
            .map(|output| {
                Mp4Writer::new(
                    &output.path,
                    &device.d3d11,
                    &device.context,
                    selection.capture_rect.width,
                    selection.capture_rect.height,
                    output.options.frames_per_second(),
                )
            })
            .transpose()?;
        Ok((device, frame_pool, session, size, writer))
    })();
    let (device, frame_pool, session, mut expected_size, mut writer) = match setup {
        Ok(resources) => resources,
        Err(error) => {
            let _ = startup.send(Err(error.to_string()));
            return Err(error);
        }
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
    let _ = startup.send(Ok(()));

    let started = Instant::now();
    let mut paused = false;
    let mut sequence = 0;
    let output_fps = output
        .as_ref()
        .map(|output| output.options.frames_per_second())
        .unwrap_or(30);
    let mut output_clock = OutputClock::new(output_fps);
    let mut latest_frame: Option<PreparedFrame> = None;
    let mut stopping = false;
    let mut fatal_media_error = None;
    while !stopping {
        while let Ok(command) = control.try_recv() {
            match command {
                WorkerControl::Action(action, acknowledge) => {
                    paused = matches!(action, WorkerAction::Pause);
                    match action {
                        WorkerAction::Pause => output_clock.pause(),
                        WorkerAction::Resume if latest_frame.is_some() => {
                            output_clock.resume(Instant::now())
                        }
                        WorkerAction::Resume => {}
                    }
                    let _ = acknowledge.send(());
                }
                WorkerControl::Stop => stopping = true,
            }
        }
        if stopping {
            break;
        }

        let wait = if writer.is_some() && !paused && latest_frame.is_some() {
            output_clock.wait_duration(Instant::now(), Duration::from_millis(20))
        } else {
            Duration::from_millis(20)
        };
        match frame_receiver.recv_timeout(wait) {
            Ok(()) => {
                let frame = match frame_pool.TryGetNextFrame() {
                    Ok(frame) => frame,
                    Err(error) => {
                        callback(Err(error.into()));
                        continue;
                    }
                };
                let content_size = frame.ContentSize()?;
                if content_size.Width <= 0 || content_size.Height <= 0 {
                    callback(Err(PlatformError::InvalidFrameSize {
                        width: content_size.Width,
                        height: content_size.Height,
                    }));
                    continue;
                }
                if content_size != expected_size {
                    drop(frame);
                    if let Err(error) = validate_crop(
                        selection.capture_rect,
                        content_size.Width as u32,
                        content_size.Height as u32,
                    ) {
                        callback(Err(error));
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
                let prepared = prepare_frame(&frame, &device, Some(selection.capture_rect))?;
                if writer.is_some() {
                    latest_frame = Some(prepared);
                    if !paused && output_clock.next_deadline.is_none() {
                        output_clock.start(Instant::now());
                    }
                } else if !paused {
                    sequence += 1;
                    callback(Ok(frame_info(&prepared, started, sequence)));
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        if !paused && output_clock.is_due(Instant::now()) {
            if let (Some(writer), Some(frame)) = (writer.as_mut(), latest_frame.as_ref()) {
                sequence += 1;
                match writer.write_texture(&frame.texture) {
                    Ok(()) => callback(Ok(frame_info(frame, started, sequence))),
                    Err(error) => {
                        let message = error.to_string();
                        callback(Err(PlatformError::MediaFoundation(message.clone())));
                        fatal_media_error = Some(message);
                        break;
                    }
                }
                output_clock.advance();
            }
        }
    }

    frame_pool.RemoveFrameArrived(token)?;
    session.Close()?;
    frame_pool.Close()?;
    if let Some(message) = fatal_media_error {
        return Err(PlatformError::MediaFoundation(message));
    }
    if let Some(writer) = writer {
        writer.finalize()?;
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
    let prepared = prepare_frame(frame, device, crop)?;
    if let Some(writer) = writer {
        writer.write_texture(&prepared.texture)?;
    }
    Ok(frame_info(&prepared, started, sequence))
}

fn prepare_frame(
    frame: &Direct3D11CaptureFrame,
    device: &D3dDevice,
    crop: Option<PhysicalRect>,
) -> Result<PreparedFrame, PlatformError> {
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
        let cropped = crop_texture(device, &source_texture, crop)?;
        let mut description = D3D11_TEXTURE2D_DESC::default();
        unsafe { cropped.GetDesc(&mut description) };
        (description.Width, description.Height, cropped)
    } else {
        (size.Width as u32, size.Height as u32, source_texture)
    };
    Ok(PreparedFrame {
        texture,
        width,
        height,
        system_relative_time_100ns: frame.SystemRelativeTime()?.Duration,
    })
}

fn frame_info(frame: &PreparedFrame, started: Instant, sequence: u64) -> CapturedFrameInfo {
    CapturedFrameInfo {
        sequence,
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
    let destination = destination.ok_or_else(windows::core::Error::from_win32)?;
    let source_box = D3D11_BOX {
        left: crop.x as u32,
        top: crop.y as u32,
        front: 0,
        right: crop.x as u32 + crop.width,
        bottom: crop.y as u32 + crop.height,
        back: 1,
    };
    unsafe {
        device.context.CopySubresourceRegion(
            &destination,
            0,
            0,
            0,
            0,
            source,
            0,
            Some(&source_box),
        );
        device.context.Flush();
    }
    Ok(destination)
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
        assert!(clock.is_due(started));
        clock.advance();
        assert!(!clock.is_due(started + Duration::from_micros(33_332)));
        assert!(clock.is_due(started + Duration::from_micros(33_334)));
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
            assert_eq!(
                Mp4CaptureOptions::new(fps).unwrap().frames_per_second(),
                fps
            );
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
    fn output_clock_uses_the_configured_frame_rate() {
        let started = Instant::now();
        let mut clock = OutputClock::new(60);
        clock.start(started);
        clock.advance();
        assert!(!clock.is_due(started + Duration::from_micros(16_665)));
        assert!(clock.is_due(started + Duration::from_micros(16_667)));
    }
}
