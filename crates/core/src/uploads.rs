//! Private upload snapshots and the durable queue journal. Each prepared file has a folder in the upload cache: its
//! `data/` folder holds only the payload, `upload.json` records it once it is ready, and a `send/` folder may hold a
//! hard link under a new cloud name. `queue.json` lists every row and its state.
use std::collections::HashSet;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions, Permissions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use nix::fcntl::OFlag;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::profile::{flush_directory, hex, private_directory, random_hex, write_json};
use crate::{Failure, Result, tr};

const BLOCK: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum State {
    #[default]
    Queued,
    Checking,
    Uploading,
    Verifying,
    Completed,
    Paused,
    Conflict,
    Skipped,
    Error,
}

impl State {
    /// Whether the queue is working on the row. Such rows pause with the queue and reopen paused.
    pub fn active(self) -> bool {
        matches!(
            self,
            State::Queued | State::Checking | State::Uploading | State::Verifying
        )
    }
}

/// A file in the upload queue, bound to the identity of the profile that added it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Upload {
    pub identifier: String,
    /// The cloud filename; Keep both changes it.
    pub name: String,
    /// The cloud folder, relative to iCloud Drive.
    pub destination: String,
    pub size: u64,
    /// The SHA-256 of the snapshot.
    pub digest: String,
    pub profile: String,
    #[serde(default)]
    pub state: State,
    #[serde(default)]
    pub progress: u8,
    /// The snapshot filename, which never changes. Version 1 journals lack it; reading fills it in.
    #[serde(default)]
    pub source_name: String,
}

impl Upload {
    fn decode(value: Value) -> Result<Upload> {
        let mut item: Upload = serde_json::from_value(value).map_err(unreadable)?;
        if !hexadecimal(&item.identifier, 32) {
            return Err(invalid("Invalid upload identifier."));
        }
        component(&item.name)?;
        if item.source_name.is_empty() {
            item.source_name = item.name.clone();
        }
        component(&item.source_name)?;
        folder(&item.destination)?;
        if !hexadecimal(&item.digest, 64) || !hexadecimal(&item.profile, 64) {
            return Err(invalid("Invalid upload fingerprint."));
        }
        if item.progress > 100 {
            return Err(invalid("Invalid upload state."));
        }
        Ok(item)
    }

    /// The file on the rclone remote.
    pub fn remote(&self) -> String {
        format!("icloud:{}/{}", self.destination, self.name)
    }
}

/// Require a single filename without path separators, `:`, or control characters.
fn component(name: &str) -> Result<()> {
    if matches!(name, "" | "." | "..") || name.contains(['/', '\\', ':']) || name.chars().any(|c| c < ' ') {
        return Err(Failure::Invalid(tr!(
            "Choose a regular filename without path separators or control characters."
        )));
    }
    Ok(())
}

/// Require a folder relative to iCloud Drive made of such filenames.
pub(crate) fn folder(value: &str) -> Result<()> {
    if value.is_empty() || value.starts_with('/') {
        return Err(Failure::Invalid(tr!("Enter a folder relative to iCloud Drive.")));
    }
    value.split('/').try_for_each(component)
}

/// The error of a preparation that the queue paused. Unlike other system errors, it is no storage failure.
pub(crate) fn preparation_paused() -> Failure {
    Failure::Io(io::Error::new(
        io::ErrorKind::Interrupted,
        "Upload preparation was paused.",
    ))
}

/// The folders of prepared files in the upload cache. Only the location is kept, so worker threads use copies.
#[derive(Clone, Debug)]
pub struct Snapshots {
    root: PathBuf,
}

impl Snapshots {
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub fn directory(&self, item: &Upload) -> PathBuf {
        self.root.join(&item.identifier).join("data")
    }

    pub fn payload(&self, item: &Upload) -> PathBuf {
        self.directory(item).join(&item.source_name)
    }

    fn prepared(&self, item: &Upload) -> PathBuf {
        self.root.join(&item.identifier).join("send")
    }

    /// Copy and hash a regular file into a new private snapshot, then record it. A failed or cancelled preparation
    /// leaves nothing behind unless its record was written; the next start recovers such a snapshot.
    pub fn stage(&self, source: &Path, destination: &str, profile: &str, cancelled: &AtomicBool) -> Result<Upload> {
        let name = source.file_name().and_then(|name| name.to_str()).unwrap_or_default();
        component(name)?;
        folder(destination)?;
        let identifier = random_hex()?;
        let base = self.root.join(&identifier);
        let data = base.join("data");
        private_directory(&base)?;
        private_directory(&data)?;
        let payload = data.join(name);
        let result = copy_snapshot(source, &payload, cancelled).and_then(|(size, digest)| {
            fs::set_permissions(&payload, Permissions::from_mode(0o600))?;
            flush_directory(&data)?;
            let item = Upload {
                identifier,
                name: name.to_owned(),
                destination: destination.to_owned(),
                size,
                digest,
                profile: profile.to_owned(),
                state: State::Queued,
                progress: 0,
                source_name: name.to_owned(),
            };
            write_json(&base.join("upload.json"), &item)?;
            flush_directory(&self.root)?;
            Ok(item)
        });
        if result.is_err() {
            abandon(&base, &payload);
        }
        result
    }

    /// Require private snapshot folders whose data folder holds only the payload, and return the payload.
    fn check_path(&self, item: &Upload) -> Result<PathBuf> {
        check_private_directory(&self.root.join(&item.identifier))?;
        let directory = self.directory(item);
        check_private_directory(&directory)?;
        let names = names(&directory)?;
        if names.len() != 1 || names[0] != *item.source_name {
            return Err(invalid("The pending upload folder contains unexpected files."));
        }
        Ok(directory.join(&item.source_name))
    }

    /// Require the payload to be unchanged since it was prepared.
    pub fn verify(&self, item: &Upload) -> Result<()> {
        self.verify_cancellable(item, &AtomicBool::new(false))
    }

    /// Check a snapshot as `verify` does; a set `cancelled` stops reading early, as a pause stops preparation.
    pub fn verify_cancellable(&self, item: &Upload, cancelled: &AtomicBool) -> Result<()> {
        let mut file = open_unfollowed(&self.check_path(item)?)?;
        let info = file.metadata()?;
        if !info.is_file() || !private(&info) || info.size() != item.size {
            return Err(invalid("The pending upload file is unsafe."));
        }
        let mut digest = Sha256::new();
        read_blocks(&mut file, |block| {
            if cancelled.load(Ordering::SeqCst) {
                return Err(preparation_paused());
            }
            digest.update(block);
            Ok(())
        })?;
        if hex(&digest.finalize()) != item.digest {
            return Err(invalid(
                "The pending upload changed. Its cloud destination was left untouched.",
            ));
        }
        Ok(())
    }

    /// Remove a verified snapshot with its folder. Only known files are removed, and nothing when a check fails.
    pub(crate) fn discard(&self, item: &Upload) {
        let base = self.root.join(&item.identifier);
        let _ = self.verify(item).and_then(|()| {
            self.clear_prepared(item)?;
            fs::remove_file(self.payload(item))?;
            fs::remove_dir(self.directory(item))?;
            match fs::remove_file(base.join("upload.json")) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error.into()),
                _ => {}
            }
            Ok(fs::remove_dir(&base)?)
        });
    }

    /// Remove the `send/` folder of a renamed upload when it holds only links to the payload.
    fn clear_prepared(&self, item: &Upload) -> Result<()> {
        let prepared = self.prepared(item);
        let Some(info) = lstat(&prepared)? else {
            return Ok(());
        };
        if !info.is_dir() || !private(&info) {
            return Err(invalid("The upload source folder is unsafe."));
        }
        let payload = fs::symlink_metadata(self.payload(item))?;
        let entries: Vec<PathBuf> = names(&prepared)?.iter().map(|name| prepared.join(name)).collect();
        for entry in &entries {
            let info = fs::symlink_metadata(entry)?;
            if !info.is_file() || (info.dev(), info.ino()) != (payload.dev(), payload.ino()) {
                return Err(invalid("The upload source folder contains unexpected files."));
            }
        }
        for entry in &entries {
            fs::remove_file(entry)?;
        }
        Ok(fs::remove_dir(&prepared)?)
    }

    /// The folder to pass to `rclone copy`: the data folder, or for a renamed row a `send/` folder with one hard link
    /// under the cloud name. The payload itself is never renamed.
    pub(crate) fn copy_directory(&self, item: &Upload) -> Result<PathBuf> {
        self.check_path(item)?;
        if item.name == item.source_name {
            return Ok(self.directory(item));
        }
        self.clear_prepared(item)?;
        let prepared = self.prepared(item);
        private_directory(&prepared)?;
        fs::hard_link(self.payload(item), prepared.join(&item.name))?;
        Ok(prepared)
    }

    /// Version 1 renamed the payload for Keep both: a pending row takes the name of its folder's sole file.
    fn recover_legacy_name(&self, item: &mut Upload) -> Result<()> {
        let directory = self.directory(item);
        let names = check_private_directory(&self.root.join(&item.identifier))
            .and_then(|()| check_private_directory(&directory))
            .and_then(|()| Ok(names(&directory)?));
        match names {
            Ok(names) if names.len() == 1 => {
                let name = names[0].to_str().unwrap_or_default();
                component(name)?;
                item.source_name = name.to_owned();
                Ok(())
            }
            Ok(_) => Ok(()),
            Err(Failure::Io(error)) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
}

/// The upload queue: its rows, saved durably in `queue.json`, and their snapshots.
pub struct UploadStore {
    pub snapshots: Snapshots,
    pub items: Vec<Upload>,
    /// A warning about preparation folders without a record. They are kept and never upload on their own.
    pub recovery_warning: String,
    journal: PathBuf,
}

#[derive(Serialize)]
struct Journal<'a> {
    version: u8,
    items: &'a [Upload],
}

impl UploadStore {
    /// Read the queue in `root`, accepting journal versions 1 and 2 without rewriting it. Rows in progress reopen
    /// paused, and ready snapshots missing from the journal return as paused rows.
    pub fn open(root: &Path) -> Result<UploadStore> {
        private_directory(root)?;
        let mut store = UploadStore {
            snapshots: Snapshots { root: root.to_owned() },
            items: Vec::new(),
            recovery_warning: String::new(),
            journal: root.join("queue.json"),
        };
        if lstat(&store.journal)?.is_some() {
            let content = read_private_json(&store.journal)?;
            let version = content
                .get("version")
                .and_then(Value::as_u64)
                .filter(|version| matches!(version, 1 | 2));
            let (Some(version), Some(Value::Array(items))) = (version, content.get("items")) else {
                return Err(invalid("The upload journal version is unsupported."));
            };
            store.items = items.iter().cloned().map(Upload::decode).collect::<Result<_>>()?;
            let identifiers: HashSet<&str> = store.items.iter().map(|item| item.identifier.as_str()).collect();
            if identifiers.len() != store.items.len() {
                return Err(invalid("Duplicate upload identifiers."));
            }
            for item in &mut store.items {
                if version == 1 && !matches!(item.state, State::Completed | State::Skipped) {
                    store.snapshots.recover_legacy_name(item)?;
                }
                if item.state.active() {
                    item.state = State::Paused;
                }
            }
        }
        store.recover_pending()?;
        Ok(store)
    }

    /// Add ready snapshots missing from the rows as paused rows, and report preparation folders without a record.
    pub(crate) fn recover_pending(&mut self) -> Result<()> {
        let mut known: HashSet<String> = self.items.iter().map(|item| item.identifier.clone()).collect();
        let mut names = names(&self.snapshots.root)?;
        names.sort();
        let mut incomplete = false;
        for name in names {
            let Some(name) = name
                .to_str()
                .filter(|name| hexadecimal(name, 32) && !known.contains(*name))
            else {
                continue;
            };
            let directory = self.snapshots.root.join(name);
            check_private_directory(&directory)?;
            let record = directory.join("upload.json");
            if lstat(&record)?.is_none() {
                incomplete = true;
                continue;
            }
            let mut item = Upload::decode(read_private_json(&record)?)?;
            if item.identifier != name {
                return Err(invalid("A pending upload record has a different identifier."));
            }
            item.state = State::Paused;
            item.progress = 0;
            known.insert(item.identifier.clone());
            self.items.push(item);
        }
        self.recovery_warning = if incomplete {
            tr!("Incomplete preparation files were preserved in the upload cache. Add their original files again.")
        } else {
            String::new()
        };
        Ok(())
    }

    /// Replace the journal atomically and durably.
    pub fn save(&self) -> Result<()> {
        Ok(write_json(
            &self.journal,
            &Journal {
                version: 2,
                items: &self.items,
            },
        )?)
    }

    /// Queue a conflicting row again under a new random cloud name. The snapshot keeps its filename, so the journal
    /// finds it whether or not the save happened; a failed save restores the row.
    pub fn keep_both(&mut self, index: usize) -> Result<()> {
        self.snapshots.check_path(&self.items[index])?;
        let previous = self.items[index].clone();
        let item = &mut self.items[index];
        item.name = alternative_name(&item.name)?;
        item.state = State::Queued;
        item.progress = 0;
        if let Err(error) = self.save() {
            self.items[index] = previous;
            return Err(error);
        }
        Ok(())
    }
}

/// `stem (random).suffix`, with the stem and suffix of Python's `PurePosixPath` cut to 180 and 30 bytes.
fn alternative_name(name: &str) -> io::Result<String> {
    let (stem, suffix) = match name.rfind('.') {
        Some(dot) if dot > 0 && dot < name.len() - 1 => name.split_at(dot),
        _ => (name, ""),
    };
    Ok(format!(
        "{} ({}){}",
        prefix(stem, 180),
        &random_hex()?[..12],
        prefix(suffix, 30)
    ))
}

fn prefix(text: &str, bytes: usize) -> &str {
    let mut end = bytes.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Copy a regular source file that does not change meanwhile; return its size and SHA-256.
fn copy_snapshot(source: &Path, payload: &Path, cancelled: &AtomicBool) -> Result<(u64, String)> {
    let mut original = open_unfollowed(source)?;
    let mut snapshot = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(payload)?;
    let before = original.metadata()?;
    if !before.is_file() {
        return Err(invalid("Only regular files can be uploaded."));
    }
    let mut digest = Sha256::new();
    read_blocks(&mut original, |block| {
        if cancelled.load(Ordering::SeqCst) {
            return Err(preparation_paused());
        }
        snapshot.write_all(block)?;
        digest.update(block);
        Ok(())
    })?;
    let identity = |info: &fs::Metadata| {
        (
            info.ino(),
            info.size(),
            info.mtime(),
            info.mtime_nsec(),
            info.ctime(),
            info.ctime_nsec(),
        )
    };
    if identity(&before) != identity(&original.metadata()?) {
        return Err(invalid(
            "The source file changed while preparing its upload. Add it again when saved.",
        ));
    }
    snapshot.sync_all()?;
    Ok((before.size(), hex(&digest.finalize())))
}

/// Remove a failed preparation, unless its record exists and the next start can recover it.
fn abandon(base: &Path, payload: &Path) {
    if !matches!(lstat(&base.join("upload.json")), Ok(None)) {
        return;
    }
    let _ = fs::remove_file(payload);
    let _ = fs::remove_dir(base.join("data"));
    let _ = fs::remove_dir(base);
}

fn read_blocks(reader: &mut impl Read, mut each: impl FnMut(&[u8]) -> Result<()>) -> Result<()> {
    let mut block = vec![0; BLOCK];
    loop {
        match reader.read(&mut block) {
            Ok(0) => return Ok(()),
            Ok(count) => each(&block[..count])?,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error.into()),
        }
    }
}

/// Open for reading without following a final symbolic link or waiting for a pipe writer.
fn open_unfollowed(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags((OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK).bits())
        .open(path)
}

fn read_private_json(path: &Path) -> Result<Value> {
    let info = fs::symlink_metadata(path)?;
    if !info.is_file() || !private(&info) {
        return Err(invalid("Upload metadata must be a private regular file."));
    }
    let mut bytes = Vec::new();
    open_unfollowed(path)?.read_to_end(&mut bytes)?;
    serde_json::from_slice(&bytes).map_err(unreadable)
}

fn check_private_directory(path: &Path) -> Result<()> {
    let info = fs::symlink_metadata(path)?;
    if !info.is_dir() || !private(&info) {
        return Err(invalid("A pending upload folder is unsafe."));
    }
    Ok(())
}

/// Owned by this user and closed to others.
fn private(info: &fs::Metadata) -> bool {
    info.uid() == nix::unistd::getuid().as_raw() && info.mode() & 0o077 == 0
}

/// The metadata of `path` itself, or none when it does not exist.
fn lstat(path: &Path) -> io::Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(info) => Ok(Some(info)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn names(directory: &Path) -> io::Result<Vec<OsString>> {
    fs::read_dir(directory)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect()
}

fn hexadecimal(value: &str, length: usize) -> bool {
    value.len() == length && value.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn invalid(message: &str) -> Failure {
    Failure::Invalid(message.to_owned())
}

fn unreadable(error: serde_json::Error) -> Failure {
    Failure::Invalid(error.to_string())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::profile::sha256;

    /// Makes a folder read-only until dropped, so saves in it fail like on a full disk.
    pub(crate) struct ReadOnly(PathBuf);

    impl ReadOnly {
        pub(crate) fn new(path: &Path) -> ReadOnly {
            fs::set_permissions(path, Permissions::from_mode(0o500)).unwrap();
            ReadOnly(path.to_owned())
        }
    }

    impl Drop for ReadOnly {
        fn drop(&mut self) {
            let _ = fs::set_permissions(&self.0, Permissions::from_mode(0o700));
        }
    }

    struct Setup {
        temporary: tempfile::TempDir,
        store: UploadStore,
        source: PathBuf,
        cancelled: AtomicBool,
    }

    fn setup() -> Setup {
        let temporary = tempfile::tempdir().unwrap();
        let store = UploadStore::open(&temporary.path().join("queue")).unwrap();
        let source = temporary.path().join("sample.bin");
        fs::write(&source, b"Original local bytes\n").unwrap();
        Setup {
            temporary,
            store,
            source,
            cancelled: AtomicBool::new(false),
        }
    }

    impl Setup {
        fn stage(&self) -> Upload {
            let profile = "a".repeat(64);
            self.store
                .snapshots
                .stage(&self.source, "Test uploads", &profile, &self.cancelled)
                .unwrap()
        }

        /// Stage the source as the first row and save the queue.
        fn add(&mut self) -> Upload {
            let item = self.stage();
            self.store.items.push(item.clone());
            self.store.save().unwrap();
            item
        }

        fn reopen(&self) -> UploadStore {
            UploadStore::open(self.store.snapshots.root()).unwrap()
        }

        fn journal(&self) -> Value {
            serde_json::from_slice(&fs::read(&self.store.journal).unwrap()).unwrap()
        }

        fn write_journal(&self, content: &Value) {
            fs::write(&self.store.journal, content.to_string()).unwrap();
        }
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().mode() & 0o777
    }

    #[test]
    fn a_snapshot_is_private_independent_and_holds_only_the_payload() {
        let mut setup = setup();
        let item = setup.add();
        fs::write(&setup.source, b"Later source edit").unwrap();
        setup.store.snapshots.verify(&item).unwrap();
        let payload = setup.store.snapshots.payload(&item);
        assert_eq!(fs::read(&payload).unwrap(), b"Original local bytes\n");
        assert_eq!(names(&setup.store.snapshots.directory(&item)).unwrap(), ["sample.bin"]);
        assert_eq!(mode(&payload), 0o600);
        assert_eq!(mode(&setup.store.journal), 0o600);
        assert_eq!(item.digest, sha256(b"Original local bytes\n"));
    }

    #[test]
    fn an_interrupted_queue_reopens_paused_without_uploading_again() {
        let mut setup = setup();
        setup.add();
        setup.store.items[0].state = State::Uploading;
        setup.store.save().unwrap();
        let reopened = setup.reopen();
        assert_eq!(reopened.items[0].state, State::Paused);
        reopened.snapshots.verify(&reopened.items[0]).unwrap();
    }

    #[test]
    fn a_changed_snapshot_is_refused_and_kept() {
        let mut setup = setup();
        let item = setup.add();
        let payload = setup.store.snapshots.payload(&item);
        fs::write(&payload, b"Changed local bytes!\n").unwrap();
        assert!(matches!(setup.store.snapshots.verify(&item), Err(Failure::Invalid(_))));
        setup.store.snapshots.discard(&item);
        assert!(payload.exists());
    }

    #[test]
    fn linked_sources_and_cancelled_copies_leave_nothing_behind() {
        let setup = setup();
        let linked = setup.temporary.path().join("linked.bin");
        std::os::unix::fs::symlink(&setup.source, &linked).unwrap();
        let profile = "a".repeat(64);
        let error = setup
            .store
            .snapshots
            .stage(&linked, "Tests", &profile, &setup.cancelled);
        assert!(matches!(error, Err(Failure::Io(_))));
        setup.cancelled.store(true, Ordering::SeqCst);
        let error = setup
            .store
            .snapshots
            .stage(&setup.source, "Tests", &profile, &setup.cancelled);
        assert!(matches!(error, Err(Failure::Io(error)) if error.kind() == io::ErrorKind::Interrupted));
        assert!(names(setup.store.snapshots.root()).unwrap().is_empty());
    }

    #[test]
    fn traversing_destinations_and_journal_paths_are_refused() {
        for destination in [
            "../other",
            "/absolute",
            "one//two",
            "one/../two",
            "icloud:other",
            "one\\two",
            "",
            "a/",
        ] {
            assert!(folder(destination).is_err(), "{destination}");
        }
        folder("Documents/Work 2026").unwrap();
        let mut setup = setup();
        let item = setup.add();
        let mut content = setup.journal();
        content["items"][0]["identifier"] = "../outside".into();
        setup.write_journal(&content);
        assert!(matches!(
            UploadStore::open(setup.store.snapshots.root()),
            Err(Failure::Invalid(_))
        ));
        assert!(setup.store.snapshots.payload(&item).exists());
    }

    #[test]
    fn invalid_rows_are_refused() {
        let item = Upload::decode(serde_json::json!({
            "identifier": "c".repeat(32), "name": "a.bin", "destination": "Uploads", "size": 1,
            "digest": "d".repeat(64), "profile": "e".repeat(64),
        }))
        .unwrap();
        assert_eq!(
            (item.state, item.progress, item.source_name.as_str()),
            (State::Queued, 0, "a.bin")
        );
        let valid = serde_json::to_value(&item).unwrap();
        for (field, value) in [
            ("identifier", Value::from("C".repeat(32))),
            ("digest", Value::from("d".repeat(63))),
            ("name", Value::from("a\nb")),
            ("source_name", Value::from("..")),
            ("state", Value::from("Unknown")),
            ("progress", Value::from(101)),
            ("size", Value::from(-1)),
            ("extra", Value::from(1)),
        ] {
            let mut row = valid.clone();
            row[field] = value;
            assert!(Upload::decode(row).is_err(), "{field}");
        }
    }

    #[test]
    fn unexpected_files_prevent_upload_and_cleanup() {
        let mut setup = setup();
        let item = setup.add();
        let unexpected = setup.store.snapshots.directory(&item).join("rclone.conf");
        fs::write(&unexpected, b"Fictional unexpected data").unwrap();
        assert!(setup.store.snapshots.verify(&item).is_err());
        setup.store.snapshots.discard(&item);
        assert!(unexpected.exists());
    }

    #[test]
    fn keep_both_changes_the_cloud_name_without_renaming_the_snapshot() {
        let mut setup = setup();
        let original = setup.add();
        let payload = setup.store.snapshots.payload(&original);
        setup.store.items[0].state = State::Conflict;
        setup.store.keep_both(0).unwrap();
        let item = setup.store.items[0].clone();
        assert!(
            item.name.starts_with("sample (") && item.name.ends_with(").bin"),
            "{}",
            item.name
        );
        assert_eq!((item.state, item.source_name.as_str()), (State::Queued, "sample.bin"));
        assert_eq!(fs::read(&setup.source).unwrap(), b"Original local bytes\n");
        assert_eq!(setup.store.snapshots.payload(&item), payload);
        let copy = setup.store.snapshots.copy_directory(&item).unwrap();
        assert_eq!(names(&copy).unwrap(), [item.name.as_str()]);
        assert_eq!(fs::read(copy.join(&item.name)).unwrap(), fs::read(&payload).unwrap());
        setup.store.snapshots.verify(&item).unwrap();
        assert_eq!(
            setup.reopen().items[0],
            Upload {
                state: State::Paused,
                ..item
            }
        );
    }

    #[test]
    fn alternative_names_keep_the_suffix_and_a_bounded_length() {
        let name = alternative_name("archive.tar.gz").unwrap();
        assert!(
            name.starts_with("archive.tar (") && name.ends_with(").gz") && name.len() == 29,
            "{name}"
        );
        let hidden = alternative_name(".profile").unwrap();
        assert!(hidden.starts_with(".profile (") && hidden.ends_with(')'), "{hidden}");
        assert!(alternative_name("trailing.").unwrap().starts_with("trailing. ("));
        // Byte 180 of the stem falls inside a two-byte letter, which is left out whole.
        let long = alternative_name(&format!("a{}.{}", "é".repeat(100), "x".repeat(40))).unwrap();
        assert_eq!(long.len(), 179 + " (123456789012)".len() + 30);
        component(&long).unwrap();
    }

    #[test]
    fn a_failed_keep_both_save_keeps_the_original_conflict_and_payload() {
        let mut setup = setup();
        setup.add();
        setup.store.items[0].state = State::Conflict;
        setup.store.save().unwrap();
        let original = setup.store.items[0].clone();
        {
            let _full = ReadOnly::new(setup.store.snapshots.root());
            assert!(matches!(setup.store.keep_both(0), Err(Failure::Io(_))));
        }
        assert_eq!(setup.store.items[0], original);
        let reopened = setup.reopen();
        assert_eq!(reopened.items[0].name, original.name);
        reopened.snapshots.verify(&reopened.items[0]).unwrap();
    }

    #[test]
    fn keep_both_never_detaches_the_snapshot_from_either_journal() {
        // A crash during the save leaves the previous or the new journal; both must find the same payload.
        let mut setup = setup();
        setup.add();
        setup.store.items[0].state = State::Conflict;
        setup.store.save().unwrap();
        let previous = fs::read(&setup.store.journal).unwrap();
        setup.store.keep_both(0).unwrap();
        for expected_original in [false, true] {
            let reopened = setup.reopen();
            let item = &reopened.items[0];
            assert_eq!(item.name == "sample.bin", expected_original);
            reopened.snapshots.verify(item).unwrap();
            assert_eq!(
                fs::read(reopened.snapshots.payload(item)).unwrap(),
                fs::read(&setup.source).unwrap()
            );
            fs::write(&setup.store.journal, &previous).unwrap();
        }
    }

    #[test]
    fn a_version_1_queue_is_read_without_moving_or_rewriting_its_data() {
        let mut setup = setup();
        let item = setup.add();
        let mut content = setup.journal();
        content["version"] = 1.into();
        content["items"][0].as_object_mut().unwrap().remove("source_name");
        setup.write_journal(&content);
        let original = fs::read(&setup.store.journal).unwrap();
        let reopened = setup.reopen();
        assert_eq!(fs::read(&setup.store.journal).unwrap(), original);
        assert_eq!(
            reopened.snapshots.payload(&reopened.items[0]),
            setup.store.snapshots.payload(&item)
        );
        reopened.snapshots.verify(&reopened.items[0]).unwrap();
        reopened.save().unwrap();
        assert_eq!(setup.journal()["version"], 2);
        assert_eq!(setup.journal()["items"][0]["source_name"], "sample.bin");
    }

    #[test]
    fn an_interrupted_version_1_rename_recovers_the_same_payload() {
        let mut setup = setup();
        let item = setup.add();
        let mut content = setup.journal();
        content["version"] = 1.into();
        content["items"][0].as_object_mut().unwrap().remove("source_name");
        content["items"][0]["state"] = "Conflict".into();
        setup.write_journal(&content);
        let renamed = setup.store.snapshots.directory(&item).join("sample (fictional).bin");
        fs::rename(setup.store.snapshots.payload(&item), &renamed).unwrap();
        let reopened = setup.reopen();
        let recovered = &reopened.items[0];
        assert_eq!(
            (recovered.state, recovered.name.as_str()),
            (State::Conflict, "sample.bin")
        );
        assert_eq!(reopened.snapshots.payload(recovered), renamed);
        reopened.snapshots.verify(recovered).unwrap();
    }

    #[test]
    fn a_failed_save_keeps_the_previous_journal_and_payload() {
        let mut setup = setup();
        setup.add();
        let original = fs::read(&setup.store.journal).unwrap();
        setup.store.items[0].state = State::Uploading;
        {
            let _full = ReadOnly::new(setup.store.snapshots.root());
            assert!(matches!(setup.store.save(), Err(Failure::Io(_))));
        }
        assert_eq!(fs::read(&setup.store.journal).unwrap(), original);
        let reopened = setup.reopen();
        reopened.snapshots.verify(&reopened.items[0]).unwrap();
    }

    #[test]
    fn a_failed_replace_removes_its_temporary_file() {
        let setup = setup();
        // A folder in place of the journal makes the rename fail after the temporary file was written.
        fs::create_dir(&setup.store.journal).unwrap();
        assert!(matches!(setup.store.save(), Err(Failure::Io(_))));
        assert_eq!(names(setup.store.snapshots.root()).unwrap(), ["queue.json"]);
    }

    #[test]
    fn incomplete_preparation_files_are_kept_and_reported() {
        let setup = setup();
        let incomplete = setup.store.snapshots.root().join("b".repeat(32)).join("data");
        private_directory(&incomplete).unwrap();
        let partial = incomplete.join("partial.bin");
        fs::write(&partial, b"Incomplete fictional bytes").unwrap();
        let reopened = setup.reopen();
        assert!(reopened.items.is_empty());
        assert!(!reopened.recovery_warning.is_empty());
        assert_eq!(fs::read(&partial).unwrap(), b"Incomplete fictional bytes");
    }

    #[test]
    fn unrelated_prepared_files_prevent_all_cleanup() {
        let mut setup = setup();
        setup.add();
        setup.store.items[0].state = State::Conflict;
        setup.store.keep_both(0).unwrap();
        let item = setup.store.items[0].clone();
        let unrelated = setup
            .store
            .snapshots
            .copy_directory(&item)
            .unwrap()
            .join("unrecognized.bin");
        fs::write(&unrelated, b"Keep this local file").unwrap();
        setup.store.snapshots.discard(&item);
        assert!(setup.store.snapshots.payload(&item).exists());
        assert!(unrelated.exists());
    }

    #[test]
    fn a_discarded_snapshot_leaves_no_files() {
        let mut setup = setup();
        setup.add();
        setup.store.items[0].state = State::Conflict;
        setup.store.keep_both(0).unwrap();
        let item = setup.store.items[0].clone();
        setup.store.snapshots.copy_directory(&item).unwrap();
        setup.store.snapshots.discard(&item);
        assert_eq!(names(setup.store.snapshots.root()).unwrap(), ["queue.json"]);
    }

    #[test]
    fn committed_terminal_rows_do_not_return_as_pending() {
        let mut setup = setup();
        setup.add();
        setup.store.items[0].state = State::Completed;
        setup.store.save().unwrap();
        let reopened = setup.reopen();
        assert_eq!(reopened.items.len(), 1);
        assert_eq!(reopened.items[0].state, State::Completed);
    }

    #[test]
    fn a_recorded_snapshot_survives_a_failed_preparation() {
        // A preparation that fails after writing its record, such as on an uncertain folder flush, keeps it.
        let setup = setup();
        let item = setup.stage();
        abandon(
            &setup.store.snapshots.root().join(&item.identifier),
            &setup.store.snapshots.payload(&item),
        );
        let reopened = setup.reopen();
        assert_eq!(reopened.items.len(), 1);
        reopened.snapshots.verify(&reopened.items[0]).unwrap();
    }

    #[test]
    fn a_linked_recovery_record_is_refused_without_touching_its_target() {
        let setup = setup();
        let item = setup.stage();
        let record = setup.store.snapshots.root().join(&item.identifier).join("upload.json");
        let original = fs::read(&record).unwrap();
        let outside = setup.temporary.path().join("outside.json");
        fs::rename(&record, &outside).unwrap();
        std::os::unix::fs::symlink(&outside, &record).unwrap();
        assert!(matches!(
            UploadStore::open(setup.store.snapshots.root()),
            Err(Failure::Invalid(_))
        ));
        assert_eq!(fs::read(&outside).unwrap(), original);
        assert!(setup.store.snapshots.payload(&item).exists());
    }

    #[test]
    fn a_staged_snapshot_survives_a_crash_before_the_queue_is_saved() {
        let setup = setup();
        let item = setup.stage();
        let reopened = setup.reopen();
        assert_eq!(reopened.items.len(), 1);
        assert_eq!(
            (reopened.items[0].identifier.as_str(), reopened.items[0].state),
            (item.identifier.as_str(), State::Paused)
        );
        reopened.snapshots.verify(&reopened.items[0]).unwrap();
    }
}
