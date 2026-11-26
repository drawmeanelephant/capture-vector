//! Shared installers for integration-test fake executables.

/// Never write the executable inode in this process: a sibling fork could
/// inherit that fd and make Linux exec fail with ETXTBSY
/// (rust-lang/rust#114554). Unlike cv-engine's child-copy installer, this
/// stays inside cv-app's process firewall: write and chmod a sibling while
/// it is non-executable, then atomically rename it into place.
#[cfg(unix)]
pub fn install_executable(bin: &std::path::Path, contents: &str) {
    use std::os::unix::fs::PermissionsExt;

    let staging = bin.parent().unwrap().join("staging-contents");
    std::fs::write(&staging, contents).unwrap();
    std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::rename(&staging, bin).unwrap();
}
