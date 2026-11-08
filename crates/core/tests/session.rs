//! The connection controller against stand-ins for rclone and fusermount3; mounts are read from a marker file.
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use icloud_core::connection::Connection;
use icloud_core::paths::Paths;
use icloud_core::profile::Mount;
use icloud_core::session::{Session, State};

const KEY: &str = "native-fixture-key";
const PROFILE: &[u8] = b"RCLONE_ENCRYPT_V0:\nfictional-config\n";
const STUB: &str = r#"#!/bin/sh
name=$(basename "$0")
printf '%s %s\n' "$name" "$*" >> "@LOG@"
if [ "$name" = fusermount3 ]; then
    [ -e "@BUSY@" ] && exit 1
    rm -f "@MARKER@"
    exit 0
fi
[ "$1" = --ask-password=false ] || exit 90
shift
case "$1" in
    listremotes)
        [ "$RCLONE_CONFIG_PASS" = native-fixture-key ] || exit 1
        echo icloud: ;;
    mount)
        [ "$RCLONE_CONFIG_PASS" = native-fixture-key ] || exit 91
        [ -e "@FAILS@" ] && exit 1
        while [ "$#" -gt 0 ]; do
            [ "$1" = --devname ] && device=$2
            shift
        done
        printf '%s' "$device" > "@MARKER@"
        while [ -e "@MARKER@" ]; do
            # Like rclone, report the refused cookies first, then end the mount.
            if [ -e "@PCS@" ]; then
                echo 'requestPCS(fixture): server returned success but cookies still missing: []' >&2
                rm -f "@MARKER@"
                exit 1
            fi
            sleep 0.05
        done ;;
    *) exit 92 ;;
esac
"#;

struct Bench {
    root: tempfile::TempDir,
    paths: Paths,
    session: Rc<Session>,
}

impl Bench {
    fn new() -> Bench {
        let root = tempfile::Builder::new().prefix("icloud-session-").tempdir().unwrap();
        let bin = root.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let script = STUB
            .replace("@LOG@", root.path().join("calls.log").to_str().unwrap())
            .replace("@BUSY@", root.path().join("busy").to_str().unwrap())
            .replace("@FAILS@", root.path().join("mount-fails").to_str().unwrap())
            .replace("@PCS@", root.path().join("pcs-refused").to_str().unwrap())
            .replace("@MARKER@", root.path().join("mounted").to_str().unwrap());
        for name in ["rclone", "fusermount3"] {
            fs::write(bin.join(name), &script).unwrap();
            fs::set_permissions(bin.join(name), fs::Permissions::from_mode(0o755)).unwrap();
        }
        let config = root.path().join("account.conf");
        fs::write(&config, PROFILE).unwrap();
        fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();
        let paths = Paths {
            config,
            mount: root.path().join("mount"),
            cache: root.path().join("cache"),
            runtime: root.path().join("runtime"),
            helper: Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/session.sh"),
        };
        let session = Session::new(paths.clone());
        let search = std::env::join_paths(
            [bin]
                .into_iter()
                .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        );
        session.set_search_path(Some(search.unwrap()));
        let marker = root.path().join("mounted");
        let uid = nix::unistd::getuid();
        session.set_mount_reader(move |path| {
            let source = fs::read_to_string(&marker).ok()?;
            Some(Mount {
                identifier: 42,
                path: path.to_path_buf(),
                source,
                filesystem: "fuse.rclone".into(),
                options: ["ro".to_owned(), format!("user_id={uid}")].into(),
            })
        });
        Bench { root, paths, session }
    }

    fn file(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }

    fn wait(&self, done: impl Fn(&Session) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done(&self.session) {
            assert!(
                Instant::now() < deadline,
                "timed out in {:?}: {}",
                self.session.state(),
                self.session.message()
            );
            self.session.tick();
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// How many times rclone was asked to mount.
    fn mounts(&self) -> usize {
        fs::read_to_string(self.file("calls.log"))
            .unwrap_or_default()
            .lines()
            .filter(|line| line.starts_with("rclone --ask-password=false mount "))
            .count()
    }

    fn connect(&self) {
        self.session.start(KEY);
        assert_eq!(self.session.state(), State::Connecting);
        self.wait(|session| session.state() == State::Available);
    }

    fn disconnect(&self) {
        let _ = fs::remove_file(self.file("busy"));
        self.session.stop();
        self.wait(|session| !session.running() && session.state() != State::Disconnecting);
    }
}

#[test]
fn unlock_mount_and_disconnect_without_modifying_the_saved_profile() {
    let bench = Bench::new();
    bench.connect();
    assert!(bench.session.unlocked() && bench.session.owns_mount());
    assert!(!bench.session.profile_id().is_empty());
    bench.session.stop();
    bench.wait(|session| session.state() == State::Disconnected);
    assert_eq!(fs::read(&bench.paths.config).unwrap(), PROFILE);
    assert!(!bench.file("mounted").exists());
    assert!(!bench.session.unlocked() && bench.session.profile_id().is_empty());
    assert!(!fs::read_to_string(bench.file("calls.log")).unwrap().contains(KEY));
    assert_eq!(
        fs::read_dir(bench.paths.runtime).unwrap().count(),
        0,
        "the private profile copy remains"
    );
}

#[test]
fn a_wrong_key_is_a_safe_error_and_starts_no_mount() {
    let bench = Bench::new();
    bench.session.start("wrong-fixture-key");
    bench.wait(|session| session.state() == State::Error);
    assert!(bench.session.message().contains("local key"));
    assert!(!bench.session.client_running() && !bench.file("mounted").exists());
    assert_eq!(fs::read(&bench.paths.config).unwrap(), PROFILE);
}

#[test]
fn a_busy_unmount_keeps_the_client() {
    let bench = Bench::new();
    bench.connect();
    fs::write(bench.file("busy"), "").unwrap();
    bench.session.stop();
    bench.wait(|session| session.state() == State::Error);
    assert!(bench.file("mounted").exists() && bench.session.client_running());
    bench.disconnect();
}

#[test]
fn a_replaced_mount_is_not_disconnected() {
    let bench = Bench::new();
    bench.connect();
    let device = fs::read_to_string(bench.file("mounted")).unwrap();
    fs::write(bench.file("mounted"), "other:").unwrap();
    bench.session.tick();
    bench.session.stop();
    assert_eq!(bench.session.state(), State::Error);
    assert!(bench.file("mounted").exists());
    assert!(
        !fs::read_to_string(bench.file("calls.log"))
            .unwrap()
            .contains("fusermount3")
    );
    fs::write(bench.file("mounted"), device).unwrap();
    bench.disconnect();
}

#[test]
fn connecting_can_be_cancelled_before_a_mount_exists() {
    let bench = Bench::new();
    bench.session.start(KEY);
    bench.session.stop();
    bench.wait(|session| session.state() == State::Disconnected && !session.running());
    assert!(!bench.file("mounted").exists());
    assert!(!bench.session.unlocked());
}

#[test]
fn a_dropped_connection_reconnects_with_the_same_key() {
    let bench = Bench::new();
    bench.session.set_reconnect_delays(&[Duration::ZERO, Duration::ZERO]);
    bench.connect();
    // The fixture mount and its client end together, as when rclone exits on its own.
    fs::remove_file(bench.file("mounted")).unwrap();
    bench.wait(|session| session.state() != State::Available);
    bench.wait(|session| session.state() == State::Available);
    assert_eq!(bench.mounts(), 2);
    assert!(bench.session.unlocked());
    assert!(!fs::read_to_string(bench.file("calls.log")).unwrap().contains(KEY));
    bench.disconnect();
}

#[test]
fn reconnection_gives_up_after_a_few_attempts_and_forgets_the_key() {
    let bench = Bench::new();
    bench.session.set_reconnect_delays(&[Duration::ZERO, Duration::ZERO]);
    bench.connect();
    fs::write(bench.file("mount-fails"), "").unwrap();
    fs::remove_file(bench.file("mounted")).unwrap();
    bench.wait(|session| {
        bench.mounts() == 3 && session.state() == State::Error && !session.reconnecting() && !session.client_running()
    });
    assert!(
        bench.session.message().contains("Check your network"),
        "{}",
        bench.session.message()
    );
    // Nothing is left to reconnect with: the key is forgotten.
    for _ in 0..20 {
        bench.session.tick();
    }
    assert_eq!(bench.mounts(), 3);
    assert!(!bench.session.unlocked() && bench.session.profile_id().is_empty());
}

#[test]
fn a_first_connection_that_fails_is_not_retried() {
    let bench = Bench::new();
    bench.session.set_reconnect_delays(&[Duration::ZERO, Duration::ZERO]);
    fs::write(bench.file("mount-fails"), "").unwrap();
    bench.session.start(KEY);
    bench.wait(|session| session.state() == State::Error && !session.client_running());
    for _ in 0..20 {
        bench.session.tick();
        std::thread::sleep(Duration::from_millis(25));
    }
    assert_eq!(bench.mounts(), 1);
    assert!(!bench.session.reconnecting());
}

#[test]
fn disconnecting_cancels_a_pending_reconnection() {
    let bench = Bench::new();
    bench.connect();
    fs::remove_file(bench.file("mounted")).unwrap();
    bench.wait(|session| session.reconnecting());
    assert!(
        bench
            .session
            .message()
            .contains("Reconnecting automatically in 30 seconds"),
        "{}",
        bench.session.message()
    );
    bench.session.stop();
    bench.wait(|session| session.state() == State::Disconnected);
    assert!(!bench.session.reconnecting());
    assert_eq!(bench.mounts(), 1);
}

#[test]
fn an_advanced_data_protection_refusal_explains_web_access_and_is_not_retried() {
    let bench = Bench::new();
    bench.session.set_reconnect_delays(&[Duration::ZERO, Duration::ZERO]);
    bench.connect();
    fs::write(bench.file("pcs-refused"), "").unwrap();
    bench.wait(|session| session.state() == State::Error && !session.client_running());
    for _ in 0..20 {
        bench.session.tick();
        std::thread::sleep(Duration::from_millis(25));
    }
    let message = bench.session.message();
    assert!(message.contains("Advanced Data Protection"), "{message}");
    assert!(!message.contains("requestPCS"));
    assert!(!bench.session.reconnecting());
    assert_eq!(bench.mounts(), 1);
}
