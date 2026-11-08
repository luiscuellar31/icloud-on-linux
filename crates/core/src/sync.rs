//! Synced folders: iCloud Drive folders paired with private local copies under `iCloud Sync/`, kept in two-way sync
//! by rclone bisync, one run at a time and only while the connection is unlocked. The window calls `tick` every second
//! for as long as the app runs, also while disconnected; `changed` asks it to render again, and `idle` tells it that
//! no run is left after `pause`.
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::ffi::{OsStr, OsString};
use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::LazyLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use nix::sys::signal::Signal as UnixSignal;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::connection::Connection;
use crate::process::{Event, Process};
use crate::profile::{delete_path, private_directory, random_hex, write_json};
use crate::signal::Signal;
use crate::uploads::{folder, read_private_json};
use crate::{Failure, Result, tr};

/// Editor lock and temporary files are transient; syncing them would only create conflicts.
const EXCLUDES: [&str; 5] = [".~lock.*#", "~$*", ".goutputstream-*", "*.swp", "*.part"];
const OPTIONS: [&str; 18] = [
    "--compare",
    "size,modtime",
    "--create-empty-src-dirs",
    "--resilient",
    "--recover",
    "--max-lock",
    "2m",
    "--conflict-resolve",
    "newer",
    "--conflict-loser",
    "num",
    "--conflict-suffix",
    "conflict",
    "--suffix-keep-extension",
    "--contimeout",
    "15s",
    "--timeout",
    "45s",
];
/// Deletions, or changes in a folder of at most this many items, continue without review.
const SMALL_CHANGE: u64 = 5;
const POLL: Duration = Duration::from_secs(120);
const RETRY: Duration = Duration::from_secs(60);
const CASE_RETRY: Duration = Duration::from_secs(5);
const SCAN: Duration = Duration::from_secs(5);
const KILL_AFTER: Duration = Duration::from_secs(30);
const LOG_LIMIT: usize = 64 * 1024;

static CONFLICT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\.conflict\d+(\.[^.]*)?$").unwrap());
static DELETES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"too many deletes \(>\d+%, (\d+) of (\d+)\) on (Path[12])").unwrap());
static CHANGED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"all files were changed on (Path[12])").unwrap());
static EMPTY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"Empty current (Path[12]) listing").unwrap());
static IDENTIFIER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9a-f]{32}$").unwrap());
static PROFILE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-f0-9]{64}$").unwrap());

/// What the window shows for a folder.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum State {
    #[default]
    Waiting,
    Paused,
    Syncing,
    ChangesPending,
    UpToDate,
    NeedsReview,
    Error,
}

impl State {
    /// The English name, for reports.
    pub fn name(self) -> &'static str {
        match self {
            State::Waiting => "Waiting",
            State::Paused => "Paused",
            State::Syncing => "Syncing",
            State::ChangesPending => "Changes pending",
            State::UpToDate => "Up to date",
            State::NeedsReview => "Needs review",
            State::Error => "Error",
        }
    }

    /// The state's name in the current language.
    pub fn label(self) -> String {
        match self {
            State::Waiting => tr!("Waiting"),
            State::Paused => tr!("Paused"),
            State::Syncing => tr!("Syncing"),
            State::ChangesPending => tr!("Changes pending"),
            State::UpToDate => tr!("Up to date"),
            State::NeedsReview => tr!("Needs review"),
            State::Error => tr!("Error"),
        }
    }
}

/// What a stopped folder needs before it syncs again, and what the user approved for its next run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Review {
    /// Many items were deleted or changed: one run with `--force` applies them.
    Confirm,
    /// A side is empty or the sync history is missing: a copy-only run restores the pair.
    Repair,
}

impl Review {
    /// The English name, for reports.
    pub fn name(self) -> &'static str {
        match self {
            Review::Confirm => "confirm",
            Review::Repair => "repair",
        }
    }
}

/// An iCloud Drive folder and its local copy. The first five fields are saved; the others describe this session.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SyncFolder {
    pub identifier: String,
    pub local: PathBuf,
    /// The folder relative to iCloud Drive.
    pub remote: String,
    /// The identity of the profile that added the pair; no other profile syncs it.
    pub profile: String,
    /// Whether the first copy finished, so runs may also propagate deletions.
    pub ready: bool,
    #[serde(skip)]
    pub state: State,
    #[serde(skip)]
    pub message: String,
    /// Local changes found by the last scan.
    #[serde(skip)]
    pub pending: usize,
    #[serde(skip)]
    pub conflicts: usize,
    #[serde(skip)]
    pub attention: Option<Review>,
    /// Why the folder needs review.
    #[serde(skip)]
    pub notice: String,
    #[serde(skip)]
    approved: Option<Review>,
    #[serde(skip)]
    manual: bool,
    #[serde(skip)]
    failed: bool,
    /// The local entries after the last successful run.
    #[serde(skip)]
    baseline: Option<Rc<Snapshot>>,
    /// When the folder is due again; `None` means now.
    #[serde(skip)]
    next_run: Option<Instant>,
}

impl SyncFolder {
    /// How many local items the last successful run of this session left.
    pub fn items(&self) -> Option<usize> {
        self.baseline.as_ref().map(|baseline| baseline.len())
    }

    fn valid(&self) -> bool {
        IDENTIFIER.is_match(&self.identifier)
            && self.local.is_absolute()
            && folder(&self.remote).is_ok()
            && PROFILE.is_match(&self.profile)
    }
}

/// The saved folder list, shared with the Python version of the app.
#[derive(Serialize, Deserialize)]
struct List<T> {
    version: u64,
    folders: T,
}

/// Local entries by path relative to the folder.
type Snapshot = BTreeMap<OsString, Entry>;

/// What a scan compares: folders by presence only, other entries also by size and modification time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Entry {
    directory: bool,
    size: u64,
    modified: (i64, i64),
}

/// The commands of one run for a folder: the current one first, then those that follow it after a success.
struct Job {
    identifier: String,
    commands: Vec<Vec<OsString>>,
    backups: PathBuf,
}

struct Run {
    job: Job,
    process: Process,
    /// The end of the error output, where bisync explains why it stopped.
    log: Vec<u8>,
    interrupted: Option<Instant>,
}

/// The synced folders of this computer and the bisync run that keeps one of them in sync.
pub struct SyncedFolders {
    pub changed: Signal,
    /// Emitted when no run is left after `pause`, so the connection may close.
    pub idle: Signal,
    connection: Rc<dyn Connection>,
    root: PathBuf,
    local_root: PathBuf,
    folders: RefCell<Vec<SyncFolder>>,
    run: RefCell<Option<Run>>,
    paused: Cell<bool>,
    message: RefCell<String>,
    scanned_at: Cell<Option<Instant>>,
    scan_interval: Cell<Duration>,
    // Signals wait in these flags until `notify`, so no slot runs while a borrow is held.
    dirty: Cell<bool>,
    idle_due: Cell<bool>,
}

impl SyncedFolders {
    /// Open the saved folder list. An unsafe or unsupported list is refused and left unchanged.
    pub fn new(connection: Rc<dyn Connection>) -> Result<Rc<SyncedFolders>> {
        let paths = connection.paths();
        let root = paths.cache.join("sync");
        let local_root = paths.mount.parent().unwrap_or(&paths.mount).join("iCloud Sync");
        private_directory(&root)?;
        let store = root.join("folders.json");
        let folders = match fs::symlink_metadata(&store) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
            _ => load(&store)?,
        };
        let sync = Rc::new(SyncedFolders {
            changed: Signal::default(),
            idle: Signal::default(),
            connection,
            root,
            local_root,
            folders: RefCell::new(folders),
            run: RefCell::new(None),
            paused: Cell::new(false),
            message: RefCell::new(String::new()),
            scanned_at: Cell::new(None),
            scan_interval: Cell::new(SCAN),
            dirty: Cell::new(false),
            idle_due: Cell::new(false),
        });
        let weak = Rc::downgrade(&sync);
        sync.connection.changed().connect(move || {
            if let Some(sync) = weak.upgrade() {
                sync.connection_changed();
            }
        });
        Ok(sync)
    }

    /// Where new local copies are created.
    pub fn local_root(&self) -> &Path {
        &self.local_root
    }

    /// Why the last change to the folder list failed, or nothing.
    pub fn message(&self) -> String {
        self.message.borrow().clone()
    }

    pub fn folders(&self) -> Vec<SyncFolder> {
        self.folders.borrow().clone()
    }

    pub fn folder(&self, identifier: &str) -> Option<SyncFolder> {
        self.folders
            .borrow()
            .iter()
            .find(|folder| folder.identifier == identifier)
            .cloned()
    }

    /// Whether bisync runs; the connection must stay open until it ends.
    pub fn busy(&self) -> bool {
        self.run.borrow().is_some()
    }

    /// The folder that is syncing now; it cannot be removed meanwhile.
    pub fn current(&self) -> Option<String> {
        self.run.borrow().as_ref().map(|run| run.job.identifier.clone())
    }

    /// Local files replaced or deleted from iCloud, in one folder per run.
    pub fn backups(&self, identifier: &str) -> PathBuf {
        self.root.join(identifier).join("backups")
    }

    /// Pair the iCloud Drive folder `remote`, relative to the drive, with a new private local folder. Returns whether
    /// it was added; otherwise `message` says why.
    pub fn add(&self, remote: &str) -> bool {
        if !self.connection.unlocked() {
            return false;
        }
        let message = match self.create(remote) {
            Ok(folder) => {
                let mut folders = self.folders.borrow_mut();
                folders.push(folder);
                if self.save(&folders).is_ok() {
                    String::new()
                } else {
                    if let Some(folder) = folders.pop() {
                        let _ = fs::remove_dir(folder.local);
                    }
                    tr!("The synced folder list could not be saved. Check local space and permissions.")
                }
            }
            Err(Failure::Invalid(message)) => message,
            Err(Failure::Io(_)) => tr!("The local synced folder could not be created."),
        };
        let added = message.is_empty();
        *self.message.borrow_mut() = message;
        self.dirty.set(true);
        self.notify();
        added
    }

    /// Stop syncing a folder unless it is syncing now. The local copy, the iCloud folder, and its Backups are kept.
    pub fn remove(&self, identifier: &str) {
        if self.current().as_deref() == Some(identifier) {
            return;
        }
        {
            let mut folders = self.folders.borrow_mut();
            let Some(index) = folders.iter().position(|folder| folder.identifier == identifier) else {
                return;
            };
            let folder = folders.remove(index);
            if self.save(&folders).is_ok() {
                let _ = delete_path(&self.work(identifier));
            } else {
                folders.insert(index, folder);
                *self.message.borrow_mut() =
                    tr!("The synced folder list could not be saved. Check local space and permissions.");
            }
        }
        self.dirty.set(true);
        self.notify();
    }

    /// Stop syncing every folder and delete the sync state and Backups; the local copies stay.
    pub fn forget(&self) -> Result<()> {
        self.folders.borrow_mut().clear();
        self.message.borrow_mut().clear();
        let result = delete_path(&self.root)
            .map_err(Failure::from)
            .and_then(|()| private_directory(&self.root));
        self.dirty.set(true);
        self.notify();
        result
    }

    /// Sync one folder, or every folder, as soon as possible.
    pub fn sync_now(&self, identifier: Option<&str>) {
        for folder in self.folders.borrow_mut().iter_mut() {
            if identifier.is_none_or(|identifier| identifier == folder.identifier) {
                folder.manual = true;
            }
        }
        self.tick();
    }

    /// Continue a folder that needs review with one run of the kind it asked for.
    pub fn approve(&self, identifier: &str) {
        let approved = find(&mut self.folders.borrow_mut(), identifier).and_then(|folder| {
            folder.approved = Some(folder.attention.take()?);
            folder.manual = true;
            Some(())
        });
        if approved.is_some() {
            self.tick();
        }
    }

    /// Stop syncing until the connection is unlocked again. A running bisync is interrupted with SIGINT, which lets
    /// it finish started transfers and save its state, and is killed 30 seconds later; `idle` follows once it ends.
    pub fn pause(&self) {
        self.interrupt();
        self.notify();
    }

    /// Collect the running bisync, kill it 30 seconds after an interruption, or start the first due folder.
    pub fn tick(&self) {
        if !self.collect() {
            self.schedule();
        }
        self.notify();
    }

    fn connection_changed(&self) {
        if self.connection.unlocked() {
            if self.paused.replace(false) {
                // Catch up with changes made elsewhere while disconnected.
                for folder in self.folders.borrow_mut().iter_mut() {
                    folder.manual = true;
                }
            }
        } else {
            self.interrupt();
        }
        self.dirty.set(true);
        self.notify();
    }

    fn interrupt(&self) {
        self.paused.set(true);
        match self.run.borrow_mut().as_mut() {
            // A second SIGINT would make bisync quit at once instead of saving its state.
            Some(run) if run.interrupted.is_none() => {
                run.process.signal(UnixSignal::SIGINT);
                run.interrupted = Some(Instant::now());
            }
            Some(_) => {}
            None => self.idle_due.set(true),
        }
    }

    /// Handle the running process's output and exit. Returns whether a process was running.
    fn collect(&self) -> bool {
        let ended = {
            let mut slot = self.run.borrow_mut();
            let Some(run) = slot.as_mut() else {
                return false;
            };
            let mut code = None;
            for event in run.process.events() {
                match event {
                    Event::Stderr(bytes) => {
                        run.log.extend_from_slice(&bytes);
                        let excess = run.log.len().saturating_sub(LOG_LIMIT);
                        run.log.drain(..excess);
                    }
                    Event::Stdout(_) => {}
                    Event::Exited(value) => code = Some(value),
                }
            }
            match code {
                Some(code) => slot.take().map(|run| (run, code)),
                None => {
                    if run.interrupted.is_some_and(|at| at.elapsed() >= KILL_AFTER) {
                        run.process.signal(UnixSignal::SIGKILL);
                    }
                    None
                }
            }
        };
        if let Some((run, code)) = ended {
            self.end(run.job, &run.log, code);
        }
        true
    }

    fn schedule(&self) {
        if self.paused.get() || !self.connection.unlocked() {
            for folder in self.folders.borrow_mut().iter_mut() {
                self.show(folder, State::Paused, tr!("Connect to sync this folder."));
            }
            return;
        }
        let profile = self.connection.profile_id();
        let now = Instant::now();
        let scan = self
            .scanned_at
            .get()
            .is_none_or(|at| now.duration_since(at) >= self.scan_interval.get());
        let mut scanning = Duration::ZERO;
        let mut due = None;
        for folder in self.folders.borrow_mut().iter_mut() {
            if folder.profile != profile {
                let message = tr!("Connect with the saved session that added this folder to sync it.");
                self.show(folder, State::Paused, message);
                continue;
            }
            if folder.attention.is_some() {
                let notice = folder.notice.clone();
                self.show(folder, State::NeedsReview, notice);
                continue;
            }
            if let Some(baseline) = folder.baseline.as_ref().filter(|_| scan) {
                let started = Instant::now();
                folder.pending = changes(baseline, &snapshot(&folder.local));
                scanning += started.elapsed();
                if folder.pending > 0 && !folder.failed {
                    let message = tr!("{count} local change(s) waiting to sync.", count = folder.pending);
                    self.show(folder, State::ChangesPending, message);
                }
            }
            // After a failure, local changes wait for the retry time instead of restarting at once.
            let wanted = folder.approved.is_some()
                || folder.manual
                || folder.next_run.is_none_or(|at| now >= at)
                || (!folder.failed && (folder.pending > 0 || !folder.ready));
            if wanted && due.is_none() {
                due = Some(folder.identifier.clone());
            }
        }
        if scan {
            // Large folders are scanned less often, so scanning never dominates the app.
            self.scanned_at.set(Some(now));
            self.scan_interval.set(SCAN.max(scanning * 10));
        }
        if let Some(identifier) = due {
            self.run(&identifier);
        }
    }

    fn run(&self, identifier: &str) {
        let backups = self.backups(identifier).join(timestamp(SystemTime::now()));
        let job = {
            let mut folders = self.folders.borrow_mut();
            let Some(folder) = find(&mut folders, identifier) else {
                return;
            };
            if let Err(error) = prepare(folder, &self.work(identifier)) {
                folder.manual = false;
                folder.failed = true;
                folder.next_run = Some(Instant::now() + RETRY);
                let message = match error {
                    Failure::Invalid(message) => message,
                    Failure::Io(_) => tr!("The sync state folder is unavailable."),
                };
                self.show(folder, State::Error, message);
                return;
            }
            // iCloud Drive treats names that differ only in letter case as one item, so one could replace the other.
            if let Some((first, second)) = case_collision(&snapshot(&folder.local)) {
                folder.manual = false;
                folder.failed = true;
                folder.next_run = Some(Instant::now() + CASE_RETRY);
                let message = tr!(
                    "\"{first}\" and \"{second}\" differ only in letter case, which iCloud Drive treats as the same \
                     name. Rename one of them to continue syncing.",
                    first = first,
                    second = second,
                );
                self.show(folder, State::Error, message);
                return;
            }
            folder.manual = false;
            let message = if folder.ready {
                tr!("Syncing...")
            } else {
                tr!("Copying the iCloud folder to this computer...")
            };
            self.show(folder, State::Syncing, message);
            let mut commands = vec![self.arguments(folder, &backups)];
            if folder.approved == Some(Review::Repair) {
                // Restoring may need an iCloud folder deleted elsewhere; mkdir leaves an existing one unchanged.
                let target = format!("icloud:{}", folder.remote);
                commands.insert(
                    0,
                    ["--ask-password=false", "mkdir", &target].map(OsString::from).to_vec(),
                );
            }
            Job {
                identifier: identifier.to_owned(),
                commands,
                backups,
            }
        };
        self.start(job);
    }

    fn arguments(&self, folder: &SyncFolder, backups: &Path) -> Vec<OsString> {
        let mut arguments: Vec<OsString> = vec![
            "--ask-password=false".into(),
            "bisync".into(),
            folder.local.clone().into(),
            format!("icloud:{}", folder.remote).into(),
            "--workdir".into(),
            self.work(&folder.identifier).into(),
            "--backup-dir1".into(),
            backups.into(),
        ];
        arguments.extend(OPTIONS.map(OsString::from));
        for pattern in EXCLUDES {
            arguments.extend(["--exclude", pattern].map(OsString::from));
        }
        if !folder.ready || folder.approved == Some(Review::Repair) || empty(folder.baseline.as_deref()) {
            // These runs only copy, never delete: where both sides differ, iCloud wins and Backups keep local bytes.
            // A folder with no files yet also needs them, because normal runs refuse an empty side.
            arguments.extend(["--resync-mode", "path2"].map(OsString::from));
        }
        if folder.approved == Some(Review::Confirm) {
            arguments.push("--force".into());
        }
        arguments
    }

    fn start(&self, job: Job) {
        let mut command = self.connection.rclone();
        command.args(&job.commands[0]);
        match Process::start(command, None, true) {
            Ok(process) => {
                *self.run.borrow_mut() = Some(Run {
                    job,
                    process,
                    log: Vec::new(),
                    interrupted: None,
                });
            }
            Err(_) => self.end(job, &[], 1),
        }
    }

    /// Start the job's next command after a success, or record how the run ended.
    fn end(&self, mut job: Job, log: &[u8], code: i32) {
        if self.paused.get() {
            if let Some(folder) = find(&mut self.folders.borrow_mut(), &job.identifier) {
                self.show(
                    folder,
                    State::Paused,
                    tr!("Sync paused. It continues after you reconnect."),
                );
            }
            self.idle_due.set(true);
        } else if code == 0 && job.commands.len() > 1 {
            job.commands.remove(0);
            self.start(job);
        } else {
            self.finished(&job.identifier, code, &String::from_utf8_lossy(log), &job.backups);
        }
    }

    fn finished(&self, identifier: &str, code: i32, log: &str, backups: &Path) {
        let mut folders = self.folders.borrow_mut();
        let Some(index) = folders.iter().position(|folder| folder.identifier == identifier) else {
            return;
        };
        if code == 0 && !folders[index].ready {
            folders[index].ready = true;
            if self.save(&folders).is_err() {
                folders[index].ready = false;
            }
        }
        let folder = &mut folders[index];
        folder.approved = None;
        folder.failed = code != 0;
        if code == 0 {
            let baseline = snapshot(&folder.local);
            let previous = folder.conflicts;
            folder.conflicts = baseline.keys().filter(|path| conflict_copy(path)).count();
            folder.baseline = Some(Rc::new(baseline));
            folder.pending = 0;
            // bisync records a resolved conflict's winner only on its next run; until then, deleting it would
            // restore it.
            folder.manual |= folder.conflicts > previous;
            folder.next_run = Some(Instant::now() + POLL);
            let mut message = tr!("Up to date.");
            if folder.conflicts > 0 {
                let kept = tr!(
                    "{count} conflict copy(ies) kept: the newer version keeps the name and the other ends with \
                     .conflict and a number.",
                    count = folder.conflicts,
                );
                message = format!("{message} {kept}");
            }
            if backups.exists() {
                message = format!(
                    "{message} {}",
                    tr!("Replaced or deleted local files were saved in Backups.")
                );
            }
            self.show(folder, State::UpToDate, message);
            return;
        }
        let (review, notice) = if log.contains("Safety abort") {
            let deletes = DELETES
                .captures_iter(log)
                .filter_map(|found| {
                    Some((
                        found[1].parse::<u64>().ok()?,
                        found[2].parse::<u64>().ok()?,
                        found[3].to_owned(),
                    ))
                })
                .max();
            let changed: Vec<&str> = CHANGED
                .captures_iter(log)
                .filter_map(|found| Some(found.get(1)?.as_str()))
                .collect();
            // bisync's ratios also stop renaming or editing a few files in a small folder. Such changes stay
            // recoverable from Recently Deleted and Backups, so they continue once without review.
            let small = match &deletes {
                Some((count, _, _)) => *count <= SMALL_CHANGE,
                None => {
                    !changed.is_empty()
                        && folder
                            .baseline
                            .as_ref()
                            .is_some_and(|baseline| baseline.len() as u64 <= SMALL_CHANGE)
                }
            };
            if small {
                folder.approved = Some(Review::Confirm);
                folder.manual = true;
                self.show(
                    folder,
                    State::Syncing,
                    tr!("Applying a few deletions, renames, or edits..."),
                );
                return;
            }
            let notice = match deletes {
                Some((count, total, path)) => tr!(
                    "{count} of {total} items were deleted or renamed {side} since the last sync, so nothing was \
                     synced. Continue only if that was intended.",
                    count = count,
                    total = total,
                    side = side(&path),
                ),
                None => tr!(
                    "Every item changed {side} since the last sync, for example after a clock change, so nothing was \
                     synced. Continue only if that was intended.",
                    side = changed.first().map_or_else(|| tr!("on one side"), |path| side(path)),
                ),
            };
            (Review::Confirm, notice)
        } else if log.contains("Empty current Path") {
            let where_ = EMPTY
                .captures(log)
                .map_or_else(|| tr!("on one side"), |found| side(&found[1]));
            let notice = tr!(
                "The synced folder is empty or missing {side}, so nothing was synced; sync never removes everything \
                 at once. Continue to copy the files back, or stop syncing to keep both sides as they are.",
                side = where_,
            );
            (Review::Repair, notice)
        } else if code == 7 && (log.contains("cannot find prior") || listing_errors(&self.work(identifier))) {
            let notice = tr!(
                "The sync history is missing or could not be trusted. Continue to restore it; where both sides \
                 differ, the iCloud version is used and the local copy is kept in Backups."
            );
            (Review::Repair, notice)
        } else {
            folder.next_run = Some(Instant::now() + RETRY);
            let message = tr!("Sync failed and will retry in a minute. Check the connection and free space.");
            self.show(folder, State::Error, message);
            return;
        };
        folder.attention = Some(review);
        folder.notice = notice.clone();
        self.show(folder, State::NeedsReview, notice);
    }

    fn save(&self, folders: &[SyncFolder]) -> io::Result<()> {
        write_json(&self.root.join("folders.json"), &List { version: 1, folders })
    }

    fn work(&self, identifier: &str) -> PathBuf {
        self.root.join(identifier).join("work")
    }

    fn show(&self, folder: &mut SyncFolder, state: State, message: String) {
        if folder.state != state || folder.message != message {
            folder.state = state;
            folder.message = message;
            self.dirty.set(true);
        }
    }

    /// Emit the signals that changes requested, once no borrow is held.
    fn notify(&self) {
        if self.dirty.replace(false) {
            self.changed.emit();
        }
        if self.idle_due.replace(false) {
            self.idle.emit();
        }
    }

    /// Validate `remote` and create its new private local folder.
    fn create(&self, remote: &str) -> Result<SyncFolder> {
        let remote = remote.trim_matches('/');
        if remote.is_empty() {
            return Err(Failure::Invalid(tr!(
                "Choose a folder inside iCloud Drive; syncing the whole drive is not supported."
            )));
        }
        folder(remote)?;
        // iCloud Drive names are case-insensitive, and nested pairs would sync the same files twice.
        let new = remote.to_lowercase();
        let overlaps = self.folders.borrow().iter().any(|folder| {
            let other = folder.remote.to_lowercase();
            Path::new(&new).starts_with(&other) || Path::new(&other).starts_with(&new)
        });
        if overlaps {
            return Err(Failure::Invalid(tr!(
                "This folder is already synced or overlaps a synced folder."
            )));
        }
        let identifier = random_hex()?;
        DirBuilder::new().recursive(true).mode(0o700).create(&self.local_root)?;
        let name = remote.rsplit('/').next().unwrap_or(remote);
        let mut local = self.local_root.join(name);
        let mut number = 2;
        while fs::symlink_metadata(&local).is_ok() {
            local = self.local_root.join(format!("{name} {number}"));
            number += 1;
        }
        DirBuilder::new().mode(0o700).create(&local)?;
        Ok(SyncFolder {
            identifier,
            local,
            remote: remote.to_owned(),
            profile: self.connection.profile_id(),
            ..SyncFolder::default()
        })
    }
}

fn load(store: &Path) -> Result<Vec<SyncFolder>> {
    let content = read_private_json(store)?;
    let unsupported = || Failure::Invalid("The synced folder list is unsupported.".to_owned());
    if content.get("version").and_then(serde_json::Value::as_u64) != Some(1) {
        return Err(unsupported());
    }
    let list: List<Vec<SyncFolder>> = serde_json::from_value(content).map_err(|_| unsupported())?;
    if !list.folders.iter().all(SyncFolder::valid) {
        return Err(Failure::Invalid("A synced folder entry is invalid.".to_owned()));
    }
    Ok(list.folders)
}

fn find<'a>(folders: &'a mut [SyncFolder], identifier: &str) -> Option<&'a mut SyncFolder> {
    folders.iter_mut().find(|folder| folder.identifier == identifier)
}

/// Check the local copy, create the private work folder, and remove its locks: only this app instance runs bisync
/// there, one folder at a time, so a lock is left by an interrupted run.
fn prepare(folder: &SyncFolder, work: &Path) -> Result<()> {
    if folder.approved == Some(Review::Repair) && fs::symlink_metadata(&folder.local).is_err() {
        DirBuilder::new().recursive(true).mode(0o700).create(&folder.local)?;
    }
    if fs::symlink_metadata(&folder.local).is_ok_and(|info| !info.is_dir()) {
        return Err(Failure::Invalid(tr!(
            "The local synced folder must be a regular folder."
        )));
    }
    private_directory(work)?;
    for entry in fs::read_dir(work)? {
        let entry = entry?;
        if entry.file_name().as_bytes().ends_with(b".lck") {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

/// Every entry under `root` except transient files. Links are recorded, never followed.
fn snapshot(root: &Path) -> Snapshot {
    let mut entries = Snapshot::new();
    let mut folders = vec![PathBuf::new()];
    while let Some(folder) = folders.pop() {
        let Ok(reader) = fs::read_dir(root.join(&folder)) else {
            continue;
        };
        for entry in reader.flatten() {
            let Ok(info) = entry.metadata() else {
                continue;
            };
            let path = folder.join(entry.file_name());
            if info.is_dir() {
                folders.push(path.clone());
            }
            if !excluded(&entry.file_name()) {
                let directory = info.is_dir();
                let entry = Entry {
                    directory,
                    size: if directory { 0 } else { info.size() },
                    modified: if directory {
                        (0, 0)
                    } else {
                        (info.mtime(), info.mtime_nsec())
                    },
                };
                entries.insert(path.into_os_string(), entry);
            }
        }
    }
    entries
}

fn excluded(name: &OsStr) -> bool {
    let name = name.as_bytes();
    // Every pattern has a single `*`.
    EXCLUDES.iter().chain(&["*.partial"]).any(|pattern| {
        let (prefix, suffix) = pattern.split_once('*').unwrap_or((pattern, ""));
        name.len() >= prefix.len() + suffix.len()
            && name.starts_with(prefix.as_bytes())
            && name.ends_with(suffix.as_bytes())
    })
}

/// How many entries were added, changed, or removed.
fn changes(baseline: &Snapshot, current: &Snapshot) -> usize {
    current
        .iter()
        .filter(|(path, entry)| baseline.get(*path) != Some(entry))
        .count()
        + baseline.keys().filter(|path| !current.contains_key(*path)).count()
}

fn case_collision(snapshot: &Snapshot) -> Option<(String, String)> {
    let mut seen: HashMap<String, &OsStr> = HashMap::new();
    for path in snapshot.keys() {
        if let Some(other) = seen.insert(path.to_string_lossy().to_lowercase(), path) {
            return Some((
                other.to_string_lossy().into_owned(),
                path.to_string_lossy().into_owned(),
            ));
        }
    }
    None
}

fn conflict_copy(path: &OsStr) -> bool {
    Path::new(path)
        .file_name()
        .is_some_and(|name| CONFLICT.is_match(&name.to_string_lossy()))
}

/// Whether a successful run left only folders, which bisync treats as an empty side.
fn empty(baseline: Option<&Snapshot>) -> bool {
    baseline.is_some_and(|baseline| baseline.values().all(|entry| entry.directory))
}

/// Whether bisync left a listing it could not trust.
fn listing_errors(work: &Path) -> bool {
    fs::read_dir(work).is_ok_and(|entries| {
        entries
            .flatten()
            .any(|entry| entry.file_name().as_bytes().ends_with(b".lst-err"))
    })
}

fn side(path: &str) -> String {
    if path == "Path1" {
        tr!("on this computer")
    } else {
        tr!("in iCloud")
    }
}

/// The name of a run's Backups folder, in UTC.
fn timestamp(time: SystemTime) -> String {
    let seconds = time.duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_secs());
    // Days since 1970 to a civil date, after Howard Hinnant's `civil_from_days`, with eras of 400 years from March 1.
    let days = seconds / 86_400 + 719_468;
    let (era, day_of_era) = (days / 146_097, days % 146_097);
    let year_of_era = (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = era * 400 + year_of_era + u64::from(month <= 2);
    let time = seconds % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02} {:02}.{:02}.{:02} UTC",
        time / 3_600,
        time / 60 % 60,
        time % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    const KEY: &str = "native-fixture-key";

    /// An unlocked connection whose rclone is `true`, which exits at once, or a shell script when one is set.
    struct Stub {
        paths: Paths,
        unlocked: Cell<bool>,
        profile: RefCell<String>,
        script: RefCell<Option<String>>,
        starts: Cell<usize>,
        changed: Signal,
    }

    impl Connection for Stub {
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
            self.starts.set(self.starts.get() + 1);
            let mut command = match self.script.borrow().as_deref() {
                Some(script) => {
                    let mut command = Command::new("sh");
                    command.args(["-c", script, "rclone"]);
                    command
                }
                None => Command::new("true"),
            };
            command.env("RCLONE_CONFIG_PASS", KEY);
            command
        }

        fn changed(&self) -> &Signal {
            &self.changed
        }
    }

    struct Case {
        root: tempfile::TempDir,
        connection: Rc<Stub>,
        sync: Rc<SyncedFolders>,
    }

    impl Case {
        fn new() -> Case {
            let root = tempfile::tempdir().unwrap();
            let paths = Paths {
                config: root.path().join("rclone.conf"),
                mount: root.path().join("iCloud"),
                cache: root.path().join("cache"),
                runtime: root.path().join("runtime"),
                helper: root.path().join("session.sh"),
            };
            let connection = Rc::new(Stub {
                paths,
                unlocked: Cell::new(true),
                profile: RefCell::new("a".repeat(64)),
                script: RefCell::new(None),
                starts: Cell::new(0),
                changed: Signal::default(),
            });
            let sync = SyncedFolders::new(connection.clone()).unwrap();
            Case { root, connection, sync }
        }

        fn add(&self, remote: &str) -> String {
            assert!(self.sync.add(remote), "{}", self.sync.message());
            self.sync.folders().last().unwrap().identifier.clone()
        }

        fn folder(&self, identifier: &str) -> SyncFolder {
            self.sync.folder(identifier).unwrap()
        }

        fn starts(&self) -> usize {
            self.connection.starts.get()
        }

        fn store(&self) -> PathBuf {
            self.root.path().join("cache/sync/folders.json")
        }

        fn saved(&self) -> serde_json::Value {
            serde_json::from_slice(&fs::read(self.store()).unwrap()).unwrap()
        }

        /// End the started run as bisync would, and return the arguments of its last command.
        fn finish(&self, code: i32, log: &str) -> Vec<OsString> {
            let run = self.sync.run.borrow_mut().take().expect("a run was started");
            self.sync.finished(&run.job.identifier, code, log, &run.job.backups);
            self.sync.notify();
            run.job.commands.last().unwrap().clone()
        }

        fn synced(&self, files: &[&str]) -> String {
            let identifier = self.add("Documents");
            for name in files {
                fs::write(self.folder(&identifier).local.join(name), "Synced fixture\n").unwrap();
            }
            self.sync.tick();
            self.finish(0, "");
            identifier
        }
    }

    fn has(arguments: &[OsString], name: &str) -> bool {
        arguments.iter().any(|argument| argument == name)
    }

    fn value<'a>(arguments: &'a [OsString], name: &str) -> &'a OsStr {
        let index = arguments.iter().position(|argument| argument == name).unwrap();
        &arguments[index + 1]
    }

    fn wait(mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !done() {
            assert!(Instant::now() < deadline, "the condition did not hold in time");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn added_folders_are_private_bound_to_the_profile_and_never_overlap() {
        let case = Case::new();
        let folder = case.folder(&case.add("Documents/Work"));
        assert_eq!(folder.local, case.root.path().join("iCloud Sync/Work"));
        assert_eq!(fs::metadata(&folder.local).unwrap().mode() & 0o777, 0o700);
        assert_eq!(folder.profile, "a".repeat(64));
        let saved = case.saved();
        assert_eq!(saved["folders"][0]["remote"], "Documents/Work");
        assert_eq!(saved["folders"][0]["ready"], false);
        for overlapping in ["documents/WORK", "Documents", "Documents/Work/Inner", "", "/"] {
            assert!(!case.sync.add(overlapping), "{overlapping}");
        }
        assert_eq!(case.sync.folders().len(), 1);
        assert!(case.sync.message().contains("whole drive"));
        let second = case.folder(&case.add("Projects/Work"));
        assert_eq!(second.local.file_name().unwrap(), "Work 2");
    }

    #[test]
    fn the_first_sync_only_copies_and_safety_options_are_always_used() {
        let case = Case::new();
        let identifier = case.add("Documents");
        case.sync.tick();
        let arguments = case.finish(0, "");
        let folder = case.folder(&identifier);
        assert_eq!(
            arguments[1..4],
            [
                OsString::from("bisync"),
                folder.local.clone().into(),
                "icloud:Documents".into()
            ]
        );
        for option in [
            "--resilient",
            "--recover",
            "--create-empty-src-dirs",
            "--suffix-keep-extension",
        ] {
            assert!(has(&arguments, option), "{option}");
        }
        for (name, expected) in [
            ("--resync-mode", "path2"),
            ("--max-lock", "2m"),
            ("--conflict-resolve", "newer"),
            ("--compare", "size,modtime"),
            ("--exclude", ".~lock.*#"),
        ] {
            assert_eq!(value(&arguments, name), expected);
        }
        let root = case.root.path().join("cache/sync").join(&identifier);
        assert_eq!(Path::new(value(&arguments, "--workdir")), root.join("work"));
        assert_eq!(
            Path::new(value(&arguments, "--backup-dir1")).parent(),
            Some(&*root.join("backups"))
        );
        assert!(!has(&arguments, "--force"));
        assert!(
            !arguments
                .iter()
                .any(|argument| argument.to_string_lossy().contains(KEY))
        );
        assert!(folder.ready);
        assert_eq!(case.saved()["folders"][0]["ready"], true);
        assert_eq!(folder.state, State::UpToDate);
    }

    #[test]
    fn local_changes_start_a_normal_sync_that_can_propagate_deletions() {
        let case = Case::new();
        let identifier = case.synced(&["notes.txt"]);
        case.sync.tick();
        assert_eq!(case.starts(), 1);
        fs::write(case.folder(&identifier).local.join("created.txt"), "New local file\n").unwrap();
        case.sync.scanned_at.set(None);
        case.sync.tick();
        assert_eq!(case.folder(&identifier).pending, 1);
        assert_eq!(case.starts(), 2);
        assert!(!has(&case.finish(0, ""), "--resync-mode"));
    }

    #[test]
    fn a_folder_without_files_keeps_copying_until_both_sides_have_files() {
        let case = Case::new();
        case.synced(&[]);
        case.sync.sync_now(None);
        assert!(has(&case.finish(0, ""), "--resync-mode"));
    }

    #[test]
    fn mass_changes_wait_for_confirmation_and_force_only_one_run() {
        let case = Case::new();
        let identifier = case.synced(&["notes.txt"]);
        case.sync.sync_now(None);
        case.finish(
            1,
            "ERROR : Safety abort: too many deletes (>50%, 6 of 8) on Path1 \"/fixture\". Run with --force if desired.",
        );
        let folder = case.folder(&identifier);
        assert_eq!(
            (folder.state, folder.attention),
            (State::NeedsReview, Some(Review::Confirm))
        );
        assert!(
            folder
                .message
                .contains("6 of 8 items were deleted or renamed on this computer")
        );
        case.sync.sync_now(None);
        assert_eq!(case.starts(), 2);
        case.sync.approve(&identifier);
        let forced = case.finish(0, "");
        assert!(has(&forced, "--force"));
        assert!(!has(&forced, "--resync-mode"));
        case.sync.sync_now(None);
        assert!(!has(&case.finish(0, ""), "--force"));
    }

    #[test]
    fn a_few_renames_or_edits_continue_once_without_review() {
        let case = Case::new();
        let identifier = case.synced(&["a.txt", "b.txt"]);
        case.sync.sync_now(None);
        case.finish(
            1,
            "ERROR : Safety abort: too many deletes (>50%, 2 of 2) on Path1 \"/fixture\". Run with --force if desired.",
        );
        let folder = case.folder(&identifier);
        assert_eq!((folder.attention, folder.state), (None, State::Syncing));
        case.sync.tick();
        assert!(has(&case.finish(0, ""), "--force"));
        assert_eq!(case.folder(&identifier).state, State::UpToDate);
        let lone = case.add("Single");
        fs::write(case.folder(&lone).local.join("only.txt"), "The only file\n").unwrap();
        case.sync.tick();
        case.finish(0, "");
        case.sync.sync_now(Some(&lone));
        case.finish(
            1,
            "ERROR : Safety abort: all files were changed on Path1 \"/fixture\". Run with --force if desired.",
        );
        assert_eq!(case.folder(&lone).attention, None);
        case.sync.tick();
        assert!(has(&case.finish(0, ""), "--force"));
    }

    #[test]
    fn every_item_changing_in_a_larger_folder_needs_review() {
        let case = Case::new();
        let identifier = case.synced(&["f0.txt", "f1.txt", "f2.txt", "f3.txt", "f4.txt", "f5.txt"]);
        case.sync.sync_now(None);
        case.finish(
            1,
            "ERROR : Safety abort: all files were changed on Path2 \"icloud:Documents\". Run with --force if desired.",
        );
        let folder = case.folder(&identifier);
        assert_eq!(folder.attention, Some(Review::Confirm));
        assert!(folder.message.contains("Every item changed in iCloud"));
    }

    #[test]
    fn an_empty_side_or_untrusted_history_needs_a_copy_only_repair() {
        let case = Case::new();
        let identifier = case.synced(&["notes.txt"]);
        case.sync.sync_now(None);
        case.finish(
            7,
            "ERROR : Empty current Path2 listing. Cannot sync to an empty directory",
        );
        let folder = case.folder(&identifier);
        assert_eq!(folder.attention, Some(Review::Repair));
        assert!(folder.message.contains("empty or missing in iCloud"));
        assert!(folder.local.join("notes.txt").exists());
        case.sync.approve(&identifier);
        let first = case.sync.run.borrow().as_ref().unwrap().job.commands[0].clone();
        assert_eq!(first, ["--ask-password=false", "mkdir", "icloud:Documents"]);
        let repaired = case.finish(0, "");
        assert_eq!(value(&repaired, "--resync-mode"), "path2");
        assert!(!has(&repaired, "--force"));
        case.sync.sync_now(None);
        case.finish(
            7,
            "ERROR : Bisync critical error: cannot find prior Path1 or Path2 listings",
        );
        assert_eq!(case.folder(&identifier).attention, Some(Review::Repair));
        case.sync.approve(&identifier);
        case.finish(0, "");
        let work = case.root.path().join("cache/sync").join(&identifier).join("work");
        fs::write(work.join("pair.path1.lst-err"), "locked\n").unwrap();
        case.sync.sync_now(None);
        case.finish(7, "ERROR : Bisync critical error");
        assert_eq!(case.folder(&identifier).attention, Some(Review::Repair));
    }

    #[test]
    fn failures_retry_later_even_with_local_changes() {
        let case = Case::new();
        let identifier = case.synced(&["notes.txt"]);
        case.sync.sync_now(None);
        case.finish(1, "ERROR : network unreachable");
        assert_eq!(case.folder(&identifier).state, State::Error);
        fs::write(case.folder(&identifier).local.join("edited.txt"), "Waiting for retry\n").unwrap();
        case.sync.scanned_at.set(None);
        case.sync.tick();
        assert_eq!((case.folder(&identifier).pending, case.starts()), (1, 2));
        case.sync.folders.borrow_mut()[0].next_run = None;
        case.sync.tick();
        assert_eq!(case.starts(), 3);
    }

    // Other systems may keep temporary files on folders that ignore letter case.
    #[cfg(target_os = "linux")]
    #[test]
    fn names_that_differ_only_in_case_pause_the_folder_until_renamed() {
        let case = Case::new();
        let identifier = case.synced(&["notes.txt"]);
        let local = case.folder(&identifier).local;
        fs::write(local.join("Notes.txt"), "Same name for iCloud\n").unwrap();
        case.sync.sync_now(None);
        assert_eq!(case.starts(), 1);
        let folder = case.folder(&identifier);
        assert_eq!(folder.state, State::Error);
        assert!(
            folder
                .message
                .contains("\"Notes.txt\" and \"notes.txt\" differ only in letter case")
        );
        fs::rename(local.join("Notes.txt"), local.join("Notes copy.txt")).unwrap();
        case.sync.folders.borrow_mut()[0].next_run = None;
        case.sync.tick();
        assert_eq!(case.starts(), 2);
    }

    #[test]
    fn another_profile_never_syncs_a_folder() {
        let case = Case::new();
        let identifier = case.add("Documents");
        case.sync.folders.borrow_mut()[0].profile = "b".repeat(64);
        case.sync.tick();
        assert_eq!(case.starts(), 0);
        assert_eq!(case.folder(&identifier).state, State::Paused);
    }

    #[test]
    fn conflict_copies_are_counted_and_transient_files_ignored() {
        let case = Case::new();
        let identifier = case.add("Documents");
        let local = case.folder(&identifier).local;
        for name in [
            "report.conflict1.odt",
            "notes.conflict2",
            "plain.txt",
            ".~lock.report.odt#",
            "download.bin.1a2b.partial",
        ] {
            fs::write(local.join(name), "Fixture\n").unwrap();
        }
        case.sync.tick();
        case.finish(0, "");
        let folder = case.folder(&identifier);
        assert_eq!(folder.conflicts, 2);
        let names: Vec<OsString> = snapshot(&local).into_keys().collect();
        assert_eq!(names, ["notes.conflict2", "plain.txt", "report.conflict1.odt"]);
        assert!(folder.message.contains("conflict"));
        assert!(case.sync.folders.borrow()[0].manual);
        case.sync.tick();
        case.finish(0, "");
        assert_eq!(case.starts(), 2);
        assert!(!case.sync.folders.borrow()[0].manual);
    }

    #[test]
    fn disconnect_interrupts_bisync_gracefully_and_keeps_the_key_out_of_arguments() {
        let case = Case::new();
        let report = case.root.path().join("report");
        let path = report.display();
        // The stub records its arguments and whether the key reached its environment, then waits for SIGINT.
        *case.connection.script.borrow_mut() = Some(format!(
            "trap 'kill $sleeper; exit 0' INT\n\
             printf '%s\\n' \"$@\" > '{path}.part'\n\
             if [ \"$RCLONE_CONFIG_PASS\" = '{KEY}' ]; then echo key-in-environment >> '{path}.part'; fi\n\
             mv '{path}.part' '{path}'\n\
             sleep 30 & sleeper=$!\n\
             wait $sleeper\n"
        ));
        let idle = Rc::new(Cell::new(false));
        let flag = idle.clone();
        case.sync.idle.connect(move || flag.set(true));
        let identifier = case.add("Documents");
        case.sync.tick();
        wait(|| report.exists());
        case.connection.unlocked.set(false);
        case.connection.changed.emit();
        wait(|| {
            case.sync.tick();
            !case.sync.busy()
        });
        let details = fs::read_to_string(&report).unwrap();
        assert!(details.contains("key-in-environment"));
        assert!(!details.contains(KEY));
        assert!(idle.get());
        let folder = case.folder(&identifier);
        assert_eq!(folder.state, State::Paused);
        assert!(!folder.ready);
        // Reconnecting catches up with changes made elsewhere meanwhile.
        case.connection.unlocked.set(true);
        case.connection.changed.emit();
        assert!(case.sync.folders.borrow()[0].manual);
    }

    #[test]
    fn an_unsafe_folder_list_is_refused() {
        let case = Case::new();
        let store = case.store();
        fs::write(&store, r#"{"version": 1, "folders": []}"#).unwrap();
        fs::set_permissions(&store, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(SyncedFolders::new(case.connection.clone()).is_err());
        fs::set_permissions(&store, fs::Permissions::from_mode(0o600)).unwrap();
        let invalid = r#"{"version": 1, "folders": [{"identifier": "x", "local": "relative", "remote": "A", "profile": "PROFILE", "ready": true}]}"#;
        fs::write(&store, invalid.replace("PROFILE", &"a".repeat(64))).unwrap();
        assert!(SyncedFolders::new(case.connection.clone()).is_err());
    }

    #[test]
    fn folder_lists_saved_by_the_python_app_keep_working() {
        let case = Case::new();
        let store = case.store();
        let python = r#"{"version": 1, "folders": [{"identifier": "IDENTIFIER", "local": "/home/fixture/iCloud Sync/Docs", "remote": "Docs", "profile": "PROFILE", "ready": true}]}"#
            .replace("IDENTIFIER", &"0".repeat(32))
            .replace("PROFILE", &"a".repeat(64));
        fs::write(&store, &python).unwrap();
        fs::set_permissions(&store, fs::Permissions::from_mode(0o600)).unwrap();
        let sync = SyncedFolders::new(case.connection.clone()).unwrap();
        let folder = sync.folder(&"0".repeat(32)).unwrap();
        assert_eq!(folder.local, Path::new("/home/fixture/iCloud Sync/Docs"));
        assert_eq!((folder.remote.as_str(), folder.ready), ("Docs", true));
        sync.save(&sync.folders()).unwrap();
        let expected: serde_json::Value = serde_json::from_str(&python).unwrap();
        assert_eq!(case.saved(), expected);
        assert_eq!(fs::metadata(&store).unwrap().mode() & 0o777, 0o600);
    }

    #[test]
    fn stopping_keeps_local_copies_and_backups_and_forgetting_clears_the_state() {
        let case = Case::new();
        let identifier = case.synced(&["notes.txt"]);
        let running = case.add("Projects");
        case.sync.tick();
        case.sync.remove(&running);
        assert_eq!(case.sync.folders().len(), 2);
        case.finish(0, "");
        let local = case.folder(&identifier).local;
        fs::create_dir_all(case.sync.backups(&identifier)).unwrap();
        case.sync.remove(&identifier);
        let left: Vec<String> = case
            .sync
            .folders()
            .into_iter()
            .map(|folder| folder.identifier)
            .collect();
        assert_eq!(left, [running]);
        assert_eq!(case.saved()["folders"].as_array().unwrap().len(), 1);
        assert!(local.join("notes.txt").exists() && case.sync.backups(&identifier).is_dir());
        assert!(!case.sync.work(&identifier).exists());
        case.sync.forget().unwrap();
        assert!(case.sync.folders().is_empty());
        assert!(local.join("notes.txt").exists());
        assert_eq!(fs::read_dir(case.root.path().join("cache/sync")).unwrap().count(), 0);
    }

    #[test]
    fn backup_folders_are_named_by_the_time_in_utc() {
        let at = |seconds| timestamp(UNIX_EPOCH + Duration::from_secs(seconds));
        assert_eq!(at(1_794_130_392), "2026-11-08 09.33.12 UTC");
        assert_eq!(at(1_709_251_199), "2024-02-29 23.59.59 UTC");
        assert_eq!(at(951_868_800), "2000-03-01 00.00.00 UTC");
        assert_eq!(at(0), "1970-01-01 00.00.00 UTC");
    }
}
