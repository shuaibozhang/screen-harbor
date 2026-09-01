use std::{os::windows::ffi::OsStrExt, path::PathBuf};

use windows::{
    core::{HSTRING, PCWSTR},
    Win32::{
        Foundation::ERROR_CANCELLED,
        System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_INPROC_SERVER},
        UI::Shell::{
            FileOpenDialog, IFileOpenDialog, SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION,
            FOF_NOERRORUI, FOF_SILENT, FOS_FORCEFILESYSTEM, FOS_PATHMUSTEXIST, FOS_PICKFOLDERS,
            FO_DELETE, SHFILEOPSTRUCTW, SIGDN_FILESYSPATH,
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
    if paths.is_empty() {
        return Err("no recording paths were selected".to_owned());
    }
    if paths.iter().any(|path| !path.exists()) {
        return Err("one or more recording paths are no longer available".to_owned());
    }
    let mut sources = Vec::new();
    for path in paths {
        sources.extend(path.as_os_str().encode_wide());
        sources.push(0);
    }
    sources.push(0);
    let flags = FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT;
    let mut operation = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: PCWSTR(sources.as_ptr()),
        fFlags: flags.0 as u16,
        ..Default::default()
    };
    let result = unsafe { SHFileOperationW(&mut operation) };
    if result != 0 {
        return Err(format!(
            "Windows Shell recycle operation failed with code 0x{result:08X}"
        ));
    }
    if operation.fAnyOperationsAborted.as_bool() {
        return Err("Windows Shell recycle operation was cancelled".to_owned());
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
}
