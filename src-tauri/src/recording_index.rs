use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::RwLock,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use frameflow_core::Resolution;
use frameflow_media::PreviewFormat;

const SCHEMA_VERSION: u32 = 2;
const MANIFEST_SCHEMA_VERSION: u32 = 3;
const MANIFEST_FILE_NAME: &str = "recording.json";

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecordingSummary {
    pub(crate) id: String,
    pub(crate) name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) duration_seconds: Option<u64>,
    pub(crate) created_label: String,
    pub(crate) main_status: OutputStatus,
    pub(crate) preview_status: OutputStatus,
    pub(crate) cover_status: OutputStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) cover_path: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecordingDetails {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) created_label: String,
    pub(crate) duration_seconds: Option<f64>,
    pub(crate) main: ArtifactDetails,
    pub(crate) preview: ArtifactDetails,
    pub(crate) cover: ArtifactDetails,
    pub(crate) high_resolution_cover: ArtifactDetails,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArtifactDetails {
    pub(crate) status: OutputStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) file_size_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) frames_per_second: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OutputStatus {
    NotRequested,
    Pending,
    Processing,
    Completed,
    Failed,
}

impl OutputStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::NotRequested => "not_requested",
            Self::Pending => "pending",
            Self::Processing => "processing",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    fn from_database(value: &str) -> Self {
        match value {
            "pending" => Self::Pending,
            "processing" => Self::Processing,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            _ => Self::NotRequested,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct RecordingManifest {
    schema_version: u32,
    id: String,
    display_name: String,
    created_at_ms: u64,
    duration_ms: u64,
    #[serde(default)]
    preview_profile: PreviewProfile,
    outputs: ManifestOutputs,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PreviewProfile {
    pub(crate) format: PreviewFormat,
    pub(crate) maximum_width: u32,
    pub(crate) maximum_height: u32,
    pub(crate) frames_per_second: u16,
    pub(crate) include_audio: bool,
}

impl Default for PreviewProfile {
    fn default() -> Self {
        Self {
            format: PreviewFormat::Mp4,
            maximum_width: 854,
            maximum_height: 480,
            frames_per_second: 15,
            include_audio: true,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ManifestOutputs {
    main: OutputArtifact,
    preview: OutputArtifact,
    cover: OutputArtifact,
    #[serde(default = "unrequested_artifact")]
    high_resolution_cover: OutputArtifact,
}

fn unrequested_artifact() -> OutputArtifact {
    OutputArtifact {
        status: OutputStatus::NotRequested,
        path: None,
        file_size_bytes: None,
        error: None,
        width: None,
        height: None,
        frames_per_second: None,
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct OutputArtifact {
    status: OutputStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    file_size_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    frames_per_second: Option<u16>,
}

#[derive(Clone, Debug)]
pub(crate) struct PreviewJobSpec {
    pub(crate) id: String,
    pub(crate) input_path: PathBuf,
    pub(crate) temporary_path: PathBuf,
    pub(crate) output_path: PathBuf,
    pub(crate) profile: PreviewProfile,
}

#[derive(Clone, Debug)]
pub(crate) struct CoverJobSpec {
    pub(crate) id: String,
    pub(crate) input_path: PathBuf,
    pub(crate) preview_temporary_path: PathBuf,
    pub(crate) preview_output_path: PathBuf,
    pub(crate) high_temporary_path: PathBuf,
    pub(crate) high_output_path: PathBuf,
    pub(crate) position_ms: u64,
    pub(crate) source_resolution: Option<Resolution>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RecordingDeletionTarget {
    pub(crate) id: String,
    pub(crate) path: PathBuf,
}

pub(crate) struct RecordingIndex {
    database_path: PathBuf,
    recording_directories: RwLock<Vec<PathBuf>>,
}

struct DiscoveredRecording {
    id: String,
    group_directory: PathBuf,
    main_path: PathBuf,
    manifest_path: Option<PathBuf>,
    name: String,
    created_at_ms: u64,
    file_size_bytes: u64,
    duration_ms: Option<u64>,
    main_status: OutputStatus,
    preview_status: OutputStatus,
}

impl RecordingManifest {
    fn from_recording(
        recording: &DiscoveredRecording,
        duration_ms: u64,
        resolution: Resolution,
        frames_per_second: u16,
        preview_profile: Option<PreviewProfile>,
        cover_enabled: bool,
    ) -> Self {
        let preview_enabled = preview_profile.is_some();
        let preview_profile = preview_profile.unwrap_or_default();
        let preview_name = format!(
            "preview.{}",
            match preview_profile.format {
                PreviewFormat::Mp4 => "mp4",
                PreviewFormat::Webm => "webm",
                PreviewFormat::AnimatedWebp => "webp",
            }
        );
        Self {
            schema_version: MANIFEST_SCHEMA_VERSION,
            id: recording.id.clone(),
            display_name: recording.name.clone(),
            created_at_ms: recording.created_at_ms,
            duration_ms,
            preview_profile,
            outputs: ManifestOutputs {
                main: OutputArtifact {
                    status: OutputStatus::Completed,
                    path: recording
                        .main_path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned()),
                    file_size_bytes: Some(recording.file_size_bytes),
                    error: None,
                    width: Some(resolution.width),
                    height: Some(resolution.height),
                    frames_per_second: Some(frames_per_second),
                },
                preview: if preview_enabled {
                    OutputArtifact {
                        status: OutputStatus::Pending,
                        path: Some(preview_name),
                        file_size_bytes: None,
                        error: None,
                        width: None,
                        height: None,
                        frames_per_second: None,
                    }
                } else {
                    unrequested_artifact()
                },
                cover: if cover_enabled {
                    OutputArtifact {
                        status: OutputStatus::Pending,
                        path: Some("cover-preview.webp".to_owned()),
                        file_size_bytes: None,
                        error: None,
                        width: None,
                        height: None,
                        frames_per_second: None,
                    }
                } else {
                    unrequested_artifact()
                },
                high_resolution_cover: if cover_enabled {
                    OutputArtifact {
                        status: OutputStatus::Pending,
                        path: Some("cover-high.webp".to_owned()),
                        file_size_bytes: None,
                        error: None,
                        width: None,
                        height: None,
                        frames_per_second: None,
                    }
                } else {
                    unrequested_artifact()
                },
            },
        }
    }
}

impl RecordingIndex {
    pub(crate) fn open(
        database_path: impl Into<PathBuf>,
        recording_directories: impl IntoIterator<Item = PathBuf>,
    ) -> Result<Self, String> {
        let recording_directories = normalize_recording_directories(recording_directories)?;
        let index = Self {
            database_path: database_path.into(),
            recording_directories: RwLock::new(recording_directories),
        };
        if let Some(parent) = index.database_path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        if let Err(error) = index.initialize() {
            if !index.database_path.exists() || error.contains("newer than supported") {
                return Err(error);
            }
            index.archive_unreadable_database()?;
            index.initialize().map_err(|rebuild_error| {
                format!("failed to rebuild unreadable recording index ({error}): {rebuild_error}")
            })?;
        }
        Ok(index)
    }

    fn archive_unreadable_database(&self) -> Result<(), String> {
        let timestamp = system_time_millis(SystemTime::now()).unwrap_or_default();
        let backup = self
            .database_path
            .with_file_name(format!("recordings.corrupt-{timestamp}.sqlite3"));
        fs::rename(&self.database_path, &backup)
            .map_err(|error| format!("failed to archive unreadable recording index: {error}"))?;
        for suffix in ["-wal", "-shm"] {
            let sidecar =
                PathBuf::from(format!("{}{suffix}", self.database_path.to_string_lossy()));
            if sidecar.exists() {
                fs::remove_file(sidecar).map_err(|error| error.to_string())?;
            }
        }
        Ok(())
    }

    fn initialize(&self) -> Result<(), String> {
        self.with_connection(|connection| {
            let version = connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
                .map_err(|error| error.to_string())?;
            if version > SCHEMA_VERSION {
                return Err(format!(
                    "recording index schema {version} is newer than supported schema {SCHEMA_VERSION}"
                ));
            }
            connection
                .execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")
                .map_err(|error| error.to_string())?;
            if version == 1 {
                connection
                    .execute_batch(
                        "BEGIN IMMEDIATE;
                         ALTER TABLE recordings RENAME TO recordings_v1;
                         CREATE TABLE recordings (
                             id TEXT PRIMARY KEY NOT NULL,
                             group_directory TEXT NOT NULL,
                             display_name TEXT NOT NULL,
                             duration_ms INTEGER,
                             created_at_ms INTEGER NOT NULL,
                             main_path TEXT NOT NULL UNIQUE,
                             main_status TEXT NOT NULL,
                             main_size_bytes INTEGER NOT NULL,
                             preview_status TEXT NOT NULL,
                             manifest_path TEXT
                         );
                         INSERT INTO recordings (
                             id, group_directory, display_name, duration_ms, created_at_ms,
                             main_path, main_status, main_size_bytes, preview_status, manifest_path
                         ) SELECT id, '', file_name, duration_ms, created_at_ms,
                             file_path, 'completed', file_size_bytes, 'not_requested', NULL
                           FROM recordings_v1;
                         DROP TABLE recordings_v1;
                         CREATE INDEX recordings_created_at ON recordings(created_at_ms DESC);
                         PRAGMA user_version = 2;
                         COMMIT;",
                    )
                    .map_err(|error| error.to_string())?;
            } else {
                connection
                    .execute_batch(
                        "CREATE TABLE IF NOT EXISTS recordings (
                             id TEXT PRIMARY KEY NOT NULL,
                             group_directory TEXT NOT NULL,
                             display_name TEXT NOT NULL,
                             duration_ms INTEGER,
                             created_at_ms INTEGER NOT NULL,
                             main_path TEXT NOT NULL UNIQUE,
                             main_status TEXT NOT NULL,
                             main_size_bytes INTEGER NOT NULL,
                             preview_status TEXT NOT NULL,
                             manifest_path TEXT
                         );
                         CREATE INDEX IF NOT EXISTS recordings_created_at
                             ON recordings(created_at_ms DESC);
                         PRAGMA user_version = 2;",
                    )
                    .map_err(|error| error.to_string())?;
            }
            Ok(())
        })
    }

    pub(crate) fn reconcile(&self) -> Result<(), String> {
        let directories = self.recording_directories()?;
        let available_directories = directories
            .iter()
            .filter(|directory| directory.is_dir())
            .collect::<Vec<_>>();
        let mut recordings = Vec::new();
        for directory in &directories {
            recordings.extend(discover_recordings(directory)?);
        }
        self.with_connection(|connection| {
            let transaction = connection
                .transaction()
                .map_err(|error| error.to_string())?;
            let existing_recordings = {
                let mut statement = transaction
                    .prepare("SELECT id, group_directory, main_path FROM recordings")
                    .map_err(|error| error.to_string())?;
                let recordings = statement
                    .query_map([], |row| {
                        let id = row.get::<_, String>(0)?;
                        let group_directory = row.get::<_, String>(1)?;
                        let main_path = row.get::<_, String>(2)?;
                        let path = if group_directory.is_empty() {
                            PathBuf::from(main_path)
                        } else {
                            PathBuf::from(group_directory)
                        };
                        Ok((id, path))
                    })
                    .map_err(|error| error.to_string())?
                    .collect::<Result<HashMap<_, _>, _>>()
                    .map_err(|error| error.to_string())?;
                recordings
            };
            let mut discovered_ids = HashSet::with_capacity(recordings.len());
            for recording in recordings {
                discovered_ids.insert(recording.id.clone());
                transaction
                    .execute(
                        "INSERT INTO recordings (
                             id, group_directory, display_name, duration_ms, created_at_ms,
                             main_path, main_status, main_size_bytes, preview_status, manifest_path
                         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                         ON CONFLICT(id) DO UPDATE SET
                             group_directory = excluded.group_directory,
                             display_name = excluded.display_name,
                             duration_ms = COALESCE(excluded.duration_ms, recordings.duration_ms),
                             created_at_ms = excluded.created_at_ms,
                             main_path = excluded.main_path,
                             main_status = excluded.main_status,
                             main_size_bytes = excluded.main_size_bytes,
                             preview_status = excluded.preview_status,
                             manifest_path = excluded.manifest_path",
                        params![
                            recording.id,
                            recording.group_directory.to_string_lossy(),
                            recording.name,
                            recording.duration_ms.map(to_sql_integer),
                            to_sql_integer(recording.created_at_ms),
                            recording.main_path.to_string_lossy(),
                            recording.main_status.as_str(),
                            to_sql_integer(recording.file_size_bytes),
                            recording.preview_status.as_str(),
                            recording
                                .manifest_path
                                .as_ref()
                                .map(|path| path.to_string_lossy().into_owned()),
                        ],
                    )
                    .map_err(|error| error.to_string())?;
            }
            for (stale_id, indexed_path) in existing_recordings
                .iter()
                .filter(|(id, _)| !discovered_ids.contains(*id))
            {
                // Only remove entries owned by a root that is currently
                // available. Entries on disconnected drives remain indexed.
                if available_directories
                    .iter()
                    .any(|directory| path_is_within(indexed_path, directory))
                {
                    transaction
                        .execute("DELETE FROM recordings WHERE id = ?1", [stale_id])
                        .map_err(|error| error.to_string())?;
                }
            }
            transaction.commit().map_err(|error| error.to_string())
        })
    }

    pub(crate) fn register_directory(&self, directory: PathBuf) -> Result<(), String> {
        let directory = prepare_recording_directory(&directory)?;
        let mut directories = self
            .recording_directories
            .write()
            .map_err(|_| "recording directory registry is unavailable".to_owned())?;
        if !directories.iter().any(|existing| existing == &directory) {
            directories.push(directory);
        }
        Ok(())
    }

    pub(crate) fn synchronize_directories(
        &self,
        directories: impl IntoIterator<Item = PathBuf>,
    ) -> Result<(), String> {
        let synchronized = normalize_recording_directories(directories)?;
        let mut registered = self
            .recording_directories
            .write()
            .map_err(|_| "recording directory registry is unavailable".to_owned())?;
        for directory in synchronized {
            if !registered.iter().any(|existing| existing == &directory) {
                registered.push(directory);
            }
        }
        Ok(())
    }

    fn recording_directories(&self) -> Result<Vec<PathBuf>, String> {
        self.recording_directories
            .read()
            .map(|directories| directories.clone())
            .map_err(|_| "recording directory registry is unavailable".to_owned())
    }

    fn containing_directory(&self, path: &Path) -> Result<PathBuf, String> {
        let canonical_path = path.canonicalize().map_err(|_| {
            "recording file is missing; refresh the recording list and try again".to_owned()
        })?;
        self.recording_directories()?
            .into_iter()
            .filter_map(|directory| directory.canonicalize().ok())
            .find(|directory| canonical_path.starts_with(directory))
            .ok_or_else(|| "recording path is outside the configured output directories".to_owned())
    }

    pub(crate) fn record_completed(
        &self,
        output_path: &Path,
        duration: Duration,
        resolution: Resolution,
        frames_per_second: u16,
        preview_profile: Option<PreviewProfile>,
        cover_enabled: bool,
    ) -> Result<String, String> {
        let mut recording = inspect_recording(output_path)?
            .ok_or_else(|| "finalized recording is not a readable MP4".to_owned())?;
        let duration_ms = duration.as_millis().min(u128::from(u64::MAX)) as u64;
        recording.duration_ms = Some(duration_ms);
        if is_group_main(output_path) {
            let manifest = RecordingManifest::from_recording(
                &recording,
                duration_ms,
                resolution,
                frames_per_second,
                preview_profile,
                cover_enabled,
            );
            let manifest_path = recording.group_directory.join(MANIFEST_FILE_NAME);
            write_manifest_atomically(&manifest_path, &manifest)?;
            recording.manifest_path = Some(manifest_path);
            recording.preview_status = if manifest.outputs.preview.status == OutputStatus::Pending {
                OutputStatus::Pending
            } else {
                OutputStatus::NotRequested
            };
        }
        self.with_connection(|connection| {
            connection
                .execute(
                    "INSERT INTO recordings (
                         id, group_directory, display_name, duration_ms, created_at_ms,
                         main_path, main_status, main_size_bytes, preview_status, manifest_path
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                     ON CONFLICT(id) DO UPDATE SET
                         group_directory = excluded.group_directory,
                         display_name = excluded.display_name,
                         duration_ms = excluded.duration_ms,
                         created_at_ms = excluded.created_at_ms,
                         main_path = excluded.main_path,
                         main_status = excluded.main_status,
                         main_size_bytes = excluded.main_size_bytes,
                         preview_status = excluded.preview_status,
                         manifest_path = excluded.manifest_path",
                    params![
                        recording.id,
                        recording.group_directory.to_string_lossy(),
                        recording.name,
                        to_sql_integer(duration_ms),
                        to_sql_integer(recording.created_at_ms),
                        recording.main_path.to_string_lossy(),
                        recording.main_status.as_str(),
                        to_sql_integer(recording.file_size_bytes),
                        recording.preview_status.as_str(),
                        recording
                            .manifest_path
                            .as_ref()
                            .map(|path| path.to_string_lossy().into_owned()),
                    ],
                )
                .map(|_| recording.id.clone())
                .map_err(|error| error.to_string())
        })
    }

    pub(crate) fn prepare_preview_for_path(
        &self,
        main_path: &Path,
    ) -> Result<PreviewJobSpec, String> {
        let manifest_path = main_path
            .parent()
            .ok_or_else(|| "recording has no group directory".to_owned())?
            .join(MANIFEST_FILE_NAME);
        let mut manifest = read_manifest(&manifest_path)?;
        if manifest.outputs.preview.status == OutputStatus::Completed {
            return Err("preview is already completed".to_owned());
        }
        let preview_name = manifest
            .outputs
            .preview
            .path
            .as_deref()
            .unwrap_or("preview.mp4");
        let group_directory = manifest_path
            .parent()
            .ok_or_else(|| "recording manifest has no group directory".to_owned())?;
        let output_path = safe_artifact_path(group_directory, preview_name)?;
        let input_path = main_path
            .canonicalize()
            .map_err(|error| format!("recording master is unavailable: {error}"))?;
        if self.containing_directory(&input_path).is_err() {
            return Err("recording group is outside the configured output directory".to_owned());
        }
        let expected_main = manifest
            .outputs
            .main
            .path
            .as_deref()
            .ok_or_else(|| "recording manifest has no master path".to_owned())?;
        if safe_artifact_path(group_directory, expected_main)?
            .canonicalize()
            .map_err(|error| error.to_string())?
            != input_path
        {
            return Err("recording manifest does not match the indexed master".to_owned());
        }
        manifest.outputs.preview.status = OutputStatus::Processing;
        manifest.outputs.preview.error = None;
        manifest.outputs.preview.file_size_bytes = None;
        write_manifest_atomically(&manifest_path, &manifest)?;
        self.update_preview_index(&manifest.id, OutputStatus::Processing)?;
        Ok(PreviewJobSpec {
            id: manifest.id,
            input_path,
            temporary_path: output_path.with_extension(format!(
                "{}.part",
                output_path
                    .extension()
                    .and_then(|value| value.to_str())
                    .unwrap_or("mp4")
            )),
            output_path,
            profile: manifest.preview_profile,
        })
    }

    pub(crate) fn prepare_preview_for_id(&self, id: &str) -> Result<PreviewJobSpec, String> {
        let main_path = self.main_path_for_id(id)?;
        self.prepare_preview_for_path(&main_path)
    }

    pub(crate) fn recoverable_preview_paths(&self) -> Result<Vec<PathBuf>, String> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT main_path FROM recordings
                     WHERE preview_status IN ('pending', 'processing')
                       AND manifest_path IS NOT NULL
                     ORDER BY created_at_ms ASC",
                )
                .map_err(|error| error.to_string())?;
            let paths = statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(|error| error.to_string())?
                .map(|path| path.map(PathBuf::from).map_err(|error| error.to_string()))
                .collect();
            paths
        })
    }

    pub(crate) fn finish_preview(
        &self,
        job: &PreviewJobSpec,
        result: Result<(), String>,
    ) -> Result<(), String> {
        let manifest_path = job
            .input_path
            .parent()
            .ok_or_else(|| "recording has no group directory".to_owned())?
            .join(MANIFEST_FILE_NAME);
        let mut manifest = read_manifest(&manifest_path)?;
        let (status, error) = match result {
            Ok(()) => {
                let size = fs::metadata(&job.output_path)
                    .map_err(|error| format!("completed preview is unavailable: {error}"))?
                    .len();
                manifest.outputs.preview.file_size_bytes = Some(size);
                if let (Some(width), Some(height)) =
                    (manifest.outputs.main.width, manifest.outputs.main.height)
                {
                    let preview = preview_resolution(
                        Resolution { width, height },
                        Resolution {
                            width: job.profile.maximum_width,
                            height: job.profile.maximum_height,
                        },
                    );
                    manifest.outputs.preview.width = Some(preview.width);
                    manifest.outputs.preview.height = Some(preview.height);
                }
                manifest.outputs.preview.frames_per_second = Some(job.profile.frames_per_second);
                (OutputStatus::Completed, None)
            }
            Err(error) => {
                manifest.outputs.preview.file_size_bytes = None;
                manifest.outputs.preview.width = None;
                manifest.outputs.preview.height = None;
                manifest.outputs.preview.frames_per_second = None;
                (OutputStatus::Failed, Some(error))
            }
        };
        manifest.outputs.preview.status = status;
        manifest.outputs.preview.error = error;
        write_manifest_atomically(&manifest_path, &manifest)?;
        self.update_preview_index(&job.id, status)
    }

    pub(crate) fn prepare_cover_for_id(&self, id: &str) -> Result<CoverJobSpec, String> {
        let main_path = self.main_path_for_id(id)?;
        let manifest_path = main_path
            .parent()
            .ok_or_else(|| "recording has no group directory".to_owned())?
            .join(MANIFEST_FILE_NAME);
        let mut manifest = read_manifest(&manifest_path)?;
        if manifest.outputs.cover.status == OutputStatus::Completed
            && matches!(
                manifest.outputs.high_resolution_cover.status,
                OutputStatus::Completed | OutputStatus::NotRequested
            )
        {
            return Err("covers are already completed".to_owned());
        }
        manifest.outputs.cover.path = Some("cover-preview.webp".to_owned());
        manifest.outputs.high_resolution_cover.path = Some("cover-high.webp".to_owned());
        let group_directory = manifest_path
            .parent()
            .ok_or_else(|| "recording manifest has no group directory".to_owned())?;
        let preview_output_path = safe_artifact_path(
            group_directory,
            manifest
                .outputs
                .cover
                .path
                .as_deref()
                .unwrap_or("cover-preview.webp"),
        )?;
        let high_output_path = safe_artifact_path(
            group_directory,
            manifest
                .outputs
                .high_resolution_cover
                .path
                .as_deref()
                .unwrap_or("cover-high.webp"),
        )?;
        let source_resolution = artifact_resolution(&manifest.outputs.main);
        let input_path = main_path
            .canonicalize()
            .map_err(|error| format!("cover source is unavailable: {error}"))?;
        if self.containing_directory(&input_path).is_err() {
            return Err("cover source is outside the configured output directory".to_owned());
        }
        manifest.outputs.cover.status = OutputStatus::Processing;
        manifest.outputs.cover.error = None;
        manifest.outputs.cover.file_size_bytes = None;
        manifest.outputs.high_resolution_cover.status = OutputStatus::Processing;
        manifest.outputs.high_resolution_cover.error = None;
        manifest.outputs.high_resolution_cover.file_size_bytes = None;
        write_manifest_atomically(&manifest_path, &manifest)?;
        Ok(CoverJobSpec {
            id: manifest.id,
            input_path,
            preview_temporary_path: preview_output_path.with_extension("webp.part"),
            preview_output_path,
            high_temporary_path: high_output_path.with_extension("webp.part"),
            high_output_path,
            position_ms: (manifest.duration_ms / 10).min(1_000),
            source_resolution,
        })
    }

    pub(crate) fn recoverable_cover_ids(&self) -> Result<Vec<String>, String> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT id, manifest_path FROM recordings
                     WHERE manifest_path IS NOT NULL ORDER BY created_at_ms ASC",
                )
                .map_err(|error| error.to_string())?;
            let rows = statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|error| error.to_string())?;
            let mut ids = Vec::new();
            for row in rows {
                let (id, manifest_path) = row.map_err(|error| error.to_string())?;
                let manifest = read_manifest(Path::new(&manifest_path))?;
                if (matches!(
                    manifest.outputs.cover.status,
                    OutputStatus::Pending | OutputStatus::Processing
                ) || matches!(
                    manifest.outputs.high_resolution_cover.status,
                    OutputStatus::Pending | OutputStatus::Processing
                )) && !matches!(
                    manifest.outputs.preview.status,
                    OutputStatus::Pending | OutputStatus::Processing
                ) {
                    ids.push(id);
                }
            }
            Ok(ids)
        })
    }

    pub(crate) fn finish_cover(
        &self,
        job: &CoverJobSpec,
        result: Result<(), String>,
    ) -> Result<(), String> {
        let manifest_path = job
            .preview_output_path
            .parent()
            .ok_or_else(|| "recording has no group directory".to_owned())?
            .join(MANIFEST_FILE_NAME);
        let mut manifest = read_manifest(&manifest_path)?;
        let (status, error) = match result {
            Ok(()) => {
                manifest.outputs.cover.file_size_bytes = Some(
                    fs::metadata(&job.preview_output_path)
                        .map_err(|error| {
                            format!("completed preview cover is unavailable: {error}")
                        })?
                        .len(),
                );
                manifest.outputs.high_resolution_cover.file_size_bytes = Some(
                    fs::metadata(&job.high_output_path)
                        .map_err(|error| {
                            format!("completed high-resolution cover is unavailable: {error}")
                        })?
                        .len(),
                );
                if let Some(source) = job.source_resolution {
                    let cover = preview_resolution(
                        source,
                        Resolution {
                            width: 320,
                            height: 180,
                        },
                    );
                    manifest.outputs.cover.width = Some(cover.width);
                    manifest.outputs.cover.height = Some(cover.height);
                    let high = preview_resolution(
                        source,
                        Resolution {
                            width: 1920,
                            height: 1080,
                        },
                    );
                    manifest.outputs.high_resolution_cover.width = Some(high.width);
                    manifest.outputs.high_resolution_cover.height = Some(high.height);
                }
                (OutputStatus::Completed, None)
            }
            Err(error) => {
                manifest.outputs.cover.file_size_bytes = None;
                manifest.outputs.cover.width = None;
                manifest.outputs.cover.height = None;
                manifest.outputs.high_resolution_cover.file_size_bytes = None;
                manifest.outputs.high_resolution_cover.width = None;
                manifest.outputs.high_resolution_cover.height = None;
                (OutputStatus::Failed, Some(error))
            }
        };
        manifest.outputs.cover.frames_per_second = None;
        manifest.outputs.cover.status = status;
        manifest.outputs.cover.error = error;
        manifest.outputs.high_resolution_cover.frames_per_second = None;
        manifest.outputs.high_resolution_cover.status = status;
        manifest.outputs.high_resolution_cover.error = manifest.outputs.cover.error.clone();
        write_manifest_atomically(&manifest_path, &manifest)
    }

    fn update_preview_index(&self, id: &str, status: OutputStatus) -> Result<(), String> {
        self.with_connection(|connection| {
            let updated = connection
                .execute(
                    "UPDATE recordings SET preview_status = ?2 WHERE id = ?1",
                    params![id, status.as_str()],
                )
                .map_err(|error| error.to_string())?;
            if updated == 1 {
                Ok(())
            } else {
                Err("recording is no longer indexed".to_owned())
            }
        })
    }

    pub(crate) fn recent(&self, limit: usize) -> Result<Vec<RecordingSummary>, String> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT id, display_name, duration_ms, created_at_ms,
                            main_status, preview_status, manifest_path
                     FROM recordings
                     ORDER BY created_at_ms DESC
                     LIMIT ?1",
                )
                .map_err(|error| error.to_string())?;
            let recordings = statement
                .query_map([to_sql_integer(limit as u64)], |row| {
                    let duration_ms = row.get::<_, Option<i64>>(2)?;
                    let created_at_ms = row.get::<_, i64>(3)?.max(0) as u64;
                    let manifest_path = row.get::<_, Option<String>>(6)?;
                    let (cover_status, cover_path) = manifest_path
                        .as_deref()
                        .and_then(|path| read_manifest(Path::new(path)).ok())
                        .map(|manifest| {
                            let group = Path::new(manifest_path.as_deref().unwrap_or_default())
                                .parent()
                                .unwrap_or_else(|| Path::new(""));
                            let path = manifest
                                .outputs
                                .cover
                                .path
                                .as_deref()
                                .and_then(|name| safe_artifact_path(group, name).ok());
                            (
                                manifest.outputs.cover.status,
                                path.map(|path| path.to_string_lossy().into_owned()),
                            )
                        })
                        .unwrap_or((OutputStatus::NotRequested, None));
                    Ok(RecordingSummary {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        duration_seconds: duration_ms.map(|value| value.max(0) as u64 / 1_000),
                        created_label: created_label(created_at_ms),
                        main_status: OutputStatus::from_database(&row.get::<_, String>(4)?),
                        preview_status: OutputStatus::from_database(&row.get::<_, String>(5)?),
                        cover_status,
                        cover_path,
                    })
                })
                .map_err(|error| error.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())?;
            Ok(recordings)
        })
    }

    pub(crate) fn details(&self, id: &str) -> Result<RecordingDetails, String> {
        let row = self
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT display_name, duration_ms, created_at_ms, main_status,
                            main_size_bytes, preview_status, manifest_path
                     FROM recordings WHERE id = ?1",
                        [id],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, Option<i64>>(1)?,
                                row.get::<_, i64>(2)?,
                                row.get::<_, String>(3)?,
                                row.get::<_, i64>(4)?,
                                row.get::<_, String>(5)?,
                                row.get::<_, Option<String>>(6)?,
                            ))
                        },
                    )
                    .optional()
                    .map_err(|error| error.to_string())
            })?
            .ok_or_else(|| "recording is no longer available".to_owned())?;
        let (name, duration_ms, created_at_ms, main_status, main_size, preview_status, manifest) =
            row;
        let main_path = self.path_for_id(id)?;
        if let Some(manifest_path) = manifest {
            let manifest_path = PathBuf::from(manifest_path);
            let manifest = read_manifest(&manifest_path)?;
            let group_directory = manifest_path
                .parent()
                .ok_or_else(|| "recording manifest has no group directory".to_owned())?;
            if group_directory
                .canonicalize()
                .map_err(|error| error.to_string())?
                != main_path
                    .parent()
                    .ok_or_else(|| "recording master has no group directory".to_owned())?
                    .canonicalize()
                    .map_err(|error| error.to_string())?
            {
                return Err("recording manifest is outside the indexed group".to_owned());
            }
            return Ok(RecordingDetails {
                id: id.to_owned(),
                name: manifest.display_name,
                created_label: created_label(manifest.created_at_ms),
                duration_seconds: Some(manifest.duration_ms as f64 / 1_000.0),
                main: artifact_details(group_directory, &manifest.outputs.main)?,
                preview: artifact_details(group_directory, &manifest.outputs.preview)?,
                cover: artifact_details(group_directory, &manifest.outputs.cover)?,
                high_resolution_cover: artifact_details(
                    group_directory,
                    &manifest.outputs.high_resolution_cover,
                )?,
            });
        }
        Ok(RecordingDetails {
            id: id.to_owned(),
            name,
            created_label: created_label(created_at_ms.max(0) as u64),
            duration_seconds: duration_ms.map(|value| value.max(0) as f64 / 1_000.0),
            main: ArtifactDetails {
                status: OutputStatus::from_database(&main_status),
                path: Some(main_path.to_string_lossy().into_owned()),
                file_size_bytes: Some(main_size.max(0) as u64),
                width: None,
                height: None,
                frames_per_second: None,
                error: None,
            },
            preview: ArtifactDetails {
                status: OutputStatus::from_database(&preview_status),
                path: None,
                file_size_bytes: None,
                width: None,
                height: None,
                frames_per_second: None,
                error: None,
            },
            cover: ArtifactDetails {
                status: OutputStatus::NotRequested,
                path: None,
                file_size_bytes: None,
                width: None,
                height: None,
                frames_per_second: None,
                error: None,
            },
            high_resolution_cover: ArtifactDetails {
                status: OutputStatus::NotRequested,
                path: None,
                file_size_bytes: None,
                width: None,
                height: None,
                frames_per_second: None,
                error: None,
            },
        })
    }

    pub(crate) fn path_for_id(&self, id: &str) -> Result<PathBuf, String> {
        let path = self.main_path_for_id(id)?;
        let canonical_path = path.canonicalize().map_err(|_| {
            "recording file is missing; refresh the recording list and try again".to_owned()
        })?;
        self.containing_directory(&canonical_path)?;
        Ok(canonical_path)
    }

    pub(crate) fn deletion_targets(
        &self,
        ids: &[String],
    ) -> Result<Vec<RecordingDeletionTarget>, String> {
        if ids.is_empty() {
            return Err("select at least one recording to delete".to_owned());
        }
        let unique = ids.iter().collect::<HashSet<_>>();
        if unique.len() != ids.len() {
            return Err("recording deletion request contains duplicate IDs".to_owned());
        }
        let targets = ids
            .iter()
            .map(|id| {
                let (main_path, manifest_path) = self
                    .with_connection(|connection| {
                        connection
                            .query_row(
                                "SELECT main_path, manifest_path FROM recordings WHERE id = ?1",
                                [id],
                                |row| {
                                    Ok((
                                        PathBuf::from(row.get::<_, String>(0)?),
                                        row.get::<_, Option<String>>(1)?.map(PathBuf::from),
                                    ))
                                },
                            )
                            .optional()
                            .map_err(|error| error.to_string())
                    })?
                    .ok_or_else(|| format!("recording {id} is no longer indexed"))?;
                let main_path = main_path
                    .canonicalize()
                    .map_err(|error| format!("recording {id} is unavailable: {error}"))?;
                let recordings_directory = self.containing_directory(&main_path)?;
                let path = if let Some(manifest_path) = manifest_path {
                    let manifest_path = manifest_path.canonicalize().map_err(|error| {
                        format!("recording {id} manifest is unavailable: {error}")
                    })?;
                    let group = manifest_path
                        .parent()
                        .ok_or_else(|| format!("recording {id} has no group directory"))?;
                    if group == recordings_directory
                        || !group.starts_with(&recordings_directory)
                        || main_path.parent() != Some(group)
                    {
                        return Err(format!("recording {id} group boundary is invalid"));
                    }
                    group.to_path_buf()
                } else {
                    let recovered_group = group_id_from_main_path(&main_path)
                        .filter(|group_id| group_id == id)
                        .and_then(|_| main_path.parent())
                        .filter(|group| {
                            *group != recordings_directory
                                && group.starts_with(&recordings_directory)
                        });
                    recovered_group.map(Path::to_path_buf).unwrap_or(main_path)
                };
                Ok(RecordingDeletionTarget {
                    id: id.clone(),
                    path,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let unique_paths = targets
            .iter()
            .map(|target| &target.path)
            .collect::<HashSet<_>>();
        if unique_paths.len() != targets.len() {
            return Err("recording deletion targets overlap".to_owned());
        }
        Ok(targets)
    }

    pub(crate) fn forget_recordings(&self, ids: &[String]) -> Result<(), String> {
        self.with_connection(|connection| {
            let transaction = connection
                .transaction()
                .map_err(|error| error.to_string())?;
            for id in ids {
                transaction
                    .execute("DELETE FROM recordings WHERE id = ?1", [id])
                    .map_err(|error| error.to_string())?;
            }
            transaction.commit().map_err(|error| error.to_string())
        })
    }

    fn main_path_for_id(&self, id: &str) -> Result<PathBuf, String> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT main_path FROM recordings WHERE id = ?1",
                    [id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(|error| error.to_string())
        })?
        .map(PathBuf::from)
        .ok_or_else(|| "recording is no longer available".to_owned())
    }

    fn with_connection<T>(
        &self,
        operation: impl FnOnce(&mut Connection) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut connection =
            Connection::open(&self.database_path).map_err(|error| error.to_string())?;
        connection
            .busy_timeout(Duration::from_secs(2))
            .map_err(|error| error.to_string())?;
        operation(&mut connection)
    }
}

fn artifact_details(
    group_directory: &Path,
    artifact: &OutputArtifact,
) -> Result<ArtifactDetails, String> {
    let path = artifact
        .path
        .as_deref()
        .map(|path| safe_artifact_path(group_directory, path))
        .transpose()?;
    let available_path = if artifact.status == OutputStatus::Completed {
        path.filter(|path| path.is_file())
            .map(|path| path.to_string_lossy().into_owned())
    } else {
        None
    };
    Ok(ArtifactDetails {
        status: artifact.status,
        path: available_path,
        file_size_bytes: artifact.file_size_bytes,
        width: artifact.width,
        height: artifact.height,
        frames_per_second: artifact.frames_per_second,
        error: artifact.error.clone(),
    })
}

fn artifact_resolution(artifact: &OutputArtifact) -> Option<Resolution> {
    Some(Resolution {
        width: artifact.width?,
        height: artifact.height?,
    })
}

fn preview_resolution(source: Resolution, maximum: Resolution) -> Resolution {
    let scale = (f64::from(maximum.width) / f64::from(source.width))
        .min(f64::from(maximum.height) / f64::from(source.height))
        .min(1.0);
    let even = |value: f64, limit: u32| (((value / 2.0).round() as u32) * 2).max(2).min(limit & !1);
    Resolution {
        width: even(f64::from(source.width) * scale, maximum.width),
        height: even(f64::from(source.height) * scale, maximum.height),
    }
}

fn normalize_recording_directories(
    directories: impl IntoIterator<Item = PathBuf>,
) -> Result<Vec<PathBuf>, String> {
    let mut normalized = Vec::new();
    for directory in directories {
        if !directory.is_absolute() {
            return Err("recording directory must be an absolute path".to_owned());
        }
        let directory = if directory.exists() {
            directory
                .canonicalize()
                .map_err(|error| format!("failed to resolve recording directory: {error}"))?
        } else {
            directory
        };
        if !normalized.iter().any(|existing| existing == &directory) {
            normalized.push(directory);
        }
    }
    if normalized.is_empty() {
        return Err("at least one recording directory is required".to_owned());
    }
    Ok(normalized)
}

fn prepare_recording_directory(directory: &Path) -> Result<PathBuf, String> {
    if !directory.is_absolute() {
        return Err("recording directory must be an absolute path".to_owned());
    }
    fs::create_dir_all(directory)
        .map_err(|error| format!("failed to create recording directory: {error}"))?;
    directory
        .canonicalize()
        .map_err(|error| format!("failed to resolve recording directory: {error}"))
}

fn discover_recordings(directory: &Path) -> Result<Vec<DiscoveredRecording>, String> {
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut pending = vec![directory.to_path_buf()];
    let mut manifest_paths = Vec::new();
    let mut video_paths = Vec::new();
    while let Some(directory) = pending.pop() {
        let entries = fs::read_dir(&directory).map_err(|error| error.to_string())?;
        for entry in entries {
            let entry = entry.map_err(|error| error.to_string())?;
            let file_type = entry.file_type().map_err(|error| error.to_string())?;
            if file_type.is_dir() && !file_type.is_symlink() {
                pending.push(entry.path());
            } else if file_type.is_file() {
                if entry.file_name() == MANIFEST_FILE_NAME {
                    manifest_paths.push(entry.path());
                } else if entry
                    .path()
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("mp4"))
                {
                    video_paths.push(entry.path());
                }
            }
        }
    }
    let mut recordings = Vec::new();
    let mut grouped_main_paths = HashSet::new();
    for manifest_path in manifest_paths {
        match inspect_manifest(&manifest_path) {
            Ok(Some(recording)) => {
                grouped_main_paths.insert(recording.main_path.clone());
                recordings.push(recording);
            }
            Ok(None) => {}
            Err(error) => eprintln!(
                "ignored invalid recording manifest {}: {error}",
                manifest_path.display()
            ),
        }
    }
    for video_path in video_paths {
        if grouped_main_paths.contains(&video_path) || is_preview_artifact(&video_path) {
            continue;
        }
        if let Ok(Some(recording)) = inspect_recording(&video_path) {
            recordings.push(recording);
        }
    }
    Ok(recordings)
}

fn inspect_recording(path: &Path) -> Result<Option<DiscoveredRecording>, String> {
    if !path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("mp4"))
        || !has_mp4_file_type_box(path)?
    {
        return Ok(None);
    }
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "recording file name is not valid Unicode".to_owned())?
        .to_owned();
    let group_id = group_id_from_main_path(path);
    let created_at_ms = group_id
        .as_deref()
        .and_then(recording_timestamp_from_group_id)
        .or_else(|| recording_timestamp_from_name(&file_name))
        .or_else(|| system_time_millis(metadata.created().ok()?))
        .or_else(|| system_time_millis(metadata.modified().ok()?))
        .unwrap_or_default();
    let id = group_id
        .or_else(|| {
            recording_timestamp_from_name(&file_name).map(|timestamp| timestamp.to_string())
        })
        .map(|timestamp| timestamp.to_string())
        .unwrap_or_else(|| format!("import-{:016x}", stable_path_hash(path)));
    let name = if is_group_main(path) {
        format!("ScreenHarbor-{id}.mp4")
    } else {
        file_name
    };
    Ok(Some(DiscoveredRecording {
        id,
        group_directory: path.parent().unwrap_or(Path::new("")).to_path_buf(),
        main_path: path.to_path_buf(),
        manifest_path: None,
        name,
        created_at_ms,
        file_size_bytes: metadata.len(),
        duration_ms: None,
        main_status: OutputStatus::Completed,
        preview_status: OutputStatus::NotRequested,
    }))
}

fn inspect_manifest(path: &Path) -> Result<Option<DiscoveredRecording>, String> {
    let manifest = read_manifest(path)?;
    if manifest.schema_version > MANIFEST_SCHEMA_VERSION {
        return Err(format!(
            "manifest schema {} is newer than supported schema {}",
            manifest.schema_version, MANIFEST_SCHEMA_VERSION
        ));
    }
    let group_directory = path
        .parent()
        .ok_or_else(|| "recording manifest has no parent directory".to_owned())?;
    let Some(relative_main_path) = manifest.outputs.main.path.as_deref() else {
        return Ok(None);
    };
    let main_path = safe_artifact_path(group_directory, relative_main_path)?;
    let Some(mut recording) = inspect_recording(&main_path)? else {
        return Ok(None);
    };
    recording.id = manifest.id;
    recording.name = manifest.display_name;
    recording.created_at_ms = manifest.created_at_ms;
    recording.duration_ms = Some(manifest.duration_ms);
    recording.main_status = manifest.outputs.main.status;
    recording.preview_status = manifest.outputs.preview.status;
    recording.manifest_path = Some(path.to_path_buf());
    Ok(Some(recording))
}

fn read_manifest(path: &Path) -> Result<RecordingManifest, String> {
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}

fn safe_artifact_path(group_directory: &Path, relative_path: &str) -> Result<PathBuf, String> {
    let relative_path = Path::new(relative_path);
    let mut components = relative_path.components();
    if !matches!(components.next(), Some(std::path::Component::Normal(_)))
        || components.next().is_some()
    {
        return Err("recording artifact path must be a relative file name".to_owned());
    }
    Ok(group_directory.join(relative_path))
}

fn write_manifest_atomically(path: &Path, manifest: &RecordingManifest) -> Result<(), String> {
    let temporary_path = path.with_extension("json.part");
    let bytes = serde_json::to_vec_pretty(manifest).map_err(|error| error.to_string())?;
    let mut file = File::create(&temporary_path).map_err(|error| error.to_string())?;
    file.write_all(&bytes).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    drop(file);
    replace_file(&temporary_path, path)
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> Result<(), String> {
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
fn replace_file(source: &Path, destination: &Path) -> Result<(), String> {
    fs::rename(source, destination).map_err(|error| error.to_string())
}

fn is_group_main(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("main.mp4"))
        && group_id_from_main_path(path).is_some()
}

fn is_preview_artifact(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.eq_ignore_ascii_case("preview.mp4")
                || name.eq_ignore_ascii_case("preview.webm")
                || name.eq_ignore_ascii_case("preview.webp")
        })
}

fn group_id_from_main_path(path: &Path) -> Option<String> {
    if !path.file_name()?.to_str()?.eq_ignore_ascii_case("main.mp4") {
        return None;
    }
    let group_name = path.parent()?.file_name()?.to_str()?;
    strip_recording_prefix(group_name).map(ToOwned::to_owned)
}

fn recording_timestamp_from_group_id(id: &str) -> Option<u64> {
    id.parse().ok()
}

fn has_mp4_file_type_box(path: &Path) -> Result<bool, String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut header = [0_u8; 12];
    let bytes_read = file.read(&mut header).map_err(|error| error.to_string())?;
    Ok(bytes_read == header.len() && &header[4..8] == b"ftyp")
}

fn recording_timestamp_from_name(name: &str) -> Option<u64> {
    strip_recording_prefix(name)?
        .strip_suffix(".mp4")?
        .parse()
        .ok()
}

fn strip_recording_prefix(name: &str) -> Option<&str> {
    name.strip_prefix("ScreenHarbor-")
        .or_else(|| name.strip_prefix("FrameFlow-"))
}

fn system_time_millis(time: SystemTime) -> Option<u64> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
}

fn created_label(created_at_ms: u64) -> String {
    let now_ms = system_time_millis(SystemTime::now()).unwrap_or(created_at_ms);
    let elapsed_seconds = now_ms.saturating_sub(created_at_ms) / 1_000;
    match elapsed_seconds {
        0..=59 => "刚刚".to_owned(),
        60..=3_599 => format!("{} 分钟前", elapsed_seconds / 60),
        3_600..=86_399 => format!("{} 小时前", elapsed_seconds / 3_600),
        _ => format!("{} 天前", elapsed_seconds / 86_400),
    }
}

fn stable_path_hash(path: &Path) -> u64 {
    path.to_string_lossy()
        .bytes()
        .fold(0xcbf29ce484222325, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
        })
}

fn path_is_within(path: &Path, directory: &Path) -> bool {
    if path.starts_with(directory) {
        return true;
    }
    let mut ancestor = path;
    let mut missing_components = Vec::new();
    while !ancestor.exists() {
        let Some(name) = ancestor.file_name() else {
            return false;
        };
        missing_components.push(name.to_owned());
        let Some(parent) = ancestor.parent() else {
            return false;
        };
        ancestor = parent;
    }
    let Ok(mut resolved) = ancestor.canonicalize() else {
        return false;
    };
    for component in missing_components.iter().rev() {
        resolved.push(component);
    }
    resolved.starts_with(directory)
}

fn to_sql_integer(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestWorkspace(PathBuf);

    impl TestWorkspace {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "frameflow-recording-index-{name}-{}",
                system_time_millis(SystemTime::now()).unwrap_or_default()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn index(&self) -> RecordingIndex {
            RecordingIndex::open(self.0.join("index.sqlite3"), [self.0.join("recordings")]).unwrap()
        }

        fn recording(&self, name: &str) -> PathBuf {
            let directory = self.0.join("recordings");
            fs::create_dir_all(&directory).unwrap();
            let path = directory.join(name);
            fs::write(&path, b"\0\0\0\x18ftypisom\0\0\0\0isomiso2").unwrap();
            path
        }

        fn group_recording(&self, id: &str) -> PathBuf {
            let directory = self.0.join("recordings").join(format!("ScreenHarbor-{id}"));
            fs::create_dir_all(&directory).unwrap();
            let path = directory.join("main.mp4");
            fs::write(&path, b"\0\0\0\x18ftypisom\0\0\0\0isomiso2").unwrap();
            path
        }

        fn complete(&self, index: &RecordingIndex, path: &Path, duration: Duration) {
            index
                .record_completed(
                    path,
                    duration,
                    Resolution {
                        width: 1920,
                        height: 1080,
                    },
                    60,
                    Some(PreviewProfile::default()),
                    true,
                )
                .unwrap();
        }
    }

    #[test]
    fn recording_names_accept_current_and_legacy_prefixes() {
        assert_eq!(
            recording_timestamp_from_name("ScreenHarbor-1700000000098.mp4"),
            Some(1_700_000_000_098)
        );
        assert_eq!(
            recording_timestamp_from_name("FrameFlow-1700000000099.mp4"),
            Some(1_700_000_000_099)
        );
    }

    impl Drop for TestWorkspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn completed_recording_survives_reopening_the_index() {
        let workspace = TestWorkspace::new("reopen");
        let path = workspace.recording("FrameFlow-1700000000000.mp4");
        workspace.complete(&workspace.index(), &path, Duration::from_millis(5_400));

        let recent = workspace.index().recent(5).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].id, "1700000000000");
        assert_eq!(recent[0].duration_seconds, Some(5));
    }

    #[test]
    fn reconciliation_discovers_existing_recordings() {
        let workspace = TestWorkspace::new("discover");
        workspace.recording("FrameFlow-1700000000001.mp4");
        let index = workspace.index();
        index.reconcile().unwrap();

        let recent = index.recent(5).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].duration_seconds, None);
    }

    #[test]
    fn reconciliation_preserves_duration_after_a_file_moves() {
        let workspace = TestWorkspace::new("move");
        let source = workspace.recording("FrameFlow-1700000000002.mp4");
        let index = workspace.index();
        workspace.complete(&index, &source, Duration::from_secs(8));
        let destination_directory = workspace.0.join("recordings").join("archive");
        fs::create_dir_all(&destination_directory).unwrap();
        fs::rename(
            &source,
            destination_directory.join("FrameFlow-1700000000002.mp4"),
        )
        .unwrap();

        index.reconcile().unwrap();

        assert_eq!(index.recent(5).unwrap()[0].duration_seconds, Some(8));
    }

    #[test]
    fn reconciliation_removes_missing_and_ignores_invalid_files() {
        let workspace = TestWorkspace::new("repair");
        let recording = workspace.recording("FrameFlow-1700000000003.mp4");
        let index = workspace.index();
        workspace.complete(&index, &recording, Duration::from_secs(2));
        fs::remove_file(recording).unwrap();
        fs::write(
            workspace
                .0
                .join("recordings")
                .join("FrameFlow-1700000000004.mp4"),
            b"not an mp4",
        )
        .unwrap();

        index.reconcile().unwrap();

        assert!(index.recent(5).unwrap().is_empty());
    }

    #[test]
    fn reconciliation_removes_a_recording_group_deleted_outside_the_app() {
        let workspace = TestWorkspace::new("externally-deleted-group");
        let recording = workspace.group_recording("1700000000100");
        let group_directory = recording.parent().unwrap().to_path_buf();
        let index = workspace.index();
        workspace.complete(&index, &recording, Duration::from_secs(2));
        assert_eq!(index.recent(5).unwrap().len(), 1);

        fs::remove_dir_all(group_directory).unwrap();
        index.reconcile().unwrap();

        assert!(index.recent(5).unwrap().is_empty());
    }

    #[test]
    fn indexed_paths_are_resolved_inside_the_recording_directory() {
        let workspace = TestWorkspace::new("resolve");
        let recording = workspace.recording("FrameFlow-1700000000005.mp4");
        let index = workspace.index();
        workspace.complete(&index, &recording, Duration::from_secs(2));

        assert_eq!(
            index.path_for_id("1700000000005").unwrap(),
            recording.canonicalize().unwrap()
        );
    }

    #[test]
    fn registered_directories_keep_old_and_new_recordings_in_one_library() {
        let workspace = TestWorkspace::new("multiple-roots");
        let old = workspace.recording("FrameFlow-1700000000020.mp4");
        let index = workspace.index();
        workspace.complete(&index, &old, Duration::from_secs(2));

        let second_root = workspace.0.join("second-recordings");
        index.register_directory(second_root.clone()).unwrap();
        let new = second_root.join("FrameFlow-1700000000021.mp4");
        fs::write(&new, b"\0\0\0\x18ftypisom\0\0\0\0isomiso2").unwrap();
        workspace.complete(&index, &new, Duration::from_secs(3));
        index.reconcile().unwrap();

        let recent = index.recent(10).unwrap();
        assert_eq!(recent.len(), 2);
        assert!(recent
            .iter()
            .any(|recording| recording.id == "1700000000020"));
        assert!(recent
            .iter()
            .any(|recording| recording.id == "1700000000021"));
        assert_eq!(
            index.path_for_id("1700000000020").unwrap(),
            old.canonicalize().unwrap()
        );
        assert_eq!(
            index.path_for_id("1700000000021").unwrap(),
            new.canonicalize().unwrap()
        );
    }

    #[test]
    fn late_loaded_directories_are_synchronized_before_reconciliation() {
        let workspace = TestWorkspace::new("late-loaded-root");
        let index = workspace.index();
        let late_root = workspace.0.join("late-recordings");
        fs::create_dir_all(&late_root).unwrap();
        let recording = late_root.join("FrameFlow-1700000000030.mp4");
        fs::write(&recording, b"\0\0\0\x18ftypisom\0\0\0\0isomiso2").unwrap();

        index.synchronize_directories([late_root.clone()]).unwrap();
        index.reconcile().unwrap();

        assert!(index
            .recent(10)
            .unwrap()
            .iter()
            .any(|candidate| candidate.id == "1700000000030"));
    }

    #[test]
    fn unavailable_historical_directory_does_not_erase_its_index_entries() {
        let workspace = TestWorkspace::new("offline-root");
        let index = workspace.index();
        let second_root = workspace.0.join("removable-recordings");
        index.register_directory(second_root.clone()).unwrap();
        let recording = second_root.join("FrameFlow-1700000000022.mp4");
        fs::write(&recording, b"\0\0\0\x18ftypisom\0\0\0\0isomiso2").unwrap();
        workspace.complete(&index, &recording, Duration::from_secs(3));
        fs::remove_dir_all(second_root).unwrap();

        index.reconcile().unwrap();

        assert!(index
            .recent(10)
            .unwrap()
            .iter()
            .any(|entry| entry.id == "1700000000022"));
    }

    #[test]
    fn recovered_group_without_manifest_is_a_master_only_recording() {
        let workspace = TestWorkspace::new("recovered-group");
        let recording = workspace.group_recording("1700000000023");
        let index = workspace.index();
        index.reconcile().unwrap();

        let recent = index.recent(1).unwrap();
        assert_eq!(recent[0].main_status, OutputStatus::Completed);
        assert_eq!(recent[0].preview_status, OutputStatus::NotRequested);
        assert_eq!(
            index
                .deletion_targets(&["1700000000023".to_owned()])
                .unwrap()[0]
                .path,
            recording.parent().unwrap().canonicalize().unwrap()
        );
    }

    #[test]
    fn unreadable_database_is_archived_and_rebuilt_from_recordings() {
        let workspace = TestWorkspace::new("corrupt");
        workspace.recording("FrameFlow-1700000000006.mp4");
        fs::write(workspace.0.join("index.sqlite3"), b"not a sqlite database").unwrap();

        let index = workspace.index();
        index.reconcile().unwrap();

        assert_eq!(index.recent(5).unwrap().len(), 1);
        assert!(fs::read_dir(&workspace.0).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("recordings.corrupt-")));
    }

    #[test]
    fn completed_group_writes_manifest_and_restores_independent_output_statuses() {
        let workspace = TestWorkspace::new("manifest");
        let path = workspace.group_recording("1700000000007");
        let index = workspace.index();
        workspace.complete(&index, &path, Duration::from_millis(9_800));

        let manifest_path = path.parent().unwrap().join(MANIFEST_FILE_NAME);
        let manifest: RecordingManifest =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        assert_eq!(manifest.id, "1700000000007");
        assert_eq!(manifest.outputs.main.status, OutputStatus::Completed);
        assert_eq!(manifest.outputs.main.width, Some(1920));
        assert_eq!(manifest.outputs.main.height, Some(1080));
        assert_eq!(manifest.outputs.main.frames_per_second, Some(60));
        assert_eq!(manifest.outputs.preview.status, OutputStatus::Pending);
        assert_eq!(manifest.outputs.main.path.as_deref(), Some("main.mp4"));

        index.reconcile().unwrap();
        let recent = index.recent(1).unwrap();
        assert_eq!(recent[0].duration_seconds, Some(9));
        assert_eq!(recent[0].main_status, OutputStatus::Completed);
        assert_eq!(recent[0].preview_status, OutputStatus::Pending);
    }

    #[test]
    fn preview_status_transitions_are_persisted_in_manifest_and_index() {
        let workspace = TestWorkspace::new("preview-status");
        let path = workspace.group_recording("1700000000010");
        let index = workspace.index();
        workspace.complete(&index, &path, Duration::from_secs(3));

        let job = index.prepare_preview_for_path(&path).unwrap();
        assert_eq!(
            index.recent(1).unwrap()[0].preview_status,
            OutputStatus::Processing
        );
        fs::write(&job.output_path, b"preview").unwrap();
        index.finish_preview(&job, Ok(())).unwrap();

        let manifest = read_manifest(&path.parent().unwrap().join(MANIFEST_FILE_NAME)).unwrap();
        assert_eq!(manifest.outputs.preview.status, OutputStatus::Completed);
        assert_eq!(manifest.outputs.preview.file_size_bytes, Some(7));
        assert_eq!(manifest.outputs.preview.error, None);
        assert_eq!(manifest.outputs.preview.width, Some(854));
        assert_eq!(manifest.outputs.preview.height, Some(480));
        assert_eq!(manifest.outputs.preview.frames_per_second, Some(15));
        assert_eq!(
            index.recent(1).unwrap()[0].preview_status,
            OutputStatus::Completed
        );
        assert!(index.recoverable_preview_paths().unwrap().is_empty());

        let details = index.details("1700000000010").unwrap();
        assert_eq!(details.duration_seconds, Some(3.0));
        assert_eq!(details.main.width, Some(1920));
        assert_eq!(details.preview.width, Some(854));
        assert_eq!(
            details.preview.path,
            Some(job.output_path.to_string_lossy().into_owned())
        );
    }

    #[test]
    fn disabled_preview_marks_preview_and_covers_as_not_requested() {
        let workspace = TestWorkspace::new("preview-disabled");
        let path = workspace.group_recording("1700000000019");
        let index = workspace.index();

        index
            .record_completed(
                &path,
                Duration::from_secs(3),
                Resolution {
                    width: 1920,
                    height: 1080,
                },
                60,
                None,
                false,
            )
            .unwrap();

        let details = index.details("1700000000019").unwrap();
        assert_eq!(details.preview.status, OutputStatus::NotRequested);
        assert_eq!(details.cover.status, OutputStatus::NotRequested);
        assert_eq!(
            details.high_resolution_cover.status,
            OutputStatus::NotRequested
        );
        assert!(index.recoverable_preview_paths().unwrap().is_empty());
        assert!(index.recoverable_cover_ids().unwrap().is_empty());
    }

    #[test]
    fn webm_preview_profile_is_frozen_in_the_recording_group() {
        let workspace = TestWorkspace::new("webm-preview-profile");
        let path = workspace.group_recording("1700000000020");
        let index = workspace.index();
        let profile = PreviewProfile {
            format: PreviewFormat::Webm,
            maximum_width: 1280,
            maximum_height: 720,
            frames_per_second: 15,
            include_audio: true,
        };
        index
            .record_completed(
                &path,
                Duration::from_secs(3),
                Resolution {
                    width: 1920,
                    height: 1080,
                },
                60,
                Some(profile),
                true,
            )
            .unwrap();

        let job = index.prepare_preview_for_path(&path).unwrap();
        assert_eq!(job.output_path.file_name().unwrap(), "preview.webm");
        assert_eq!(job.temporary_path.file_name().unwrap(), "preview.webm.part");
        assert_eq!(job.profile.format, PreviewFormat::Webm);
        assert_eq!(job.profile.maximum_width, 1280);
        fs::write(&job.output_path, b"preview").unwrap();
        index.finish_preview(&job, Ok(())).unwrap();

        let manifest = read_manifest(&path.parent().unwrap().join(MANIFEST_FILE_NAME)).unwrap();
        assert_eq!(manifest.outputs.preview.width, Some(1280));
        assert_eq!(manifest.outputs.preview.height, Some(720));
    }

    #[test]
    fn animated_webp_preview_uses_the_recording_group_contract() {
        let workspace = TestWorkspace::new("animated-webp-preview-profile");
        let path = workspace.group_recording("1700000000021");
        let index = workspace.index();
        let profile = PreviewProfile {
            format: PreviewFormat::AnimatedWebp,
            maximum_width: 640,
            maximum_height: 360,
            frames_per_second: 10,
            include_audio: false,
        };
        index
            .record_completed(
                &path,
                Duration::from_secs(3),
                Resolution {
                    width: 1920,
                    height: 1080,
                },
                30,
                Some(profile),
                true,
            )
            .unwrap();

        let job = index.prepare_preview_for_path(&path).unwrap();
        assert_eq!(job.output_path.file_name().unwrap(), "preview.webp");
        assert_eq!(job.temporary_path.file_name().unwrap(), "preview.webp.part");
        assert_eq!(job.profile.format, PreviewFormat::AnimatedWebp);
        assert!(!job.profile.include_audio);
    }

    #[test]
    fn failed_preview_keeps_the_master_and_can_be_prepared_for_retry() {
        let workspace = TestWorkspace::new("preview-retry");
        let path = workspace.group_recording("1700000000011");
        let index = workspace.index();
        workspace.complete(&index, &path, Duration::from_secs(3));

        let job = index.prepare_preview_for_path(&path).unwrap();
        index
            .finish_preview(&job, Err("encoder unavailable".to_owned()))
            .unwrap();
        let manifest = read_manifest(&path.parent().unwrap().join(MANIFEST_FILE_NAME)).unwrap();
        assert_eq!(manifest.outputs.main.status, OutputStatus::Completed);
        assert_eq!(manifest.outputs.preview.status, OutputStatus::Failed);
        assert_eq!(
            manifest.outputs.preview.error.as_deref(),
            Some("encoder unavailable")
        );
        assert!(path.is_file());

        let retry = index.prepare_preview_for_id("1700000000011").unwrap();
        assert_eq!(retry.input_path, path.canonicalize().unwrap());
        assert_eq!(
            index.recent(1).unwrap()[0].preview_status,
            OutputStatus::Processing
        );
    }

    #[test]
    fn webp_covers_are_generated_from_the_master_and_exposed_to_the_library() {
        let workspace = TestWorkspace::new("cover-status");
        let path = workspace.group_recording("1700000000012");
        let index = workspace.index();
        workspace.complete(&index, &path, Duration::from_secs(12));

        let preview = index.prepare_preview_for_path(&path).unwrap();
        fs::write(&preview.output_path, b"preview").unwrap();
        index.finish_preview(&preview, Ok(())).unwrap();
        let cover = index.prepare_cover_for_id("1700000000012").unwrap();
        assert_eq!(cover.input_path, path.canonicalize().unwrap());
        assert_eq!(cover.position_ms, 1_000);
        assert_eq!(
            cover.source_resolution,
            Some(Resolution {
                width: 1920,
                height: 1080
            })
        );
        fs::write(&cover.preview_output_path, b"webp").unwrap();
        fs::write(&cover.high_output_path, b"high-webp").unwrap();
        index.finish_cover(&cover, Ok(())).unwrap();

        let recent = index.recent(1).unwrap();
        assert_eq!(recent[0].cover_status, OutputStatus::Completed);
        assert_eq!(
            recent[0].cover_path,
            Some(cover.preview_output_path.to_string_lossy().into_owned())
        );
        let details = index.details("1700000000012").unwrap();
        assert_eq!(details.cover.status, OutputStatus::Completed);
        assert_eq!(details.cover.width, Some(320));
        assert_eq!(details.cover.height, Some(180));
        assert_eq!(details.cover.file_size_bytes, Some(4));
        assert_eq!(
            details.high_resolution_cover.status,
            OutputStatus::Completed
        );
        assert_eq!(details.high_resolution_cover.width, Some(1920));
        assert_eq!(details.high_resolution_cover.height, Some(1080));
        assert_eq!(details.high_resolution_cover.file_size_bytes, Some(9));
        assert!(index.recoverable_cover_ids().unwrap().is_empty());
    }

    #[test]
    fn cover_failure_is_retryable_and_falls_back_to_the_master() {
        let workspace = TestWorkspace::new("cover-retry");
        let path = workspace.group_recording("1700000000013");
        let index = workspace.index();
        workspace.complete(&index, &path, Duration::from_secs(2));
        let preview = index.prepare_preview_for_path(&path).unwrap();
        index
            .finish_preview(&preview, Err("preview failed".to_owned()))
            .unwrap();

        let cover = index.prepare_cover_for_id("1700000000013").unwrap();
        assert_eq!(cover.input_path, path.canonicalize().unwrap());
        index
            .finish_cover(&cover, Err("webp failed".to_owned()))
            .unwrap();
        assert!(path.is_file());
        let retry = index.prepare_cover_for_id("1700000000013").unwrap();
        assert_eq!(retry.input_path, path.canonicalize().unwrap());
    }

    #[test]
    fn schema_one_database_is_migrated_without_moving_legacy_files() {
        let workspace = TestWorkspace::new("migration");
        let recording = workspace.recording("FrameFlow-1700000000008.mp4");
        let database_path = workspace.0.join("index.sqlite3");
        let connection = Connection::open(&database_path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE recordings (
                     id TEXT PRIMARY KEY NOT NULL,
                     file_path TEXT NOT NULL UNIQUE,
                     file_name TEXT NOT NULL,
                     duration_ms INTEGER,
                     created_at_ms INTEGER NOT NULL,
                     file_size_bytes INTEGER NOT NULL
                 );
                 PRAGMA user_version = 1;",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO recordings VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    "1700000000008",
                    recording.to_string_lossy(),
                    "FrameFlow-1700000000008.mp4",
                    4_200_i64,
                    1_700_000_000_008_i64,
                    24_i64,
                ],
            )
            .unwrap();
        drop(connection);

        let index = workspace.index();
        let recent = index.recent(1).unwrap();
        assert_eq!(recent[0].duration_seconds, Some(4));
        assert_eq!(recent[0].preview_status, OutputStatus::NotRequested);
        assert_eq!(
            index.path_for_id("1700000000008").unwrap(),
            recording.canonicalize().unwrap()
        );

        let version = Connection::open(database_path)
            .unwrap()
            .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn deletion_targets_preserve_group_and_legacy_boundaries() {
        let workspace = TestWorkspace::new("deletion-boundaries");
        let group = workspace.group_recording("1700000000014");
        let legacy = workspace.recording("FrameFlow-1700000000015.mp4");
        let index = workspace.index();
        workspace.complete(&index, &group, Duration::from_secs(4));
        workspace.complete(&index, &legacy, Duration::from_secs(5));

        let targets = index
            .deletion_targets(&["1700000000014".to_owned(), "1700000000015".to_owned()])
            .unwrap();
        assert_eq!(
            targets[0].path,
            group.parent().unwrap().canonicalize().unwrap()
        );
        assert_eq!(targets[1].path, legacy.canonicalize().unwrap());
    }

    #[test]
    fn forgetting_recordings_updates_the_index_without_touching_files() {
        let workspace = TestWorkspace::new("forget-recordings");
        let first = workspace.recording("FrameFlow-1700000000016.mp4");
        let second = workspace.recording("FrameFlow-1700000000017.mp4");
        let index = workspace.index();
        workspace.complete(&index, &first, Duration::from_secs(1));
        workspace.complete(&index, &second, Duration::from_secs(1));

        index
            .forget_recordings(&["1700000000016".to_owned()])
            .unwrap();

        assert_eq!(index.recent(5).unwrap().len(), 1);
        assert!(first.is_file());
        assert!(second.is_file());
    }
}
