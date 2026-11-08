//! The visible window with the fictional profile: connect with the local key, upload files and decide a conflict,
//! disconnect during an upload and resume it, and end the app during an upload; then, in a new window, connect, open
//! the file manager, and disconnect. It runs only when `WAYLAND_DISPLAY` or `DISPLAY` is set; Open is clicked only
//! with `--expect-file-manager NAME`, which then waits for that process to start.
#[path = "../../../core/tests/support/mod.rs"]
mod support;
mod ui;
mod uploads;

use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::{env, fs};

use adw::prelude::*;
use gtk::gio;
use icloud_core::session::{Session, State};
use icloud_on_linux::window::Window;

use ui::{button, shows, wait};

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

/// The processes with this name that run as the current user.
fn processes(name: &str) -> Vec<i32> {
    let uid = nix::unistd::getuid().as_raw();
    fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| {
            entry.metadata().is_ok_and(|metadata| metadata.uid() == uid)
                && fs::read_to_string(entry.path().join("comm")).is_ok_and(|comm| comm.trim_end() == name)
        })
        .filter_map(|entry| entry.file_name().to_str()?.parse().ok())
        .collect()
}

fn running(name: &str) -> bool {
    !processes(name).is_empty()
}

/// Close the file manager that Open started, as a user closes its window before disconnecting.
fn close(name: &str) {
    for process in processes(name) {
        let _ = nix::sys::signal::kill(nix::unistd::Pid::from_raw(process), nix::sys::signal::Signal::SIGTERM);
    }
    wait(30, &format!("{name} closes"), || !running(name));
}

fn main() {
    if env::var_os("WAYLAND_DISPLAY").is_none() && env::var_os("DISPLAY").is_none() {
        println!("SKIP: the visible window check needs a display.");
        return;
    }
    let mut arguments = env::args().skip(1);
    let mut file_manager = None;
    while let Some(argument) = arguments.next() {
        if argument == "--expect-file-manager" {
            file_manager = arguments.next();
        }
    }
    adw::init().expect("GTK could not open the display");
    let application = adw::Application::builder()
        .application_id("io.github.luiscuellar31.IcloudOnLinux.Check")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    application.register(gio::Cancellable::NONE).unwrap();

    let fixture = support::Fixture::new();
    fs::write(fixture.source.join("sample.txt"), "Native desktop fixture\n").unwrap();
    let _unmount = Unmount(fixture.paths.mount.clone());

    // A file manager that shows the folder keeps it busy, which correctly stops a disconnect, so the uploads and
    // ending the app come first and Open is checked last, in a new window that closes the file manager first.
    {
        let (session, window) = show_window(&application, &fixture);
        ui::connect(&window, &session);
        uploads::check(&window, &session, &fixture);
        uploads::check_shut_down(&window, &session, &fixture);
    }

    let (session, window) = show_window(&application, &fixture);
    ui::connect(&window, &session);
    let open = button(&window, "Open iCloud Drive");
    assert!(open.is_sensitive());
    if let Some(name) = &file_manager {
        open.emit_clicked();
        wait(30, &format!("{name} starts"), || running(name));
        assert_eq!(session.state(), State::Available, "{}", session.message());
        println!("PASS: Open started {name} for the private mount.");
        close(name);
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

/// A new session and window for the fixture, shown on screen.
fn show_window(application: &adw::Application, fixture: &support::Fixture) -> (Rc<Session>, Rc<Window>) {
    let session = Session::new(fixture.paths.clone());
    let window = Window::new(application, session.clone());
    window.window.present();
    wait(30, "the window is shown", || window.window.is_mapped());
    (session, window)
}
