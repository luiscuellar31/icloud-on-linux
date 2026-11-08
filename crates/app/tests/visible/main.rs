//! The visible window with the fictional profile: connect with the local key, choose another language while
//! connected, sync a folder and continue its review, upload files and decide a conflict, disconnect during an upload
//! and resume it, and end the app during an upload; then, in a new window, connect, open the file manager, and
//! disconnect. It runs only when `WAYLAND_DISPLAY` or `DISPLAY` is set; Open is clicked only with
//! `--expect-file-manager NAME`, which then waits for that process to start.
mod folders;
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
use icloud_on_linux::app::App;

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

    // A file manager that shows the folder keeps it busy, which correctly stops a disconnect, so synced folders,
    // uploads, and ending the app come first and Open is checked last, in a new window that closes the file manager
    // first.
    {
        let app = show_app(&application, &fixture);
        ui::connect(&app.window(), &app.parts.session);
        check_language(&app, &fixture);
        folders::check(&app, &fixture);
        uploads::check(&app, &fixture);
        uploads::check_shut_down(&app, &fixture);
    }

    let app = show_app(&application, &fixture);
    let (session, window) = (&app.parts.session, app.window());
    ui::connect(&window, session);
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

/// The app with a new session for the fixture, its window shown on screen.
fn show_app(application: &adw::Application, fixture: &support::Fixture) -> Rc<App> {
    let app = App::new(application, Session::new(fixture.paths.clone()));
    wait(30, "the window is shown", || app.window().window.is_mapped());
    app
}

/// Choose Spanish and then English again while connected. Each choice is saved and shows a new window in that
/// language, which keeps the folder for uploads, while the connection stays up.
fn check_language(app: &App, fixture: &support::Fixture) {
    let session = &app.parts.session;
    let window = app.window();
    ui::find(&window, |_: &gtk::Expander| true)
        .expect("the uploads section is not shown")
        .set_expanded(true);
    let destination = |row: &adw::EntryRow| row.title().contains("Upload to");
    wait(10, "the uploads section opens", || {
        ui::find(&window, destination).is_some()
    });
    ui::find(&window, destination).unwrap().set_text("Kept uploads");
    for (choice, connected) in [("es", "Conectado"), ("en", "Connected")] {
        let old = app.window();
        let language = old.window.lookup_action("language").unwrap();
        language.activate(Some(&choice.to_variant()));
        wait(10, "the window is replaced", || {
            !Rc::ptr_eq(&app.window(), &old) && old.window.application().is_none()
        });
        let window = app.window();
        wait(30, "the new window is shown", || window.window.is_mapped());
        assert!(shows(&window, connected), "the window does not say {connected}");
        assert!(
            ui::find(&window, |row: &adw::EntryRow| row.text() == "Kept uploads").is_some(),
            "the new window lost the folder for uploads"
        );
        assert_eq!(session.state(), State::Available, "{}", session.message());
        assert!(session.current_mount().is_some());
        let saved = fs::read_to_string(fixture.paths.settings()).unwrap();
        assert!(
            saved.lines().any(|line| line == format!("language={choice}")),
            "{saved}"
        );
    }
    println!("PASS: choosing a language showed the window in it, kept the connection, and saved the choice.");
}
