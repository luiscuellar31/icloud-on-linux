//! The uploads part of the visible check: queue files, decide a conflict with Keep both, disconnect during an upload
//! before reconnecting and resuming it, and end the app during an upload. Files are queued through the window's
//! uploads part, as its file dialog does, because the dialog cannot be driven from a check.
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::{env, fs};

use adw::prelude::*;
use icloud_core::session::{Session, State};
use icloud_core::transfers::Transfers;
use icloud_core::uploads::{State as Row, Upload, UploadStore};
use icloud_on_linux::app::App;
use icloud_on_linux::window::Window;

use crate::support::Fixture;
use crate::ui::{self, button, shows, wait};

fn last(transfers: &Transfers) -> Option<Upload> {
    transfers.items().pop()
}

fn row(transfers: &Transfers, identifier: &str) -> Upload {
    let items = transfers.items();
    items.into_iter().find(|item| item.identifier == identifier).unwrap()
}

/// A search path whose `rclone` copies at 128 KiB/s, so that a disconnect can interrupt an upload.
fn slow_rclone(root: &Path) -> std::ffi::OsString {
    let path = env::var_os("PATH").unwrap_or_default();
    let real = env::split_paths(&path)
        .map(|folder| folder.join("rclone"))
        .find(|path| path.is_file())
        .expect("rclone must be installed for this check");
    let folder = root.join("slow");
    fs::create_dir_all(&folder).unwrap();
    let script = format!(
        "#!/bin/sh\ncase \" $* \" in *\" copy \"*) exec '{real}' --bwlimit 128k \"$@\" ;; esac\nexec '{real}' \"$@\"\n",
        real = real.display()
    );
    fs::write(folder.join("rclone"), script).unwrap();
    fs::set_permissions(folder.join("rclone"), fs::Permissions::from_mode(0o755)).unwrap();
    env::join_paths([folder].into_iter().chain(env::split_paths(&path))).unwrap()
}

/// Queue a large file with slowed copies and wait until its upload sends bytes.
fn start_slow_upload(window: &Window, session: &Session, transfers: &Transfers, file: PathBuf) -> Upload {
    session.set_search_path(Some(slow_rclone(file.parent().unwrap())));
    fs::write(&file, b"Pending window fixture\n".repeat(160_000)).unwrap();
    let count = transfers.items().len() + 1;
    window.uploads.add(vec![file]);
    wait(60, "the slowed upload sends bytes", || {
        transfers.items().len() == count
            && last(transfers).is_some_and(|item| item.state == Row::Uploading && item.progress > 0)
    });
    last(transfers).unwrap()
}

/// The upload stopped as Paused with its snapshot intact.
fn paused(fixture: &Fixture, transfers: &Transfers, identifier: &str) {
    let item = row(transfers, identifier);
    assert!(!transfers.busy() && item.state == Row::Paused, "{item:?}");
    let store = UploadStore::open(&fixture.paths.cache.join("uploads")).unwrap();
    store.snapshots.verify(&item).unwrap();
}

pub fn check(app: &App, fixture: &Fixture) {
    let (window, session) = (&app.window(), &app.parts.session);
    let transfers = app
        .parts
        .transfers
        .clone()
        .expect("the upload queue could not be opened");
    let section = ui::find(window, |_: &gtk::Expander| true).expect("the uploads section is not shown");
    section.set_expanded(true);
    let destination = |row: &adw::EntryRow| row.title().contains("Upload to");
    wait(10, "the uploads section opens", || {
        ui::find(window, destination).is_some()
    });
    ui::find(window, destination).unwrap().set_text("Window uploads");
    // Closed again, so that a conflict must open it.
    section.set_expanded(false);

    let workspace = fixture.root.path();
    let backend = fixture.source.join("Window uploads");
    let source = workspace.join("window.bin");
    fs::write(&source, b"Upload window fixture\n").unwrap();
    window.uploads.add(vec![source.clone()]);
    wait(60, "the first upload is verified", || {
        last(&transfers).is_some_and(|item| item.state == Row::Completed)
    });
    assert_eq!(
        fs::read(backend.join("window.bin")).unwrap(),
        b"Upload window fixture\n"
    );

    fs::write(&source, b"Another window fixture\n").unwrap();
    window.uploads.add(vec![source.clone()]);
    wait(60, "the conflict is shown", || {
        transfers.items().len() == 2 && last(&transfers).is_some_and(|item| item.state == Row::Conflict)
    });
    let conflict = last(&transfers).unwrap();
    assert!(section.is_expanded(), "a new conflict did not open the uploads section");
    assert!(
        shows(window, "Connected · Review needed"),
        "the status does not ask for a decision"
    );
    // Both rows show the same file name; the conflict is the second one.
    let list = ui::find(window, |listed: &adw::ActionRow| listed.title() == conflict.name)
        .and_then(|listed| listed.parent())
        .and_downcast::<gtk::ListBox>()
        .expect("the queue is not shown");
    let listed = list.row_at_index(1).and_downcast::<adw::ActionRow>().unwrap();
    assert_eq!(listed.title(), conflict.name);
    list.select_row(Some(&listed));
    assert!(button(window, "Skip").is_sensitive() && button(window, "Keep both").is_sensitive());
    button(window, "Keep both").emit_clicked();
    wait(60, "Keep both uploads the second file", || {
        row(&transfers, &conflict.identifier).state == Row::Completed
    });
    let kept = row(&transfers, &conflict.identifier);
    assert_eq!(
        fs::read(backend.join("window.bin")).unwrap(),
        b"Upload window fixture\n"
    );
    assert_eq!(fs::read(backend.join(&kept.name)).unwrap(), b"Another window fixture\n");
    println!("PASS: the queue showed a conflict, and Keep both preserved the cloud original.");

    let pending = workspace.join("pending-window.bin");
    let upload = start_slow_upload(window, session, &transfers, pending.clone());
    let disconnect = window.window.lookup_action("disconnect").unwrap();
    assert!(disconnect.is_enabled());
    disconnect.activate(None);
    wait(60, "the session disconnects", || {
        session.state() == State::Disconnected && !session.running()
    });
    assert!(session.current_mount().is_none());
    assert!(
        shows(window, "Disconnected"),
        "the window does not say it is disconnected"
    );
    assert!(button(window, "Connect").is_drawable());
    paused(fixture, &transfers, &upload.identifier);
    session.set_search_path(None);

    ui::connect(window, session);
    assert_eq!(last(&transfers).unwrap().state, Row::Paused);
    button(window, "Resume uploads").emit_clicked();
    wait(60, "the resumed upload is verified", || {
        last(&transfers).is_some_and(|item| item.state == Row::Completed)
    });
    assert_eq!(
        fs::read(backend.join("pending-window.bin")).unwrap(),
        fs::read(&pending).unwrap()
    );
    println!(
        "PASS: Disconnect paused an active upload before closing the connection; reconnecting and Resume finished it."
    );
}

/// Ending the app, as the desktop session does with SIGTERM, pauses an active upload before disconnecting and
/// closes the window afterwards.
pub fn check_shut_down(app: &App, fixture: &Fixture) {
    let (window, session) = (&app.window(), &app.parts.session);
    let transfers = app.parts.transfers.clone().unwrap();
    let upload = start_slow_upload(window, session, &transfers, fixture.root.path().join("ending.bin"));
    // A destroyed window is hidden and released from the app; the check still holds a reference to it.
    let closed = || !window.window.is_visible() && window.window.application().is_none();
    nix::sys::signal::raise(nix::sys::signal::Signal::SIGTERM).unwrap();
    wait(60, "the app closes", || closed() || session.state() == State::Error);
    assert!(closed(), "the app did not close: {}", session.message());
    assert!(session.state() == State::Disconnected && !session.running() && session.current_mount().is_none());
    paused(fixture, &transfers, &upload.identifier);
    println!("PASS: ending the app paused the active upload, disconnected, and then closed the window.");
}
