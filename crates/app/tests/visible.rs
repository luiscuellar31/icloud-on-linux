//! The visible window on a Wayland display with the fictional profile: connect with the local key, open the file
//! manager, and disconnect. It runs only when `WAYLAND_DISPLAY` is set; Open is clicked only with
//! `--expect-file-manager NAME`, which then waits for that process to start.
#[path = "../../core/tests/support/mod.rs"]
mod support;

use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use std::{env, fs};

use adw::prelude::*;
use gtk::{gio, glib};
use icloud_core::session::{Session, State};
use icloud_on_linux::window::Window;

/// Unmount the fixture if a failed check leaves it mounted.
struct Unmount(PathBuf);

impl Drop for Unmount {
    fn drop(&mut self) {
        let _ = Command::new("fusermount3")
            .arg("-uz")
            .arg(&self.0)
            .stderr(Stdio::null())
            .status();
    }
}

/// Run the main loop until `done` holds, failing after `seconds`.
fn wait(seconds: u64, what: &str, done: impl Fn() -> bool) {
    let context = glib::MainContext::default();
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        while context.iteration(false) {}
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Every widget under `root`, depth first.
fn widgets(root: &gtk::Widget) -> Vec<gtk::Widget> {
    let mut found = vec![root.clone()];
    let mut child = root.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();
        found.extend(widgets(&widget));
    }
    found
}

/// The first widget of type `T` on screen that `matches` accepts.
fn find<T: IsA<gtk::Widget>>(window: &Window, matches: impl Fn(&T) -> bool) -> Option<T> {
    widgets(window.window.upcast_ref())
        .into_iter()
        .filter_map(|widget| widget.downcast::<T>().ok())
        .find(|widget| widget.is_drawable() && matches(widget))
}

fn button(window: &Window, label: &str) -> gtk::Button {
    find(window, |button: &gtk::Button| button.label().as_deref() == Some(label))
        .unwrap_or_else(|| panic!("the {label} button is not shown"))
}

fn shows(window: &Window, text: &str) -> bool {
    find(window, |label: &gtk::Label| label.label() == text).is_some()
}

/// Whether a process with this name runs as the current user.
fn running(name: &str) -> bool {
    let uid = nix::unistd::getuid().as_raw();
    fs::read_dir("/proc").into_iter().flatten().flatten().any(|entry| {
        entry.metadata().is_ok_and(|metadata| metadata.uid() == uid)
            && fs::read_to_string(entry.path().join("comm")).is_ok_and(|comm| comm.trim_end() == name)
    })
}

fn main() {
    if env::var_os("WAYLAND_DISPLAY").is_none() {
        println!("SKIP: the visible window check needs a Wayland display.");
        return;
    }
    let mut arguments = env::args().skip(1);
    let mut file_manager = None;
    while let Some(argument) = arguments.next() {
        if argument == "--expect-file-manager" {
            file_manager = arguments.next();
        }
    }
    adw::init().expect("GTK could not open the Wayland display");
    let application = adw::Application::builder()
        .application_id("io.github.luiscuellar31.IcloudOnLinux.Check")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    application.register(gio::Cancellable::NONE).unwrap();

    let fixture = support::Fixture::new();
    fs::write(fixture.source.join("sample.txt"), "Native desktop fixture\n").unwrap();
    let _unmount = Unmount(fixture.paths.mount.clone());
    let session = Session::new(fixture.paths.clone());
    let window = Window::new(&application, session.clone());
    window.window.present();
    wait(30, "the window is shown", || window.window.is_mapped());

    let key = find(&window, |row: &adw::PasswordEntryRow| row.title() == "Local key")
        .expect("the Local key field is not shown");
    key.set_text(support::KEY);
    button(&window, "Connect").emit_clicked();
    wait(60, "the connection finishes", || {
        matches!(session.state(), State::Available | State::Error)
    });
    assert_eq!(session.state(), State::Available, "{}", session.message());
    assert!(key.text().is_empty(), "the window kept the local key after connecting");
    assert!(shows(&window, "Connected"), "the window does not say it is connected");
    let open = button(&window, "Open iCloud Drive");
    assert!(open.is_sensitive());
    if let Some(name) = &file_manager {
        open.emit_clicked();
        wait(30, &format!("{name} starts"), || running(name));
        assert_eq!(session.state(), State::Available, "{}", session.message());
        println!("PASS: Open started {name} for the private mount.");
    }

    let disconnect = window.window.lookup_action("disconnect").unwrap();
    assert!(disconnect.is_enabled());
    disconnect.activate(None);
    wait(30, "the session disconnects", || {
        session.state() == State::Disconnected && !session.running()
    });
    assert!(session.current_mount().is_none());
    assert!(
        shows(&window, "Disconnected"),
        "the window does not say it is disconnected"
    );
    assert!(button(&window, "Connect").is_drawable());
    window.window.destroy();
    println!("PASS: the visible window connected with the local key, showed the connection, and disconnected.");
}
