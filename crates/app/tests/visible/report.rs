//! The diagnostic report part of the visible check: saved while a folder is synced and named uploads wait, it
//! describes them without their names, the fixture's paths, or the key.
use std::fs;

use icloud_on_linux::app::App;

use crate::support::{Fixture, KEY};

pub fn check(app: &App, fixture: &Fixture) {
    let path = fixture.root.path().join("report.txt");
    app.window().write_report(&path).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    let root = fixture.root.path();
    let fixture_name = root.file_name().unwrap().to_str().unwrap();
    for private in [
        root.to_str().unwrap(),
        fixture_name,
        "Docs",
        "file7",
        "ending.bin",
        "window.bin",
        "Window uploads",
        "Kept uploads",
        KEY,
    ] {
        assert!(!text.contains(private), "{private} appears in the report:\n{text}");
    }
    for expected in [
        "State: Available",
        "Synced folders: 1",
        "Paused 1",
        "Language: en (showing en)",
        "GTK: 4.",
        "libadwaita: 1.",
        "Version: ",
    ] {
        assert!(
            text.contains(expected),
            "{expected} is missing from the report:\n{text}"
        );
    }
    println!("PASS: the diagnostic report described the app without the fixture's names, paths, or key.");
}
