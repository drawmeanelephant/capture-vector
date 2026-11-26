//! Platform-specific open helpers for lock files that must not follow a
//! final-component symlink. Parent components are still checked separately;
//! these helpers close the final-component swap window as far as std/os
//! flags allow.

use std::fs::{File, OpenOptions};
use std::path::Path;

/// Open an existing regular file without following a final-component
/// symlink/reparse point. On Windows the reparse point itself may open; it
/// is never the attacker's target, and callers keep their metadata refusal.
pub(crate) fn open_existing_no_follow(path: &Path, write: bool) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    if write {
        options.write(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NOCTTY);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options.open(path)
}

/// Open a file for read/write, creating it atomically when absent. Existing
/// files are opened with final-component no-follow flags rather than
/// `create(true)`, which would follow a swapped symlink.
pub(crate) fn open_create_no_follow(path: &Path) -> std::io::Result<File> {
    for _ in 0..2 {
        match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path)
        {
            Ok(f) => return Ok(f),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                match open_existing_no_follow(path, true) {
                    Ok(f) => return Ok(f),
                    // This implementation never removes lock files; a foreign
                    // removal raced us, so retry as the creator once.
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(e) => return Err(e),
                }
            }
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("lock file {} disappeared while opening", path.display()),
    ))
}
