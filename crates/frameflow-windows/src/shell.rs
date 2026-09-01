use std::{
    ffi::OsString,
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
};

use windows::{
    core::HSTRING,
    Win32::{
        Foundation::ERROR_CANCELLED,
        System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_INPROC_SERVER},
        UI::Shell::{
            FileOpenDialog, FileOperation, IFileOpenDialog, IFileOperation, IShellItem,
            SHCreateItemFromParsingName, FOFX_RECYCLEONDELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION,
            FOF_NOERRORUI, FOF_SILENT, FOS_FORCEFILESYSTEM, FOS_PATHMUSTEXIST, FOS_PICKFOLDERS,
            SIGDN_FILESYSPATH,
        },
    },
};

use crate::com::ComApartment;

pub fn choose_recording_directory() -> Result<Option<PathBuf>, String> {
    let _apartment = ComApartment::enter_apartment_threaded().map_err(|error| error.to_string())?;
    let dialog: IFileOpenDialog = unsafe {
        CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)
            .map_err(|error| error.to_string())?
    };
    unsafe {
        let options = dialog.GetOptions().map_err(|error| error.to_string())?;
        dialog
            .SetOptions(options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST)
            .map_err(|error| error.to_string())?;
        dialog
            .SetTitle(&HSTRING::from("选择 FrameFlow 录像保存位置"))
            .map_err(|error| error.to_string())?;
        if let Err(error) = dialog.Show(None) {
            if error.code() == windows::core::HRESULT::from_win32(ERROR_CANCELLED.0) {
                return Ok(None);
            }
            return Err(error.to_string());
        }
        let item = dialog.GetResult().map_err(|error| error.to_string())?;
        let path = item
            .GetDisplayName(SIGDN_FILESYSPATH)
            .map_err(|error| error.to_string())?;
        let value = path.to_string().map_err(|error| error.to_string());
        CoTaskMemFree(Some(path.0.cast()));
        value.map(PathBuf::from).map(Some)
    }
}

pub fn move_paths_to_recycle_bin(paths: &[PathBuf]) -> Result<(), String> {
    validate_recycle_paths(paths)?;

    let _apartment = ComApartment::enter_apartment_threaded().map_err(|error| error.to_string())?;
    let operation: IFileOperation = unsafe {
        CoCreateInstance(&FileOperation, None, CLSCTX_INPROC_SERVER)
            .map_err(|error| format!("failed to create Windows recycle operation: {error}"))?
    };
    let flags =
        FOF_ALLOWUNDO | FOFX_RECYCLEONDELETE | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT;
    unsafe {
        operation
            .SetOperationFlags(flags)
            .map_err(|error| format!("failed to configure Windows recycle operation: {error}"))?;
        for path in paths {
            let shell_path = HSTRING::from(shell_parsing_path(path));
            let item: IShellItem =
                SHCreateItemFromParsingName(&shell_path, None).map_err(|error| {
                    format!(
                        "Windows Shell could not resolve recycle target '{}': {error}",
                        path.display()
                    )
                })?;
            operation.DeleteItem(&item, None).map_err(|error| {
                format!(
                    "failed to queue '{}' for the Windows Recycle Bin: {error}",
                    path.display()
                )
            })?;
        }
        operation
            .PerformOperations()
            .map_err(|error| format!("Windows recycle operation failed: {error}"))?;
    }
    if unsafe { operation.GetAnyOperationsAborted() }
        .map_err(|error| format!("failed to read Windows recycle operation status: {error}"))?
        .as_bool()
    {
        return Err("Windows Shell recycle operation was cancelled".to_owned());
    }
    Ok(())
}

fn shell_parsing_path(path: &Path) -> OsString {
    const VERBATIM_PREFIX: &[u16] = &[b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16];
    const VERBATIM_UNC_PREFIX: &[u16] = &[
        b'\\' as u16,
        b'\\' as u16,
        b'?' as u16,
        b'\\' as u16,
        b'U' as u16,
        b'N' as u16,
        b'C' as u16,
        b'\\' as u16,
    ];

    let path = path.as_os_str().encode_wide().collect::<Vec<_>>();
    if path.starts_with(VERBATIM_UNC_PREFIX) {
        let mut normalized = vec![b'\\' as u16, b'\\' as u16];
        normalized.extend_from_slice(&path[VERBATIM_UNC_PREFIX.len()..]);
        OsString::from_wide(&normalized)
    } else if path.starts_with(VERBATIM_PREFIX) {
        OsString::from_wide(&path[VERBATIM_PREFIX.len()..])
    } else {
        OsString::from_wide(&path)
    }
}

fn validate_recycle_paths(paths: &[PathBuf]) -> Result<(), String> {
    if paths.is_empty() {
        return Err("no recording paths were selected".to_owned());
    }
    if paths.iter().any(|path| !path.is_absolute()) {
        return Err("one or more recording paths are not absolute".to_owned());
    }
    if paths.iter().any(|path| path.parent().is_none()) {
        return Err("refusing to move a filesystem root to the Recycle Bin".to_owned());
    }
    if paths.iter().any(|path| !path.exists()) {
        return Err("one or more recording paths are no longer available".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_recycle_request_is_rejected_before_calling_windows() {
        assert_eq!(
            move_paths_to_recycle_bin(&[]).unwrap_err(),
            "no recording paths were selected"
        );
    }

    #[test]
    fn relative_recycle_target_is_rejected() {
        assert_eq!(
            validate_recycle_paths(&[PathBuf::from("recording")]).unwrap_err(),
            "one or more recording paths are not absolute"
        );
    }

    #[test]
    fn filesystem_root_is_rejected() {
        assert_eq!(
            validate_recycle_paths(&[PathBuf::from(r"C:\")]).unwrap_err(),
            "refusing to move a filesystem root to the Recycle Bin"
        );
    }

    #[test]
    fn shell_path_removes_verbatim_drive_prefix() {
        assert_eq!(
            shell_parsing_path(Path::new(r"\\?\C:\Users\录制\FrameFlow-1")),
            OsString::from(r"C:\Users\录制\FrameFlow-1")
        );
    }

    #[test]
    fn shell_path_converts_verbatim_unc_prefix() {
        assert_eq!(
            shell_parsing_path(Path::new(r"\\?\UNC\server\share\FrameFlow-1")),
            OsString::from(r"\\server\share\FrameFlow-1")
        );
    }

    #[test]
    fn shell_path_preserves_standard_path() {
        assert_eq!(
            shell_parsing_path(Path::new(r"C:\Users\录制\FrameFlow-1")),
            OsString::from(r"C:\Users\录制\FrameFlow-1")
        );
    }
}
