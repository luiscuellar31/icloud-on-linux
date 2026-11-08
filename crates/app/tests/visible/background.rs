//! The background part of the visible check: Start at login writes and removes the autostart entry in temporary
//! folders, and while a tray icon is shown, closing the window keeps the app running and connected. The tray watcher
//! is a stand-in on the check's own session bus.
use std::fs;

use adw::prelude::*;
use gtk::gio;
use icloud_core::autostart::{Autostart, NAME};
use icloud_core::connection::Connection;
use icloud_core::session::State;
use icloud_on_linux::app::App;

use crate::support::Fixture;
use crate::ui::wait;

const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_XML: &str = r#"<node><interface name="org.kde.StatusNotifierWatcher">
  <method name="RegisterStatusNotifierItem"><arg name="service" type="s" direction="in"/></method>
</interface></node>"#;

/// A StatusNotifierWatcher that accepts every icon, as the tray of a desktop does.
struct Watcher {
    connection: gio::DBusConnection,
    registration: Option<gio::RegistrationId>,
    owner: Option<gio::OwnerId>,
}

impl Watcher {
    fn start(connection: &gio::DBusConnection) -> Watcher {
        let node = gio::DBusNodeInfo::for_xml(WATCHER_XML).unwrap();
        let info = node.lookup_interface(WATCHER).unwrap();
        let registration = connection
            .register_object("/StatusNotifierWatcher", &info)
            .method_call(|_, _, _, _, _, _, invocation| invocation.return_value(None))
            .build()
            .unwrap();
        let owner =
            gio::bus_own_name_on_connection(connection, WATCHER, gio::BusNameOwnerFlags::NONE, |_, _| {}, |_, _| {});
        Watcher {
            connection: connection.clone(),
            registration: Some(registration),
            owner: Some(owner),
        }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.take() {
            gio::bus_unown_name(owner);
        }
        if let Some(registration) = self.registration.take() {
            let _ = self.connection.unregister_object(registration);
        }
    }
}

pub fn check(app: &App, fixture: &Fixture) {
    start_at_login(app, fixture);
    close_to_tray(app);
}

fn start_at_login(app: &App, fixture: &Fixture) {
    let root = fixture.root.path();
    let autostart = Autostart {
        entry: root.join("config-home/autostart").join(NAME),
        legacy: root.join("config-home/autostart/icloud-drive.desktop"),
        launcher: root.join("bin/icloud-on-linux"),
        applications: vec![root.join("data-home/applications")],
    };
    fs::create_dir_all(&autostart.applications[0]).unwrap();
    fs::write(
        autostart.applications[0].join(NAME),
        "[Desktop Entry]\nType=Application\nName=iCloud on Linux\nExec=icloud-on-linux\n",
    )
    .unwrap();
    app.parts.autostart.replace(autostart.clone());
    // The window reads the entries when it renders, after any change it hears of.
    let refresh = || app.parts.session.changed().emit();
    refresh();
    let login = app.window().window.lookup_action("login").unwrap();
    assert!(
        login.is_enabled(),
        "Start at login is unavailable with an installed entry"
    );
    assert_eq!(login.state(), Some(false.to_variant()));
    login.activate(None);
    let text = fs::read_to_string(&autostart.entry).unwrap();
    assert!(text.contains("Exec=icloud-on-linux --background\n"), "{text}");
    assert_eq!(login.state(), Some(true.to_variant()));
    login.activate(None);
    assert!(!autostart.entry.exists());
    assert_eq!(login.state(), Some(false.to_variant()));

    // The Python app's entry runs the same launcher, so it counts as this app's and is removed too.
    let legacy = format!(
        "[Desktop Entry]\nType=Application\nExec=\"{}\" --background\n",
        autostart.launcher.display()
    );
    fs::create_dir_all(autostart.legacy.parent().unwrap()).unwrap();
    fs::write(&autostart.legacy, legacy).unwrap();
    refresh();
    assert_eq!(login.state(), Some(true.to_variant()));
    login.activate(None);
    assert!(!autostart.legacy.exists() && !autostart.entry.exists());
    println!("PASS: Start at login wrote the autostart entry with --background and removed it, and the Python one.");
}

fn close_to_tray(app: &App) {
    let window = app.window();
    let connection = window
        .window
        .application()
        .and_then(|application| application.dbus_connection())
        .expect("the check runs on a session bus");
    let _watcher = Watcher::start(&connection);
    wait(10, "the tray icon is shown", || app.tray_shown());
    window.window.close();
    wait(10, "the window hides", || !window.window.is_visible());
    assert!(
        window.window.application().is_some(),
        "closing the window ended the app"
    );
    let session = &app.parts.session;
    assert_eq!(session.state(), State::Available, "{}", session.message());
    assert!(session.current_mount().is_some());
    app.present();
    wait(30, "the window is shown again", || window.window.is_mapped());
    println!("PASS: with a tray icon, closing the window kept the app running and connected.");
}
