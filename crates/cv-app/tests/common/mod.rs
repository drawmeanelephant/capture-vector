//! Shared installers for integration-test fake executables.

/// Never open the executable inode for writing in this process: a sibling
/// fork could inherit that fd and make Linux exec fail with ETXTBSY
/// (rust-lang/rust#114554). The destination writer belongs only to `cp`.
#[cfg(unix)]
pub fn install_executable(bin: &std::path::Path, contents: &str) {
    use std::os::unix::fs::PermissionsExt;

    let staging = bin.parent().unwrap().join("staging-contents");
    std::fs::write(&staging, contents).unwrap();
    let status = std::process::Command::new("cp")
        .arg(&staging)
        .arg(bin)
        .status()
        .unwrap();
    assert!(status.success(), "cp into the executable path failed");
    std::fs::remove_file(&staging).unwrap();
    std::fs::set_permissions(bin, std::fs::Permissions::from_mode(0o755)).unwrap();
}
