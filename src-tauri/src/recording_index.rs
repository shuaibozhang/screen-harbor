use std::{
    collections::HashSet,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

const SCHEMA_VERSION: u32 = 1;

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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OutputStatus {
    Pending,
    Completed,
}

pub(crate) struct RecordingIndex {
    database_path: PathBuf,
    recordings_directory: PathBuf,
}

struct DiscoveredRecording {
    id: String,
    path: PathBuf,
    name: String,
    created_at_ms: u64,
    file_size_bytes: u64,
}

impl RecordingIndex {
    pub(crate) fn open(
        database_path: impl Into<PathBuf>,
        recordings_directory: impl Into<PathBuf>,
    ) -> Result<Self, String> {
        let index = Self {
            database_path: database_path.into(),
            recordings_directory: recordings_directory.into(),
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
                .execute_batch(&format!(
                    "PRAGMA journal_mode = WAL;
                     PRAGMA synchronous = NORMAL;
                     CREATE TABLE IF NOT EXISTS recordings (
                         id TEXT PRIMARY KEY NOT NULL,
                         file_path TEXT NOT NULL UNIQUE,
                         file_name TEXT NOT NULL,
                         duration_ms INTEGER,
                         created_at_ms INTEGER NOT NULL,
                         file_size_bytes INTEGER NOT NULL
                     );
                     CREATE INDEX IF NOT EXISTS recordings_created_at
                         ON recordings(created_at_ms DESC);
                     PRAGMA user_version = {SCHEMA_VERSION};"
                ))
                .map_err(|error| error.to_string())
        })
    }

    pub(crate) fn reconcile(&self) -> Result<(), String> {
        let recordings = discover_recordings(&self.recordings_directory)?;
        self.with_connection(|connection| {
            let transaction = connection
                .transaction()
                .map_err(|error| error.to_string())?;
            let existing_ids = {
                let mut statement = transaction
                    .prepare("SELECT id FROM recordings")
                    .map_err(|error| error.to_string())?;
                let ids = statement
                    .query_map([], |row| row.get::<_, String>(0))
                    .map_err(|error| error.to_string())?
                    .collect::<Result<HashSet<_>, _>>()
                    .map_err(|error| error.to_string())?;
                ids
            };
            let mut discovered_ids = HashSet::with_capacity(recordings.len());
            for recording in recordings {
                discovered_ids.insert(recording.id.clone());
                transaction
                    .execute(
                        "INSERT INTO recordings (
                             id, file_path, file_name, duration_ms, created_at_ms, file_size_bytes
                         ) VALUES (?1, ?2, ?3, NULL, ?4, ?5)
                         ON CONFLICT(id) DO UPDATE SET
                             file_path = excluded.file_path,
                             file_name = excluded.file_name,
                             created_at_ms = excluded.created_at_ms,
                             file_size_bytes = excluded.file_size_bytes",
                        params![
                            recording.id,
                            recording.path.to_string_lossy(),
                            recording.name,
                            to_sql_integer(recording.created_at_ms),
                            to_sql_integer(recording.file_size_bytes),
                        ],
                    )
                    .map_err(|error| error.to_string())?;
            }
            for stale_id in existing_ids.difference(&discovered_ids) {
                transaction
                    .execute("DELETE FROM recordings WHERE id = ?1", [stale_id])
                    .map_err(|error| error.to_string())?;
            }
            transaction.commit().map_err(|error| error.to_string())
        })
    }

    pub(crate) fn record_completed(
        &self,
        output_path: &Path,
        duration: Duration,
    ) -> Result<(), String> {
        let recording = inspect_recording(output_path)?
            .ok_or_else(|| "finalized recording is not a readable MP4".to_owned())?;
        let duration_ms = duration.as_millis().min(i64::MAX as u128) as i64;
        self.with_connection(|connection| {
            connection
                .execute(
                    "INSERT INTO recordings (
                         id, file_path, file_name, duration_ms, created_at_ms, file_size_bytes
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT(id) DO UPDATE SET
                         file_path = excluded.file_path,
                         file_name = excluded.file_name,
                         duration_ms = excluded.duration_ms,
                         created_at_ms = excluded.created_at_ms,
                         file_size_bytes = excluded.file_size_bytes",
                    params![
                        recording.id,
                        recording.path.to_string_lossy(),
                        recording.name,
                        duration_ms,
                        to_sql_integer(recording.created_at_ms),
                        to_sql_integer(recording.file_size_bytes),
                    ],
                )
                .map(|_| ())
                .map_err(|error| error.to_string())
        })
    }

    pub(crate) fn recent(&self, limit: usize) -> Result<Vec<RecordingSummary>, String> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT id, file_name, duration_ms, created_at_ms
                     FROM recordings
                     ORDER BY created_at_ms DESC
                     LIMIT ?1",
                )
                .map_err(|error| error.to_string())?;
            let recordings = statement
                .query_map([to_sql_integer(limit as u64)], |row| {
                    let duration_ms = row.get::<_, Option<i64>>(2)?;
                    let created_at_ms = row.get::<_, i64>(3)?.max(0) as u64;
                    Ok(RecordingSummary {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        duration_seconds: duration_ms.map(|value| value.max(0) as u64 / 1_000),
                        created_label: created_label(created_at_ms),
                        main_status: OutputStatus::Completed,
                        preview_status: OutputStatus::Pending,
                    })
                })
                .map_err(|error| error.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())?;
            Ok(recordings)
        })
    }

    pub(crate) fn path_for_id(&self, id: &str) -> Result<PathBuf, String> {
        let path = self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT file_path FROM recordings WHERE id = ?1",
                    [id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(|error| error.to_string())
        })?;
        let path = path
            .map(PathBuf::from)
            .ok_or_else(|| "recording is no longer available".to_owned())?;
        let canonical_directory = self
            .recordings_directory
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let canonical_path = path.canonicalize().map_err(|_| {
            "recording file is missing; refresh the recording list and try again".to_owned()
        })?;
        if !canonical_path.starts_with(canonical_directory) {
            return Err("recording path is outside the configured output directory".to_owned());
        }
        Ok(canonical_path)
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

fn discover_recordings(directory: &Path) -> Result<Vec<DiscoveredRecording>, String> {
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut pending = vec![directory.to_path_buf()];
    let mut recordings = Vec::new();
    while let Some(directory) = pending.pop() {
        let entries = fs::read_dir(&directory).map_err(|error| error.to_string())?;
        for entry in entries {
            let entry = entry.map_err(|error| error.to_string())?;
            let file_type = entry.file_type().map_err(|error| error.to_string())?;
            if file_type.is_dir() && !file_type.is_symlink() {
                pending.push(entry.path());
            } else if file_type.is_file() {
                if let Ok(Some(recording)) = inspect_recording(&entry.path()) {
                    recordings.push(recording);
                }
            }
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
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "recording file name is not valid Unicode".to_owned())?
        .to_owned();
    let created_at_ms = recording_timestamp_from_name(&name)
        .or_else(|| system_time_millis(metadata.created().ok()?))
        .or_else(|| system_time_millis(metadata.modified().ok()?))
        .unwrap_or_default();
    let id = recording_timestamp_from_name(&name)
        .map(|timestamp| timestamp.to_string())
        .unwrap_or_else(|| format!("import-{:016x}", stable_path_hash(path)));
    Ok(Some(DiscoveredRecording {
        id,
        path: path.to_path_buf(),
        name,
        created_at_ms,
        file_size_bytes: metadata.len(),
    }))
}

fn has_mp4_file_type_box(path: &Path) -> Result<bool, String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut header = [0_u8; 12];
    let bytes_read = file.read(&mut header).map_err(|error| error.to_string())?;
    Ok(bytes_read == header.len() && &header[4..8] == b"ftyp")
}

fn recording_timestamp_from_name(name: &str) -> Option<u64> {
    name.strip_prefix("FrameFlow-")?
        .strip_suffix(".mp4")?
        .parse()
        .ok()
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
            RecordingIndex::open(self.0.join("index.sqlite3"), self.0.join("recordings")).unwrap()
        }

        fn recording(&self, name: &str) -> PathBuf {
            let directory = self.0.join("recordings");
            fs::create_dir_all(&directory).unwrap();
            let path = directory.join(name);
            fs::write(&path, b"\0\0\0\x18ftypisom\0\0\0\0isomiso2").unwrap();
            path
        }
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
        workspace
            .index()
            .record_completed(&path, Duration::from_millis(5_400))
            .unwrap();

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
        index
            .record_completed(&source, Duration::from_secs(8))
            .unwrap();
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
        index
            .record_completed(&recording, Duration::from_secs(2))
            .unwrap();
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
    fn indexed_paths_are_resolved_inside_the_recording_directory() {
        let workspace = TestWorkspace::new("resolve");
        let recording = workspace.recording("FrameFlow-1700000000005.mp4");
        let index = workspace.index();
        index
            .record_completed(&recording, Duration::from_secs(2))
            .unwrap();

        assert_eq!(
            index.path_for_id("1700000000005").unwrap(),
            recording.canonicalize().unwrap()
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
}
