//! Synced folders against the real iCloud account, in a new test folder that is deleted at the end.
//!
//! From a desktop terminal on Linux, with the app closed, run:
//!
//!     cargo test --package icloud-core --test icloud_sync -- --account
//!
//! It asks for confirmation and, unless RCLONE_CONFIG_PASS is set, for the local key of the saved profile
//! (RCLONE_CONFIG, or else ~/.config/icloud-linux/rclone.conf). It mounts a private copy read-only in a temporary
//! workspace, writes about 110 MB into icloud-on-linux-test-<id> at the top of iCloud Drive, prints timings and the
//! names iCloud refused, and moves that folder to Recently Deleted at the end. `--small` uses 50 files and skips the
//! 100 MB file, `--yes` skips the confirmation, and `--fictional` runs the same steps against a local stand-in.
//! Without `--account` or `--fictional` it only prints SKIP, so `cargo test` never contacts the account.
mod support;

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, IsTerminal, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::rc::Rc;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use icloud_core::connection::Connection;
use icloud_core::paths::{Paths, home};
use icloud_core::process::{Event, Process, without_rclone_variables};
use icloud_core::profile;
use icloud_core::signal::Signal;
use icloud_core::sync::{State, SyncFolder, SyncedFolders};
use nix::sys::signal::Signal as UnixSignal;
use nix::sys::termios::{self, LocalFlags, SetArg};
use regex::Regex;
use sha2::{Digest, Sha256};

static RATE_LIMITS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b429\b|too many requests|rate limit|throttl").unwrap());
static OPAQUE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[A-Za-z0-9+/=_-]{32,}").unwrap());
static TEST_ROOT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^icloud-on-linux-test-[0-9a-f]{32}$").unwrap());
const FINAL: [State; 3] = [State::UpToDate, State::Error, State::NeedsReview];
const MEGABYTE: u64 = 1024 * 1024;

/// Names that iCloud Drive may refuse or change.
fn odd_names() -> Vec<String> {
    let mut names: Vec<String> = [
        "emoji \u{1f4dd}.txt",
        "acentos canci\u{f3}n \u{f1}and\u{fa}.txt",
        // The decomposed form: an e followed by a combining acute accent.
        "cafe\u{301} decomposed.txt",
        "ends with space ",
        "ends with dot.",
        ".leading dot.txt",
        "colon: name.txt",
        "question? mark.txt",
        "asterisk * name.txt",
        "pipe | name.txt",
        "quote \" name.txt",
        "angle < > name.txt",
        "backslash \\ name.txt",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    names.push(format!("long {}.txt", "x".repeat(245)));
    names.push(format!("largo {}.txt", "\u{f1}".repeat(120)));
    names
}

/// Why the check stopped, named like the exception of the Python check.
struct Stop {
    kind: &'static str,
    message: String,
}

impl Stop {
    fn new(kind: &'static str, message: impl Into<String>) -> Stop {
        Stop {
            kind,
            message: message.into(),
        }
    }
}

impl From<io::Error> for Stop {
    fn from(error: io::Error) -> Stop {
        Stop::new("OSError", error.to_string())
    }
}

fn ensure(condition: bool, message: impl FnOnce() -> String) -> Result<(), Stop> {
    if condition {
        Ok(())
    } else {
        Err(Stop::new("AssertionError", message()))
    }
}

/// Hide long opaque values and shorten the line: rclone errors carry no cookies, but may carry identifiers.
fn redact(line: &str) -> String {
    OPAQUE.replace_all(line, "[...]").chars().take(300).collect()
}

/// The profile copy and its key, unlocked while the private read-only mount is up, as the app's connection is.
struct Account {
    paths: Paths,
    key: (&'static str, String),
    profile: String,
    unlocked: Cell<bool>,
    changed: Signal,
}

impl Account {
    fn new(paths: Paths, key: (&'static str, String)) -> io::Result<Account> {
        let profile = profile::sha256(&fs::read(&paths.config)?);
        Ok(Account {
            paths,
            key,
            profile,
            unlocked: Cell::new(false),
            changed: Signal::default(),
        })
    }

    fn command(&self) -> Command {
        let mut command = Command::new("rclone");
        without_rclone_variables(&mut command)
            .env("RCLONE_CONFIG", &self.paths.config)
            .env(self.key.0, &self.key.1);
        command
    }

    /// Run rclone at the ERROR log level and pass its output to `sink`; fail on an error or after `seconds`.
    fn run<S: AsRef<OsStr>>(&self, arguments: &[S], seconds: u64, mut sink: impl FnMut(&[u8])) -> Result<(), Stop> {
        let name = arguments
            .first()
            .map(|argument| argument.as_ref().to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut command = self.command();
        command
            .args(["--ask-password=false", "--log-level", "ERROR"])
            .args(arguments);
        let process = Process::start(command, None, true)?;
        let deadline = Instant::now() + Duration::from_secs(seconds);
        loop {
            let events = process.events();
            let quiet = events.is_empty();
            for event in events {
                match event {
                    Event::Stdout(bytes) => sink(&bytes),
                    Event::Stderr(_) => {}
                    Event::Exited(0) => return Ok(()),
                    Event::Exited(code) => {
                        let message = format!("rclone {name} failed with exit code {code}");
                        return Err(Stop::new("RuntimeError", message));
                    }
                }
            }
            if Instant::now() > deadline {
                process.signal(UnixSignal::SIGKILL);
                let message = format!("rclone {name} did not finish within {seconds} seconds");
                return Err(Stop::new("TimeoutExpired", message));
            }
            if quiet {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    fn output<S: AsRef<OsStr>>(&self, arguments: &[S], seconds: u64) -> Result<Vec<u8>, Stop> {
        let mut output = Vec::new();
        self.run(arguments, seconds, |bytes| output.extend_from_slice(bytes))?;
        Ok(output)
    }
}

impl Connection for Account {
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
        self.command()
    }

    fn changed(&self) -> &Signal {
        &self.changed
    }
}

struct Check {
    account: Rc<Account>,
    sync: Rc<SyncedFolders>,
    root: String,
    marker: String,
    folder: RefCell<String>,
    runs: Rc<RefCell<Vec<(i32, String)>>>,
    results: RefCell<Vec<(String, String, Option<f64>)>>,
    created: Cell<bool>,
    client: RefCell<Option<Process>>,
    phase: Cell<&'static str>,
}

impl Check {
    fn target(&self, relative: &str) -> String {
        format!("icloud:{}/{relative}", self.root)
    }

    fn listing(&self, folder: &str) -> Result<BTreeMap<String, u64>, Stop> {
        let output = self
            .account
            .output(&["lsjson", "-R", "--files-only", self.target(folder).as_str()], 600)?;
        let entries: Vec<serde_json::Value> =
            serde_json::from_slice(&output).map_err(|error| Stop::new("JSONDecodeError", error.to_string()))?;
        Ok(entries
            .iter()
            .filter_map(|entry| Some((entry["Path"].as_str()?.to_owned(), entry["Size"].as_u64()?)))
            .collect())
    }

    fn folder(&self) -> SyncFolder {
        self.sync
            .folder(&self.folder.borrow())
            .expect("the test folder is synced")
    }

    fn note(&self, name: impl Into<String>, outcome: impl Into<String>, seconds: Option<f64>) {
        let (name, outcome) = (name.into(), outcome.into());
        let timing = seconds.map_or_else(String::new, |seconds| format!(" ({seconds:.0} s)"));
        println!("{name}: {outcome}{timing}");
        self.results.borrow_mut().push((name, outcome, seconds));
    }

    /// Tick sync as the app's timer does until `done`, printing progress every 30 seconds.
    fn wait(&self, seconds: u64, label: &str, mut done: impl FnMut() -> bool) -> Result<f64, Stop> {
        let started = Instant::now();
        let mut report = 30;
        loop {
            self.sync.tick();
            if done() {
                return Ok(started.elapsed().as_secs_f64());
            }
            let elapsed = started.elapsed().as_secs();
            if elapsed > seconds {
                let message = format!("{label} did not finish within {seconds} seconds");
                return Err(Stop::new("TimeoutError", message));
            }
            if elapsed >= report {
                let state = match self.sync.folders().first() {
                    Some(folder) => folder.state.label().to_lowercase(),
                    None => self.phase.get().to_owned(),
                };
                println!("  ... {label}: {state} after {elapsed} s");
                report += 30;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Whether a run ended after the first `before` and the folder rests on two ticks in a row: a run can leave
    /// another due on the next tick, as when new conflict copies need the winner recorded.
    fn settled(&self, before: usize) -> impl FnMut() -> bool + '_ {
        let mut streak = 0;
        move || {
            let rests = self.runs.borrow().len() > before && !self.sync.busy() && FINAL.contains(&self.folder().state);
            streak = if rests { streak + 1 } else { 0 };
            streak >= 2
        }
    }

    fn synced(&self, seconds: u64) -> Result<f64, Stop> {
        let before = self.runs.borrow().len();
        self.sync.sync_now(Some(&self.folder.borrow()));
        self.wait(seconds, "sync", self.settled(before))
    }

    /// Whether the mount client still runs, reading its exit if it has ended.
    fn client_running(&self) -> bool {
        let mut client = self.client.borrow_mut();
        let ended = client
            .as_ref()
            .is_some_and(|process| process.events().iter().any(|event| matches!(event, Event::Exited(_))));
        if ended {
            *client = None;
        }
        client.is_some()
    }

    /// Mount the profile read-only inside the private workspace, as the app connects, and never at ~/iCloud.
    fn connect(&self) -> Result<(), Stop> {
        let paths = &self.account.paths;
        for folder in [&paths.mount, &paths.cache.join("vfs")] {
            profile::private_directory(folder).map_err(|error| Stop::new("RuntimeError", error.to_string()))?;
        }
        let device = format!("icloud-on-linux-{}", random_hex()?);
        let mut command = self.account.command();
        command
            .args(["--ask-password=false", "mount", "icloud:"])
            .arg(&paths.mount)
            .args(["--read-only", "--devname", &device, "--cache-dir"])
            .arg(paths.cache.join("vfs"))
            .args([
                "--vfs-cache-mode",
                "full",
                "--vfs-cache-max-size",
                "1G",
                "--dir-cache-time",
                "10s",
            ]);
        *self.client.borrow_mut() = Some(Process::start(command, None, true)?);
        let mounted =
            || profile::read_mount(&paths.mount).is_some_and(|mount| mount.source == device && mount.compatible());
        self.wait(180, "connection", || mounted() || !self.client_running())?;
        if !mounted() {
            let message = "The connection did not open: the private read-only mount did not start";
            return Err(Stop::new("RuntimeError", message));
        }
        self.account.unlocked.set(true);
        self.phase.set("available");
        Ok(())
    }

    /// Unmount and let the mount client end, as disconnecting does.
    fn disconnect(&self) -> Result<(), Stop> {
        self.account.unlocked.set(false);
        self.phase.set("disconnecting");
        let mount = &self.account.paths.mount;
        if profile::read_mount(mount).is_some() {
            Command::new("fusermount3").arg("-u").arg(mount).status()?;
        }
        let ended = self.wait(60, "disconnect", || !self.client_running());
        if let Some(client) = self.client.borrow().as_ref() {
            client.signal(UnixSignal::SIGKILL);
        }
        ended.map(|_| ())
    }

    fn steps(&self, workspace: &Path, count: usize, big: u64) -> Result<(), Stop> {
        self.connect()?;
        let seed = workspace.join("seed");
        fs::create_dir_all(seed.join("data"))?;
        fs::write(seed.join("owner.json"), &self.marker)?;
        fs::write(seed.join("data/seed.txt"), "Seed file from the sync check\n")?;
        let root = format!("icloud:{}", self.root);
        self.account
            .output(&[OsStr::new("copy"), seed.as_os_str(), OsStr::new(&root)], 600)?;
        self.created.set(true);
        let runs = self.runs.clone();
        self.sync
            .observe_runs(move |_, code, log| runs.borrow_mut().push((code, log.to_owned())));
        if !self.sync.add(&format!("{}/data", self.root)) {
            return Err(Stop::new("RuntimeError", self.sync.message()));
        }
        *self.folder.borrow_mut() = self.sync.folders()[0].identifier.clone();
        let local = self.folder().local;
        let seconds = self.wait(600, "first sync", self.settled(0))?;
        let folder = self.folder();
        ensure(
            folder.state == State::UpToDate && local.join("seed.txt").exists(),
            || folder.message.clone(),
        )?;
        self.note("First sync downloads the folder", "passed", Some(seconds));

        for index in 0..count {
            let path = local.join(format!("bulk/part-{:02}/small-{index:03}.bin", index % 10));
            write_random(&path, small_size()?)?;
        }
        let seconds = self.synced(3600)?;
        let uploaded = self.listing("data/bulk")?;
        let expected = sizes(&local.join("bulk"))?;
        let state = self.folder().state;
        ensure(state == State::UpToDate && uploaded == expected, || {
            format!("({}, {}, {})", state.label(), uploaded.len(), expected.len())
        })?;
        let rate = format!("passed, {:.1} files/s", count as f64 / seconds);
        self.note(format!("{count} small files upload"), rate, Some(seconds));

        if big > 0 {
            let digest = write_random(&local.join("big.bin"), big)?;
            let seconds = self.synced(3600)?;
            let folder = self.folder();
            ensure(folder.state == State::UpToDate, || folder.message.clone())?;
            let started = Instant::now();
            let mut hasher = Sha256::new();
            self.account
                .run(&["cat", self.target("data/big.bin").as_str()], 3600, |bytes| {
                    hasher.update(bytes)
                })?;
            ensure(hex(&hasher.finalize()) == digest, || {
                "the downloaded file differs from the uploaded one".to_owned()
            })?;
            let rate = format!("passed, {:.1} MB/s up", big as f64 / seconds / 1e6);
            let total = seconds + started.elapsed().as_secs_f64();
            self.note("100 MB file uploads and downloads intact", rate, Some(total));
        }

        let deep = (1..=15).fold(local.clone(), |path, level| path.join(format!("level-{level:02}")));
        fs::create_dir_all(&deep)?;
        fs::write(deep.join("deep.txt"), "Fifteen folders deep\n")?;
        let relative = deep
            .join("deep.txt")
            .strip_prefix(&local)
            .map_or_else(|_| String::new(), |path| path.to_string_lossy().into_owned());
        let seconds = self.synced(900)?;
        let folder = self.folder();
        ensure(
            folder.state == State::UpToDate && self.listing("data")?.contains_key(&relative),
            || folder.message.clone(),
        )?;
        self.note("A file 15 folders deep syncs", "passed", Some(seconds));

        let odd = odd_names();
        let names = local.join("names");
        fs::create_dir(&names)?;
        for name in &odd {
            fs::write(names.join(name), format!("Name check: {name}\n"))?;
        }
        let before = self.runs.borrow().len();
        let seconds = self.synced(900)?;
        let arrived: BTreeSet<String> = self
            .listing("data")?
            .into_keys()
            .filter_map(|path| path.strip_prefix("names/").map(str::to_owned))
            .collect();
        let refused: Vec<&String> = odd.iter().filter(|name| !arrived.contains(*name)).collect();
        let outcome = format!(
            "{} of {} kept their exact name; folder state {}",
            odd.len() - refused.len(),
            odd.len(),
            self.folder().state.label()
        );
        self.note("Unusual names", outcome, Some(seconds));
        for name in &refused {
            println!("  not in iCloud under the same name: {name:?}");
        }
        for name in arrived.iter().filter(|name| !odd.contains(name)) {
            println!("  in iCloud under another name: {name:?}");
        }
        for line in self.runs.borrow()[before..]
            .iter()
            .flat_map(|(_, log)| log.lines())
            .filter(|line| line.contains("ERROR"))
            .take(10)
        {
            println!("  rclone: {}", redact(line));
        }
        // Two more runs show whether the folder settles with those names; then they are removed.
        let seconds = self.synced(900)? + self.synced(900)?;
        let here = fs::read_dir(&names)?
            .map(|entry| Ok(entry?.file_name().to_string_lossy().into_owned()))
            .collect::<io::Result<BTreeSet<String>>>()?;
        let state = self.folder().state;
        let outcome = if state == State::UpToDate && here == arrived {
            "passed".to_owned()
        } else {
            format!(
                "not settled: state {}, {} names differ between the two sides",
                state.label(),
                here.symmetric_difference(&arrived).count()
            )
        };
        self.note("The folder settles with those names", outcome, Some(seconds));
        for entry in fs::read_dir(&names)? {
            fs::remove_file(entry?.path())?;
        }
        fs::remove_dir(&names)?;
        let seconds = self.synced(900)?;
        let state = self.folder().state;
        if state != State::UpToDate {
            let message = format!(
                "the folder did not recover after the names were removed: {}",
                state.label()
            );
            return Err(Stop::new("RuntimeError", message));
        }
        self.note("The folder recovers after removing them", "passed", Some(seconds));

        let edit = workspace.join("remote-edit.txt");
        fs::write(&edit, "Changed on another device\n")?;
        let target = self.target("data/remote-edit.txt");
        self.account
            .output(&[OsStr::new("copyto"), edit.as_os_str(), OsStr::new(&target)], 600)?;
        let seconds = self.synced(900)?;
        let arrived = fs::read_to_string(local.join("remote-edit.txt")).unwrap_or_default();
        ensure(arrived == "Changed on another device\n", || self.folder().message)?;
        self.note("A change made elsewhere reaches this computer", "passed", Some(seconds));

        let mut removed: Vec<PathBuf> = fs::read_dir(local.join("bulk/part-07"))?
            .map(|entry| Ok(entry?.path()))
            .collect::<io::Result<_>>()?;
        removed.retain(|path| path.extension() == Some(OsStr::new("bin")));
        for path in &removed {
            fs::remove_file(path)?;
        }
        let seconds = self.synced(1800)?;
        let left = self
            .listing("data/bulk")?
            .keys()
            .filter(|path| path.starts_with("part-07/"))
            .count();
        let state = self.folder().state;
        ensure(state == State::UpToDate && left == 0, || {
            format!("({}, {left})", state.label())
        })?;
        let name = format!("{} local deletions reach iCloud without a review", removed.len());
        self.note(name, "passed", Some(seconds));

        let started = Instant::now();
        let total = self.listing("data")?.len();
        let seconds = started.elapsed().as_secs_f64();
        self.note(
            format!("Listing {total} files in the test folder"),
            "passed",
            Some(seconds),
        );
        let started = Instant::now();
        let top = self.account.output(&["lsf", "--max-depth", "1", "icloud:"], 600)?;
        let top = top.split(|byte| *byte == b'\n').filter(|line| !line.is_empty()).count();
        let name = format!("Listing the top of iCloud Drive ({top} items, names not shown)");
        self.note(name, "passed", Some(started.elapsed().as_secs_f64()));
        let limits: usize = self
            .runs
            .borrow()
            .iter()
            .map(|(_, log)| RATE_LIMITS.find_iter(log).count())
            .sum();
        self.note(
            "Rate limit messages from Apple during the sync runs",
            limits.to_string(),
            None,
        );
        Ok(())
    }

    /// Stop syncing, disconnect, and delete the test folder after checking its owner marker. Returns whether all
    /// of it worked.
    fn finish(&self) -> bool {
        let mut clean = true;
        self.sync.pause();
        let stopped = self.wait(90, "stopping sync", || !self.sync.busy()).map(|_| ());
        for result in [stopped, self.disconnect()] {
            if let Err(stop) = result {
                println!("{}: {}", stop.kind, redact(&stop.message));
                clean = false;
            }
        }
        if self.created.get() {
            match self.purge() {
                Ok(()) => println!(
                    "Cleanup: {} was deleted; in iCloud it goes to Recently Deleted.",
                    self.root
                ),
                Err(stop) => {
                    println!(
                        "Cleanup failed ({}). Delete the folder {} from iCloud Drive yourself.",
                        redact(&stop.message),
                        self.root
                    );
                    clean = false;
                }
            }
        }
        clean
    }

    /// Delete the test folder, only when its name and its owner marker show that this run created it.
    fn purge(&self) -> Result<(), Stop> {
        if !TEST_ROOT.is_match(&self.root) {
            return Err(Stop::new("RuntimeError", "refusing a path outside a fresh test folder"));
        }
        let owner = self.account.output(&["cat", self.target("owner.json").as_str()], 600)?;
        if owner != self.marker.as_bytes() {
            return Err(Stop::new("RuntimeError", "the owner marker changed"));
        }
        self.account
            .output(&["purge", format!("icloud:{}", self.root).as_str()], 3600)?;
        Ok(())
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn random_hex() -> io::Result<String> {
    let mut bytes = [0; 16];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(hex(&bytes))
}

/// A size from 1024 to 4096 bytes.
fn small_size() -> io::Result<u64> {
    let mut bytes = [0; 2];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(1024 + u64::from(u16::from_le_bytes(bytes)) % 3073)
}

/// Write `size` random bytes, which nothing can compress, and return their SHA-256.
fn write_random(path: &Path, size: u64) -> io::Result<String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut random = File::open("/dev/urandom")?;
    let mut file = File::create(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; MEGABYTE as usize];
    let mut left = size;
    while left > 0 {
        let chunk = &mut buffer[..left.min(MEGABYTE) as usize];
        random.read_exact(chunk)?;
        file.write_all(chunk)?;
        hasher.update(&*chunk);
        left -= chunk.len() as u64;
    }
    Ok(hex(&hasher.finalize()))
}

/// Every `.bin` file under `root`, by its path relative to `root`, with its size.
fn sizes(root: &Path) -> io::Result<BTreeMap<String, u64>> {
    let mut found = BTreeMap::new();
    let mut folders = vec![root.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for entry in fs::read_dir(&folder)? {
            let (path, info) = {
                let entry = entry?;
                (entry.path(), entry.metadata()?)
            };
            if info.is_dir() {
                folders.push(path);
            } else if let (Some("bin"), Ok(relative)) =
                (path.extension().and_then(OsStr::to_str), path.strip_prefix(root))
            {
                found.insert(relative.to_string_lossy().into_owned(), info.len());
            }
        }
    }
    Ok(found)
}

fn installed(name: &str) -> bool {
    env::split_paths(&env::var_os("PATH").unwrap_or_default()).any(|folder| folder.join(name).is_file())
}

fn require_linux() -> Result<(), String> {
    if nix::unistd::getuid().is_root() {
        return Err("Run this check as a Linux desktop user".to_owned());
    }
    if let Some(command) = ["rclone", "fusermount3"].into_iter().find(|name| !installed(name)) {
        return Err(format!("Required command is missing: {command}"));
    }
    nix::sys::stat::umask(nix::sys::stat::Mode::from_bits_truncate(0o077));
    Ok(())
}

fn confirmed(root: &str, count: usize, big: u64) -> io::Result<bool> {
    let size = format!("{:.0} MB", (big + count as u64 * 4096) as f64 / 1e6);
    print!(
        "This creates {root} in your iCloud Drive, writes about {size}, and moves it to Recently Deleted at the \
         end. Continue? [y/N] "
    );
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    let answer = answer.trim().to_lowercase();
    Ok(["y", "yes", "s", "si", "s\u{ed}"].contains(&answer.as_str()))
}

/// Read the local key from the terminal without showing it.
fn read_key() -> Result<String, String> {
    let stdin = io::stdin();
    if !stdin.is_terminal() {
        return Err("Set RCLONE_CONFIG_PASS or run this check in a terminal to type the local key".to_owned());
    }
    print!("Local configuration key: ");
    io::stdout().flush().map_err(|error| error.to_string())?;
    let shown = termios::tcgetattr(&stdin).map_err(|error| error.to_string())?;
    let mut hidden = shown.clone();
    hidden.local_flags.remove(LocalFlags::ECHO);
    termios::tcsetattr(&stdin, SetArg::TCSANOW, &hidden).map_err(|error| error.to_string())?;
    let mut key = String::new();
    let read = stdin.read_line(&mut key);
    let restored = termios::tcsetattr(&stdin, SetArg::TCSANOW, &shown);
    println!();
    read.map_err(|error| error.to_string())?;
    restored.map_err(|error| error.to_string())?;
    Ok(key.trim_end_matches(['\n', '\r']).to_owned())
}

/// The saved profile, checked and copied privately with its key, as the cloud environment of the Python checks.
fn account_workspace() -> Result<(tempfile::TempDir, Account), String> {
    let source = env::var_os("RCLONE_CONFIG")
        .filter(|value| !value.is_empty())
        .map_or_else(|| home().join(".config/icloud-linux/rclone.conf"), PathBuf::from);
    profile::check_profile(&source).map_err(|error| error.to_string())?;
    let saved = ["RCLONE_CONFIG_PASS", "_RCLONE_CONFIG_KEY"]
        .into_iter()
        .find_map(|name| Some((name, env::var(name).ok().filter(|value| !value.is_empty())?)));
    let key = match saved {
        Some(key) => key,
        None => ("RCLONE_CONFIG_PASS", read_key()?),
    };
    if key.1.is_empty() {
        return Err("The local configuration key cannot be empty".to_owned());
    }
    let workspace = tempfile::Builder::new()
        .prefix("icloud-real-sync-")
        .tempdir()
        .map_err(|error| error.to_string())?;
    let config = workspace.path().join("rclone.conf");
    let copied =
        fs::copy(&source, &config).and_then(|_| fs::set_permissions(&config, fs::Permissions::from_mode(0o600)));
    copied.map_err(|error| error.to_string())?;
    let folder = workspace.path();
    let paths = Paths {
        config,
        mount: folder.join("mount"),
        cache: folder.join("cache"),
        runtime: folder.join("runtime"),
        helper: folder.join("session.sh"),
    };
    let account = Account::new(paths, key).map_err(|error| error.to_string())?;
    Ok((workspace, account))
}

fn fictional_workspace() -> Result<(tempfile::TempDir, Account), String> {
    let support::Fixture { root, paths, .. } = support::Fixture::new();
    let account = Account::new(paths, ("RCLONE_CONFIG_PASS", support::KEY.to_owned()));
    Ok((root, account.map_err(|error| error.to_string())?))
}

/// Run every step; returns whether all of them and the cleanup worked.
fn check(account_mode: bool, small: bool, yes: bool) -> Result<bool, String> {
    require_linux()?;
    let (count, big) = if small { (50, 0) } else { (300, 100 * MEGABYTE) };
    let id = random_hex().map_err(|error| error.to_string())?;
    let root = format!("icloud-on-linux-test-{id}");
    let marker = format!(r#"{{"suite": "icloud-on-linux", "run": "{id}"}}"#);
    if account_mode && !yes && !confirmed(&root, count, big).map_err(|error| error.to_string())? {
        println!("Nothing was written.");
        return Ok(true);
    }
    let (workspace, account) = if account_mode {
        account_workspace()?
    } else {
        fictional_workspace()?
    };
    let account = Rc::new(account);
    let sync = SyncedFolders::new(account.clone()).map_err(|error| error.to_string())?;
    let check = Check {
        account: account.clone(),
        sync,
        root: root.clone(),
        marker,
        folder: RefCell::default(),
        runs: Rc::default(),
        results: RefCell::default(),
        created: Cell::new(false),
        client: RefCell::new(None),
        phase: Cell::new("connecting"),
    };
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| check.steps(workspace.path(), count, big)));
    let stopped = match outcome {
        Ok(Ok(())) => None,
        Ok(Err(stop)) => Some(format!("{}: {}", stop.kind, redact(&stop.message))),
        Err(panic) => {
            let message = panic
                .downcast_ref::<&str>()
                .map(|text| text.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_default();
            Some(format!("panic: {}", redact(&message)))
        }
    };
    if let Some(outcome) = &stopped {
        check.note("Stopped early", outcome.clone(), None);
    }
    let clean = check.finish();
    let results = check.results.take();
    drop(check);
    // A failed unmount must never turn cleanup into deleting through the mount.
    if profile::read_mount(&account.paths.mount).is_some() {
        let kept = workspace.keep();
        println!(
            "Private workspace retained because it is still mounted: {}",
            kept.display()
        );
    }
    println!("\nSummary");
    for (name, outcome, seconds) in &results {
        let timing = seconds.map_or_else(String::new, |seconds| format!(" in {seconds:.0} s"));
        println!("  {name}: {outcome}{timing}");
    }
    if account_mode {
        println!(
            "\nOn your iPhone, iPad, or Mac, check that Recently Deleted in Files shows {root}, and that the part-07 \
             files deleted from this computer appear there too."
        );
    }
    Ok(clean && stopped.is_none())
}

fn main() -> ExitCode {
    let arguments: Vec<String> = env::args().skip(1).collect();
    let flag = |name: &str| arguments.iter().any(|argument| argument == name);
    let (account, fictional) = (flag("--account"), flag("--fictional"));
    if !account && !fictional {
        println!("SKIP: the iCloud sync check runs only with --account (the real account) or --fictional.");
        return ExitCode::SUCCESS;
    }
    if account && fictional {
        eprintln!("Choose either --account or --fictional.");
        return ExitCode::from(2);
    }
    if !cfg!(target_os = "linux") {
        println!("SKIP: the iCloud sync check needs Linux.");
        return ExitCode::SUCCESS;
    }
    match check(account, flag("--small"), flag("--yes")) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}
