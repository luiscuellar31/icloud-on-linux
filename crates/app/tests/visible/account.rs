//! The account part of the visible check: Sign out of this computer removes the fictional session, upload queue, and
//! synced folders and keeps the local copy; then Change Apple account also moves the local copy to the Trash, removes
//! the iCloud Sync place, and opens sign-in. The keyring, the sidebar places, and the Trash are stand-ins inside the
//! fixture, so nothing of the desktop's own changes.
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use adw::prelude::*;
use icloud_core::connection::Connection;
use icloud_core::keyring::Keyring;
use icloud_core::places::Places;
use icloud_core::sync::State as FolderState;
use icloud_core::uploads::State as Row;
use icloud_on_linux::app::App;
use icloud_on_linux::window::Window;

use crate::support::Fixture;
use crate::ui::{self, button, wait};

const SIGNED_OUT: &str = "Signed out of this computer. Nothing was removed from iCloud.";

/// A secret-tool that only records each command in `calls` beside it.
fn keyring(folder: &Path) -> Keyring {
    fs::create_dir_all(folder).unwrap();
    let tool = folder.join("secret-tool");
    fs::write(
        &tool,
        "#!/bin/sh\nprintf '%s\\n' \"$1\" >> \"$(dirname \"$0\")/calls\"\n",
    )
    .unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    let mut path = folder.as_os_str().to_owned();
    path.push(":/usr/bin:/bin");
    Keyring::with_search_path(path)
}

/// Sync the iCloud folder `Docs` and wait for its first copy; returns the local copy.
fn sync_docs(app: &App, fixture: &Fixture) -> PathBuf {
    let sync = app.parts.sync.clone().unwrap();
    app.window().folders.add_chosen(&fixture.paths.mount.join("Docs"));
    let added = sync.folders().pop().unwrap_or_else(|| panic!("{}", sync.message()));
    wait(90, "the first copy finishes", || {
        !sync.busy() && sync.folder(&added.identifier).unwrap().state == FolderState::UpToDate
    });
    added.local
}

fn shows_text(window: &Window, part: &str) -> bool {
    ui::find(window, |label: &gtk::Label| label.label().contains(part)).is_some()
}

/// Run the menu `action`, check that the confirmation shows `listed`, and confirm with `accept`, moving the local
/// copies to the Trash when `trash` is set.
fn confirm(window: &Window, action: &str, listed: &[&str], accept: &str, trash: bool) {
    window.window.lookup_action(action).unwrap().activate(None);
    let shown = |found: &gtk::Button| found.label().as_deref() == Some(accept);
    wait(10, "the confirmation is shown", || ui::find(window, shown).is_some());
    for part in listed {
        assert!(shows_text(window, part), "the confirmation does not show {part}");
    }
    let option = |check: &gtk::CheckButton| {
        check
            .label()
            .is_some_and(|label| label.starts_with("Also move the local copies"))
    };
    ui::find(window, option)
        .expect("the Trash option is missing")
        .set_active(trash);
    button(window, accept).emit_clicked();
}

pub fn check(app: &App, fixture: &Fixture) {
    let root = fixture.root.path();
    let (session, settings) = (&app.parts.session, &app.parts.settings);
    let (transfers, sync) = (app.parts.transfers.clone().unwrap(), app.parts.sync.clone().unwrap());
    let places = Places {
        gtk: root.join("config-home/gtk-3.0/bookmarks"),
        kde: root.join("data-home/user-places.xbel"),
    };
    app.parts.places.replace(places.clone());
    let calls = root.join("keyring/calls");
    app.parts.keyring.replace(Some(keyring(&root.join("keyring"))));
    let trash = root.join("Trash");
    fs::create_dir_all(&trash).unwrap();
    let bin = trash.clone();
    *app.parts.trash.borrow_mut() =
        Box::new(move |path: &Path| fs::rename(path, bin.join(path.file_name().unwrap())).is_ok());
    let profile = fs::read(&fixture.paths.config).unwrap();

    // The upload that ending the app paused is still pending.
    assert!(transfers.items().iter().any(|item| item.state == Row::Paused));
    ui::connect(&app.window(), session);
    let kept = sync_docs(app, fixture);
    // A synced folder and named uploads are what the diagnostic report must describe without naming.
    crate::report::check(app, fixture);
    settings.set_apple_signed_in_at(1_800_000_000).unwrap();
    let listed = [
        "1 pending upload(s)",
        "Syncing of 1 folder(s)",
        "Nothing is removed from iCloud.",
    ];
    confirm(&app.window(), "sign-out", &listed, "Sign out", false);
    wait(60, "signing out finishes", || session.message() == SIGNED_OUT);
    assert!(!fixture.paths.config.exists(), "the fictional session was kept");
    assert!(transfers.items().is_empty() && sync.folders().is_empty());
    assert_eq!(fs::read_to_string(kept.join("file7.txt")).unwrap(), "Synced file 7\n");
    assert!(fs::read_to_string(&places.gtk).unwrap().contains(" iCloud Sync\n"));
    assert_eq!(settings.apple_signed_in_at(), None);
    wait(10, "the keyring is cleared", || {
        fs::read_to_string(&calls).is_ok_and(|text| text.lines().any(|line| line == "clear"))
    });
    assert!(button(&app.window(), "Sign in to iCloud").is_drawable());
    println!("PASS: Sign out removed the fictional session, uploads, and synced folders, and kept the local copy.");

    // Signed in again with the same profile, a folder is synced into an otherwise empty iCloud Sync.
    fs::write(&fixture.paths.config, &profile).unwrap();
    fs::set_permissions(&fixture.paths.config, fs::Permissions::from_mode(0o600)).unwrap();
    for leftover in fs::read_dir(sync.local_root()).unwrap().flatten() {
        fs::remove_dir_all(leftover.path()).unwrap();
    }
    session.changed().emit();
    ui::connect(&app.window(), session);
    let copy = sync_docs(app, fixture);
    let window = app.window();
    let sign_in = |row: &adw::EntryRow| row.title() == "Apple account";
    let listed = ["To use another Apple account", "Syncing of 1 folder(s)"];
    confirm(&window, "change-account", &listed, "Sign out and continue", true);
    wait(60, "sign-in opens", || ui::find(&window, sign_in).is_some());
    assert_eq!(session.message(), SIGNED_OUT);
    assert!(!fixture.paths.config.exists());
    assert!(!copy.exists() && !sync.local_root().exists());
    assert_eq!(
        fs::read_to_string(trash.join("Docs/file7.txt")).unwrap(),
        "Synced file 7\n"
    );
    assert!(!fs::read_to_string(&places.gtk).unwrap().contains("iCloud Sync"));
    button(&window, "Cancel").emit_clicked();
    wait(10, "sign-in closes", || ui::find(&window, sign_in).is_none());
    println!(
        "PASS: Change Apple account moved the local copy to the Trash, removed its sidebar place, and opened sign-in."
    );
}
