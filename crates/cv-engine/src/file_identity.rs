//! Stable file identity for files open elsewhere in the workspace.
//! Device+file-index identity distinguishes case-only aliases on
//! case-insensitive volumes from genuinely distinct case-only files.

use std::path::Path;

#[cfg(unix)]
pub fn identity(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(path).ok()?;
    Some((m.dev(), m.ino()))
}

#[cfg(windows)]
pub fn identity(path: &Path) -> Option<(u64, u64)> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    let file = std::fs::File::open(path).ok()?;
    let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
    let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, info.as_mut_ptr()) };
    if ok == 0 {
        return None;
    }
    let info = unsafe { info.assume_init() };
    Some((
        info.dwVolumeSerialNumber.into(),
        ((info.nFileIndexHigh as u64) << 32) | u64::from(info.nFileIndexLow),
    ))
}

#[cfg(not(any(unix, windows)))]
pub fn identity(_: &Path) -> Option<(u64, u64)> {
    None
}
