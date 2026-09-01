use std::{
    env,
    ffi::OsString,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use serde::{Deserialize, Serialize};

const MEDIA_WORKER_PROTOCOL_VERSION: u16 = 3;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewFormat {
    Mp4,
    Webm,
}

impl PreviewFormat {
    fn extension(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Webm => "webm",
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewJob {
    pub input_path: PathBuf,
    pub temporary_path: PathBuf,
    pub output_path: PathBuf,
    pub format: PreviewFormat,
    pub maximum_width: u32,
    pub maximum_height: u32,
    pub frames_per_second: u16,
    pub include_audio: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoverJob {
    pub input_path: PathBuf,
    pub preview: CoverOutput,
    pub high_resolution: CoverOutput,
    pub position_ms: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoverOutput {
    pub temporary_path: PathBuf,
    pub output_path: PathBuf,
    pub maximum_width: u32,
    pub maximum_height: u32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "job", rename_all = "snake_case")]
pub enum MediaJob {
    Preview(PreviewJob),
    Cover(CoverJob),
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct MediaWorkerRequest {
    protocol_version: u16,
    job: MediaJob,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct MediaWorkerResponse {
    protocol_version: u16,
    success: bool,
    error: Option<String>,
}

pub struct MediaWorkerClient;

impl MediaWorkerClient {
    pub fn run(job: MediaJob) -> Result<(), String> {
        let executable = env::current_exe()
            .map_err(|error| format!("failed to locate media worker executable: {error}"))?;
        let mut command = Command::new(executable);
        command
            .arg("--media-worker")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        hide_console_window(&mut command);
        let mut child = command
            .spawn()
            .map_err(|error| format!("failed to start media worker: {error}"))?;
        let request = MediaWorkerRequest {
            protocol_version: MEDIA_WORKER_PROTOCOL_VERSION,
            job,
        };
        let request_json = serde_json::to_vec(&request).map_err(|error| error.to_string())?;
        child
            .stdin
            .take()
            .ok_or_else(|| "media worker input pipe is unavailable".to_owned())?
            .write_all(&request_json)
            .map_err(|error| format!("failed to send media worker request: {error}"))?;
        let output = child
            .wait_with_output()
            .map_err(|error| format!("failed to wait for media worker: {error}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            return Err(if stderr.is_empty() {
                format!("media worker exited with {}", output.status)
            } else {
                format!("media worker exited with {}: {stderr}", output.status)
            });
        }
        let response: MediaWorkerResponse = serde_json::from_slice(&output.stdout)
            .map_err(|error| format!("media worker returned invalid JSON: {error}"))?;
        if response.protocol_version != MEDIA_WORKER_PROTOCOL_VERSION {
            return Err(format!(
                "media worker protocol {} is unsupported",
                response.protocol_version
            ));
        }
        if response.success {
            Ok(())
        } else {
            Err(response
                .error
                .unwrap_or_else(|| "media worker failed without an error message".to_owned()))
        }
    }
}

pub fn run_media_worker_stdio() -> Result<(), String> {
    let mut input = Vec::new();
    std::io::stdin()
        .read_to_end(&mut input)
        .map_err(|error| format!("failed to read media worker request: {error}"))?;
    let request: MediaWorkerRequest =
        serde_json::from_slice(&input).map_err(|error| format!("invalid media job: {error}"))?;
    let result = if request.protocol_version != MEDIA_WORKER_PROTOCOL_VERSION {
        Err(format!(
            "media worker protocol {} is unsupported; expected {}",
            request.protocol_version, MEDIA_WORKER_PROTOCOL_VERSION
        ))
    } else {
        run_media_job(&request.job)
    };
    let response = MediaWorkerResponse {
        protocol_version: MEDIA_WORKER_PROTOCOL_VERSION,
        success: result.is_ok(),
        error: result.err(),
    };
    serde_json::to_writer(std::io::stdout().lock(), &response)
        .map_err(|error| format!("failed to write media worker response: {error}"))
}

pub fn run_media_job(job: &MediaJob) -> Result<(), String> {
    match job {
        MediaJob::Preview(job) => run_preview_job(job),
        MediaJob::Cover(job) => run_cover_job(job),
    }
}

pub fn run_preview_job(job: &PreviewJob) -> Result<(), String> {
    validate_preview_job(job)?;
    let ffmpeg = resolve_ffmpeg()?;
    if job.temporary_path.exists() {
        fs::remove_file(&job.temporary_path)
            .map_err(|error| format!("failed to remove stale preview: {error}"))?;
    }
    let mut command = Command::new(ffmpeg);
    command.args(ffmpeg_arguments(job));
    hide_console_window(&mut command);
    let output = command
        .output()
        .map_err(|error| format!("failed to start FFmpeg preview encoder: {error}"))?;
    if !output.status.success() {
        let _ = fs::remove_file(&job.temporary_path);
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(if stderr.is_empty() {
            format!("FFmpeg preview encoder exited with {}", output.status)
        } else {
            format!("FFmpeg preview encoder failed: {stderr}")
        });
    }
    let metadata = fs::metadata(&job.temporary_path)
        .map_err(|error| format!("FFmpeg did not create a preview file: {error}"))?;
    if metadata.len() == 0 {
        let _ = fs::remove_file(&job.temporary_path);
        return Err("FFmpeg created an empty preview file".to_owned());
    }
    publish_file(&job.temporary_path, &job.output_path)
        .map_err(|error| format!("failed to publish preview file: {error}"))
}

pub fn run_cover_job(job: &CoverJob) -> Result<(), String> {
    validate_cover_job(job)?;
    let ffmpeg = resolve_ffmpeg()?;
    for output in [&job.preview, &job.high_resolution] {
        if output.temporary_path.exists() {
            fs::remove_file(&output.temporary_path)
                .map_err(|error| format!("failed to remove stale cover: {error}"))?;
        }
    }
    for cover in [&job.preview, &job.high_resolution] {
        if let Err(error) = run_cover_encoder(&ffmpeg, job, cover) {
            let _ = fs::remove_file(&job.preview.temporary_path);
            let _ = fs::remove_file(&job.high_resolution.temporary_path);
            return Err(error);
        }
    }
    publish_file(
        &job.high_resolution.temporary_path,
        &job.high_resolution.output_path,
    )
    .map_err(|error| format!("failed to publish high-resolution cover: {error}"))?;
    publish_file(&job.preview.temporary_path, &job.preview.output_path)
        .map_err(|error| format!("failed to publish preview cover: {error}"))
}

fn run_cover_encoder(ffmpeg: &Path, job: &CoverJob, cover: &CoverOutput) -> Result<(), String> {
    let mut command = Command::new(ffmpeg);
    command.args(cover_ffmpeg_arguments(job, cover));
    hide_console_window(&mut command);
    let output = command
        .output()
        .map_err(|error| format!("failed to start FFmpeg cover generator: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(if stderr.is_empty() {
            format!("FFmpeg cover generator exited with {}", output.status)
        } else {
            format!("FFmpeg cover generator failed: {stderr}")
        });
    }
    let metadata = fs::metadata(&cover.temporary_path)
        .map_err(|error| format!("FFmpeg did not create a WebP cover: {error}"))?;
    if metadata.len() == 0 {
        return Err("FFmpeg created an empty WebP cover".to_owned());
    }
    Ok(())
}

#[cfg(windows)]
fn publish_file(source: &Path, destination: &Path) -> Result<(), String> {
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
    let moved = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        Err(std::io::Error::last_os_error().to_string())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn publish_file(source: &Path, destination: &Path) -> Result<(), String> {
    fs::rename(source, destination).map_err(|error| error.to_string())
}

fn validate_preview_job(job: &PreviewJob) -> Result<(), String> {
    if !job.input_path.is_file() {
        return Err("preview input is missing".to_owned());
    }
    if job.maximum_width < 2
        || job.maximum_height < 2
        || job.frames_per_second == 0
        || job.frames_per_second > 60
    {
        return Err("preview profile is invalid".to_owned());
    }
    let input_parent = job
        .input_path
        .parent()
        .ok_or_else(|| "preview input has no recording group".to_owned())?
        .canonicalize()
        .map_err(|error| error.to_string())?;
    for path in [&job.temporary_path, &job.output_path] {
        let parent = path
            .parent()
            .ok_or_else(|| "preview output has no recording group".to_owned())?
            .canonicalize()
            .map_err(|error| error.to_string())?;
        if parent != input_parent {
            return Err("preview files must stay inside the recording group".to_owned());
        }
    }
    let output_name = format!("preview.{}", job.format.extension());
    let temporary_name = format!("{output_name}.part");
    if job
        .temporary_path
        .file_name()
        .and_then(|name| name.to_str())
        != Some(temporary_name.as_str())
        || job.output_path.file_name().and_then(|name| name.to_str()) != Some(output_name.as_str())
    {
        return Err("preview output names do not match the recording group contract".to_owned());
    }
    Ok(())
}

fn validate_cover_job(job: &CoverJob) -> Result<(), String> {
    if !job.input_path.is_file() {
        return Err("cover input is missing".to_owned());
    }
    let input_parent = job
        .input_path
        .parent()
        .ok_or_else(|| "cover input has no recording group".to_owned())?
        .canonicalize()
        .map_err(|error| error.to_string())?;
    for output in [&job.preview, &job.high_resolution] {
        if output.maximum_width < 2 || output.maximum_height < 2 {
            return Err("cover profile is invalid".to_owned());
        }
        for path in [&output.temporary_path, &output.output_path] {
            let parent = path
                .parent()
                .ok_or_else(|| "cover output has no recording group".to_owned())?
                .canonicalize()
                .map_err(|error| error.to_string())?;
            if parent != input_parent {
                return Err("cover files must stay inside the recording group".to_owned());
            }
        }
    }
    if job
        .preview
        .temporary_path
        .file_name()
        .and_then(|name| name.to_str())
        != Some("cover-preview.webp.part")
        || job
            .preview
            .output_path
            .file_name()
            .and_then(|name| name.to_str())
            != Some("cover-preview.webp")
        || job
            .high_resolution
            .temporary_path
            .file_name()
            .and_then(|name| name.to_str())
            != Some("cover-high.webp.part")
        || job
            .high_resolution
            .output_path
            .file_name()
            .and_then(|name| name.to_str())
            != Some("cover-high.webp")
    {
        return Err("cover output names do not match the recording group contract".to_owned());
    }
    Ok(())
}

fn ffmpeg_arguments(job: &PreviewJob) -> Vec<OsString> {
    let scale = format!(
        "scale=w='min({},iw)':h='min({},ih)':force_original_aspect_ratio=decrease:force_divisible_by=2",
        job.maximum_width, job.maximum_height
    );
    let mut arguments = vec![
        "-y".into(),
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-i".into(),
        job.input_path.as_os_str().to_owned(),
        "-map".into(),
        "0:v:0".into(),
    ];
    if job.include_audio {
        arguments.extend(["-map".into(), "0:a:0?".into()]);
    } else {
        arguments.push("-an".into());
    }
    arguments.extend([
        "-vf".into(),
        scale.into(),
        "-r".into(),
        job.frames_per_second.to_string().into(),
        "-fps_mode".into(),
        "cfr".into(),
    ]);
    match job.format {
        PreviewFormat::Mp4 => {
            arguments.extend([
                "-c:v".into(),
                "h264_mf".into(),
                "-rate_control".into(),
                "quality".into(),
                "-quality".into(),
                "70".into(),
                "-pix_fmt".into(),
                "yuv420p".into(),
            ]);
            if job.include_audio {
                arguments.extend(["-c:a".into(), "aac_mf".into(), "-b:a".into(), "96k".into()]);
            }
            arguments.extend([
                "-movflags".into(),
                "+faststart".into(),
                "-f".into(),
                "mp4".into(),
            ]);
        }
        PreviewFormat::Webm => {
            arguments.extend([
                "-c:v".into(),
                "libvpx-vp9".into(),
                "-crf".into(),
                "34".into(),
                "-b:v".into(),
                "0".into(),
                "-deadline".into(),
                "realtime".into(),
                "-cpu-used".into(),
                "6".into(),
                "-pix_fmt".into(),
                "yuv420p".into(),
            ]);
            if job.include_audio {
                arguments.extend(["-c:a".into(), "libopus".into(), "-b:a".into(), "96k".into()]);
            }
            arguments.extend(["-f".into(), "webm".into()]);
        }
    }
    arguments.push(job.temporary_path.as_os_str().to_owned());
    arguments
}

fn cover_ffmpeg_arguments(job: &CoverJob, output: &CoverOutput) -> Vec<OsString> {
    let scale = format!(
        "scale=w='min({},iw)':h='min({},ih)':force_original_aspect_ratio=decrease:force_divisible_by=2",
        output.maximum_width, output.maximum_height
    );
    vec![
        "-y".into(),
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-ss".into(),
        format!("{:.3}", job.position_ms as f64 / 1_000.0).into(),
        "-i".into(),
        job.input_path.as_os_str().to_owned(),
        "-map".into(),
        "0:v:0".into(),
        "-frames:v".into(),
        "1".into(),
        "-vf".into(),
        scale.into(),
        "-c:v".into(),
        "libwebp".into(),
        "-q:v".into(),
        "75".into(),
        "-f".into(),
        "webp".into(),
        output.temporary_path.as_os_str().to_owned(),
    ]
}

fn resolve_ffmpeg() -> Result<PathBuf, String> {
    if let Some(path) = env::var_os("FRAMEFLOW_FFMPEG").map(PathBuf::from) {
        if path.is_file() {
            return Ok(path);
        }
    }
    if let Ok(executable) = env::current_exe() {
        for candidate in bundled_ffmpeg_candidates(&executable) {
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    if let Some(path) = executable_on_path("ffmpeg.exe") {
        return Ok(path);
    }
    if let Some(local_app_data) = env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        let root = local_app_data.join("Programs").join("FFmpeg");
        if let Some(path) = find_managed_ffmpeg(&root) {
            return Ok(path);
        }
    }
    Err("FFmpeg is unavailable; install it or place ffmpeg.exe beside FrameFlow".to_owned())
}

fn bundled_ffmpeg_candidates(executable: &Path) -> [PathBuf; 6] {
    let directory = executable.parent().unwrap_or_else(|| Path::new(""));
    [
        executable.with_file_name("ffmpeg.exe"),
        directory.join("ffmpeg").join("ffmpeg.exe"),
        directory.join("ffmpeg").join("bin").join("ffmpeg.exe"),
        directory.join("resources").join("ffmpeg.exe"),
        directory
            .join("resources")
            .join("ffmpeg")
            .join("ffmpeg.exe"),
        directory
            .join("resources")
            .join("ffmpeg")
            .join("bin")
            .join("ffmpeg.exe"),
    ]
}

fn executable_on_path(name: &str) -> Option<PathBuf> {
    env::split_paths(&env::var_os("PATH")?).find_map(|directory| {
        let candidate = directory.join(name);
        candidate.is_file().then_some(candidate)
    })
}

fn find_managed_ffmpeg(root: &Path) -> Option<PathBuf> {
    let direct = root.join("bin").join("ffmpeg.exe");
    if direct.is_file() {
        return Some(direct);
    }
    fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .find_map(|entry| {
            let candidate = entry.path().join("bin").join("ffmpeg.exe");
            candidate.is_file().then_some(candidate)
        })
}

#[cfg(windows)]
fn hide_console_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x0800_4000);
}

#[cfg(not(windows))]
fn hide_console_window(_command: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_arguments_use_media_foundation_and_a_bounded_even_scale() {
        let job = PreviewJob {
            input_path: "main.mp4".into(),
            temporary_path: "preview.mp4.part".into(),
            output_path: "preview.mp4".into(),
            format: PreviewFormat::Mp4,
            maximum_width: 854,
            maximum_height: 480,
            frames_per_second: 15,
            include_audio: true,
        };
        let arguments = ffmpeg_arguments(&job)
            .into_iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(arguments.windows(2).any(|pair| pair == ["-c:v", "h264_mf"]));
        assert!(arguments.windows(2).any(|pair| pair == ["-c:a", "aac_mf"]));
        assert!(arguments
            .iter()
            .any(|argument| argument.contains("force_divisible_by=2")));
        assert!(arguments.windows(2).any(|pair| pair == ["-r", "15"]));
        assert_eq!(
            arguments.last().map(String::as_str),
            Some("preview.mp4.part")
        );
    }

    #[test]
    fn worker_protocol_round_trips_paths_without_command_line_escaping() {
        let request = MediaWorkerRequest {
            protocol_version: MEDIA_WORKER_PROTOCOL_VERSION,
            job: MediaJob::Preview(PreviewJob {
                input_path: r"D:\录制 文件\main.mp4".into(),
                temporary_path: r"D:\录制 文件\preview.mp4.part".into(),
                output_path: r"D:\录制 文件\preview.mp4".into(),
                format: PreviewFormat::Mp4,
                maximum_width: 854,
                maximum_height: 480,
                frames_per_second: 15,
                include_audio: true,
            }),
        };
        let json = serde_json::to_vec(&request).unwrap();
        assert_eq!(
            serde_json::from_slice::<MediaWorkerRequest>(&json)
                .unwrap()
                .job,
            request.job
        );
    }

    #[test]
    fn webm_preview_arguments_use_vp9_and_opus() {
        let job = PreviewJob {
            input_path: "main.mp4".into(),
            temporary_path: "preview.webm.part".into(),
            output_path: "preview.webm".into(),
            format: PreviewFormat::Webm,
            maximum_width: 1280,
            maximum_height: 720,
            frames_per_second: 15,
            include_audio: true,
        };
        let arguments = ffmpeg_arguments(&job)
            .into_iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(arguments
            .windows(2)
            .any(|pair| pair == ["-c:v", "libvpx-vp9"]));
        assert!(arguments.windows(2).any(|pair| pair == ["-c:a", "libopus"]));
        assert!(arguments.windows(2).any(|pair| pair == ["-f", "webm"]));
    }

    #[test]
    fn cover_arguments_seek_once_and_emit_bounded_webp() {
        let job = CoverJob {
            input_path: "preview.mp4".into(),
            preview: CoverOutput {
                temporary_path: "cover-preview.webp.part".into(),
                output_path: "cover-preview.webp".into(),
                maximum_width: 320,
                maximum_height: 180,
            },
            high_resolution: CoverOutput {
                temporary_path: "cover-high.webp.part".into(),
                output_path: "cover-high.webp".into(),
                maximum_width: 1920,
                maximum_height: 1080,
            },
            position_ms: 1_250,
        };
        let arguments = cover_ffmpeg_arguments(&job, &job.preview)
            .into_iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(arguments.windows(2).any(|pair| pair == ["-ss", "1.250"]));
        assert!(arguments.windows(2).any(|pair| pair == ["-c:v", "libwebp"]));
        assert!(arguments
            .iter()
            .any(|argument| argument.contains("force_divisible_by=2")));
        assert_eq!(
            arguments.last().map(String::as_str),
            Some("cover-preview.webp.part")
        );
    }

    #[test]
    fn bundled_runtime_layouts_are_checked_before_machine_installations() {
        let candidates =
            bundled_ffmpeg_candidates(Path::new(r"C:\Program Files\FrameFlow\frameflow.exe"));
        assert_eq!(
            candidates[1],
            PathBuf::from(r"C:\Program Files\FrameFlow\ffmpeg\ffmpeg.exe")
        );
        assert_eq!(
            candidates[4],
            PathBuf::from(r"C:\Program Files\FrameFlow\resources\ffmpeg\ffmpeg.exe")
        );
    }
}
