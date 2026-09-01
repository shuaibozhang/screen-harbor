use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Serialize;

const PART_FILE_NAME: &str = "main.mp4.part";
const MAIN_FILE_NAME: &str = "main.mp4";
const GROUP_PREFIX: &str = "FrameFlow-";

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InterruptedRecording {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) created_label: String,
    pub(crate) file_size_bytes: u64,
}

pub(crate) fn recover_publishable_recordings(roots: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    let mut recovered = Vec::new();
    for candidate in part_candidates(roots)? {
        let main = candidate.group_directory.join(MAIN_FILE_NAME);
        if main.exists() {
            continue;
        }
        match is_publishable_mp4(&candidate.part_path) {
            Ok(true) => match fs::rename(&candidate.part_path, &main) {
                Ok(()) => recovered.push(main),
                Err(error) => eprintln!(
                    "failed to publish recovered recording {}: {error}",
                    candidate.part_path.display()
                ),
            },
            Ok(false) => {}
            Err(error) => eprintln!(
                "failed to inspect interrupted recording {}: {error}",
                candidate.part_path.display()
            ),
        }
    }
    Ok(recovered)
}

pub(crate) fn interrupted_recordings(
    roots: &[PathBuf],
) -> Result<Vec<InterruptedRecording>, String> {
    let mut recordings = part_candidates(roots)?
        .into_iter()
        .filter(|candidate| !candidate.group_directory.join(MAIN_FILE_NAME).exists())
        .map(|candidate| {
            let metadata = fs::metadata(&candidate.part_path).map_err(|error| error.to_string())?;
            let created_at_ms = candidate
                .id
                .parse::<u64>()
                .ok()
                .or_else(|| system_time_millis(metadata.created().ok()?))
                .or_else(|| system_time_millis(metadata.modified().ok()?))
                .unwrap_or_default();
            Ok(InterruptedRecording {
                name: format!("FrameFlow-{}.mp4", candidate.id),
                id: candidate.id,
                created_label: created_label(created_at_ms),
                file_size_bytes: metadata.len(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    recordings.sort_by(|left, right| right.id.cmp(&left.id));
    Ok(recordings)
}

pub(crate) fn interrupted_group_for_id(roots: &[PathBuf], id: &str) -> Result<PathBuf, String> {
    if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("interrupted recording ID is invalid".to_owned());
    }
    part_candidates(roots)?
        .into_iter()
        .find(|candidate| candidate.id == id)
        .filter(|candidate| !candidate.group_directory.join(MAIN_FILE_NAME).exists())
        .map(|candidate| candidate.group_directory)
        .ok_or_else(|| "interrupted recording is no longer available".to_owned())
}

struct PartCandidate {
    id: String,
    group_directory: PathBuf,
    part_path: PathBuf,
}

fn part_candidates(roots: &[PathBuf]) -> Result<Vec<PartCandidate>, String> {
    let mut candidates = Vec::new();
    for root in roots {
        if !root.is_dir() {
            continue;
        }
        for entry in fs::read_dir(root).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let file_type = entry.file_type().map_err(|error| error.to_string())?;
            if !file_type.is_dir() || file_type.is_symlink() {
                continue;
            }
            let Some(group_name) = entry.file_name().to_str().map(ToOwned::to_owned) else {
                continue;
            };
            let Some(id) = group_name.strip_prefix(GROUP_PREFIX) else {
                continue;
            };
            if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_digit()) {
                continue;
            }
            let group_directory = entry.path();
            let part_path = group_directory.join(PART_FILE_NAME);
            if part_path.is_file() {
                candidates.push(PartCandidate {
                    id: id.to_owned(),
                    group_directory,
                    part_path,
                });
            }
        }
    }
    Ok(candidates)
}

fn is_publishable_mp4(path: &Path) -> Result<bool, String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let length = file.metadata().map_err(|error| error.to_string())?.len();
    let mut offset = 0_u64;
    let mut has_ftyp = false;
    let mut has_mdat = false;
    let mut has_moov = false;
    while offset.saturating_add(8) <= length {
        file.seek(SeekFrom::Start(offset))
            .map_err(|error| error.to_string())?;
        let mut header = [0_u8; 8];
        file.read_exact(&mut header)
            .map_err(|error| error.to_string())?;
        let short_size = u32::from_be_bytes(header[0..4].try_into().unwrap());
        let box_type = &header[4..8];
        let (box_size, header_size) = if short_size == 1 {
            let mut extended = [0_u8; 8];
            file.read_exact(&mut extended)
                .map_err(|error| error.to_string())?;
            (u64::from_be_bytes(extended), 16_u64)
        } else if short_size == 0 {
            (length.saturating_sub(offset), 8_u64)
        } else {
            (u64::from(short_size), 8_u64)
        };
        if box_size < header_size || offset.saturating_add(box_size) > length {
            return Ok(false);
        }
        has_ftyp |= box_type == b"ftyp";
        has_mdat |= box_type == b"mdat";
        has_moov |= box_type == b"moov";
        offset = offset.saturating_add(box_size);
        if short_size == 0 {
            break;
        }
    }
    Ok(offset == length && has_ftyp && has_mdat && has_moov)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "frameflow-recovery-{name}-{}",
                system_time_millis(SystemTime::now()).unwrap_or_default()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn part(&self, id: &str, boxes: &[(&[u8; 4], &[u8])]) -> PathBuf {
            let group = self.0.join(format!("FrameFlow-{id}"));
            fs::create_dir_all(&group).unwrap();
            let part = group.join(PART_FILE_NAME);
            let mut file = File::create(&part).unwrap();
            for (kind, payload) in boxes {
                file.write_all(&((8 + payload.len()) as u32).to_be_bytes())
                    .unwrap();
                file.write_all(*kind).unwrap();
                file.write_all(payload).unwrap();
            }
            part
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn only_complete_top_level_mp4_is_publishable() {
        let directory = TestDirectory::new("boxes");
        let complete = directory.part(
            "1700000000100",
            &[(b"ftyp", b"isom"), (b"mdat", b"video"), (b"moov", b"index")],
        );
        let incomplete =
            directory.part("1700000000101", &[(b"ftyp", b"isom"), (b"mdat", b"video")]);
        assert!(is_publishable_mp4(&complete).unwrap());
        assert!(!is_publishable_mp4(&incomplete).unwrap());
    }

    #[test]
    fn recovery_publishes_complete_parts_and_keeps_incomplete_parts_visible() {
        let directory = TestDirectory::new("publish");
        let complete = directory.part(
            "1700000000102",
            &[(b"ftyp", b"isom"), (b"mdat", b"video"), (b"moov", b"index")],
        );
        let incomplete =
            directory.part("1700000000103", &[(b"ftyp", b"isom"), (b"mdat", b"video")]);
        let recovered = recover_publishable_recordings(std::slice::from_ref(&directory.0)).unwrap();
        assert_eq!(recovered, vec![complete.with_file_name(MAIN_FILE_NAME)]);
        assert!(!complete.exists());
        assert!(incomplete.exists());
        let interrupted = interrupted_recordings(std::slice::from_ref(&directory.0)).unwrap();
        assert_eq!(interrupted.len(), 1);
        assert_eq!(interrupted[0].id, "1700000000103");
    }

    #[test]
    fn interrupted_lookup_rejects_untrusted_ids_and_non_group_paths() {
        let directory = TestDirectory::new("boundaries");
        directory.part("1700000000104", &[(b"ftyp", b"isom")]);
        assert!(interrupted_group_for_id(std::slice::from_ref(&directory.0), "../escape").is_err());
        assert_eq!(
            interrupted_group_for_id(std::slice::from_ref(&directory.0), "1700000000104",).unwrap(),
            directory.0.join("FrameFlow-1700000000104")
        );
    }
}
