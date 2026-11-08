//! Child processes beside the main loop. Reader threads and a waiter forward the output and the exit code over a
//! channel; components collect them from their timers, as the window drives them.
use std::env;
use std::io::{self, Read, Write};
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use nix::sys::signal::{self, Signal as UnixSignal};
use nix::unistd::Pid;
use zeroize::Zeroizing;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
    /// The exit code, or 128 plus the signal that ended the process. It always comes after the output.
    Exited(i32),
}

pub struct Process {
    pid: u32,
    child: Arc<Mutex<Option<Child>>>,
    events: Receiver<Event>,
}

/// Remove the rclone variables this app inherited, so only those a component sets reach rclone.
pub fn without_rclone_variables(command: &mut Command) -> &mut Command {
    for (name, _) in env::vars_os() {
        if name.to_string_lossy().starts_with("RCLONE_") || name.to_string_lossy().starts_with("_RCLONE_") {
            command.env_remove(name);
        }
    }
    command
}

impl Process {
    /// Start `command` with captured output, writing `input` to its standard input and then closing it. With
    /// `guard`, Linux kills the process when this app dies. Start guarded processes from the main thread: the kernel
    /// ties that signal to the thread that started them.
    pub fn start(mut command: Command, input: Option<Vec<u8>>, guard: bool) -> io::Result<Process> {
        // The input may be a key, so its bytes are cleared once written, or dropped unwritten.
        let input = input.map(Zeroizing::new);
        command
            .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if guard {
            protect(&mut command);
        }
        let mut child = command.spawn()?;
        let (sender, events) = mpsc::channel();
        if let (Some(bytes), Some(mut stdin)) = (input, child.stdin.take()) {
            // A process that exits without reading closes the pipe; the write error is expected then.
            thread::spawn(move || {
                let _ = stdin.write_all(&bytes);
            });
        }
        let readers = [
            child
                .stdout
                .take()
                .map(|stream| forward(stream, sender.clone(), Event::Stdout)),
            child
                .stderr
                .take()
                .map(|stream| forward(stream, sender.clone(), Event::Stderr)),
        ];
        let pid = child.id();
        let child = Arc::new(Mutex::new(Some(child)));
        let shared = child.clone();
        thread::spawn(move || {
            let code = loop {
                let status = shared.lock().unwrap().as_mut().map(Child::try_wait);
                match status {
                    Some(Ok(Some(status))) => break exit_code(status),
                    Some(Ok(None)) => thread::sleep(Duration::from_millis(20)),
                    _ => break -1,
                }
            };
            *shared.lock().unwrap() = None;
            // Output comes first, but a background grandchild may keep the pipes open, so the wait is bounded.
            let deadline = Instant::now() + Duration::from_secs(2);
            while readers.iter().flatten().any(|reader| !reader.is_finished()) && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            let _ = sender.send(Event::Exited(code));
        });
        Ok(Process { pid, child, events })
    }

    pub fn id(&self) -> u32 {
        self.pid
    }

    /// The events received since the last call, in order.
    pub fn events(&self) -> Vec<Event> {
        self.events.try_iter().collect()
    }

    /// Signal the process unless it has ended. The lock keeps the waiter from reaping it meanwhile, so a reused
    /// process ID can never receive the signal.
    pub fn signal(&self, signal: UnixSignal) -> bool {
        let mut child = self.child.lock().unwrap();
        let running = child
            .as_mut()
            .is_some_and(|running| matches!(running.try_wait(), Ok(None)));
        running && signal::kill(Pid::from_raw(self.pid as i32), signal).is_ok()
    }
}

fn forward(
    mut stream: impl Read + Send + 'static,
    sender: Sender<Event>,
    wrap: fn(Vec<u8>) -> Event,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        // Output can hold secrets, so the buffer is cleared when the stream ends.
        let mut buffer = Zeroizing::new(vec![0; 64 * 1024]);
        while let Ok(count @ 1..) = stream.read(&mut buffer) {
            if sender.send(wrap(buffer[..count].to_vec())).is_err() {
                break;
            }
        }
    })
}

fn exit_code(status: ExitStatus) -> i32 {
    status.code().unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
}

#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
fn protect(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    let parent = nix::unistd::getpid();
    // SAFETY: the closure runs in the child between fork and exec, and only makes the async-signal-safe prctl and
    // getppid system calls; it allocates nothing and takes no locks.
    unsafe {
        command.pre_exec(move || {
            nix::sys::prctl::set_pdeathsig(UnixSignal::SIGKILL)?;
            // The app may have died before the protection was armed; then no signal would ever come.
            if nix::unistd::getppid() != parent {
                return Err(io::Error::from_raw_os_error(nix::libc::ESRCH));
            }
            Ok(())
        });
    }
}

#[cfg(not(target_os = "linux"))]
fn protect(_command: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn finish(process: &Process) -> Vec<Event> {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut events = Vec::new();
        while !matches!(events.last(), Some(Event::Exited(_))) {
            assert!(Instant::now() < deadline, "the process did not finish");
            events.extend(process.events());
            thread::sleep(Duration::from_millis(10));
        }
        events
    }

    fn shell(script: &str) -> Command {
        let mut command = Command::new("sh");
        command.args(["-c", script]);
        command
    }

    #[test]
    fn output_input_and_exit_code_arrive_in_order() {
        let process = Process::start(
            shell("cat; printf warning >&2; exit 3"),
            Some(b"typed key".to_vec()),
            true,
        )
        .unwrap();
        let events = finish(&process);
        assert!(events.contains(&Event::Stdout(b"typed key".to_vec())));
        assert!(events.contains(&Event::Stderr(b"warning".to_vec())));
        assert_eq!(events.last(), Some(&Event::Exited(3)));
    }

    #[test]
    fn a_running_process_can_be_signalled_once() {
        let process = Process::start(shell("exec sleep 30"), None, false).unwrap();
        assert!(process.signal(UnixSignal::SIGTERM));
        assert_eq!(finish(&process).last(), Some(&Event::Exited(128 + 15)));
        assert!(!process.signal(UnixSignal::SIGTERM));
    }

    #[test]
    fn the_exit_is_reported_even_while_a_grandchild_keeps_the_output_open() {
        let started = Instant::now();
        let process = Process::start(shell("sleep 4 & echo started"), None, false).unwrap();
        let events = finish(&process);
        assert!(events.contains(&Event::Stdout(b"started\n".to_vec())));
        assert_eq!(events.last(), Some(&Event::Exited(0)));
        assert!(started.elapsed() < Duration::from_secs(4));
    }

    #[test]
    fn inherited_rclone_variables_are_removed() {
        let mut command = shell("env");
        command.env("RCLONE_CONFIG", "/chosen.conf");
        without_rclone_variables(&mut command).env("RCLONE_CONFIG", "/chosen.conf");
        let events = finish(&Process::start(command, None, false).unwrap());
        let output: Vec<u8> = events
            .iter()
            .filter_map(|event| match event {
                Event::Stdout(bytes) => Some(bytes.clone()),
                _ => None,
            })
            .flatten()
            .collect();
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("RCLONE_CONFIG=/chosen.conf"));
        assert_eq!(output.matches("RCLONE_").count(), 1);
    }
}
