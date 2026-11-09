//! Finding and waiting for widgets in the window under test.
use std::time::{Duration, Instant};

use adw::prelude::*;
use gtk::glib;
use icloud_core::session::{Session, State};
use icloud_on_linux::window::Window;

use crate::support;

/// Run the main loop until `done` holds, failing after `seconds`.
pub fn wait(seconds: u64, what: &str, done: impl Fn() -> bool) {
    let context = glib::MainContext::default();
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        while context.iteration(false) {}
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Let a dialog that just appeared finish opening: libadwaita 1.5 ignores a close that comes before its opening
/// animation starts, sooner than anyone can click.
pub fn let_open(dialog: &str) {
    let shown = Instant::now();
    wait(5, &format!("{dialog} finishes opening"), || {
        shown.elapsed() > Duration::from_millis(500)
    });
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
pub fn find<T: IsA<gtk::Widget>>(window: &Window, matches: impl Fn(&T) -> bool) -> Option<T> {
    widgets(window.window.upcast_ref())
        .into_iter()
        .filter_map(|widget| widget.downcast::<T>().ok())
        .find(|widget| widget.is_drawable() && matches(widget))
}

pub fn button(window: &Window, label: &str) -> gtk::Button {
    find(window, |button: &gtk::Button| button.label().as_deref() == Some(label))
        .unwrap_or_else(|| panic!("the {label} button is not shown"))
}

pub fn shows(window: &Window, text: &str) -> bool {
    find(window, |label: &gtk::Label| label.label() == text).is_some()
}

/// Type the fixture key, select Connect, and check that the window shows the connection without keeping the key.
pub fn connect(window: &Window, session: &Session) {
    let key = find(window, |row: &adw::PasswordEntryRow| row.title() == "Local key")
        .expect("the Local key field is not shown");
    key.set_text(support::KEY);
    button(window, "Connect").emit_clicked();
    wait(60, "the connection finishes", || {
        matches!(session.state(), State::Available | State::Error)
    });
    assert_eq!(session.state(), State::Available, "{}", session.message());
    assert!(key.text().is_empty(), "the window kept the local key after connecting");
    assert!(shows(window, "Connected"), "the window does not say it is connected");
}
