//! The synced folders part of the visible check: add a folder, continue a review of mass deletions, and stop syncing.
//! Folders are added through the window's folders part, as its folder dialog does, because the dialog cannot be
//! driven from a check.
use std::fs;

use adw::prelude::*;
use icloud_core::places::Places;
use icloud_core::sync::{Review, State as FolderState};
use icloud_on_linux::app::App;
use icloud_on_linux::window::Window;

use crate::support::Fixture;
use crate::ui::{self, button, shows, wait};

/// Wait until a dialog shows a button with this label, then click it.
fn answer(window: &Window, label: &str) {
    let shown = |found: &gtk::Button| found.label().as_deref() == Some(label);
    wait(10, &format!("the dialog offers {label}"), || {
        ui::find(window, shown).is_some()
    });
    button(window, label).emit_clicked();
}

pub fn check(app: &App, fixture: &Fixture) {
    let window = &app.window();
    let sync = app
        .parts
        .sync
        .clone()
        .expect("the synced folder list could not be opened");
    let remote = fixture.source.join("Docs");
    fs::create_dir(&remote).unwrap();
    for index in 0..8 {
        fs::write(
            remote.join(format!("file{index}.txt")),
            format!("Synced file {index}\n"),
        )
        .unwrap();
    }
    // The place goes to temporary settings, so the desktop's own sidebar stays unchanged.
    let places = Places {
        gtk: fixture.root.path().join("config-home/gtk-3.0/bookmarks"),
        kde: fixture.root.path().join("data-home/user-places.xbel"),
    };
    app.parts.places.replace(places.clone());
    window.folders.add_chosen(&fixture.paths.mount.join("Docs"));
    let added = sync.folders();
    assert_eq!(added.len(), 1, "{}", sync.message());
    let place = sync.local_root().to_str().unwrap().replace(' ', "%20");
    assert_eq!(
        fs::read_to_string(&places.gtk).unwrap(),
        format!("file://{place} iCloud Sync\n")
    );
    let (identifier, local) = (added[0].identifier.clone(), added[0].local.clone());
    let folder = || sync.folder(&identifier).unwrap();
    wait(90, "the first copy finishes", || {
        !sync.busy() && folder().state == FolderState::UpToDate
    });
    assert_eq!(fs::read_to_string(local.join("file7.txt")).unwrap(), "Synced file 7\n");
    assert!(
        shows(window, "Connected · Up to date"),
        "the status does not show the synced folder"
    );
    let row = ui::find(window, |row: &adw::ActionRow| row.title() == "Docs").expect("the folder is not listed");
    let list = row.parent().and_downcast::<gtk::ListBox>().unwrap();
    list.select_row(Some(&row));

    // Deleting most files on this computer stops sync until Review continues it.
    for index in 0..6 {
        fs::remove_file(local.join(format!("file{index}.txt"))).unwrap();
    }
    list.activate_action("folder.sync", None).unwrap();
    wait(90, "the folder asks for a review", || {
        !sync.busy() && folder().attention.is_some()
    });
    assert_eq!(folder().attention, Some(Review::Confirm), "{}", folder().message);
    assert!(
        remote.join("file0.txt").exists(),
        "files were deleted before the review"
    );
    assert!(
        shows(window, "Connected · Review needed"),
        "the status does not ask for a review"
    );
    button(window, "Review...").emit_clicked();
    answer(window, "Continue");
    wait(90, "the confirmed sync finishes", || {
        !sync.busy() && folder().state == FolderState::UpToDate
    });
    assert!(!remote.join("file0.txt").exists() && remote.join("file7.txt").exists());
    println!("PASS: a synced folder copied its iCloud files, and Review applied the deletions it had stopped.");

    list.activate_action("folder.stop", None).unwrap();
    answer(window, "Stop syncing");
    wait(10, "the folder is removed", || sync.folders().is_empty());
    assert_eq!(fs::read_to_string(local.join("file7.txt")).unwrap(), "Synced file 7\n");
    assert!(remote.join("file7.txt").exists());
    println!("PASS: Stop syncing removed the folder and kept the local copy and the iCloud folder.");
}
