//! Shared fixtures for tests that run the real rclone against a fictional local `icloud:` remote. Linux only.
#![allow(dead_code)]

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::time::{Duration, Instant};

use icloud_core::connection::Connection;
use icloud_core::paths::Paths;
use icloud_core::process::without_rclone_variables;
use icloud_core::signal::Signal;

pub const KEY: &str = "native-fixture-key";

/// A private folder with an encrypted rclone profile whose `icloud:` remote is the local `source` folder.
pub struct Fixture {
    pub root: tempfile::TempDir,
    pub source: PathBuf,
    pub paths: Paths,
}

impl Fixture {
    pub fn new() -> Fixture {
        let root = tempfile::Builder::new()
            .prefix("icloud-rust-fixture-")
            .tempdir()
            .unwrap();
        let source = root.path().join("source");
        std::fs::create_dir(&source).unwrap();
        let config = root.path().join("rclone.conf");
        let helper = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/session.sh");
        let paths = Paths {
            config: config.clone(),
            mount: root.path().join("mount"),
            cache: root.path().join("cache"),
            runtime: root.path().join("runtime"),
            helper,
        };
        let fixture = Fixture { root, source, paths };
        fixture.rclone(&[
            "config",
            "encryption",
            "set",
            "--password-command",
            "printenv RCLONE_CONFIG_PASS",
        ]);
        let source = fixture.source.to_str().unwrap().to_owned();
        fixture.rclone(&["config", "create", "icloud", "alias", "remote", &source, "--no-output"]);
        fixture
    }

    /// Run rclone with the fixture profile and key, failing the test on an error.
    pub fn rclone(&self, arguments: &[&str]) -> Vec<u8> {
        let mut command = Command::new("rclone");
        without_rclone_variables(&mut command)
            .env("RCLONE_CONFIG", &self.paths.config)
            .env("RCLONE_CONFIG_PASS", KEY)
            .arg("--ask-password=false")
            .args(arguments);
        let output = command.output().expect("rclone must be installed for these tests");
        assert!(
            output.status.success(),
            "rclone {arguments:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }

    /// A connection that is unlocked from the start and uses the fixture profile directly.
    pub fn connection(&self) -> Rc<FixtureConnection> {
        Rc::new(FixtureConnection {
            paths: self.paths.clone(),
            unlocked: Cell::new(true),
            profile: RefCell::new("f".repeat(64)),
            changed: Signal::default(),
        })
    }
}

pub struct FixtureConnection {
    paths: Paths,
    unlocked: Cell<bool>,
    profile: RefCell<String>,
    changed: Signal,
}

impl FixtureConnection {
    pub fn set_unlocked(&self, unlocked: bool) {
        self.unlocked.set(unlocked);
        self.changed.emit();
    }

    pub fn set_profile(&self, profile: &str) {
        *self.profile.borrow_mut() = profile.to_owned();
        self.changed.emit();
    }
}

impl Connection for FixtureConnection {
    fn paths(&self) -> &Paths {
        &self.paths
    }

    fn unlocked(&self) -> bool {
        self.unlocked.get()
    }

    fn profile_id(&self) -> String {
        self.profile.borrow().clone()
    }

    fn rclone(&self) -> Command {
        let mut command = Command::new("rclone");
        without_rclone_variables(&mut command)
            .env("RCLONE_CONFIG", &self.paths.config)
            .env("RCLONE_CONFIG_PASS", KEY);
        command
    }

    fn changed(&self) -> &Signal {
        &self.changed
    }
}

/// Call `tick` until `done` holds, failing after `seconds`.
pub fn wait_until(seconds: u64, mut tick: impl FnMut(), mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while !done() {
        assert!(
            Instant::now() < deadline,
            "the condition did not hold within {seconds} seconds"
        );
        tick();
        std::thread::sleep(Duration::from_millis(10));
    }
}
