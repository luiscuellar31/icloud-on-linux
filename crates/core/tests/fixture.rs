#![cfg(target_os = "linux")]
mod support;

use icloud_core::connection::Connection;

#[test]
fn the_fixture_profile_is_encrypted_and_reaches_the_local_remote() {
    let fixture = support::Fixture::new();
    std::fs::write(fixture.source.join("sample.txt"), "Fixture file\n").unwrap();
    icloud_core::profile::check_profile(&fixture.paths.config).unwrap();
    let listing = fixture.rclone(&["lsf", "icloud:"]);
    assert_eq!(listing, b"sample.txt\n");
    let connection = fixture.connection();
    let output = connection
        .rclone()
        .args(["--ask-password=false", "cat", "icloud:sample.txt"])
        .output()
        .unwrap();
    assert_eq!(output.stdout, b"Fixture file\n");
}
