//! The local key in the desktop keyring, through the Secret Service tool `secret-tool`, only while the user turns it
//! on. Every call runs beside the main loop and returns a `Request` that the window polls from its timer, so a keyring
//! that waits for an unlock prompt never blocks it.
use std::env;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;
use std::time::{Duration, Instant};

use nix::sys::signal::Signal as UnixSignal;
use zeroize::{Zeroize, Zeroizing};

use crate::process::{Event, Process};

const ATTRIBUTES: [&str; 4] = ["application", "icloud-on-linux", "secret", "local-key"];
/// Long enough for the user to answer a keyring unlock prompt.
const TIMEOUT: Duration = Duration::from_secs(120);
/// Room for the looked-up key, reserved at once so the buffer is never moved and left uncleared.
const LIMIT: usize = 4096;

#[derive(Clone, Debug, Default)]
pub struct Keyring {
    search_path: Option<OsString>,
}

/// How a keyring call ended.
pub enum Reply {
    /// The key was saved or removed.
    Done,
    /// The saved key.
    Key(Zeroizing<String>),
    /// secret-tool is missing or failed, no key is saved, or the keyring did not answer within two minutes.
    Failed,
}

/// One secret-tool call. Dropping it stops the call, so run one at a time and keep each until its reply.
pub struct Request {
    process: Option<Process>,
    lookup: bool,
    output: Zeroizing<Vec<u8>>,
    too_long: bool,
    deadline: Instant,
    done: bool,
}

impl Keyring {
    /// Look for secret-tool in `path` instead of the inherited `PATH`.
    pub fn with_search_path(path: OsString) -> Keyring {
        Keyring {
            search_path: Some(path),
        }
    }

    pub fn available(&self) -> bool {
        let path = self
            .search_path
            .clone()
            .or_else(|| env::var_os("PATH"))
            .unwrap_or_default();
        env::split_paths(&path).any(|folder| {
            fs::metadata(folder.join("secret-tool"))
                .is_ok_and(|info| info.is_file() && info.permissions().mode() & 0o111 != 0)
        })
    }

    /// Save the key. secret-tool reads it from standard input, so it never appears in a command line.
    pub fn store(&self, key: &str) -> Request {
        self.run(
            &["store", "--label=iCloud on Linux local key"],
            Some(key.as_bytes().to_vec()),
        )
    }

    pub fn lookup(&self) -> Request {
        self.run(&["lookup"], None)
    }

    pub fn clear(&self) -> Request {
        self.run(&["clear"], None)
    }

    fn run(&self, arguments: &[&str], input: Option<Vec<u8>>) -> Request {
        let mut command = Command::new("secret-tool");
        if let Some(path) = &self.search_path {
            command.env("PATH", path);
        }
        command.args(arguments).args(ATTRIBUTES);
        Request {
            process: Process::start(command, input, true).ok(),
            lookup: arguments[0] == "lookup",
            output: Zeroizing::new(Vec::with_capacity(LIMIT)),
            too_long: false,
            deadline: Instant::now() + TIMEOUT,
            done: false,
        }
    }
}

impl Request {
    /// The reply once secret-tool has ended, or `Failed` after two minutes; `None` before and after that.
    pub fn poll(&mut self) -> Option<Reply> {
        if self.done {
            return None;
        }
        let Some(process) = &self.process else {
            self.done = true;
            return Some(Reply::Failed);
        };
        let mut code = None;
        for event in process.events() {
            match event {
                Event::Stdout(mut bytes) => {
                    if self.lookup && self.output.len() + bytes.len() <= LIMIT {
                        self.output.extend_from_slice(&bytes);
                    } else {
                        self.too_long |= self.lookup;
                    }
                    bytes.zeroize();
                }
                Event::Stderr(_) => {}
                Event::Exited(value) => code = Some(value),
            }
        }
        let reply = match code {
            Some(0) if self.lookup => self.key(),
            Some(0) => Reply::Done,
            Some(_) => Reply::Failed,
            None if Instant::now() < self.deadline => return None,
            None => {
                process.signal(UnixSignal::SIGKILL);
                Reply::Failed
            }
        };
        self.done = true;
        self.process = None;
        Some(reply)
    }

    fn key(&self) -> Reply {
        // Local keys never contain line breaks, so a trailing one comes from secret-tool.
        match std::str::from_utf8(&self.output).map(|text| text.trim_end_matches('\n')) {
            Ok(key) if !key.is_empty() && !self.too_long => Reply::Key(Zeroizing::new(key.to_owned())),
            _ => Reply::Failed,
        }
    }
}

impl Drop for Request {
    fn drop(&mut self) {
        if let Some(process) = &self.process {
            process.signal(UnixSignal::SIGKILL);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const KEY: &str = "keyring-fixture-key";
    /// Keeps one secret beside itself and logs every command line it receives.
    const STUB: &str = r#"#!/bin/sh
here=$(dirname "$0")
printf '%s\n' "$*" >> "$here/calls"
command=$1
shift
case $command in
store)
    case $1 in --label=*) shift ;; *) exit 2 ;; esac
    printf '%s' "$*" > "$here/attributes"
    cat > "$here/secret" ;;
lookup)
    [ -f "$here/secret" ] && [ "$(cat "$here/attributes")" = "$*" ] || exit 1
    cat "$here/secret"
    echo ;;
clear)
    rm -f "$here/secret" "$here/attributes" ;;
esac
"#;

    fn install(folder: &Path, script: &str) -> Keyring {
        let tool = folder.join("secret-tool");
        fs::write(&tool, script).unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
        let mut path = OsString::from(folder);
        path.push(":/usr/bin:/bin");
        Keyring::with_search_path(path)
    }

    fn finish(mut request: Request) -> Reply {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(reply) = request.poll() {
                assert!(request.poll().is_none());
                return reply;
            }
            assert!(Instant::now() < deadline, "secret-tool did not finish");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn the_key_is_stored_found_and_cleared_without_appearing_in_arguments() {
        let root = tempfile::tempdir().unwrap();
        let keyring = install(root.path(), STUB);
        assert!(keyring.available());
        assert!(matches!(finish(keyring.lookup()), Reply::Failed));
        assert!(matches!(finish(keyring.store(KEY)), Reply::Done));
        assert!(matches!(finish(keyring.lookup()), Reply::Key(key) if key.as_str() == KEY));
        assert!(matches!(finish(keyring.clear()), Reply::Done));
        assert!(matches!(finish(keyring.lookup()), Reply::Failed));
        let calls = fs::read_to_string(root.path().join("calls")).unwrap();
        assert!(!calls.contains(KEY));
        let commands: Vec<&str> = calls.lines().filter_map(|line| line.split(' ').next()).collect();
        assert_eq!(commands, ["lookup", "store", "lookup", "clear", "lookup"]);
        assert!(calls.contains("store --label=iCloud on Linux local key application icloud-on-linux secret local-key"));
    }

    #[test]
    fn a_missing_tool_is_reported_as_unavailable() {
        let root = tempfile::tempdir().unwrap();
        let keyring = Keyring::with_search_path(root.path().into());
        assert!(!keyring.available());
        assert!(matches!(finish(keyring.store(KEY)), Reply::Failed));
        assert!(matches!(finish(keyring.lookup()), Reply::Failed));
    }

    #[test]
    fn a_keyring_that_does_not_answer_fails_after_the_timeout() {
        let root = tempfile::tempdir().unwrap();
        let keyring = install(root.path(), "#!/bin/sh\nexec sleep 30\n");
        let mut request = keyring.lookup();
        assert!(request.poll().is_none());
        request.deadline = Instant::now();
        assert!(matches!(request.poll(), Some(Reply::Failed)));
        assert!(request.process.is_none());
    }
}
