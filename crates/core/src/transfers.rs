//! The managed upload queue. A worker thread copies and hashes selected files into private snapshots; the main thread
//! then checks the cloud file, uploads a missing one with `rclone copy --immutable`, and releases a snapshot only after
//! an independent download matches it. Every rclone command follows a durable save of the journal, and a failed save
//! pauses the queue until a Resume saves successfully.
use std::cell::RefCell;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use nix::sys::signal::Signal::SIGKILL;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::connection::Connection;
use crate::process::{Event, Process};
use crate::profile;
use crate::signal::Signal;
use crate::uploads::{self, State, Upload, UploadStore};
use crate::{Failure, Result, tr};

/// Options of every command: no prompts, bounded waits, and no repeated attempts.
const OPTIONS: [&str; 9] = [
    "--ask-password=false",
    "--contimeout",
    "15s",
    "--timeout",
    "45s",
    "--retries",
    "1",
    "--low-level-retries",
    "1",
];
const CHECKPOINT: Duration = Duration::from_secs(1);
const OUTPUT_LIMIT: usize = 1024 * 1024;
const LINE_LIMIT: usize = 64 * 1024;

fn storage_message() -> String {
    tr!("Could not save the upload queue. Check local space and permissions, then resume uploads.")
}

/// A storage problem, unlike invalid or changed data and a paused preparation.
fn storage(error: &Failure) -> bool {
    matches!(error, Failure::Io(error) if error.kind() != io::ErrorKind::Interrupted)
}

fn interrupted(error: &Failure) -> bool {
    matches!(error, Failure::Io(error) if error.kind() == io::ErrorKind::Interrupted)
}

/// The rclone command running for the current row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    /// `lsjson --stat` of the cloud file.
    Inspect,
    /// `mkdir` of the cloud folder.
    Create,
    /// `copy` of the snapshot folder, which reports progress.
    Copy,
    /// `cat` of the cloud file, hashed as it arrives.
    Read,
}

impl Step {
    fn command(self) -> &'static str {
        match self {
            Step::Inspect => "lsjson",
            Step::Create => "mkdir",
            Step::Copy => "copy",
            Step::Read => "cat",
        }
    }
}

/// A download of the cloud file: its length, and its hash up to the snapshot size.
#[derive(Default)]
struct Download {
    bytes: u64,
    oversized: bool,
    hash: Sha256,
}

/// What a command printed, kept until it exits.
struct Capture {
    step: Step,
    /// The snapshot size, which a download must not exceed.
    size: u64,
    output: Vec<u8>,
    errors: Vec<u8>,
    download: Download,
}

impl Capture {
    fn new(step: Step, size: u64) -> Capture {
        Capture {
            step,
            size,
            output: Vec::new(),
            errors: Vec::new(),
            download: Download::default(),
        }
    }

    /// Hash a download, or keep up to 1 MiB of other output. False once the download outgrows the snapshot.
    fn output(&mut self, data: &[u8]) -> bool {
        if self.step == Step::Read {
            self.download.bytes += data.len() as u64;
            if self.download.bytes > self.size {
                self.download.oversized = true;
                return false;
            }
            self.download.hash.update(data);
        } else if self.output.len() < OUTPUT_LIMIT {
            self.output.extend_from_slice(data);
        }
        true
    }

    /// The newest copy progress in rclone's JSON log lines.
    fn errors(&mut self, data: &[u8]) -> Option<u8> {
        if self.step != Step::Copy {
            return None;
        }
        self.errors.extend_from_slice(data);
        let mut progress = None;
        if let Some(end) = self.errors.iter().rposition(|&byte| byte == b'\n') {
            let rest = self.errors.split_off(end + 1);
            progress = self
                .errors
                .split(|&byte| byte == b'\n')
                .filter_map(statistics)
                .next_back();
            self.errors = rest;
        }
        let excess = self.errors.len().saturating_sub(LINE_LIMIT);
        self.errors.drain(..excess);
        progress
    }
}

/// The percentage in an rclone statistics line, below 100 until the download check.
fn statistics(line: &[u8]) -> Option<u8> {
    let value: Value = serde_json::from_slice(line).ok()?;
    let stats = value.get("stats")?;
    let total = stats.get("totalBytes")?.as_f64().filter(|total| *total > 0.0)?;
    let bytes = stats.get("bytes").and_then(Value::as_f64).unwrap_or(0.0);
    Some((100.0 * bytes / total).clamp(0.0, 99.0) as u8)
}

/// Whether `lsjson --stat` described a folder, and the size it reported.
fn describe(output: &[u8]) -> Option<(bool, Option<u64>)> {
    let metadata: Value = serde_json::from_slice(output).ok()?;
    Some((metadata.get("IsDir")?.as_bool()?, metadata.get("Size")?.as_u64()))
}

struct Running {
    process: Process,
    capture: Capture,
}

/// The result of a worker task.
enum Outcome {
    /// The rows prepared, and the error that stopped the batch.
    Prepared(Vec<Upload>, Option<Failure>),
    /// The check of the current row's snapshot.
    Checked(Result<()>),
}

type Job = Box<dyn FnOnce() + Send>;

/// One thread for file work, which runs tasks in submission order.
struct Worker {
    jobs: Option<Sender<Job>>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    fn new() -> io::Result<Worker> {
        let (jobs, receiver) = mpsc::channel::<Job>();
        let thread = thread::Builder::new().name("uploads".into()).spawn(move || {
            for job in receiver {
                // A task that panics drops its result channel; the queue reports that as a failure.
                let _ = panic::catch_unwind(AssertUnwindSafe(job));
            }
        })?;
        Ok(Worker {
            jobs: Some(jobs),
            thread: Some(thread),
        })
    }

    fn submit<T: Send + 'static>(&self, task: impl FnOnce() -> T + Send + 'static) -> Receiver<T> {
        let (sender, result) = mpsc::channel();
        if let Some(jobs) = &self.jobs {
            let _ = jobs.send(Box::new(move || {
                let _ = sender.send(task());
            }));
        }
        result
    }
}

impl Drop for Worker {
    /// Let submitted file work finish, like the app does before it exits.
    fn drop(&mut self) {
        self.jobs.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Inner {
    store: UploadStore,
    process: Option<Running>,
    /// The preparation or snapshot check running on the worker.
    future: Option<Receiver<Outcome>>,
    /// The row whose snapshot check or command is in flight.
    current: Option<String>,
    cancelled: Arc<AtomicBool>,
    paused: bool,
    storage_error: bool,
    message: String,
    checkpoint_at: Instant,
}

impl Inner {
    fn busy(&self) -> bool {
        self.process.is_some() || self.future.is_some()
    }

    fn current(&self) -> Option<&Upload> {
        let identifier = self.current.as_deref()?;
        self.store.items.iter().find(|item| item.identifier == identifier)
    }

    fn current_mut(&mut self) -> Option<&mut Upload> {
        let identifier = self.current.as_deref()?;
        self.store.items.iter_mut().find(|item| item.identifier == identifier)
    }

    fn item_mut(&mut self, identifier: &str) -> Option<&mut Upload> {
        self.store.items.iter_mut().find(|item| item.identifier == identifier)
    }

    fn pause_rows(&mut self) {
        for item in &mut self.store.items {
            if item.state.active() {
                item.state = State::Paused;
            }
        }
    }

    /// Pause the queue: stop preparation and the running command, and pause the rows in progress.
    fn stop(&mut self) {
        self.paused = true;
        self.cancelled.store(true, Ordering::SeqCst);
        self.pause_rows();
        if let Some(running) = &self.process {
            running.process.signal(SIGKILL);
        }
    }
}

/// The upload queue. Create it with an open connection, call `tick` about every 100 ms from the main thread, and
/// render again on `changed`.
pub struct Transfers {
    connection: Rc<dyn Connection>,
    inner: RefCell<Inner>,
    worker: Worker,
    /// Emitted when rows, progress, or the message change.
    pub changed: Signal,
    /// Emitted when the queue stops working after a pause, so a disconnect can continue.
    pub idle: Signal,
}

impl Transfers {
    /// Open the queue in the upload cache. It starts paused: recovered rows wait for Resume.
    pub fn new(connection: Rc<dyn Connection>) -> Result<Rc<Transfers>> {
        let store = UploadStore::open(&connection.paths().cache.join("uploads"))?;
        let message = if store.items.is_empty() {
            tr!("Choose files to upload. Existing cloud files are preserved.")
        } else {
            tr!("Connect and resume recovered pending uploads.")
        };
        let transfers = Rc::new(Transfers {
            connection: connection.clone(),
            inner: RefCell::new(Inner {
                store,
                process: None,
                future: None,
                current: None,
                cancelled: Arc::default(),
                paused: true,
                storage_error: false,
                message,
                checkpoint_at: Instant::now(),
            }),
            worker: Worker::new()?,
            changed: Signal::default(),
            idle: Signal::default(),
        });
        let weak = Rc::downgrade(&transfers);
        connection.changed().connect(move || {
            if let Some(transfers) = weak.upgrade() {
                transfers.connection_changed();
            }
        });
        Ok(transfers)
    }

    /// The rows in queue order.
    pub fn items(&self) -> Vec<Upload> {
        self.inner.borrow().store.items.clone()
    }

    pub fn message(&self) -> String {
        self.inner.borrow().message.clone()
    }

    /// A note about preparation files kept from an interrupted run, or nothing.
    pub fn recovery_warning(&self) -> String {
        self.inner.borrow().store.recovery_warning.clone()
    }

    /// Whether a command or worker task is running. A disconnect waits for `idle` until it is not.
    pub fn busy(&self) -> bool {
        self.inner.borrow().busy()
    }

    pub fn paused(&self) -> bool {
        self.inner.borrow().paused
    }

    /// Whether saving the queue failed. New work waits until a Resume saves it.
    pub fn storage_error(&self) -> bool {
        self.inner.borrow().storage_error
    }

    /// Whether `add` accepts files now.
    pub fn can_add(&self) -> bool {
        let unlocked = self.connection.unlocked();
        let inner = self.inner.borrow();
        unlocked && inner.future.is_none() && !inner.storage_error
    }

    /// Queue regular files for the cloud folder `destination`. Private copies are prepared in the background.
    pub fn add(&self, paths: &[PathBuf], destination: &str) {
        if !self.can_add() {
            return;
        }
        if uploads::folder(destination).is_err() || paths.iter().any(|path| self.private(path)) {
            self.inner.borrow_mut().message =
                tr!("Choose regular files outside the private application folders and a relative cloud folder.");
            self.changed.emit();
            return;
        }
        let (sources, destination, profile) = (paths.to_vec(), destination.to_owned(), self.connection.profile_id());
        {
            let mut inner = self.inner.borrow_mut();
            inner.cancelled.store(false, Ordering::SeqCst);
            inner.paused = false;
            let (snapshots, cancelled) = (inner.store.snapshots.clone(), inner.cancelled.clone());
            inner.future = Some(self.worker.submit(move || {
                let mut items = Vec::new();
                for source in &sources {
                    let staged = if cancelled.load(Ordering::SeqCst) {
                        Err(uploads::preparation_paused())
                    } else {
                        snapshots.stage(source, &destination, &profile, &cancelled)
                    };
                    match staged {
                        Ok(item) => items.push(item),
                        Err(error) => return Outcome::Prepared(items, Some(error)),
                    }
                }
                Outcome::Prepared(items, None)
            }));
            inner.message = tr!("Preparing private local copies...");
        }
        self.changed.emit();
    }

    /// Whether `path` is the profile or lies in the app's private folders.
    fn private(&self, path: &Path) -> bool {
        // Missing paths compare as given: nothing missing can be uploaded.
        let real = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_owned());
        let paths = self.connection.paths();
        let resolved = real(path);
        resolved == real(&paths.config)
            || [&paths.cache, &paths.runtime]
                .iter()
                .any(|folder| resolved.starts_with(real(folder)))
            || path
                .file_name()
                .is_some_and(|name| name == "rclone.conf" || name == ".env")
    }

    /// Queue paused and failed rows of this profile again, once a save succeeds; that also clears a storage failure.
    /// Conflicts keep waiting for a decision.
    pub fn resume(&self) {
        let (unlocked, profile) = (self.connection.unlocked(), self.connection.profile_id());
        {
            let mut inner = self.inner.borrow_mut();
            if !unlocked || inner.busy() {
                return;
            }
            inner.paused = true;
            for item in &mut inner.store.items {
                if matches!(item.state, State::Paused | State::Error) && item.profile == profile {
                    item.state = State::Queued;
                    item.progress = 0;
                }
            }
        }
        if !self.notify() {
            return;
        }
        {
            let mut inner = self.inner.borrow_mut();
            inner.storage_error = false;
            inner.paused = false;
            let foreign = inner
                .store
                .items
                .iter()
                .any(|item| item.profile != profile && !matches!(item.state, State::Completed | State::Skipped));
            inner.message = if foreign {
                tr!(
                    "Files from a different saved session remain paused. Reconnect with their original profile to resume them."
                )
            } else {
                tr!("Uploads resumed. Conflicts require a decision.")
            };
        }
        self.changed.emit();
    }

    /// Pause the queue. A running command is stopped and its row keeps its snapshot; `idle` follows once it ended.
    pub fn pause(&self) {
        {
            let mut inner = self.inner.borrow_mut();
            inner.stop();
            if !inner.storage_error {
                inner.message = tr!("Uploads paused. Pending local copies are retained.");
            }
        }
        self.notify();
        if !self.busy() {
            self.idle.emit();
        }
    }

    /// Decide a conflict: Skip leaves the cloud file alone; Keep both uploads the snapshot under a new name.
    pub fn resolve(&self, identifier: &str, keep_both: bool) {
        let (unlocked, profile) = (self.connection.unlocked(), self.connection.profile_id());
        let (index, owned) = {
            let inner = self.inner.borrow();
            let Some(index) = inner.store.items.iter().position(|item| item.identifier == identifier) else {
                return;
            };
            let item = &inner.store.items[index];
            if item.state != State::Conflict || inner.busy() || inner.storage_error {
                return;
            }
            (index, item.profile == profile)
        };
        if keep_both && !(unlocked && owned) {
            self.inner.borrow_mut().message =
                tr!("Reconnect with the original saved session before keeping both versions.");
            self.changed.emit();
            return;
        }
        if keep_both {
            let result = {
                let mut inner = self.inner.borrow_mut();
                let result = inner.store.keep_both(index);
                match &result {
                    Ok(()) => inner.paused = false,
                    Err(Failure::Invalid(_)) => {
                        inner.store.items[index].state = State::Error;
                        inner.message = tr!("The local copy is unavailable or changed. The cloud file was preserved.");
                    }
                    Err(Failure::Io(_)) => {}
                }
                result
            };
            if let Err(Failure::Io(_)) = result {
                self.storage_failed();
                return;
            }
        } else {
            self.inner.borrow_mut().store.items[index].state = State::Skipped;
            if !self.notify() {
                // The decision was not saved, so the snapshot stays for it.
                if let Some(item) = self.inner.borrow_mut().item_mut(identifier) {
                    item.state = State::Conflict;
                }
                self.changed.emit();
                return;
            }
            self.discard(identifier);
        }
        self.notify();
    }

    /// Delete every pending upload with its private copy; the original files stay. Refused while busy.
    pub fn forget(&self) -> Result<()> {
        if self.busy() {
            return Err(Failure::Io(io::ErrorKind::ResourceBusy.into()));
        }
        let root = self.inner.borrow().store.snapshots.root().to_owned();
        match fs::symlink_metadata(&root) {
            Ok(info) if info.is_dir() => fs::remove_dir_all(&root)?,
            Ok(_) => fs::remove_file(&root)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let store = UploadStore::open(&root)?;
        {
            let mut inner = self.inner.borrow_mut();
            inner.store = store;
            inner.storage_error = false;
            inner.message = tr!("Choose files to upload. Existing cloud files are preserved.");
        }
        self.changed.emit();
        Ok(())
    }

    /// Advance the queue: collect command output and worker results, save during long commands, and start the next
    /// row. Call it from the main thread, which starts every rclone command.
    pub fn tick(&self) {
        self.collect_process();
        let due = {
            let inner = self.inner.borrow();
            inner.process.is_some() && !inner.paused && inner.checkpoint_at.elapsed() >= CHECKPOINT
        };
        // A save about once per second stops a long command soon after the disk fills.
        if due {
            self.notify();
        }
        self.collect_future();
        self.start_next();
        let mut inner = self.inner.borrow_mut();
        if !inner.busy() {
            // A step that stopped before its command started has paused its row.
            inner.current = None;
        }
    }

    fn connection_changed(&self) {
        if !self.connection.unlocked() {
            self.pause();
        }
        self.changed.emit();
    }

    /// Save the queue and announce the change. A failed save stops the queue.
    fn notify(&self) -> bool {
        let saved = {
            let mut inner = self.inner.borrow_mut();
            let saved = inner.store.save().is_ok();
            if saved {
                inner.checkpoint_at = Instant::now();
                if inner.storage_error {
                    inner.message = storage_message();
                }
            }
            saved
        };
        if !saved {
            self.storage_failed();
            return false;
        }
        self.changed.emit();
        true
    }

    /// Pause everything until a Resume saves the queue successfully.
    fn storage_failed(&self) {
        {
            let mut inner = self.inner.borrow_mut();
            inner.storage_error = true;
            inner.stop();
            inner.message = storage_message();
        }
        self.changed.emit();
    }

    /// Release a finished row's snapshot in the background. A snapshot that fails a check stays in the cache.
    fn discard(&self, identifier: &str) {
        let inner = self.inner.borrow();
        if let Some(item) = inner.store.items.iter().find(|item| item.identifier == identifier) {
            let (snapshots, item) = (inner.store.snapshots.clone(), item.clone());
            self.worker.submit(move || snapshots.discard(&item));
        }
    }

    /// Check the snapshot of the first queued row on the worker before any cloud command.
    fn start_next(&self) {
        if !self.connection.unlocked() {
            return;
        }
        let profile = self.connection.profile_id();
        {
            let mut inner = self.inner.borrow_mut();
            if inner.paused || inner.busy() {
                return;
            }
            let Some(index) = inner.store.items.iter().position(|item| item.state == State::Queued) else {
                return;
            };
            if inner.store.items[index].profile != profile {
                inner.store.items[index].state = State::Paused;
                inner.message = tr!("A pending file belongs to another saved session. Its upload remains paused.");
            } else {
                let (snapshots, item) = (inner.store.snapshots.clone(), inner.store.items[index].clone());
                // A pause during the check stops it, so disconnecting never waits for a large file's hash.
                let cancelled = inner.cancelled.clone();
                cancelled.store(false, Ordering::SeqCst);
                inner.current = Some(item.identifier.clone());
                inner.future = Some(
                    self.worker
                        .submit(move || Outcome::Checked(snapshots.verify_cancellable(&item, &cancelled))),
                );
                inner.store.items[index].state = State::Checking;
            }
        }
        self.notify();
    }

    /// Handle a finished worker task.
    fn collect_future(&self) {
        let outcome = {
            let mut inner = self.inner.borrow_mut();
            let outcome = match inner.future.as_ref().map(Receiver::try_recv) {
                Some(Ok(outcome)) => outcome,
                Some(Err(TryRecvError::Disconnected)) => {
                    Outcome::Checked(Err(Failure::Io(io::Error::other("The upload worker stopped."))))
                }
                _ => return,
            };
            inner.future = None;
            outcome
        };
        let failure = match outcome {
            Outcome::Prepared(items, error) => self.prepared(items, error).err(),
            Outcome::Checked(Ok(())) => {
                self.snapshot_checked();
                None
            }
            // A pause stopped the check; the row waits for Resume like any paused row.
            Outcome::Checked(Err(error)) if interrupted(&error) && self.inner.borrow().paused => {
                self.snapshot_checked();
                None
            }
            Outcome::Checked(Err(error)) => Some(error),
        };
        if let Some(error) = failure {
            self.pause();
            {
                let mut inner = self.inner.borrow_mut();
                inner.message = tr!("Could not prepare the files. Check space, permissions, and whether they changed.");
                // A row whose command still runs keeps its place until the command ends.
                if inner.process.is_none() {
                    if let Some(item) = inner.current_mut() {
                        item.state = State::Error;
                    }
                    inner.current = None;
                }
            }
            if storage(&error) {
                self.storage_failed();
            }
        }
        self.notify();
        if !self.busy() {
            self.idle.emit();
        }
    }

    /// Add a prepared batch and any ready snapshots missing from the rows. A batch that stopped early pauses the queue;
    /// its ready rows wait for Resume.
    fn prepared(&self, items: Vec<Upload>, error: Option<Failure>) -> Result<()> {
        {
            let mut inner = self.inner.borrow_mut();
            inner.store.items.extend(items);
            inner.store.recover_pending()?;
        }
        if let Some(error) = &error {
            self.pause();
            self.inner.borrow_mut().message =
                tr!("Some files could not be prepared. Ready local copies are retained; check space and source files.");
            if storage(error) {
                self.storage_failed();
            }
        }
        let mut inner = self.inner.borrow_mut();
        if inner.paused {
            inner.pause_rows();
        } else if error.is_none() {
            inner.message = tr!("Files added to the upload queue.");
        }
        Ok(())
    }

    /// The current snapshot is intact: look for the cloud file, unless the queue was paused meanwhile.
    fn snapshot_checked(&self) {
        let paused = {
            let mut inner = self.inner.borrow_mut();
            if inner.paused {
                if let Some(item) = inner.current_mut() {
                    item.state = State::Paused;
                }
                inner.current = None;
            }
            inner.paused
        };
        if !paused {
            let remote = self.inner.borrow().current().map(Upload::remote);
            if let Some(remote) = remote {
                self.run(Step::Inspect, vec![remote.into(), "--stat".into()]);
            }
        }
    }

    /// Start an rclone command for the current row after a durable checkpoint, while this app's connection is open
    /// and the row belongs to its profile.
    fn run(&self, step: Step, arguments: Vec<OsString>) {
        // The checkpoint's change signal may pause the queue, for example on a disconnect, so check again after it.
        if self.halted() || !self.notify() || self.halted() {
            return;
        }
        let (unlocked, profile) = (self.connection.unlocked(), self.connection.profile_id());
        let size = self
            .inner
            .borrow()
            .current()
            .filter(|item| unlocked && item.profile == profile)
            .map(|item| item.size);
        let Some(size) = size else {
            self.pause();
            return;
        };
        let mut command = self.connection.rclone();
        command.args(OPTIONS).arg(step.command()).args(arguments);
        let capture = Capture::new(step, size);
        match Process::start(command, None, true) {
            Ok(process) => self.inner.borrow_mut().process = Some(Running { process, capture }),
            // Like a failed command: the row keeps its snapshot and reports the error.
            Err(_) => self.finished(capture, 1),
        }
    }

    fn halted(&self) -> bool {
        let inner = self.inner.borrow();
        inner.paused || inner.storage_error
    }

    /// Handle what the running command printed, and its exit.
    fn collect_process(&self) {
        let (progress, finished) = {
            let mut inner = self.inner.borrow_mut();
            let Some(running) = inner.process.as_mut() else {
                return;
            };
            let (mut progress, mut exit) = (None, None);
            for event in running.process.events() {
                match event {
                    Event::Stdout(data) => {
                        if !running.capture.output(&data) {
                            running.process.signal(SIGKILL);
                        }
                    }
                    Event::Stderr(data) => progress = running.capture.errors(&data).or(progress),
                    Event::Exited(code) => exit = Some(code),
                }
            }
            if let (Some(progress), Some(item)) = (progress, inner.current_mut()) {
                item.progress = progress;
            }
            (
                progress,
                exit.and_then(|code| Some((inner.process.take()?.capture, code))),
            )
        };
        if progress.is_some() {
            self.changed.emit();
        }
        if let Some((capture, code)) = finished {
            self.finished(capture, code);
        }
    }

    /// Continue after a command ended, unless the queue was paused meanwhile.
    fn finished(&self, capture: Capture, code: i32) {
        if self.inner.borrow().paused {
            {
                let mut inner = self.inner.borrow_mut();
                if let Some(item) = inner.current_mut() {
                    item.state = State::Paused;
                }
                inner.current = None;
            }
            self.notify();
            self.idle.emit();
            return;
        }
        match capture.step {
            Step::Inspect => self.inspected(code, &capture.output),
            Step::Create => self.created(code),
            Step::Copy => self.uploaded(code),
            Step::Read => self.checked(code, capture.download),
        }
    }

    fn inspected(&self, code: i32, output: &[u8]) {
        let Some((destination, size)) = self
            .inner
            .borrow()
            .current()
            .map(|item| (item.destination.clone(), item.size))
        else {
            return;
        };
        match code {
            // rclone found neither the folder nor the file.
            3 | 4 => self.run(Step::Create, vec![format!("icloud:{destination}").into()]),
            0 => match describe(output) {
                Some((false, found)) if found == Some(size) => self.read_remote(false),
                Some(_) => self.conflict(),
                None => self.fail(tr!(
                    "The cloud returned an unexpected file description. Resume to check again."
                )),
            },
            _ => self.fail(tr!("Could not check the cloud file. Check your connection and resume.")),
        }
    }

    fn created(&self, code: i32) {
        if code != 0 {
            self.fail(tr!(
                "Could not open the destination folder. Check your connection and resume."
            ));
            return;
        }
        let destination = {
            let mut inner = self.inner.borrow_mut();
            let Some(item) = inner.current_mut() else {
                return;
            };
            item.state = State::Uploading;
            item.destination.clone()
        };
        if !self.notify() {
            return;
        }
        let directory = {
            let inner = self.inner.borrow();
            let Some(item) = inner.current() else {
                return;
            };
            inner.store.snapshots.copy_directory(item)
        };
        match directory {
            Ok(directory) => {
                // A folder source: rclone 1.75.2 bypasses `--immutable` for a single-file source.
                let mut arguments = vec![directory.into_os_string(), format!("icloud:{destination}").into()];
                arguments.extend(
                    [
                        "--immutable",
                        "--ignore-times",
                        "--use-json-log",
                        "--stats",
                        "500ms",
                        "--stats-log-level",
                        "NOTICE",
                    ]
                    .map(OsString::from),
                );
                self.run(Step::Copy, arguments);
            }
            Err(Failure::Io(_)) => self.storage_failed(),
            Err(Failure::Invalid(_)) => self.fail(tr!(
                "The local upload source changed. Its cloud destination was left untouched."
            )),
        }
    }

    fn uploaded(&self, code: i32) {
        match code {
            0 => self.read_remote(true),
            // A NoRetry error, which `--immutable` raises for a different existing file.
            6 => self.conflict(),
            _ => self.fail(tr!(
                "The upload stopped. The local copy is retained; resume to check the cloud and retry."
            )),
        }
    }

    /// Download the cloud file to compare it with the snapshot.
    fn read_remote(&self, verifying: bool) {
        let remote = {
            let mut inner = self.inner.borrow_mut();
            let Some(item) = inner.current_mut() else {
                return;
            };
            item.state = if verifying { State::Verifying } else { State::Checking };
            item.remote()
        };
        if self.notify() {
            self.run(Step::Read, vec![remote.into()]);
        }
    }

    /// Finish the row when the download matches the snapshot; different bytes are a conflict.
    fn checked(&self, code: i32, download: Download) {
        let Some((identifier, size, digest)) = self
            .inner
            .borrow()
            .current()
            .map(|item| (item.identifier.clone(), item.size, item.digest.clone()))
        else {
            return;
        };
        if download.oversized {
            self.conflict();
        } else if code != 0 {
            self.fail(tr!(
                "Could not verify the cloud copy. The local copy is retained; resume to check again."
            ));
        } else if download.bytes != size || profile::hex(&download.hash.finalize()) != digest {
            self.conflict();
        } else {
            {
                let mut inner = self.inner.borrow_mut();
                if let Some(item) = inner.item_mut(&identifier) {
                    item.state = State::Completed;
                    item.progress = 100;
                }
                inner.message = tr!("Upload verified. The cloud copy matches the local snapshot.");
            }
            if self.notify() {
                self.discard(&identifier);
            } else {
                // The completion was not saved, so the snapshot stays until a later check.
                if let Some(item) = self.inner.borrow_mut().item_mut(&identifier) {
                    item.state = State::Paused;
                    item.progress = 99;
                }
                self.changed.emit();
            }
            self.inner.borrow_mut().current = None;
        }
    }

    fn conflict(&self) {
        self.end_current(
            State::Conflict,
            tr!("A different cloud file exists. Skip this upload or keep both under different names."),
        );
    }

    fn fail(&self, message: String) {
        self.end_current(State::Error, message);
    }

    fn end_current(&self, state: State, message: String) {
        {
            let mut inner = self.inner.borrow_mut();
            if let Some(item) = inner.current_mut() {
                item.state = state;
            }
            inner.current = None;
            inner.message = message;
        }
        self.notify();
    }
}

impl Drop for Transfers {
    /// Stop preparation and the running command, as an app that exits does; every row is already saved.
    fn drop(&mut self) {
        self.inner.get_mut().stop();
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::fs::Permissions;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    use super::*;
    use crate::paths::Paths;
    use crate::uploads::tests::ReadOnly;

    const KEY: &str = "fictional-local-key";
    /// Logs its arguments and exits with the code in `rclone.<command>`, or 0.
    const SCRIPT: &str = "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$0.log\"\nshift 9\n\
                          if [ -f \"$0.$1\" ]; then exit \"$(cat \"$0.$1\")\"; fi\n";

    struct Fake {
        paths: Paths,
        unlocked: Cell<bool>,
        profile: String,
        changed: Signal,
        rclone: PathBuf,
    }

    impl Connection for Fake {
        fn paths(&self) -> &Paths {
            &self.paths
        }

        fn unlocked(&self) -> bool {
            self.unlocked.get()
        }

        fn profile_id(&self) -> String {
            self.profile.clone()
        }

        fn rclone(&self) -> Command {
            let mut command = Command::new(&self.rclone);
            command.env("RCLONE_CONFIG_PASS", KEY);
            command
        }

        fn changed(&self) -> &Signal {
            &self.changed
        }
    }

    /// A queue with one prepared row, not yet saved, like the Python tests start from.
    struct Setup {
        temporary: tempfile::TempDir,
        connection: Rc<Fake>,
        queue: Rc<Transfers>,
        source: PathBuf,
        identifier: String,
    }

    fn setup() -> Setup {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let rclone = root.join("rclone");
        fs::write(&rclone, SCRIPT).unwrap();
        fs::set_permissions(&rclone, Permissions::from_mode(0o700)).unwrap();
        let connection = Rc::new(Fake {
            paths: Paths {
                config: root.join("rclone.conf"),
                mount: root.join("mount"),
                cache: root.join("cache"),
                runtime: root.join("runtime"),
                helper: root.join("session.sh"),
            },
            unlocked: Cell::new(true),
            profile: "f".repeat(64),
            changed: Signal::default(),
            rclone,
        });
        let queue = Transfers::new(connection.clone()).unwrap();
        let source = root.join("upload.bin");
        fs::write(&source, b"Local queue test payload").unwrap();
        let item = {
            let inner = queue.inner.borrow();
            inner
                .store
                .snapshots
                .stage(&source, "Tests", &connection.profile, &AtomicBool::new(false))
                .unwrap()
        };
        let identifier = item.identifier.clone();
        let mut inner = queue.inner.borrow_mut();
        inner.store.items.push(item);
        inner.paused = false;
        drop(inner);
        Setup {
            temporary,
            connection,
            queue,
            source,
            identifier,
        }
    }

    impl Setup {
        fn item(&self) -> Upload {
            self.queue
                .items()
                .into_iter()
                .find(|item| item.identifier == self.identifier)
                .unwrap()
        }

        fn update(&self, change: impl FnOnce(&mut Upload)) {
            change(self.queue.inner.borrow_mut().item_mut(&self.identifier).unwrap());
        }

        fn uploads(&self) -> PathBuf {
            self.temporary.path().join("cache/uploads")
        }

        fn payload(&self) -> PathBuf {
            self.queue.inner.borrow().store.snapshots.payload(&self.item())
        }

        fn verify(&self) {
            self.queue.inner.borrow().store.snapshots.verify(&self.item()).unwrap();
        }

        /// The rclone commands started so far.
        fn commands(&self) -> String {
            fs::read_to_string(self.temporary.path().join("rclone.log")).unwrap_or_default()
        }

        fn exit(&self, command: &str, code: i32) {
            fs::write(
                self.temporary.path().join(format!("rclone.{command}")),
                code.to_string(),
            )
            .unwrap();
        }

        fn make_current(&self) {
            self.queue.inner.borrow_mut().current = Some(self.identifier.clone());
        }

        /// Run a long command for the row, as if `step` had started.
        fn start(&self, step: Step) {
            let mut command = Command::new("sleep");
            command.arg("30");
            let process = Process::start(command, None, true).unwrap();
            self.make_current();
            self.queue.inner.borrow_mut().process = Some(Running {
                process,
                capture: Capture::new(step, 0),
            });
        }

        /// Let the worker task in flight return `outcome`.
        fn finish_task(&self, outcome: Outcome) {
            let (sender, receiver) = mpsc::channel();
            sender.send(outcome).unwrap();
            self.queue.inner.borrow_mut().future = Some(receiver);
        }

        /// Tick until `done` holds, failing after ten seconds.
        fn wait(&self, done: impl Fn() -> bool) {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !done() {
                assert!(Instant::now() < deadline, "the queue did not reach the expected state");
                self.queue.tick();
                thread::sleep(Duration::from_millis(10));
            }
        }

        /// Wait until the worker finished the tasks submitted so far.
        fn settle(&self) {
            self.queue.worker.submit(|| ()).recv().unwrap();
        }
    }

    impl Drop for Setup {
        fn drop(&mut self) {
            self.queue.pause();
            let deadline = Instant::now() + Duration::from_secs(10);
            while self.queue.busy() && Instant::now() < deadline {
                self.queue.tick();
                thread::sleep(Duration::from_millis(10));
            }
        }
    }

    #[test]
    fn a_pause_stops_the_snapshot_check_without_an_error() {
        let setup = setup();
        setup.make_current();
        setup.update(|item| item.state = State::Checking);
        setup.queue.pause();
        setup.finish_task(Outcome::Checked(Err(uploads::preparation_paused())));
        setup.queue.tick();
        assert_eq!(setup.item().state, State::Paused);
        assert!(!setup.queue.storage_error() && !setup.queue.busy());
        let cancelled = AtomicBool::new(true);
        let snapshots = setup.queue.inner.borrow().store.snapshots.clone();
        let error = snapshots.verify_cancellable(&setup.item(), &cancelled).unwrap_err();
        assert!(interrupted(&error));
        setup.verify();
    }

    #[test]
    fn another_profile_cannot_receive_a_pending_upload() {
        let setup = setup();
        setup.update(|item| item.profile = "b".repeat(64));
        setup.queue.tick();
        assert!(!setup.queue.busy());
        assert_eq!(setup.item().state, State::Paused);
        assert!(setup.payload().exists());
        assert_eq!(setup.commands(), "");
    }

    #[test]
    fn a_changed_snapshot_never_reaches_the_cloud() {
        let setup = setup();
        fs::write(setup.payload(), b"Edited cache bytes").unwrap();
        setup.wait(|| setup.item().state == State::Error);
        assert!(setup.queue.paused());
        assert_eq!(setup.commands(), "");
    }

    #[test]
    fn a_failed_cloud_check_does_not_assume_the_file_is_absent() {
        let setup = setup();
        setup.exit("lsjson", 1);
        setup.wait(|| setup.item().state == State::Error);
        let commands = setup.commands();
        assert_eq!(commands.lines().count(), 1, "{commands}");
        assert!(
            commands.contains(" 1 lsjson icloud:Tests/upload.bin --stat"),
            "{commands}"
        );
        setup.verify();
    }

    #[test]
    fn no_account_configuration_or_key_enters_the_upload_source() {
        let setup = setup();
        setup.exit("lsjson", 3);
        setup.wait(|| setup.commands().contains(" copy "));
        let commands = setup.commands();
        let directory = setup.queue.inner.borrow().store.snapshots.directory(&setup.item());
        let expected = format!(" copy {} icloud:Tests --immutable ", directory.display());
        assert!(commands.contains(&expected), "{commands}");
        assert!(commands.contains(" mkdir icloud:Tests\n"), "{commands}");
        assert!(!commands.contains(KEY));
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
    }

    #[test]
    fn a_full_disk_stops_the_upload_before_copy_starts() {
        let setup = setup();
        setup.make_current();
        {
            let _full = ReadOnly::new(&setup.uploads());
            setup.queue.created(0);
        }
        assert!(!setup.queue.busy());
        assert!(setup.queue.paused() && setup.queue.storage_error());
        assert_eq!(setup.commands(), "");
        setup.verify();
    }

    #[test]
    fn a_failed_skip_checkpoint_keeps_the_pending_copy() {
        let setup = setup();
        setup.update(|item| item.state = State::Conflict);
        {
            let _full = ReadOnly::new(&setup.uploads());
            setup.queue.resolve(&setup.identifier, false);
        }
        setup.settle();
        assert_eq!(setup.item().state, State::Conflict);
        setup.verify();
    }

    #[test]
    fn a_skipped_conflict_releases_its_snapshot() {
        let setup = setup();
        setup.update(|item| item.state = State::Conflict);
        setup.queue.resolve(&setup.identifier, false);
        setup.settle();
        assert_eq!(setup.item().state, State::Skipped);
        assert!(!setup.uploads().join(&setup.identifier).exists());
        assert_eq!(setup.commands(), "");
    }

    #[test]
    fn a_failed_completion_checkpoint_keeps_the_pending_copy() {
        let setup = setup();
        setup.make_current();
        setup.update(|item| item.state = State::Verifying);
        let bytes = fs::read(&setup.source).unwrap();
        let mut download = Download {
            bytes: bytes.len() as u64,
            ..Download::default()
        };
        download.hash.update(&bytes);
        {
            let _full = ReadOnly::new(&setup.uploads());
            setup.queue.checked(0, download);
        }
        setup.settle();
        assert_eq!(setup.item().state, State::Paused);
        setup.verify();
    }

    #[test]
    fn no_command_starts_without_a_durable_checkpoint() {
        let setup = setup();
        setup.make_current();
        {
            let _full = ReadOnly::new(&setup.uploads());
            setup.queue.run(Step::Create, vec!["icloud:Tests".into()]);
        }
        assert!(!setup.queue.busy());
        assert!(setup.queue.storage_error());
        assert_eq!(setup.commands(), "");
    }

    #[test]
    fn a_storage_failure_needs_a_successful_resume() {
        let setup = setup();
        setup.queue.storage_failed();
        {
            let _full = ReadOnly::new(&setup.uploads());
            setup.queue.resume();
            assert!(setup.queue.paused() && setup.queue.storage_error());
        }
        assert!(!setup.queue.can_add());
        setup.queue.add(std::slice::from_ref(&setup.source), "Tests");
        assert!(!setup.queue.busy());
        setup.queue.resume();
        assert!(!setup.queue.storage_error() && !setup.queue.paused());
        assert_eq!(setup.item().state, State::Queued);
    }

    #[test]
    fn a_storage_failure_during_an_upload_stops_its_command() {
        let setup = setup();
        setup.update(|item| item.state = State::Uploading);
        setup.start(Step::Copy);
        setup.queue.inner.borrow_mut().checkpoint_at = Instant::now().checked_sub(CHECKPOINT).unwrap();
        {
            let _full = ReadOnly::new(&setup.uploads());
            setup.queue.tick();
            assert!(setup.queue.storage_error());
            assert_eq!(setup.item().state, State::Paused);
            // Only a killed command ends this soon.
            setup.wait(|| !setup.queue.busy());
        }
        setup.verify();
    }

    #[test]
    fn a_disconnect_during_a_checkpoint_prevents_the_next_command() {
        let setup = setup();
        setup.make_current();
        let (connection, disconnected) = (setup.connection.clone(), Cell::new(false));
        setup.queue.changed.connect(move || {
            if !disconnected.replace(true) {
                connection.unlocked.set(false);
                connection.changed.emit();
            }
        });
        setup.queue.run(Step::Create, vec!["icloud:Tests".into()]);
        assert!(!setup.queue.busy());
        assert!(setup.queue.paused());
        assert_eq!(setup.commands(), "");
    }

    #[test]
    fn a_cloud_file_that_grows_during_verification_is_a_conflict() {
        let setup = setup();
        setup.make_current();
        setup.queue.checked(
            9,
            Download {
                oversized: true,
                ..Download::default()
            },
        );
        assert_eq!(setup.item().state, State::Conflict);
        setup.verify();
    }

    #[test]
    fn keep_both_cannot_take_over_another_profiles_conflict() {
        let setup = setup();
        setup.update(|item| {
            item.state = State::Conflict;
            item.profile = "b".repeat(64);
        });
        let name = setup.item().name;
        setup.queue.resolve(&setup.identifier, true);
        assert_eq!((setup.item().state, setup.item().name), (State::Conflict, name));
        setup.verify();
    }

    #[test]
    fn a_preparation_failure_also_stops_an_active_upload() {
        let setup = setup();
        setup.update(|item| item.state = State::Uploading);
        setup.start(Step::Copy);
        let error = Failure::Invalid("Fixture source changed during preparation".into());
        setup.finish_task(Outcome::Prepared(Vec::new(), Some(error)));
        setup.queue.tick();
        assert!(setup.queue.paused());
        assert_eq!(setup.item().state, State::Paused);
        setup.wait(|| !setup.queue.busy());
        setup.verify();
    }

    #[test]
    fn a_recovery_failure_keeps_the_active_row_until_its_command_stops() {
        let setup = setup();
        setup.update(|item| item.state = State::Uploading);
        setup.start(Step::Copy);
        // A pending folder whose record is not private makes recovery fail.
        let pending = setup.uploads().join("b".repeat(32));
        crate::profile::private_directory(&pending).unwrap();
        fs::write(pending.join("upload.json"), "{}").unwrap();
        fs::set_permissions(pending.join("upload.json"), Permissions::from_mode(0o644)).unwrap();
        setup.finish_task(Outcome::Prepared(Vec::new(), None));
        setup.queue.tick();
        assert_eq!(
            setup.queue.inner.borrow().current.as_deref(),
            Some(setup.identifier.as_str())
        );
        assert_eq!(setup.item().state, State::Paused);
        setup.wait(|| !setup.queue.busy());
        assert_eq!(setup.queue.inner.borrow().current, None);
        setup.verify();
    }

    #[test]
    fn profiles_and_private_application_files_are_never_uploaded() {
        let setup = setup();
        let config = setup.connection.paths.config.clone();
        fs::write(&config, b"Fictional profile").unwrap();
        for private in [config, setup.payload(), setup.temporary.path().join(".env")] {
            setup.queue.add(&[setup.source.clone(), private], "Tests");
            assert!(!setup.queue.busy());
            assert!(setup.queue.message().starts_with("Choose regular files outside"));
        }
        setup.queue.add(std::slice::from_ref(&setup.source), "../Tests");
        assert!(!setup.queue.busy());
    }

    #[test]
    fn downloads_stop_beyond_the_snapshot_size_and_copies_report_progress() {
        let mut capture = Capture::new(Step::Read, 4);
        assert!(capture.output(b"abcd"));
        assert!(!capture.output(b"e"));
        assert!(capture.download.oversized && capture.download.bytes == 5);
        let mut capture = Capture::new(Step::Copy, 0);
        assert_eq!(
            capture.errors(br#"{"level":"notice","stats":{"bytes":25,"totalBytes":100}}"#),
            None
        );
        assert_eq!(capture.errors(b"\n{\"stats\":{\"bytes\":50,"), Some(25));
        assert_eq!(capture.errors(b"\"totalBytes\":100}}\nnot json\n"), Some(50));
        assert_eq!(
            capture.errors(b"{\"stats\":{\"bytes\":100,\"totalBytes\":100}}\n"),
            Some(99)
        );
        assert_eq!(capture.errors(b"{\"stats\":{\"bytes\":0,\"totalBytes\":0}}\n"), None);
    }
}
