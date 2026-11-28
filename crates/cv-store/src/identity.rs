//! Physical file identity shared by lock carriers and content-input
//! inventories: device+inode on Unix, volume serial + file index on
//! Windows. `None` means the platform cannot answer — callers that need
//! identity for a safety decision must fail closed rather than guess.

use std::fs::File;

#[cfg(unix)]
pub(crate) fn identity_of_file(file: &File) -> std::io::Result<Option<(u64, u64)>> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata()?;
    Ok(Some((metadata.dev(), metadata.ino())))
}

#[cfg(windows)]
pub(crate) fn identity_of_file(file: &File) -> std::io::Result<Option<(u64, u64)>> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
    let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, info.as_mut_ptr()) };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    let info = unsafe { info.assume_init() };
    Ok(Some((
        info.dwVolumeSerialNumber.into(),
        ((info.nFileIndexHigh as u64) << 32) | u64::from(info.nFileIndexLow),
    )))
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn identity_of_file(_: &File) -> std::io::Result<Option<(u64, u64)>> {
    Ok(None)
}
