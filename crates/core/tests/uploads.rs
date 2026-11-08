#![cfg(target_os = "linux")]
mod support;

use std::cell::{Cell, RefCell};
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use icloud_core::connection::Connection;
use icloud_core::paths::Paths;
use icloud_core::process::without_rclone_variables;
use icloud_core::signal::Signal;
use icloud_core::transfers::Transfers;
use icloud_core::uploads::{State, Upload, UploadStore};
use nix::sys::signal::{Signal::SIGKILL, kill};
use nix::unistd::Pid;
use support::{Fixture, FixtureConnection, wait_until};

const CRASH_MODE: &str = "ICLOUD_TEST_CRASH_MODE";
const CRASH_ROOT: &str = "ICLOUD_TEST_CRASH_ROOT";

/// The fixture connection, limited to 128 KiB/s while `limited` holds so that an upload can be interrupted.
struct Slow {
    connection: Rc<FixtureConnection>,
    limited: Cell<bool>,
}

impl Connection for Slow {
    fn paths(&self) -> &Paths {
        self.connection.paths()
    }

    fn unlocked(&self) -> bool {
        self.connection.unlocked()
    }

    fn profile_id(&self) -> String {
        self.connection.profile_id()
    }

    fn rclone(&self) -> Command {
        let mut command = self.connection.rclone();
        if self.limited.get() {
            command.env("RCLONE_BWLIMIT", "128k");
        }
        command
    }

    fn changed(&self) -> &Signal {
        self.connection.changed()
    }
}

fn slow(fixture: &Fixture) -> Rc<Slow> {
    Rc::new(Slow {
        connection: fixture.connection(),
        limited: Cell::new(true),
    })
}

fn uploads(fixture: &Fixture) -> PathBuf {
    fixture.paths.cache.join("uploads")
}

fn verify(fixture: &Fixture, item: &Upload) {
    UploadStore::open(&uploads(fixture))
        .unwrap()
        .snapshots
        .verify(item)
        .unwrap();
}

/// Tick until the queue has `count` rows and the last one finished, then return it.
fn wait_for_row(queue: &Transfers, count: usize) -> Upload {
    let finished = || {
        let items = queue.items();
        items.len() == count
            && matches!(
                items[count - 1].state,
                State::Completed | State::Error | State::Conflict
            )
    };
    wait_until(60, || queue.tick(), finished);
    queue.items().remove(count - 1)
}

/// Pause as the app does before it disconnects, and wait until the running command ended.
fn stop(queue: &Transfers) {
    queue.pause();
    wait_until(30, || queue.tick(), || !queue.busy());
}

#[test]
fn uploads_are_verified_and_conflicts_wait_for_a_decision() {
    let fixture = Fixture::new();
    let queue = Transfers::new(fixture.connection()).unwrap();
    let backend = fixture.source.join("Uploads");
    let source = fixture.root.path().join("new.bin");
    fs::write(&source, b"Queued local upload\n").unwrap();
    queue.add(std::slice::from_ref(&source), "Uploads");
    let item = wait_for_row(&queue, 1);
    assert_eq!(item.state, State::Completed, "{}", queue.message());
    assert_eq!(fs::read(backend.join("new.bin")).unwrap(), fs::read(&source).unwrap());
    let snapshot = uploads(&fixture).join(&item.identifier);
    wait_until(10, || queue.tick(), || !snapshot.exists());

    fs::write(&source, b"Different contents!\n").unwrap();
    let original = fs::read(backend.join("new.bin")).unwrap();
    queue.add(std::slice::from_ref(&source), "Uploads");
    let conflict = wait_for_row(&queue, 2);
    assert_eq!(conflict.state, State::Conflict, "{}", queue.message());
    queue.resolve(&conflict.identifier, true);
    let kept = wait_for_row(&queue, 2);
    assert_eq!(kept.state, State::Completed, "{}", queue.message());
    assert_eq!(fs::read(backend.join("new.bin")).unwrap(), original);
    assert_eq!(fs::read(backend.join(&kept.name)).unwrap(), fs::read(&source).unwrap());

    queue.add(std::slice::from_ref(&source), "Uploads");
    let skipped = wait_for_row(&queue, 3);
    assert_eq!(skipped.state, State::Conflict, "{}", queue.message());
    queue.resolve(&skipped.identifier, false);
    assert_eq!(queue.items()[2].state, State::Skipped);
    assert_eq!(fs::read(backend.join("new.bin")).unwrap(), original);
    stop(&queue);
}

#[test]
fn an_interrupted_upload_resumes_after_a_restart_without_a_duplicate() {
    let fixture = Fixture::new();
    let backend = fixture.source.join("Uploads");
    let pending = fixture.root.path().join("pending.bin");
    fs::write(&pending, b"Interrupted upload fixture\n".repeat(160_000)).unwrap();
    let connection = slow(&fixture);
    let queue = Transfers::new(connection.clone()).unwrap();
    queue.add(std::slice::from_ref(&pending), "Uploads");
    let uploading = || {
        queue
            .items()
            .first()
            .is_some_and(|item| item.state == State::Uploading && item.progress > 0)
    };
    wait_until(60, || queue.tick(), uploading);
    stop(&queue);
    drop(queue);

    connection.limited.set(false);
    let queue = Transfers::new(connection.clone()).unwrap();
    assert_eq!(queue.items()[0].state, State::Paused);
    queue.resume();
    let item = wait_for_row(&queue, 1);
    assert_eq!(item.state, State::Completed, "{}", queue.message());
    assert_eq!(
        fs::read(backend.join("pending.bin")).unwrap(),
        fs::read(&pending).unwrap()
    );
    stop(&queue);
    drop(queue);

    // A snapshot whose upload finished before the journal recorded it finds the identical cloud file.
    let mut store = UploadStore::open(&uploads(&fixture)).unwrap();
    let profile = connection.profile_id();
    let mut duplicate = store
        .snapshots
        .stage(&pending, "Uploads", &profile, &AtomicBool::new(false))
        .unwrap();
    duplicate.state = State::Paused;
    store.items.push(duplicate);
    store.save().unwrap();
    let queue = Transfers::new(connection.clone()).unwrap();
    queue.resume();
    let duplicate = wait_for_row(&queue, 2);
    assert_eq!(duplicate.state, State::Completed, "{}", queue.message());
    let uploaded: Vec<String> = fs::read_dir(&backend)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("pending"))
        .collect();
    assert_eq!(uploaded, ["pending.bin"]);
    stop(&queue);
}

/// A local WebDAV server for the `icloud:` remote, which can stop and start again on the same address.
struct WebDav {
    root: PathBuf,
    address: String,
    process: Option<Child>,
}

impl WebDav {
    fn new(fixture: &Fixture) -> WebDav {
        let root = fixture.root.path().join("server");
        fs::create_dir_all(root.join("data")).unwrap();
        fs::write(root.join("ready.txt"), root.to_str().unwrap()).unwrap();
        let address = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .to_string();
        let url = format!("http://{address}/");
        fixture.rclone(&["config", "delete", "icloud"]);
        fixture.rclone(&[
            "config",
            "create",
            "icloud",
            "webdav",
            "url",
            &url,
            "vendor",
            "other",
            "--no-output",
        ]);
        WebDav {
            root,
            address,
            process: None,
        }
    }

    fn start(&mut self) {
        let log = fs::File::create(self.root.with_file_name("server.log")).unwrap();
        let mut command = Command::new("rclone");
        without_rclone_variables(&mut command)
            .env("RCLONE_CONFIG", self.root.with_file_name("server.conf"))
            .args(["--ask-password=false", "serve", "webdav"])
            .arg(&self.root)
            .args(["--addr", &self.address])
            .stdout(log.try_clone().unwrap())
            .stderr(log);
        self.process = Some(command.spawn().unwrap());
        wait_until(30, || {}, || self.ready());
    }

    /// Whether this server answers with its own marker file.
    fn ready(&self) -> bool {
        let Ok(mut stream) = TcpStream::connect(&self.address) else {
            return false;
        };
        let request = format!("GET /ready.txt HTTP/1.0\r\nHost: {}\r\n\r\n", self.address);
        let mut response = Vec::new();
        stream.set_read_timeout(Some(Duration::from_secs(1))).is_ok()
            && stream.write_all(request.as_bytes()).is_ok()
            && stream.read_to_end(&mut response).is_ok()
            && response.ends_with(self.root.to_str().unwrap().as_bytes())
    }

    fn stop(&mut self) {
        if let Some(mut process) = self.process.take() {
            let _ = process.kill();
            let _ = process.wait();
        }
    }
}

impl Drop for WebDav {
    fn drop(&mut self) {
        self.stop();
    }
}

#[test]
fn a_backend_outage_keeps_the_pending_copy_until_resume() {
    let fixture = Fixture::new();
    let mut server = WebDav::new(&fixture);
    server.start();
    let queue = Transfers::new(fixture.connection()).unwrap();
    server.stop();
    let source = fixture.root.path().join("outage.bin");
    fs::write(&source, b"Queued during a server outage\n").unwrap();
    queue.add(std::slice::from_ref(&source), "data");
    let item = wait_for_row(&queue, 1);
    assert_eq!(item.state, State::Error, "{}", queue.message());
    verify(&fixture, &item);
    assert!(!server.root.join("data/outage.bin").exists());
    server.start();
    queue.resume();
    let item = wait_for_row(&queue, 1);
    assert_eq!(item.state, State::Completed, "{}", queue.message());
    assert_eq!(
        fs::read(server.root.join("data/outage.bin")).unwrap(),
        fs::read(&source).unwrap()
    );
    stop(&queue);
}

#[test]
fn another_profile_leaves_old_rows_paused_and_uploads_only_new_files() {
    let fixture = Fixture::new();
    let connection = fixture.connection();
    let source = fixture.root.path().join("session.bin");
    fs::write(&source, b"Original session fixture\n").unwrap();
    let mut store = UploadStore::open(&uploads(&fixture)).unwrap();
    let mut item = store
        .snapshots
        .stage(&source, "Uploads", &connection.profile_id(), &AtomicBool::new(false))
        .unwrap();
    item.state = State::Paused;
    store.items.push(item.clone());
    store.save().unwrap();
    let queue = Transfers::new(connection.clone()).unwrap();

    connection.set_unlocked(false);
    let other = fixture.root.path().join("other-backend");
    fs::create_dir(&other).unwrap();
    fixture.rclone(&[
        "config",
        "update",
        "icloud",
        "remote",
        other.to_str().unwrap(),
        "--no-output",
    ]);
    connection.set_profile(&"b".repeat(64));
    connection.set_unlocked(true);
    queue.resume();
    assert_eq!(queue.items()[0].state, State::Paused);
    assert!(
        queue
            .message()
            .starts_with("Files from a different saved session remain paused.")
    );
    verify(&fixture, &item);
    assert!(!other.join("Uploads/session.bin").exists());
    assert!(!fixture.source.join("Uploads/session.bin").exists());

    let new_source = fixture.root.path().join("new-session.bin");
    fs::write(&new_source, b"New session fixture\n").unwrap();
    queue.add(std::slice::from_ref(&new_source), "Uploads");
    let added = wait_for_row(&queue, 2);
    assert_eq!(added.state, State::Completed, "{}", queue.message());
    assert_eq!(
        fs::read(other.join("Uploads/new-session.bin")).unwrap(),
        fs::read(&new_source).unwrap()
    );
    assert_eq!(queue.items()[0].state, State::Paused);
    assert!(!other.join("Uploads/session.bin").exists());
    stop(&queue);
}

/// The bytes another device writes in a verification scenario.
fn edited(scenario: &str) -> Vec<u8> {
    let length = match scenario {
        "verify-short" => 2048,
        "verify-long" => 8192,
        _ => 4096,
    };
    vec![b'C'; length]
}

#[test]
fn cloud_changes_before_the_copy_or_during_verification_become_conflicts() {
    let fixture = Fixture::new();
    let queue = Transfers::new(fixture.connection()).unwrap();
    let backend = fixture.source.join("Uploads");
    let (payload, competing) = (vec![b'A'; 4096], vec![b'B'; 4096]);
    // Another device writes the same cloud file when the queue announces the step of the next scenario.
    let injection: Rc<RefCell<Option<&'static str>>> = Rc::default();
    let (weak, scenario, folder, bytes) = (
        Rc::downgrade(&queue),
        injection.clone(),
        backend.clone(),
        competing.clone(),
    );
    queue.changed.connect(move || {
        let Some(item) = weak.upgrade().and_then(|queue| queue.items().pop()) else {
            return;
        };
        let mut scenario = scenario.borrow_mut();
        let written = match (*scenario, item.state) {
            (Some("before-copy"), State::Uploading) => bytes.clone(),
            (Some(name), State::Verifying) if name.starts_with("verify-") => edited(name),
            _ => return,
        };
        fs::write(folder.join(&item.name), written).unwrap();
        *scenario = None;
    });
    for scenario in ["before-copy", "verify-same", "verify-short", "verify-long"] {
        let source = fixture.root.path().join(format!("{scenario}.bin"));
        fs::write(&source, &payload).unwrap();
        let count = queue.items().len() + 1;
        *injection.borrow_mut() = Some(scenario);
        queue.add(&[source], "Uploads");
        let item = wait_for_row(&queue, count);
        assert_eq!(item.state, State::Conflict, "{scenario}: {}", queue.message());
        verify(&fixture, &item);
        let remote = backend.join(&item.name);
        let expected = if scenario == "before-copy" {
            competing.clone()
        } else {
            edited(scenario)
        };
        assert_eq!(fs::read(&remote).unwrap(), expected);
        queue.resume();
        assert_eq!(queue.items()[count - 1].state, State::Conflict);
        queue.resolve(&item.identifier, true);
        let kept = wait_for_row(&queue, count);
        assert_eq!(kept.state, State::Completed, "{scenario}: {}", queue.message());
        assert_eq!(fs::read(&remote).unwrap(), expected);
        assert_eq!(fs::read(backend.join(&kept.name)).unwrap(), payload);
    }
    stop(&queue);
}

/// The fixture of a parent test, from its folder. The placeholder `root` lies inside that folder, which the parent
/// removes.
fn fixture_at(root: &Path) -> Fixture {
    Fixture {
        root: tempfile::tempdir_in(root).unwrap(),
        source: root.join("source"),
        paths: Paths {
            config: root.join("rclone.conf"),
            mount: root.join("mount"),
            cache: root.join("cache"),
            runtime: root.join("runtime"),
            helper: Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/session.sh"),
        },
    }
}

/// Run `crash_child` in a new process of this test binary.
fn spawn_crash_child(mode: &str, root: &Path) -> Child {
    Command::new(env::current_exe().unwrap())
        .args(["crash_child", "--exact", "--ignored", "--nocapture", "--test-threads=1"])
        .env(CRASH_MODE, mode)
        .env(CRASH_ROOT, root)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

/// The child half of the crash tests, which ends killed; it does nothing unless they start it.
#[test]
#[ignore = "started by the crash tests"]
fn crash_child() {
    let (Ok(mode), Some(root)) = (env::var(CRASH_MODE), env::var_os(CRASH_ROOT).map(PathBuf::from)) else {
        return;
    };
    let profile = "a".repeat(64);
    match mode.as_str() {
        "stage" => {
            let store = UploadStore::open(&root.join("queue")).unwrap();
            let source = root.join("original.bin");
            store
                .snapshots
                .stage(&source, "Uploads", &profile, &AtomicBool::new(false))
                .unwrap();
        }
        "conflict" => {
            let mut store = UploadStore::open(&root.join("queue")).unwrap();
            store.items[0].state = State::Conflict;
            store.save().unwrap();
            store.keep_both(0).unwrap();
        }
        "upload" => upload_until_killed(&root),
        _ => panic!("unknown crash mode {mode}"),
    }
    kill(Pid::this(), SIGKILL).unwrap();
}

fn upload_until_killed(root: &Path) -> ! {
    let fixture = fixture_at(root);
    let queue = Transfers::new(slow(&fixture)).unwrap();
    queue.add(&[root.join("restart.bin")], "Uploads");
    let backend = fixture.source.join("Uploads");
    let uploading = || {
        queue.items().first().is_some_and(|item| item.state == State::Uploading)
            && fs::read_dir(&backend).is_ok_and(|entries| {
                entries
                    .flatten()
                    .any(|entry| entry.metadata().is_ok_and(|info| info.len() > 0))
            })
    };
    wait_until(60, || queue.tick(), uploading);
    let transfer = rclone_children(std::process::id())[0];
    fs::write(root.join("started.tmp"), transfer.to_string()).unwrap();
    fs::rename(root.join("started.tmp"), root.join("started")).unwrap();
    loop {
        queue.tick();
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The rclone processes whose parent is `parent`.
fn rclone_children(parent: u32) -> Vec<u32> {
    let children = fs::read_dir("/proc").unwrap().flatten().filter_map(|entry| {
        let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let (name, rest) = stat.rsplit_once(") ")?;
        let ppid: u32 = rest.split(' ').nth(1)?.parse().ok()?;
        (ppid == parent && name.ends_with("(rclone")).then_some(pid)
    });
    children.collect()
}

/// Whether the process is gone or only waits to be reaped.
fn stopped(pid: u32) -> bool {
    fs::read_to_string(format!("/proc/{pid}/stat")).map_or(true, |stat| {
        stat.rsplit_once(") ").is_some_and(|(_, rest)| rest.starts_with('Z'))
    })
}

#[test]
fn a_crash_after_preparation_or_keep_both_leaves_a_recoverable_snapshot() {
    let workspace = tempfile::tempdir().unwrap();
    let source = workspace.path().join("original.bin");
    fs::write(&source, b"Crash recovery fixture\n").unwrap();
    for mode in ["stage", "conflict"] {
        let status = spawn_crash_child(mode, workspace.path()).wait().unwrap();
        assert_eq!(status.signal(), Some(SIGKILL as i32), "{mode}");
        let store = UploadStore::open(&workspace.path().join("queue")).unwrap();
        assert_eq!(store.items.len(), 1);
        let item = &store.items[0];
        assert_eq!(item.state, State::Paused);
        store.snapshots.verify(item).unwrap();
        assert_eq!(
            fs::read(store.snapshots.payload(item)).unwrap(),
            fs::read(&source).unwrap()
        );
        if mode == "conflict" {
            assert_ne!(item.name, "original.bin");
            assert_eq!(item.source_name, "original.bin");
        }
    }
}

#[test]
fn killing_the_app_stops_its_upload_and_the_restarted_queue_recovers_it() {
    let fixture = Fixture::new();
    let root = fixture.root.path();
    assert_eq!(fixture_at(root).paths, fixture.paths);
    let source = root.join("restart.bin");
    fs::write(&source, vec![b'R'; 16 * 1024 * 1024]).unwrap();
    let mut child = spawn_crash_child("upload", root);
    let started = root.join("started");
    wait_until(90, || {}, || started.exists() || child.try_wait().unwrap().is_some());
    assert!(
        child.try_wait().unwrap().is_none(),
        "the upload child exited before the crash point"
    );
    let transfer: u32 = fs::read_to_string(&started).unwrap().parse().unwrap();
    child.kill().unwrap();
    child.wait().unwrap();
    wait_until(5, || {}, || stopped(transfer));

    let queue = Transfers::new(fixture.connection()).unwrap();
    let item = queue.items().remove(0);
    assert_eq!((queue.items().len(), item.state), (1, State::Paused));
    verify(&fixture, &item);
    queue.resume();
    let item = wait_for_row(&queue, 1);
    assert_eq!(item.state, State::Completed, "{}", queue.message());
    assert_eq!(
        fs::read(fixture.source.join("Uploads/restart.bin")).unwrap(),
        fs::read(&source).unwrap()
    );
    stop(&queue);
}
