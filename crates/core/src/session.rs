//! The encrypted, read-only connection: unlock the saved profile with the local key, mount iCloud Drive with rclone,
//! and disconnect safely. The window calls `tick` every 200 ms.
use std::cell::RefCell;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::Path;
use std::process::Command;
use std::rc::Rc;
use std::time::{Duration, Instant};

use nix::sys::signal::Signal as UnixSignal;
use tempfile::TempDir;
use zeroize::Zeroizing;

use crate::connection::Connection;
use crate::paths::Paths;
use crate::process::{Event, Process, without_rclone_variables};
use crate::profile::{self, Mount};
use crate::signal::Signal;
use crate::{Failure, tr};

const CONNECT_SECONDS: u64 = 90;
const KILL_SECONDS: u64 = 5;
/// A connection that worked and then dropped is retried after these waits. Each attempt may sign in to Apple
/// again, so attempts are few; a connection that stays up for `STABLE_SECONDS` earns the full set again.
const RECONNECT_SECONDS: [u64; 3] = [30, 120, 300];
const STABLE_SECONDS: u64 = 600;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum State {
    #[default]
    Disconnected,
    Connecting,
    Available,
    Disconnecting,
    Error,
}

impl State {
    /// The English name, for reports.
    pub fn name(self) -> &'static str {
        match self {
            State::Disconnected => "Disconnected",
            State::Connecting => "Connecting",
            State::Available => "Available",
            State::Disconnecting => "Disconnecting",
            State::Error => "Error",
        }
    }
}

type MountReader = Box<dyn Fn(&Path) -> Option<Mount>>;

pub struct Session {
    paths: Paths,
    inner: RefCell<Inner>,
    changed: Signal,
    mounts: RefCell<MountReader>,
    search_path: RefCell<Option<OsString>>,
}

#[derive(Default)]
struct Inner {
    state: State,
    message: String,
    check: Option<Process>,
    client: Option<Process>,
    unmounter: Option<Process>,
    owns_mount: bool,
    mount_id: Option<u64>,
    device: String,
    key: Zeroizing<String>,
    profile_id: String,
    profile: Option<TempDir>,
    deadline: Option<Instant>,
    stop_error: Option<String>,
    kill_client_at: Option<Instant>,
    available_at: Option<Instant>,
    retry_key: Zeroizing<String>,
    retries: usize,
    reconnect_at: Option<Instant>,
    reconnect_delays: Vec<Duration>,
}

impl Session {
    pub fn new(paths: Paths) -> Rc<Session> {
        let session = Rc::new(Session {
            paths,
            inner: RefCell::new(Inner {
                message: tr!("Connect to open your files in the file manager."),
                reconnect_delays: RECONNECT_SECONDS.map(Duration::from_secs).to_vec(),
                ..Inner::default()
            }),
            changed: Signal::default(),
            mounts: RefCell::new(Box::new(profile::read_mount)),
            search_path: RefCell::new(None),
        });
        session.refresh();
        session
    }

    /// Look for rclone, fusermount3, and the helper's commands in `path` instead of the inherited `PATH`.
    pub fn set_search_path(&self, path: Option<OsString>) {
        *self.search_path.borrow_mut() = path;
    }

    /// Wait `delays` before the automatic reconnections, instead of 30 seconds, 2 minutes, and 5 minutes.
    pub fn set_reconnect_delays(&self, delays: &[Duration]) {
        self.inner.borrow_mut().reconnect_delays = delays.to_vec();
    }

    /// Read mounts with `reader` instead of /proc/self/mountinfo.
    pub fn set_mount_reader(&self, reader: impl Fn(&Path) -> Option<Mount> + 'static) {
        *self.mounts.borrow_mut() = Box::new(reader);
    }

    pub fn state(&self) -> State {
        self.inner.borrow().state
    }

    pub fn message(&self) -> String {
        self.inner.borrow().message.clone()
    }

    pub fn busy(&self) -> bool {
        matches!(self.state(), State::Connecting | State::Disconnecting)
    }

    pub fn owns_mount(&self) -> bool {
        self.inner.borrow().owns_mount
    }

    /// Whether a key check, a mount client, or an unmount is still running.
    pub fn running(&self) -> bool {
        let inner = self.inner.borrow();
        inner.check.is_some() || inner.client.is_some() || inner.unmounter.is_some()
    }

    pub fn client_running(&self) -> bool {
        self.inner.borrow().client.is_some()
    }

    pub fn checking(&self) -> bool {
        self.inner.borrow().check.is_some()
    }

    /// Whether a dropped connection will reconnect on its own.
    pub fn reconnecting(&self) -> bool {
        self.inner.borrow().reconnect_at.is_some()
    }

    fn cancel_reconnect(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.retry_key = Zeroizing::new(String::new());
        inner.retries = 0;
        inner.reconnect_at = None;
    }

    pub fn current_mount(&self) -> Option<Mount> {
        (self.mounts.borrow())(&self.paths.mount)
    }

    /// Show a state and message, as the window does after signing in.
    pub fn set_status(&self, state: State, message: String) {
        let changed = {
            let mut inner = self.inner.borrow_mut();
            let changed = (inner.state, &inner.message) != (state, &message);
            inner.state = state;
            inner.message = message;
            changed
        };
        if changed {
            self.changed.emit();
        }
    }

    fn command(&self, program: &str, unlock: bool) -> Command {
        let mut command = Command::new(program);
        without_rclone_variables(&mut command);
        if let Some(path) = self.search_path.borrow().as_ref() {
            command.env("PATH", path);
        }
        let inner = self.inner.borrow();
        if let Some(profile) = &inner.profile {
            command.env("RCLONE_CONFIG", profile.path().join("rclone.conf"));
        }
        if unlock && !inner.key.is_empty() {
            command.env("RCLONE_CONFIG_PASS", inner.key.as_str());
        }
        command
    }

    /// The `PATH` that child processes search, when it was overridden.
    pub fn search_path(&self) -> Option<OsString> {
        self.search_path.borrow().clone()
    }

    pub(crate) fn installed(&self, name: &str) -> bool {
        let path = self
            .search_path
            .borrow()
            .clone()
            .or_else(|| env::var_os("PATH"))
            .unwrap_or_default();
        env::split_paths(&path).any(|directory| {
            fs::metadata(directory.join(name))
                .is_ok_and(|info| info.is_file() && info.permissions().mode() & 0o111 != 0)
        })
    }

    pub fn start(&self, key: &str) {
        self.cancel_reconnect();
        self.begin(key);
    }

    fn begin(&self, key: &str) {
        if self.busy() || self.client_running() || self.current_mount().is_some() {
            return;
        }
        if key.is_empty() || key.contains(['\n', '\r']) {
            self.set_status(State::Error, tr!("Enter the local key for your saved session."));
            return;
        }
        if let Err(error) = self.prepare() {
            self.clear_profile();
            let message = match error {
                Failure::Invalid(message) => message,
                Failure::Io(_) => tr!("The saved session or application folders are unavailable."),
            };
            self.set_status(State::Error, message);
            return;
        }
        let Ok(device) = profile::random_hex() else {
            self.clear_profile();
            self.set_status(
                State::Error,
                tr!("The saved session or application folders are unavailable."),
            );
            return;
        };
        {
            let mut inner = self.inner.borrow_mut();
            inner.key = Zeroizing::new(key.to_owned());
            inner.available_at = None;
            inner.device = format!("icloud-on-linux-{device}");
            inner.deadline = Some(Instant::now() + Duration::from_secs(CONNECT_SECONDS));
        }
        self.set_status(State::Connecting, tr!("Unlocking your saved session..."));
        let mut command = self.command("bash", false);
        command.arg(&self.paths.helper).arg("check");
        let input = format!("{key}\n").into_bytes();
        match Process::start(command, Some(input), true) {
            Ok(process) => self.inner.borrow_mut().check = Some(process),
            Err(_) => self.checked(24),
        }
    }

    /// Validate the folders and copy the profile privately, so rclone never rewrites the saved one.
    fn prepare(&self) -> crate::Result<()> {
        if !self.installed("rclone") || !self.installed("fusermount3") || !self.paths.helper.is_file() {
            return Err(Failure::Invalid(tr!(
                "Install rclone, FUSE 3, and the desktop app before connecting."
            )));
        }
        profile::check_profile(&self.paths.config)?;
        profile::private_directory(&self.paths.runtime)?;
        profile::private_directory(&self.paths.cache.join("vfs"))?;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.paths.mount)?;
        let link = fs::symlink_metadata(&self.paths.mount)?.file_type().is_symlink();
        if link || fs::read_dir(&self.paths.mount)?.next().is_some() {
            return Err(Failure::Invalid(tr!(
                "The iCloud folder must be empty before connecting."
            )));
        }
        let profile = tempfile::Builder::new()
            .prefix("session-")
            .tempdir_in(&self.paths.runtime)?;
        let copy = profile.path().join("rclone.conf");
        fs::copy(&self.paths.config, &copy)?;
        fs::set_permissions(&copy, fs::Permissions::from_mode(0o600))?;
        let digest = profile::sha256(&fs::read(&copy)?);
        let identity = profile::profile_identity(&self.paths.config, Some(digest))?;
        let mut inner = self.inner.borrow_mut();
        inner.profile = Some(profile);
        inner.profile_id = identity;
        Ok(())
    }

    fn checked(&self, code: i32) {
        self.inner.borrow_mut().check = None;
        if self.state() == State::Disconnecting {
            self.continue_stop();
            return;
        }
        if code != 0 {
            let message = match code {
                20 => tr!("No saved session was found. Select Sign in to set up your account."),
                21 => tr!("The saved session must be encrypted before connecting."),
                22 => tr!("Could not unlock the saved session. Check your local key."),
                23 => tr!("The saved session has no iCloud Drive connection. Select Sign in to add it."),
                _ => tr!("The session check could not be started."),
            };
            self.clear_profile();
            self.set_status(State::Error, message);
            return;
        }
        if self.current_mount().is_some() {
            self.clear_profile();
            self.set_status(
                State::Error,
                tr!("Another connection is using the iCloud folder. It was left unchanged."),
            );
            return;
        }
        self.set_status(State::Connecting, tr!("Connecting to iCloud Drive..."));
        let mut command = self.command("rclone", true);
        let device = self.inner.borrow().device.clone();
        command
            .args(["--ask-password=false", "mount", "icloud:"])
            .arg(&self.paths.mount)
            .args(["--read-only", "--devname", &device, "--cache-dir"])
            .arg(self.paths.cache.join("vfs"))
            .args([
                "--vfs-cache-mode",
                "full",
                "--vfs-cache-max-size",
                "1G",
                "--dir-cache-time",
                "10s",
            ]);
        // The client is not tied to the app: after a crash the mount stays usable and the next start can close it.
        match Process::start(command, None, false) {
            Ok(process) => self.inner.borrow_mut().client = Some(process),
            Err(_) => self.client_finished(1),
        }
    }

    fn client_finished(&self, _code: i32) {
        {
            let mut inner = self.inner.borrow_mut();
            inner.client = None;
            inner.kill_client_at = None;
        }
        if self.state() != State::Disconnecting {
            self.inner.borrow_mut().stop_error = Some(tr!(
                "The connection stopped. Check your network or renew the saved session and reconnect."
            ));
            self.set_status(State::Disconnecting, tr!("Closing the connection..."));
        }
        self.continue_stop();
    }

    /// Collect process results and compare the mount with the expected state.
    pub fn tick(&self) {
        let due = self.inner.borrow().reconnect_at.is_some_and(|at| Instant::now() >= at);
        if due {
            let key = {
                let mut inner = self.inner.borrow_mut();
                inner.reconnect_at = None;
                inner.retry_key.clone()
            };
            self.begin(&key);
            if self.state() != State::Connecting {
                self.cancel_reconnect();
            }
        }
        let finished = |process: &Option<Process>| {
            process.as_ref().and_then(|process| {
                process.events().into_iter().find_map(|event| match event {
                    Event::Exited(code) => Some(code),
                    _ => None,
                })
            })
        };
        let (check, client, unmounter) = {
            let inner = self.inner.borrow();
            (
                finished(&inner.check),
                finished(&inner.client),
                finished(&inner.unmounter),
            )
        };
        if let Some(code) = check {
            self.checked(code);
        }
        if let Some(code) = client {
            self.client_finished(code);
        }
        if let Some(code) = unmounter {
            self.unmounted(code);
        }
        let kill = {
            let mut inner = self.inner.borrow_mut();
            let due = inner.kill_client_at.is_some_and(|at| Instant::now() >= at);
            if due {
                inner.kill_client_at = None;
            }
            due
        };
        if kill {
            if let Some(client) = &self.inner.borrow().client {
                client.signal(UnixSignal::SIGKILL);
            }
        }
        self.refresh();
    }

    fn refresh(&self) {
        let current = self.current_mount();
        let state = self.state();
        if state == State::Connecting {
            let (expected, timed_out) = {
                let inner = self.inner.borrow();
                let expected = inner.client.is_some()
                    && current
                        .as_ref()
                        .is_some_and(|mount| mount.source == inner.device && mount.compatible());
                (
                    expected,
                    inner.deadline.is_some_and(|deadline| Instant::now() >= deadline),
                )
            };
            if expected {
                {
                    let mut inner = self.inner.borrow_mut();
                    inner.mount_id = current.map(|mount| mount.identifier);
                    inner.owns_mount = true;
                    inner.deadline = None;
                    inner.available_at = Some(Instant::now());
                }
                self.set_status(
                    State::Available,
                    tr!("iCloud Drive is ready to open in your file manager."),
                );
            } else if timed_out {
                self.inner.borrow_mut().stop_error =
                    Some(tr!("The connection timed out. Check your network and try again."));
                self.stop_preserving_error();
            }
        } else if state == State::Disconnecting {
        } else if let Some(current) = current {
            let (mount_id, client) = {
                let inner = self.inner.borrow();
                (inner.mount_id, inner.client.is_some())
            };
            if !current.compatible() {
                self.set_status(
                    State::Error,
                    tr!("Another filesystem is using the iCloud folder. It was left unchanged."),
                );
            } else if mount_id.is_some_and(|identifier| identifier != current.identifier) {
                self.set_status(
                    State::Error,
                    tr!("The connection changed outside this app. It was left unchanged."),
                );
            } else if !client && mount_id.is_none() {
                self.inner.borrow_mut().mount_id = Some(current.identifier);
                if current.source == "icloud:" {
                    self.set_status(
                        State::Available,
                        tr!("An existing read-only connection is available. Reconnect here to enable uploads."),
                    );
                } else {
                    self.set_status(
                        State::Error,
                        tr!("A previous connection is still mounted. Disconnect it before reconnecting."),
                    );
                }
            }
        } else if state == State::Available {
            if self.client_running() {
                self.inner.borrow_mut().stop_error =
                    Some(tr!("The mounted connection disappeared. Reconnect to try again."));
                self.stop_preserving_error();
            } else {
                self.inner.borrow_mut().mount_id = None;
                self.clear_profile();
                self.set_status(State::Disconnected, tr!("Disconnected from iCloud Drive."));
            }
        }
    }

    /// Disconnect: stop a key check, unmount the app's own mount, and end its client.
    pub fn stop(&self) {
        if self.state() != State::Disconnecting {
            self.inner.borrow_mut().stop_error = None;
            self.cancel_reconnect();
        }
        self.stop_preserving_error();
    }

    fn stop_preserving_error(&self) {
        if self.state() == State::Disconnecting {
            return;
        }
        if let Some(current) = self.current_mount() {
            let foreign = {
                let inner = self.inner.borrow();
                !current.compatible() || (Some(current.identifier) != inner.mount_id && current.source != inner.device)
            };
            if foreign {
                self.set_status(
                    State::Error,
                    tr!("The folder is used by another connection. It was left unchanged."),
                );
                return;
            }
        }
        self.set_status(State::Disconnecting, tr!("Disconnecting from iCloud Drive..."));
        if let Some(check) = &self.inner.borrow().check {
            check.signal(UnixSignal::SIGKILL);
        }
        self.continue_stop();
    }

    fn continue_stop(&self) {
        {
            let inner = self.inner.borrow();
            if inner.check.is_some() || inner.unmounter.is_some() {
                return;
            }
        }
        if let Some(current) = self.current_mount() {
            let foreign = {
                let inner = self.inner.borrow();
                !current.compatible() || (Some(current.identifier) != inner.mount_id && current.source != inner.device)
            };
            if foreign {
                self.set_status(State::Error, tr!("The connection changed. It was left unchanged."));
                return;
            }
            let mut command = self.command("fusermount3", false);
            command.arg("-u").arg(&self.paths.mount);
            match Process::start(command, None, true) {
                Ok(process) => self.inner.borrow_mut().unmounter = Some(process),
                Err(_) => self.unmounted(1),
            }
            return;
        }
        {
            let mut inner = self.inner.borrow_mut();
            if let Some(client) = &inner.client {
                client.signal(UnixSignal::SIGTERM);
                inner.kill_client_at = Some(Instant::now() + Duration::from_secs(KILL_SECONDS));
                return;
            }
            inner.mount_id = None;
            inner.owns_mount = false;
            inner.deadline = None;
        }
        // Only a connection that worked, or a retry of one, reconnects on its own; a failed first attempt does not.
        let (key, delay) = {
            let mut inner = self.inner.borrow_mut();
            let key = std::mem::take(&mut inner.key);
            let reconnect = inner.stop_error.is_some()
                && !key.is_empty()
                && (inner.available_at.is_some() || !inner.retry_key.is_empty());
            let stable = inner
                .available_at
                .is_some_and(|at| at.elapsed() >= Duration::from_secs(STABLE_SECONDS));
            if reconnect && stable {
                inner.retries = 0;
            }
            let delay = reconnect
                .then(|| inner.reconnect_delays.get(inner.retries).copied())
                .flatten();
            (key, delay)
        };
        self.clear_profile();
        if let Some(delay) = delay {
            {
                let mut inner = self.inner.borrow_mut();
                inner.retries += 1;
                inner.retry_key = key;
                inner.reconnect_at = Some(Instant::now() + delay);
                inner.stop_error = None;
            }
            self.set_status(
                State::Error,
                tr!(
                    "The connection stopped. Reconnecting automatically in {seconds} seconds.",
                    seconds = delay.as_secs()
                ),
            );
            return;
        }
        let error = self.inner.borrow_mut().stop_error.take();
        match error {
            Some(error) => {
                self.cancel_reconnect();
                self.set_status(State::Error, error);
            }
            None => self.set_status(State::Disconnected, tr!("Disconnected from iCloud Drive.")),
        }
    }

    fn unmounted(&self, code: i32) {
        self.inner.borrow_mut().unmounter = None;
        if code != 0 {
            self.set_status(
                State::Error,
                tr!("Could not disconnect. Close files in iCloud Drive and try again."),
            );
            return;
        }
        self.continue_stop();
    }

    /// Forget the key and remove the private profile copy. A scheduled reconnection keeps its key again
    /// afterwards; every other stop forgets it.
    fn clear_profile(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.key = Zeroizing::new(String::new());
        inner.retry_key = Zeroizing::new(String::new());
        inner.profile_id.clear();
        inner.profile = None;
    }
}

impl Connection for Session {
    fn paths(&self) -> &Paths {
        &self.paths
    }

    fn unlocked(&self) -> bool {
        let inner = self.inner.borrow();
        !inner.key.is_empty() && inner.profile.is_some() && inner.owns_mount && inner.state == State::Available
    }

    fn profile_id(&self) -> String {
        self.inner.borrow().profile_id.clone()
    }

    fn rclone(&self) -> Command {
        self.command("rclone", true)
    }

    fn changed(&self) -> &Signal {
        &self.changed
    }
}
