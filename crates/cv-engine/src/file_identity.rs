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
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileInformationByHandle, OPEN_EXISTING,
    };

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // Directories require BACKUP_SEMANTICS on Windows. A zero desired-access
    // query is enough for by-handle metadata and avoids opening the workspace
    // for read/write merely to observe identity.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return None;
    }
    let file = unsafe { std::fs::File::from_raw_handle(handle as _) };
    let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
    let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, info.as_mut_ptr()) };
    let info = (ok != 0).then(|| unsafe { info.assume_init() });
    drop(file);
    let info = info?;
    Some((
        info.dwVolumeSerialNumber.into(),
        ((info.nFileIndexHigh as u64) << 32) | u64::from(info.nFileIndexLow),
    ))
}

#[cfg(not(any(unix, windows)))]
pub fn identity(_: &Path) -> Option<(u64, u64)> {
    None
}

/// Identity of an already-open file, so the identity and the bytes read
/// through the same handle describe one storage object.
#[cfg(unix)]
pub fn identity_of_file(file: &std::fs::File) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let m = file.metadata().ok()?;
    Some((m.dev(), m.ino()))
}

#[cfg(windows)]
pub fn identity_of_file(file: &std::fs::File) -> Option<(u64, u64)> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
    let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, info.as_mut_ptr()) };
    let info = (ok != 0).then(|| unsafe { info.assume_init() })?;
    Some((
        info.dwVolumeSerialNumber.into(),
        ((info.nFileIndexHigh as u64) << 32) | u64::from(info.nFileIndexLow),
    ))
}

#[cfg(not(any(unix, windows)))]
pub fn identity_of_file(_: &std::fs::File) -> Option<(u64, u64)> {
    None
}
