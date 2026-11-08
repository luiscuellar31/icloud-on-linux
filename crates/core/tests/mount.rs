#![cfg(target_os = "linux")]
//! A real read-only FUSE mount of the fictional profile through rclone.
mod support;

use std::fs;
use std::process::Command;

use icloud_core::session::{Session, State};

/// Unmount the fixture if a failed assertion leaves it mounted.
struct Unmount<'a>(&'a std::path::Path);

impl Drop for Unmount<'_> {
    fn drop(&mut self) {
        let _ = Command::new("fusermount3").arg("-uz").arg(self.0).status();
    }
}

#[test]
fn a_read_only_mount_opens_files_rejects_writes_and_disconnects() {
    let fixture = support::Fixture::new();
    fs::write(fixture.source.join("sample.txt"), "Native desktop fixture\n").unwrap();
    let original = fs::read(&fixture.paths.config).unwrap();
    let _guard = Unmount(&fixture.paths.mount);
    let session = Session::new(fixture.paths.clone());
    session.start(support::KEY);
    support::wait_until(
        60,
        || session.tick(),
        || matches!(session.state(), State::Available | State::Error),
    );
    assert_eq!(session.state(), State::Available, "{}", session.message());
    let mount = session.current_mount().unwrap();
    assert!(session.owns_mount() && mount.compatible() && mount.options.contains("ro"));
    let sample = fs::read_to_string(fixture.paths.mount.join("sample.txt")).unwrap();
    assert_eq!(sample, "Native desktop fixture\n");
    let refused = fs::File::create_new(fixture.paths.mount.join("forbidden.txt")).unwrap_err();
    assert_eq!(refused.raw_os_error(), Some(nix::libc::EROFS));
    session.stop();
    support::wait_until(
        30,
        || session.tick(),
        || session.state() == State::Disconnected && !session.running(),
    );
    assert!(session.current_mount().is_none());
    assert_eq!(fs::read(&fixture.paths.config).unwrap(), original);
    assert!(!fixture.source.join("forbidden.txt").exists());
}
