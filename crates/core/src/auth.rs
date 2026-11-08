//! Sign in to iCloud and renew the saved session without a terminal. Each step runs `scripts/session.sh` on a
//! private copy of the profile; the saved profile is replaced only after the new session can read iCloud Drive.
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::time::{Duration, Instant};

use nix::sys::signal::Signal as UnixSignal;
use serde_json::{Value, json};
use tempfile::TempDir;
use zeroize::Zeroizing;

use crate::connection::Connection;
use crate::process::{Event, Process, without_rclone_variables};
use crate::profile;
use crate::session::Session;
use crate::signal::Signal;
use crate::{Failure, tr};

const STEP_SECONDS: u64 = 120;
const OUTPUT_LIMIT: usize = 1024 * 1024;
const MAX_STEPS: u32 = 16;
const STALE_COPY: Duration = Duration::from_secs(3600);

pub struct Authorization {
    session: Rc<Session>,
    inner: RefCell<Inner>,
    changed: Signal,
    saved: Signal,
}

#[derive(Default)]
struct Inner {
    process: Option<Process>,
    action: String,
    active: bool,
    question: String,
    choices: Vec<(String, String)>,
    message: String,
    temporary: Option<TempDir>,
    key: Zeroizing<String>,
    account: String,
    password: Zeroizing<String>,
    state: String,
    original: Option<Vec<u8>>,
    identity: Option<String>,
    // Setup steps print rclone's continuation state, so their output is cleared after reading.
    output: Zeroizing<Vec<u8>>,
    steps: u32,
    cancelled: bool,
    published: bool,
    deadline: Option<Instant>,
}

/// Remove private profile copies that an interrupted sign-in left behind. A running attempt is never an hour old,
/// and nothing is followed through a link.
fn remove_stale_copies(folder: &Path) {
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let old = metadata
            .modified()
            .ok()
            .and_then(|time| time.elapsed().ok())
            .is_some_and(|age| age > STALE_COPY);
        if !entry.file_name().to_string_lossy().starts_with(".signin-") || !old {
            continue;
        }
        let _ = if metadata.is_dir() {
            fs::remove_dir_all(entry.path())
        } else {
            fs::remove_file(entry.path())
        };
    }
}

/// The `[name]` section of an rclone configuration, as `rclone config show` prints it.
fn section(text: &str, name: &str) -> BTreeMap<String, String> {
    let mut current = String::new();
    let mut values = BTreeMap::new();
    for line in text.lines().map(str::trim) {
        if let Some(title) = line.strip_prefix('[').and_then(|line| line.strip_suffix(']')) {
            current = title.trim().to_owned();
        } else if current == name && !line.starts_with(['#', ';']) {
            if let Some((key, value)) = line.split_once(['=', ':']) {
                values.insert(key.trim().to_lowercase(), value.trim().to_owned());
            }
        }
    }
    values
}

impl Authorization {
    pub fn new(session: Rc<Session>) -> Rc<Authorization> {
        Rc::new(Authorization {
            session,
            inner: RefCell::default(),
            changed: Signal::default(),
            saved: Signal::default(),
        })
    }

    pub fn changed(&self) -> &Signal {
        &self.changed
    }

    /// Emitted once the new profile is published.
    pub fn saved(&self) -> &Signal {
        &self.saved
    }

    pub fn active(&self) -> bool {
        self.inner.borrow().active
    }

    pub fn running(&self) -> bool {
        self.inner.borrow().process.is_some()
    }

    /// The setup question waiting for the user: `config_2fa`, `config_2fa_sms`, or `config_2fa_phone`.
    pub fn question(&self) -> String {
        self.inner.borrow().question.clone()
    }

    /// Trusted phones to choose from, as (value, label).
    pub fn choices(&self) -> Vec<(String, String)> {
        self.inner.borrow().choices.clone()
    }

    pub fn message(&self) -> String {
        self.inner.borrow().message.clone()
    }

    fn say(&self, message: String) {
        self.inner.borrow_mut().message = message;
        self.changed.emit();
    }

    pub fn start(&self, key: &str, confirmation: &str, account: &str, password: &str) {
        let paths = self.session.paths().clone();
        if self.active()
            || self.session.busy()
            || self.session.client_running()
            || self.session.current_mount().is_some()
        {
            return;
        }
        let existing = fs::symlink_metadata(&paths.config).is_ok();
        let unsafe_text = |text: &str| text.contains(['\0', '\r', '\n']);
        if key.is_empty()
            || account.trim().is_empty()
            || password.is_empty()
            || [key, account, password].into_iter().any(unsafe_text)
        {
            self.say(tr!("Enter your local key, Apple account, and Apple password."));
            return;
        }
        if !existing && key != confirmation {
            self.say(tr!("The new local keys do not match."));
            return;
        }
        let prepared = (|| -> crate::Result<(Option<Vec<u8>>, Option<String>, TempDir)> {
            if !self.session.installed("rclone") || !paths.helper.is_file() {
                return Err(Failure::Invalid(tr!(
                    "Install rclone and the desktop app before signing in."
                )));
            }
            let folder = paths.config.parent().unwrap_or(&paths.config);
            profile::private_directory(folder)?;
            if existing {
                profile::check_profile(&paths.config)?;
            }
            let original = if existing { Some(fs::read(&paths.config)?) } else { None };
            let identity = if existing {
                Some(profile::profile_identity(&paths.config, None)?)
            } else {
                None
            };
            remove_stale_copies(folder);
            let temporary = tempfile::Builder::new().prefix(".signin-").tempdir_in(folder)?;
            if existing {
                let copy = temporary.path().join("rclone.conf");
                fs::copy(&paths.config, &copy)?;
                fs::set_permissions(&copy, fs::Permissions::from_mode(0o600))?;
            }
            Ok((original, identity, temporary))
        })();
        match prepared {
            Ok((original, identity, temporary)) => {
                {
                    let mut inner = self.inner.borrow_mut();
                    *inner = Inner {
                        active: true,
                        temporary: Some(temporary),
                        key: Zeroizing::new(key.to_owned()),
                        account: account.trim().to_owned(),
                        password: Zeroizing::new(password.to_owned()),
                        original,
                        identity,
                        ..Inner::default()
                    };
                }
                self.run(if existing { "auth-inspect" } else { "auth-init" }, "");
            }
            Err(Failure::Invalid(message)) => self.finish(message),
            Err(Failure::Io(_)) => self.finish(tr!("The saved profile or its private folder could not be opened.")),
        }
    }

    fn copy(&self) -> PathBuf {
        let inner = self.inner.borrow();
        inner
            .temporary
            .as_ref()
            .map(|folder| folder.path().join("rclone.conf"))
            .unwrap_or_default()
    }

    fn run(&self, action: &str, answer: &str) {
        if !self.active() || self.running() {
            return;
        }
        let mut command = Command::new("bash");
        without_rclone_variables(&mut command);
        if let Some(path) = self.session.search_path() {
            command.env("PATH", path);
        }
        {
            let inner = self.inner.borrow();
            // Secrets travel only in the child's environment, never in its arguments.
            command
                .arg(&self.session.paths().helper)
                .arg(action)
                .env("RCLONE_CONFIG", self.copy())
                .env("RCLONE_CONFIG_PASS", inner.key.as_str())
                .env("RCLONE_STATE", &inner.state)
                .env("RCLONE_RESULT", answer);
        }
        {
            let mut inner = self.inner.borrow_mut();
            inner.question.clear();
            inner.choices.clear();
            inner.output.clear();
            inner.action = action.to_owned();
            inner.message = if action == "auth-probe" {
                tr!("Checking access...")
            } else {
                tr!("Signing in to iCloud...")
            };
            inner.deadline = Some(Instant::now() + Duration::from_secs(STEP_SECONDS));
        }
        match Process::start(command, None, true) {
            Ok(process) => {
                self.inner.borrow_mut().process = Some(process);
                self.changed.emit();
            }
            Err(_) => self.done(1),
        }
    }

    /// Collect the running step's output and result, and cancel a step that takes too long.
    pub fn tick(&self) {
        let events = self
            .inner
            .borrow()
            .process
            .as_ref()
            .map(Process::events)
            .unwrap_or_default();
        let mut exited = None;
        for event in events {
            match event {
                Event::Stdout(bytes) => {
                    let mut inner = self.inner.borrow_mut();
                    inner.output.extend(bytes);
                    if inner.output.len() > OUTPUT_LIMIT {
                        inner.cancelled = true;
                        if let Some(process) = &inner.process {
                            process.signal(UnixSignal::SIGKILL);
                        }
                    }
                }
                Event::Stderr(_) => {}
                Event::Exited(code) => exited = Some(code),
            }
        }
        if let Some(code) = exited {
            self.done(code);
        } else if self
            .inner
            .borrow()
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.cancel();
        }
    }

    fn done(&self, code: i32) {
        let (cancelled, action) = {
            let mut inner = self.inner.borrow_mut();
            inner.process = None;
            inner.deadline = None;
            (inner.cancelled, inner.action.clone())
        };
        if cancelled {
            self.finish(tr!("Sign-in cancelled. Your saved session was preserved."));
        } else if code != 0 {
            self.finish(tr!(
                "Sign-in failed. Check your local key, Apple credentials, verification code, and connection, then try again."
            ));
        } else if let Err(()) = self.completed(&action) {
            let published = self.inner.borrow().published;
            self.finish(if published {
                tr!("The saved profile may have been updated. Reconnect to check it; pending copies are retained.")
            } else {
                tr!("The sign-in response or saved profile could not be verified. Your saved session was preserved.")
            });
        }
    }

    fn completed(&self, action: &str) -> Result<(), ()> {
        let output = std::mem::take(&mut self.inner.borrow_mut().output);
        let output = output.as_slice();
        match action {
            "auth-init" => {
                profile::check_profile(&self.copy()).map_err(|_| ())?;
                self.run("auth-create", "");
            }
            "auth-inspect" => {
                let remote = section(std::str::from_utf8(output).map_err(|_| ())?, "icloud");
                // A profile without the remote, such as an interrupted terminal setup, can receive one.
                if let Some(kind) = remote.get("type") {
                    let service = remote.get("service").map(String::as_str).unwrap_or("drive");
                    if kind != "iclouddrive" || service != "drive" {
                        return Err(());
                    }
                    let account = remote
                        .get("apple_id")
                        .map(|value| value.trim().to_lowercase())
                        .unwrap_or_default();
                    if account != self.inner.borrow().account.to_lowercase() {
                        self.finish(tr!(
                            "Use the same Apple account to renew this profile. Pending uploads were preserved."
                        ));
                        return Ok(());
                    }
                }
                self.run("auth-create", "");
            }
            "auth-probe" => self.publish().map_err(|_| ())?,
            _ => self.answered(output)?,
        }
        Ok(())
    }

    /// Read one setup step: answer what the app knows and ask the user for verification codes and phones.
    fn answered(&self, output: &[u8]) -> Result<(), ()> {
        let data: Value = serde_json::from_slice(output).map_err(|_| ())?;
        let steps = {
            let mut inner = self.inner.borrow_mut();
            inner.steps += 1;
            inner.steps
        };
        let state = data.get("State").and_then(Value::as_str).ok_or(())?;
        if steps > MAX_STEPS {
            return Err(());
        }
        self.inner.borrow_mut().state = state.to_owned();
        if state.is_empty() {
            self.run("auth-probe", "");
            return Ok(());
        }
        let option = data.get("Option").ok_or(())?;
        let name = option.get("Name").and_then(Value::as_str).ok_or(())?;
        match name {
            "service" => self.run("auth-answer", "drive"),
            "config_fs_advanced" => self.run("auth-answer", "false"),
            "apple_id" => {
                let account = self.inner.borrow().account.clone();
                self.run("auth-answer", &account);
            }
            "password" => {
                let password = std::mem::take(&mut self.inner.borrow_mut().password);
                self.run("auth-password", &password);
            }
            "config_2fa" | "config_2fa_sms" | "config_2fa_phone" => {
                let choices = if name == "config_2fa_phone" {
                    let examples = option.get("Examples").and_then(Value::as_array).ok_or(())?;
                    examples
                        .iter()
                        .map(|example| {
                            let value = match example.get("Value") {
                                Some(Value::String(text)) => text.clone(),
                                Some(other) => other.to_string(),
                                None => return Err(()),
                            };
                            let label = example.get("Help").and_then(Value::as_str).unwrap_or_default();
                            Ok((value, label.to_owned()))
                        })
                        .collect::<Result<Vec<_>, ()>>()?
                } else {
                    Vec::new()
                };
                let rejected = data
                    .get("Error")
                    .and_then(Value::as_str)
                    .is_some_and(|error| !error.is_empty());
                let message = if rejected {
                    tr!("The verification response was not accepted. Try again.")
                } else if choices.is_empty() {
                    tr!("Enter the verification code from Apple.")
                } else {
                    tr!("Choose a trusted phone number.")
                };
                {
                    let mut inner = self.inner.borrow_mut();
                    inner.question = name.to_owned();
                    inner.choices = choices;
                    inner.message = message;
                }
                self.changed.emit();
            }
            _ => return Err(()),
        }
        Ok(())
    }

    /// Answer the waiting question: a 6-digit code, `sms` to receive one by text message, or a listed phone.
    pub fn answer(&self, value: &str) {
        let (question, choices) = {
            let inner = self.inner.borrow();
            (inner.question.clone(), inner.choices.clone())
        };
        if !self.active() || self.running() || question.is_empty() {
            return;
        }
        let valid = if choices.is_empty() {
            (value.len() == 6 && value.bytes().all(|byte| byte.is_ascii_digit()))
                || (question == "config_2fa" && value == "sms")
        } else {
            choices.iter().any(|(choice, _)| choice == value)
        };
        if valid {
            self.run("auth-answer", value);
        } else if choices.is_empty() {
            self.say(tr!("Enter the 6-digit verification code."));
        } else {
            self.say(tr!("Choose a listed trusted phone number."));
        }
    }

    /// Replace or create the saved profile, keeping the identity of pending uploads across renewals.
    fn publish(&self) -> crate::Result<()> {
        let destination = self.session.paths().config.clone();
        let copy = self.copy();
        profile::check_profile(&copy)?;
        let (original, identity) = {
            let inner = self.inner.borrow();
            (inner.original.clone(), inner.identity.clone())
        };
        let unchanged = |path: &PathBuf| -> crate::Result<bool> {
            let current = if fs::symlink_metadata(path).is_ok() {
                Some(fs::read(path)?)
            } else {
                None
            };
            Ok(current == original)
        };
        let link = fs::symlink_metadata(&destination).is_ok_and(|info| info.file_type().is_symlink());
        if !unchanged(&destination)? || link || self.session.current_mount().is_some() || self.session.client_running()
        {
            return Err(Failure::Invalid(String::new()));
        }
        let digest = profile::sha256(&fs::read(&copy)?);
        let mut identities = serde_json::Map::new();
        identities.insert(digest.clone(), Value::from(identity.clone().unwrap_or(digest)));
        if let Some(original) = &original {
            identities.insert(profile::sha256(original), Value::from(identity.unwrap_or_default()));
        }
        // Identity links come first, so either profile stays usable after a crash.
        profile::write_json(
            &destination.with_file_name("profile-links.json"),
            &json!({"version": 1, "profiles": identities}),
        )?;
        fs::File::open(&copy)?.sync_all()?;
        if !unchanged(&destination)? {
            return Err(Failure::Invalid(String::new()));
        }
        if original.is_none() {
            fs::hard_link(&copy, &destination)?;
        } else {
            fs::rename(&copy, &destination)?;
        }
        self.inner.borrow_mut().published = true;
        profile::flush_directory(destination.parent().unwrap_or(&destination))?;
        self.finish(tr!(
            "Signed in. Your encrypted session is ready. Connect with your local key."
        ));
        self.saved.emit();
        Ok(())
    }

    pub fn cancel(&self) {
        let running = {
            let mut inner = self.inner.borrow_mut();
            inner.cancelled = true;
            match &inner.process {
                Some(process) => {
                    process.signal(UnixSignal::SIGKILL);
                    true
                }
                None => false,
            }
        };
        if !running && self.active() {
            self.finish(tr!("Sign-in cancelled. Your saved session was preserved."));
        }
    }

    fn finish(&self, message: String) {
        let temporary = {
            let mut inner = self.inner.borrow_mut();
            let published = inner.published;
            let temporary = inner.temporary.take();
            *inner = Inner {
                published,
                ..Inner::default()
            };
            temporary
        };
        let mut message = message;
        if let Some(temporary) = temporary {
            if temporary.close().is_err() {
                message.push(' ');
                message.push_str(&tr!("Temporary encrypted sign-in files could not be removed."));
            }
        }
        self.inner.borrow_mut().message = message;
        self.changed.emit();
    }
}
