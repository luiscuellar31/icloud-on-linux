//! Synced folders with real rclone bisync between a private local folder and a fictional `icloud:` folder.
#![cfg(target_os = "linux")]
mod support;

use std::cell::Cell;
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::time::{Duration, SystemTime};

use icloud_core::connection::Connection;
use icloud_core::paths::Paths;
use icloud_core::signal::Signal;
use icloud_core::sync::{Review, State, SyncedFolders};
use support::{Fixture, FixtureConnection, wait_until};

/// The fixture connection; while `limited`, transfers are slow enough to kill a run midway.
struct Limited {
    inner: Rc<FixtureConnection>,
    limited: Cell<bool>,
}

impl Connection for Limited {
    fn paths(&self) -> &Paths {
        self.inner.paths()
    }

    fn unlocked(&self) -> bool {
        self.inner.unlocked()
    }

    fn profile_id(&self) -> String {
        self.inner.profile_id()
    }

    fn rclone(&self) -> Command {
        let mut command = self.inner.rclone();
        if self.limited.get() {
            command.args(["--bwlimit", "2M"]);
        }
        command
    }

    fn changed(&self) -> &Signal {
        self.inner.changed()
    }
}

/// Every entry under `root`, with the bytes of its files.
fn tree(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    let mut entries = BTreeMap::new();
    let mut folders = vec![root.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for entry in fs::read_dir(&folder).unwrap() {
            let path = entry.unwrap().path();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            if path.is_dir() {
                entries.insert(relative, None);
                folders.push(path);
            } else {
                entries.insert(relative, Some(fs::read(&path).unwrap()));
            }
        }
    }
    entries
}

fn assert_same(local: &Path, remote: &Path) {
    let (local, remote) = (tree(local), tree(remote));
    assert_eq!(local.keys().collect::<Vec<_>>(), remote.keys().collect::<Vec<_>>());
    assert!(local == remote, "the file contents differ");
}

fn any_named(root: &Path, suffix: &str) -> bool {
    tree(root).keys().any(|path| path.to_string_lossy().ends_with(suffix))
}

fn random(size: usize) -> Vec<u8> {
    let mut bytes = vec![0; size];
    File::open("/dev/urandom").unwrap().read_exact(&mut bytes).unwrap();
    bytes
}

fn age(path: &Path, seconds: u64) {
    let file = File::options().write(true).open(path).unwrap();
    file.set_modified(SystemTime::now() - Duration::from_secs(seconds))
        .unwrap();
}

#[test]
fn synced_folders_follow_changes_on_both_sides_with_real_bisync() {
    let fixture = Fixture::new();
    let remote = fixture.source.join("Docs");
    fs::create_dir_all(remote.join("Reports")).unwrap();
    fs::write(remote.join("Reports/q1.txt"), "First quarter\n").unwrap();
    fs::write(remote.join("notes.txt"), "Shared notes\n").unwrap();
    for index in 0..8 {
        fs::write(remote.join(format!("bulk{index}.txt")), format!("Bulk {index}\n")).unwrap();
    }
    let connection = Rc::new(Limited {
        inner: fixture.connection(),
        limited: Cell::new(false),
    });
    let sync = SyncedFolders::new(connection.clone()).unwrap();
    assert!(sync.add("Docs"), "{}", sync.message());
    let identifier = sync.folders()[0].identifier.clone();
    let folder = || sync.folder(&identifier).unwrap();
    let local = folder().local;
    let work = fixture.paths.cache.join("sync").join(&identifier).join("work");
    let settle = |state: State| wait_until(90, || sync.tick(), || !sync.busy() && folder().state == state);
    let run = |state: State| {
        sync.sync_now(Some(&identifier));
        settle(state);
    };

    settle(State::UpToDate);
    assert_same(&local, &remote);
    assert!(folder().ready);

    // Local create, edit, rename, empty folder, and delete reach iCloud automatically; editor locks stay local.
    fs::write(local.join("created.txt"), "Created in Files\n").unwrap();
    fs::write(local.join("notes.txt"), "Edited in Dolphin\n").unwrap();
    fs::rename(local.join("Reports/q1.txt"), local.join("Reports/q1-final.txt")).unwrap();
    fs::create_dir(local.join("Empty folder")).unwrap();
    fs::remove_file(local.join("bulk0.txt")).unwrap();
    fs::write(local.join(".~lock.notes.txt#"), "Editor lock\n").unwrap();
    wait_until(
        90,
        || sync.tick(),
        || remote.join("created.txt").exists() && !sync.busy() && folder().state == State::UpToDate,
    );
    assert_eq!(
        fs::read_to_string(remote.join("notes.txt")).unwrap(),
        "Edited in Dolphin\n"
    );
    assert!(remote.join("Reports/q1-final.txt").exists() && !remote.join("Reports/q1.txt").exists());
    assert!(remote.join("Empty folder").is_dir() && !remote.join("bulk0.txt").exists());
    assert!(!remote.join(".~lock.notes.txt#").exists());
    fs::remove_file(local.join(".~lock.notes.txt#")).unwrap();

    // Changes from another device reach the local folder; a file deleted there is kept in Backups.
    fs::write(remote.join("from-phone.txt"), "Added on another device\n").unwrap();
    fs::remove_file(remote.join("bulk1.txt")).unwrap();
    run(State::UpToDate);
    assert_eq!(
        fs::read_to_string(local.join("from-phone.txt")).unwrap(),
        "Added on another device\n"
    );
    assert!(!local.join("bulk1.txt").exists());
    let kept: Vec<Option<Vec<u8>>> = tree(&sync.backups(&identifier))
        .into_iter()
        .filter(|(path, _)| path.ends_with("bulk1.txt"))
        .map(|(_, bytes)| bytes)
        .collect();
    assert_eq!(kept, [Some(b"Bulk 1\n".to_vec())]);

    // A conflict keeps both versions on both sides; the newer one keeps the name.
    fs::write(local.join("notes.txt"), "Local side of a conflict\n").unwrap();
    fs::write(remote.join("notes.txt"), "Newer remote side of a conflict\n").unwrap();
    // Files dated during or after a sync are rechecked conservatively later, so both edits are dated earlier.
    age(&local.join("notes.txt"), 60);
    age(&remote.join("notes.txt"), 30);
    run(State::UpToDate);
    for side in [&local, &remote] {
        assert_eq!(
            fs::read_to_string(side.join("notes.txt")).unwrap(),
            "Newer remote side of a conflict\n"
        );
        assert_eq!(
            fs::read_to_string(side.join("notes.conflict1.txt")).unwrap(),
            "Local side of a conflict\n"
        );
    }
    assert_eq!(folder().conflicts, 1);
    // The follow-up run records the winner, so deleting either version afterwards propagates.
    sync.tick();
    assert!(sync.busy());
    settle(State::UpToDate);
    sync.tick();
    assert!(!sync.busy());

    // Mass deletions change nothing until confirmed, then apply once.
    let mut deleted: Vec<String> = (2..8).map(|index| format!("bulk{index}.txt")).collect();
    deleted.extend(["created.txt", "from-phone.txt", "notes.conflict1.txt", "notes.txt"].map(String::from));
    for name in &deleted {
        fs::remove_file(local.join(name)).unwrap();
    }
    run(State::NeedsReview);
    assert_eq!(folder().attention, Some(Review::Confirm));
    assert!(remote.join("bulk2.txt").exists() && remote.join("notes.txt").exists());
    assert!(
        folder().message.contains("deleted or renamed on this computer"),
        "{}",
        folder().message
    );
    sync.approve(&identifier);
    settle(State::UpToDate);
    assert!(!remote.join("bulk2.txt").exists() && !remote.join("notes.txt").exists());

    // Renaming every file of a small folder syncs without review.
    let small_remote = fixture.source.join("Small");
    fs::create_dir(&small_remote).unwrap();
    for name in ["photo-1.jpg", "photo-2.jpg"] {
        fs::write(small_remote.join(name), random(4096)).unwrap();
    }
    assert!(sync.add("Small"), "{}", sync.message());
    let small_identifier = sync.folders()[1].identifier.clone();
    let small = || sync.folder(&small_identifier).unwrap();
    wait_until(90, || sync.tick(), || !sync.busy() && small().state == State::UpToDate);
    let small_local = small().local;
    fs::rename(small_local.join("photo-1.jpg"), small_local.join("renamed.jpg")).unwrap();
    fs::rename(small_local.join("photo-2.jpg"), small_local.join("beach.jpg")).unwrap();
    fs::copy(small_local.join("beach.jpg"), small_local.join("beach copy.jpg")).unwrap();
    sync.sync_now(Some(&small_identifier));
    wait_until(
        90,
        || sync.tick(),
        || !sync.busy() && matches!(small().state, State::UpToDate | State::NeedsReview),
    );
    assert!(
        small().state == State::UpToDate && small().attention.is_none(),
        "{}",
        small().message
    );
    let mut names: Vec<_> = fs::read_dir(&small_remote)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    names.sort();
    assert_eq!(names, ["beach copy.jpg", "beach.jpg", "renamed.jpg"]);
    assert_same(&small_local, &small_remote);

    // A killed sync recovers on the next run without partial files or a forced resync.
    for index in 0..3 {
        fs::write(remote.join(format!("large{index}.bin")), random(8 * 1024 * 1024)).unwrap();
    }
    connection.limited.set(true);
    sync.sync_now(Some(&identifier));
    wait_until(30, || sync.tick(), || any_named(&local, ".partial"));
    // Only this fixture's bisync has its private work folder in the command line.
    assert!(kill_mentioning(&work) > 0, "no bisync run was found to kill");
    wait_until(30, || sync.tick(), || !sync.busy());
    assert_eq!(folder().state, State::Error);
    connection.limited.set(false);
    run(State::UpToDate);
    assert_same(&local, &remote);
    assert!(!any_named(&local, ".partial") && !any_named(&work, ".lst-err"));

    // A deleted iCloud folder stops sync without local deletion; Review restores it by copying.
    fs::remove_dir_all(&remote).unwrap();
    run(State::NeedsReview);
    assert_eq!(folder().attention, Some(Review::Repair));
    assert!(local.join("Reports/q1-final.txt").exists());
    sync.approve(&identifier);
    settle(State::UpToDate);
    assert_eq!(
        fs::read_to_string(remote.join("Reports/q1-final.txt")).unwrap(),
        "First quarter\n"
    );
    assert_same(&local, &remote);

    // Lost sync history stops without changes; Review rebuilds it and normal sync resumes.
    fs::remove_dir_all(&work).unwrap();
    run(State::NeedsReview);
    assert_eq!(folder().attention, Some(Review::Repair));
    assert_same(&local, &remote);
    sync.approve(&identifier);
    settle(State::UpToDate);
    fs::write(local.join("after-repair.txt"), "Normal sync resumed\n").unwrap();
    run(State::UpToDate);
    assert!(remote.join("after-repair.txt").exists() && !any_named(&work, ".lst-err"));

    sync.pause();
    wait_until(60, || sync.tick(), || !sync.busy());
}

/// Kill the processes whose command line mentions `path`, as `pkill -KILL -f` does, without needing procps.
fn kill_mentioning(path: &Path) -> usize {
    let needle = path.as_os_str().as_bytes();
    let mut killed = 0;
    for entry in fs::read_dir("/proc").unwrap().flatten() {
        let Some(process) = entry.file_name().to_str().and_then(|name| name.parse::<i32>().ok()) else {
            continue;
        };
        let Ok(command_line) = fs::read(entry.path().join("cmdline")) else {
            continue;
        };
        if command_line.windows(needle.len()).any(|window| window == needle)
            && nix::sys::signal::kill(nix::unistd::Pid::from_raw(process), nix::sys::signal::Signal::SIGKILL).is_ok()
        {
            killed += 1;
        }
    }
    killed
}
