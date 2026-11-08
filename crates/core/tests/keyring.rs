//! The keyring against a real Secret Service: gnome-keyring in a private D-Bus session with a new keyring in a
//! temporary home, so the user's keyring is never used and no prompt can appear.
#![cfg(target_os = "linux")]

use std::env;
use std::fs::DirBuilder;
use std::os::unix::fs::DirBuilderExt;
use std::process::Command;
use std::time::{Duration, Instant};

use icloud_core::keyring::{Keyring, Reply, Request};

const KEY: &str = "keyring-fixture-key";
/// Set only inside the private session, to the temporary home.
const PRIVATE: &str = "ICLOUD_PRIVATE_KEYRING";

fn installed(name: &str) -> bool {
    env::split_paths(&env::var_os("PATH").unwrap_or_default()).any(|folder| folder.join(name).is_file())
}

fn finish(mut request: Request) -> Reply {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(reply) = request.poll() {
            return reply;
        }
        assert!(Instant::now() < deadline, "the keyring did not answer");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn the_key_is_stored_found_and_cleared_in_a_real_secret_service() {
    if !["secret-tool", "dbus-run-session", "gnome-keyring-daemon"]
        .into_iter()
        .all(installed)
    {
        println!("SKIP: this check needs secret-tool, dbus-run-session, and gnome-keyring-daemon.");
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let (home, runtime) = (root.path().join("home"), root.path().join("run"));
    for folder in [&home, &runtime] {
        DirBuilder::new().mode(0o700).create(folder).unwrap();
    }
    // The daemon creates and unlocks a login keyring there; then this test binary runs the ignored test below.
    let script = "printf fixture-unlock-password | gnome-keyring-daemon --unlock --components=secrets >/dev/null \
                  && exec \"$0\" --exact private_secret_service --ignored --nocapture";
    let mut command = Command::new("dbus-run-session");
    command
        .args(["--", "sh", "-c", script])
        .arg(env::current_exe().unwrap())
        .env("HOME", &home)
        .env("XDG_RUNTIME_DIR", &runtime)
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env(PRIVATE, &home);
    for name in [
        "DBUS_SESSION_BUS_ADDRESS",
        "DISPLAY",
        "WAYLAND_DISPLAY",
        "GNOME_KEYRING_CONTROL",
        "SSH_AUTH_SOCK",
    ] {
        command.env_remove(name);
    }
    let output = command.output().unwrap();
    let log = String::from_utf8_lossy(&output.stdout) + String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{log}");
    assert!(log.contains("PASS:"), "{log}");
}

#[test]
#[ignore = "runs inside the private session that the test above starts"]
fn private_secret_service() {
    // Outside that session this would reach the user's keyring, so it does nothing there.
    let Some(home) = env::var_os(PRIVATE) else {
        return;
    };
    assert_eq!(env::var_os("HOME"), Some(home));
    let keyring = Keyring::default();
    assert!(matches!(finish(keyring.lookup()), Reply::Failed));
    assert!(matches!(finish(keyring.store(KEY)), Reply::Done));
    assert!(matches!(finish(keyring.lookup()), Reply::Key(key) if key.as_str() == KEY));
    assert!(matches!(finish(keyring.clear()), Reply::Done));
    assert!(matches!(finish(keyring.lookup()), Reply::Failed));
    println!("PASS: the key was stored, found, and cleared in a private Secret Service.");
}
