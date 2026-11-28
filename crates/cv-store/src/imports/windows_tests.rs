use super::*;

fn create_junction(target: &Path, link: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
        FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::IO::DeviceIoControl;
    use windows_sys::Win32::System::Ioctl::FSCTL_SET_REPARSE_POINT;

    const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
    const GENERIC_WRITE: u32 = 0x4000_0000;

    let absolute = std::fs::canonicalize(target)?;
    let native = absolute.to_string_lossy().replace("\\\\?\\", "");
    let substitute: Vec<u16> = format!("\\??\\{native}")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let print: Vec<u16> = native.encode_utf16().chain(std::iter::once(0)).collect();

    std::fs::create_dir(link)?;

    let substitute_bytes = substitute.len() * 2;
    let print_bytes = print.len() * 2;
    let data_len = 8 + substitute_bytes + print_bytes;
    let mut buffer: Vec<u8> = Vec::with_capacity(8 + data_len);
    buffer.extend_from_slice(&IO_REPARSE_TAG_MOUNT_POINT.to_le_bytes());
    buffer.extend_from_slice(&(data_len as u16).to_le_bytes());
    buffer.extend_from_slice(&0u16.to_le_bytes());
    buffer.extend_from_slice(&0u16.to_le_bytes());
    buffer.extend_from_slice(&((substitute_bytes - 2) as u16).to_le_bytes());
    buffer.extend_from_slice(&(substitute_bytes as u16).to_le_bytes());
    buffer.extend_from_slice(&((print_bytes - 2) as u16).to_le_bytes());
    for unit in substitute.iter().chain(print.iter()) {
        buffer.extend_from_slice(&unit.to_le_bytes());
    }

    let wide: Vec<u16> = link
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        let error = std::io::Error::last_os_error();
        let _ = std::fs::remove_dir(link);
        return Err(error);
    }
    let mut returned = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            handle,
            FSCTL_SET_REPARSE_POINT,
            buffer.as_ptr().cast(),
            buffer.len() as u32,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    let result = if ok == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    };
    unsafe {
        CloseHandle(handle);
    }
    if result.is_err() {
        let _ = std::fs::remove_dir(link);
    }
    result
}

fn write_adoption_and_pending(root: &Path, run: &str) {
    let lock = claim_run(root, run).unwrap();
    let adoption = AdoptionRecord::new(run, "hugo", 5, "workspace", "existing");
    write_adoption(&lock, &adoption).unwrap();
    let pending = PendingAdoption::new(run, "existing", "content");
    write_pending(&lock, &pending).unwrap();
}

#[test]
fn a_junctioned_imports_dir_is_refused() {
    let d = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join(".cv")).unwrap();
    create_junction(elsewhere.path(), &d.path().join(DIR)).unwrap();
    assert!(matches!(scan(d.path()), Err(Error::Symlink(_))));
    assert!(matches!(ReviewLog::open(d.path()), Err(Error::Symlink(_))));
}

#[test]
fn a_junctioned_run_entry_is_skipped_by_scan() {
    let d = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let base = d.path().join(DIR);
    std::fs::create_dir_all(&base).unwrap();
    create_junction(elsewhere.path(), &base.join("junction-run")).unwrap();
    assert!(scan(d.path()).unwrap().is_empty());
}

#[test]
fn direct_adoption_readers_refuse_a_junctioned_run_directory() {
    let d = tempfile::tempdir().unwrap();
    write_adoption_and_pending(d.path(), "run");
    let base = d.path().join(DIR);
    let run = base.join("run");
    let moved = d.path().join("moved-run");
    std::fs::rename(&run, &moved).unwrap();
    create_junction(&moved, &run).unwrap();

    for err in [
        read_adoption_record(d.path(), "run").unwrap_err(),
        read_pending(d.path(), "run").unwrap_err(),
    ] {
        assert!(err.contains("refusing symlinked"), "{err}");
    }
    assert!(scan(d.path()).unwrap().is_empty());
    assert!(scan_adoptions(d.path()).unwrap().is_empty());
}

#[test]
fn direct_adoption_readers_refuse_a_junctioned_imports_dir() {
    let d = tempfile::tempdir().unwrap();
    write_adoption_and_pending(d.path(), "run");
    let imports = d.path().join(DIR);
    let moved = d.path().join(".cv/imports-real");
    std::fs::rename(&imports, &moved).unwrap();
    create_junction(&moved, &imports).unwrap();

    for err in [
        read_adoption_record(d.path(), "run").unwrap_err(),
        read_pending(d.path(), "run").unwrap_err(),
    ] {
        assert!(err.contains("refusing symlinked"), "{err}");
    }
}
