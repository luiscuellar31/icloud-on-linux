//! The running app: the connection, the upload queue, and synced folders, which keep running for as long as the app
//! does, the window that shows them, and the tray icon. Choosing another language replaces the window and leaves the
//! parts running; closing the window can leave them running in the tray.
use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::rc::{Rc, Weak};
use std::time::Duration;

use adw::prelude::*;
use gtk::{gio, glib};
use icloud_core::autostart::Autostart;
use icloud_core::connection::Connection;
use icloud_core::keyring::{Keyring, Reply, Request};
use icloud_core::places::Places;
use icloud_core::session::Session;
use icloud_core::settings::Settings;
use icloud_core::sync::SyncedFolders;
use icloud_core::transfers::Transfers;
use icloud_core::{i18n, tr};

use crate::tray::{Item, Tray};
use crate::window::{Window, with};

/// How long a login start waits for a tray icon before it shows the window.
const TRAY_WAIT: Duration = Duration::from_secs(10);

/// What every window of the app shows and changes.
pub struct Parts {
    pub session: Rc<Session>,
    /// The upload queue, unless it could not be opened.
    pub transfers: Option<Rc<Transfers>>,
    /// The synced folders, unless their list could not be opened.
    pub sync: Option<Rc<SyncedFolders>>,
    pub settings: Settings,
    /// The language choice in use, which applies even when it could not be saved.
    pub language: Cell<&'static str>,
    /// The desktop keyring, when secret-tool is installed.
    pub keyring: RefCell<Option<Keyring>>,
    /// Where Start at login puts the autostart entry.
    pub autostart: RefCell<Autostart>,
    /// Where Add folder puts the iCloud Sync place.
    pub places: RefCell<Places>,
    /// Disconnect waits for uploads and sync to stop, because they use the unlocked connection.
    pub disconnect_pending: Cell<bool>,
    /// The app closes once uploads, sync, and the connection have stopped.
    pub closing: Cell<bool>,
    /// Sign-in opens once a renewal has disconnected.
    pub renew_pending: Cell<bool>,
    /// Save the key in the keyring once the key the user typed unlocks the connection.
    pub remember_key: Cell<bool>,
    pub told: Told,
}

/// What the app has already shown or told the user, so each new problem stands out once.
#[derive(Default)]
pub struct Told {
    /// The connection problem last shown.
    pub error: RefCell<String>,
    /// Synced folders waiting for a review.
    pub reviews: RefCell<BTreeSet<String>>,
    /// Uploads waiting for a decision, and a storage failure.
    pub uploads: RefCell<BTreeSet<String>>,
    /// The warning that the Apple session ends soon.
    pub expiry: Cell<bool>,
    /// That a closed window leaves the app running in the tray.
    pub background: Cell<bool>,
}

/// Call `tick` on `part` every `interval` for as long as it exists.
fn every<T: 'static>(interval: Duration, part: &Rc<T>, tick: fn(&T)) {
    let weak = Rc::downgrade(part);
    glib::timeout_add_local(interval, move || match weak.upgrade() {
        Some(part) => {
            tick(&part);
            glib::ControlFlow::Continue
        }
        None => glib::ControlFlow::Break,
    });
}

impl Parts {
    /// Open the upload queue and the synced folders of `session` and keep them all running.
    pub fn new(session: Rc<Session>) -> Rc<Parts> {
        let settings = Settings::new(session.paths().settings());
        let keyring = Keyring::default();
        let parts = Rc::new(Parts {
            sync: SyncedFolders::new(session.clone()).ok(),
            transfers: Transfers::new(session.clone()).ok(),
            language: Cell::new(settings.language()),
            settings,
            session,
            keyring: RefCell::new(keyring.available().then_some(keyring)),
            autostart: RefCell::new(Autostart::for_user()),
            places: RefCell::new(Places::for_user()),
            disconnect_pending: Cell::new(false),
            closing: Cell::new(false),
            renew_pending: Cell::new(false),
            remember_key: Cell::new(false),
            told: Told::default(),
        });
        every(Duration::from_millis(200), &parts.session, Session::tick);
        if let Some(transfers) = &parts.transfers {
            every(Duration::from_millis(100), transfers, Transfers::tick);
            let weak = Rc::downgrade(&parts);
            transfers.idle.connect(move || with(&weak, |parts| parts.idle()));
        }
        // Sync ticks also while disconnected, to keep the folder states current.
        if let Some(sync) = &parts.sync {
            every(Duration::from_secs(1), sync, SyncedFolders::tick);
            let weak = Rc::downgrade(&parts);
            sync.idle.connect(move || with(&weak, |parts| parts.idle()));
        }
        parts
    }

    /// Whether an Apple account is signed in on this computer.
    pub fn signed_in(&self) -> bool {
        std::fs::symlink_metadata(&self.session.paths().config).is_ok()
    }

    /// Whether the app connects with the key saved in the desktop keyring.
    pub fn unlocks_automatically(&self) -> bool {
        self.keyring.borrow().is_some() && self.settings.unlock_automatically()
    }

    /// Whether uploads or sync still use the unlocked connection.
    pub fn busy(&self) -> bool {
        self.transfers.as_ref().is_some_and(|transfers| transfers.busy())
            || self.sync.as_ref().is_some_and(|sync| sync.busy())
    }

    /// Pause uploads and sync, then disconnect once they have stopped.
    pub fn disconnect(&self) {
        self.disconnect_pending.set(true);
        if let Some(transfers) = &self.transfers {
            transfers.pause();
        }
        if let Some(sync) = &self.sync {
            sync.pause();
        }
        self.idle();
    }

    /// Uploads and sync both use the unlocked connection, so it closes only after both stop.
    fn idle(&self) {
        if self.disconnect_pending.get() && !self.busy() {
            self.disconnect_pending.set(false);
            self.session.stop();
        }
    }
}

/// What a keyring call is for.
enum Call {
    Lookup,
    Store,
    Clear,
}

/// The tray icon and the entries of its menu.
struct TrayMenu {
    tray: Rc<Tray>,
    show: Item,
    open: Item,
    sync: Option<Item>,
    quit: Item,
}

impl TrayMenu {
    fn new(connection: &gio::DBusConnection, app: &Weak<App>, sync: bool) -> Option<TrayMenu> {
        let tray = Tray::new(connection, "icloud-on-linux", "iCloud on Linux", "folder-remote").ok()?;
        let run = |action: fn(&Rc<App>)| {
            let app = app.clone();
            move || with(&app, action)
        };
        let show = tray.add_item("", run(|app| app.present()));
        let open = tray.add_item("", run(|app| app.window().open_folder()));
        let sync = sync.then(|| tray.add_item("", run(|app| app.sync_now())));
        tray.add_separator();
        let quit = tray.add_item("", run(|app| app.window().quit()));
        tray.connect_activate(run(|app| app.present()));
        let menu = TrayMenu {
            tray,
            show,
            open,
            sync,
            quit,
        };
        menu.label();
        Some(menu)
    }

    /// Name the entries in the current language.
    fn label(&self) {
        let mut entries = vec![(self.show, tr!("Show window")), (self.open, tr!("Open iCloud Drive"))];
        entries.extend(self.sync.map(|sync| (sync, tr!("Sync now"))));
        entries.push((self.quit, tr!("Quit")));
        for (item, label) in entries {
            self.tray.set_label(item, &label);
        }
    }
}

/// The running app: its parts, the window that shows them, and the tray icon.
pub struct App {
    application: adw::Application,
    pub parts: Rc<Parts>,
    window: RefCell<Rc<Window>>,
    tray: Option<TrayMenu>,
    /// The keyring call that runs, one at a time.
    keyring: RefCell<Option<(Call, Request)>>,
}

impl App {
    /// Open the parts of `session` and show them in a new window. With `background`, as at login, the app stays in
    /// the tray when it can connect with the key saved in the keyring.
    pub fn new(application: &adw::Application, session: Rc<Session>, background: bool) -> Rc<App> {
        let parts = Parts::new(session);
        let app = Rc::new_cyclic(|app: &Weak<App>| App {
            application: application.clone(),
            parts: parts.clone(),
            window: RefCell::new(Window::new(application, &parts, app.clone())),
            tray: application
                .dbus_connection()
                .and_then(|connection| TrayMenu::new(&connection, app, parts.sync.is_some())),
            keyring: RefCell::default(),
        });
        app.watch(&app.window());
        // The parts announce changes here once, whichever window shows them now.
        let render = || {
            let weak = Rc::downgrade(&app);
            move || with(&weak, |app| app.window().render())
        };
        parts.session.changed().connect(render());
        if let Some(transfers) = &parts.transfers {
            transfers.changed.connect(render());
        }
        if let Some(sync) = &parts.sync {
            sync.changed.connect(render());
        }
        // The end of the desktop session closes the app without asking.
        for signal in [nix::libc::SIGTERM, nix::libc::SIGINT] {
            let weak = Rc::downgrade(&app);
            glib_unix::unix_signal_add_local(signal, move || {
                with(&weak, |app| app.window().shut_down());
                glib::ControlFlow::Continue
            });
        }
        let connecting = app.unlock_automatically();
        if background && connecting && app.tray.is_some() {
            // The window opens if the keyring has no key or no tray icon appears.
            let weak = Rc::downgrade(&app);
            glib::timeout_add_local_once(TRAY_WAIT, move || {
                with(&weak, |app| {
                    if !app.tray_shown() {
                        app.present();
                    }
                })
            });
        } else {
            app.present();
        }
        // Rendered once the app holds the window, so a hidden one announces what needs the user.
        app.window().render();
        app
    }

    /// The window that shows the app now.
    pub fn window(&self) -> Rc<Window> {
        self.window.borrow().clone()
    }

    /// Show the window, as opening the app again or clicking the tray icon does.
    pub fn present(&self) {
        let window = self.window();
        // On Wayland, the token from the tray click lets the window come to the front.
        if let Some(token) = self.tray.as_ref().and_then(|menu| menu.tray.take_activation_token()) {
            window.window.set_startup_id(&token);
        }
        window.window.present();
    }

    /// Whether the desktop shows the tray icon, so a closed window can leave the app running there.
    pub fn tray_shown(&self) -> bool {
        self.tray.as_ref().is_some_and(|menu| menu.tray.shown())
    }

    /// Show the status in the tray: the tooltip, and the entries that can run now.
    pub(crate) fn show_in_tray(&self, status: &str, open: bool, sync_now: bool) {
        let Some(menu) = &self.tray else {
            return;
        };
        menu.tray.set_tooltip(status);
        menu.tray.set_enabled(menu.open, open);
        if let Some(sync) = menu.sync {
            menu.tray.set_enabled(sync, sync_now);
        }
    }

    /// Tell the user through a desktop notification while the window is hidden; a shown window already says it.
    /// Clicking the notification opens the window. A later notification with the same `id` replaces it.
    pub(crate) fn announce(&self, id: &str, title: &str, body: &str) {
        if self.window().window.is_visible() {
            return;
        }
        let notification = gio::Notification::new(title);
        notification.set_body(Some(body));
        self.application.send_notification(Some(id), &notification);
    }

    fn sync_now(&self) {
        if let Some(sync) = &self.parts.sync {
            sync.sync_now(None);
        }
    }

    /// Connect with the key saved in the keyring when the user turned that on; whether the keyring was asked.
    fn unlock_automatically(self: &Rc<Self>) -> bool {
        let keyring = self.parts.keyring.borrow().clone();
        match keyring {
            Some(keyring) if self.parts.unlocks_automatically() && self.parts.signed_in() => {
                self.call_keyring(Call::Lookup, keyring.lookup());
                true
            }
            _ => false,
        }
    }

    /// Save `key` in the keyring, so the app can connect without asking for it.
    pub(crate) fn store_key(self: &Rc<Self>, key: &str) {
        let keyring = self.parts.keyring.borrow().clone();
        if let Some(keyring) = keyring {
            self.call_keyring(Call::Store, keyring.store(key));
        }
    }

    pub(crate) fn clear_key(self: &Rc<Self>) {
        let keyring = self.parts.keyring.borrow().clone();
        if let Some(keyring) = keyring {
            self.call_keyring(Call::Clear, keyring.clear());
        }
    }

    /// Run `request` and wait for its reply beside the main loop. A newer call replaces one that has not answered.
    fn call_keyring(self: &Rc<Self>, call: Call, request: Request) {
        if self.keyring.replace(Some((call, request))).is_some() {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::timeout_add_local(Duration::from_millis(100), move || match weak.upgrade() {
            Some(app) if app.keyring_answered() => glib::ControlFlow::Break,
            Some(_) => glib::ControlFlow::Continue,
            None => glib::ControlFlow::Break,
        });
    }

    /// Act on the reply of the running keyring call, if it came; whether no call waits any longer.
    fn keyring_answered(self: &Rc<Self>) -> bool {
        let reply = match self.keyring.borrow_mut().as_mut() {
            Some((_, request)) => request.poll(),
            None => return true,
        };
        let Some(reply) = reply else {
            return false;
        };
        let Some((call, _)) = self.keyring.take() else {
            return true;
        };
        match (call, reply) {
            (Call::Lookup, Reply::Key(key)) => self.parts.session.start(&key),
            // Without a saved key, the window asks for it.
            (Call::Lookup, _) => self.present(),
            (Call::Store, Reply::Failed) => {
                let _ = self.parts.settings.set_unlock_automatically(false);
                self.window().key_not_saved();
            }
            (Call::Store | Call::Clear, _) => {}
        }
        true
    }

    /// Show the app's own text in the language for `choice` from now on. The window is built again in that language;
    /// the connection, uploads, and sync keep running, and their messages switch when they next change.
    pub(crate) fn choose_language(self: &Rc<Self>, choice: &'static str) {
        if self.parts.language.replace(choice) == choice {
            return;
        }
        // The choice still applies until the app closes when it cannot be saved.
        let _ = self.parts.settings.set_language(choice);
        i18n::apply(choice);
        if let Some(menu) = &self.tray {
            menu.label();
        }
        let old = self.window();
        let window = Window::new(&self.application, &self.parts, Rc::downgrade(self));
        window.take_over(&old);
        // The old window is the one shown until now, so this announces nothing.
        window.render();
        *self.window.borrow_mut() = window.clone();
        self.watch(&window);
        window.window.present();
        // The old window is still handling the menu item that chose the language.
        glib::idle_add_local_once(move || old.window.destroy());
    }

    /// Quit when the window that shows the app closes, so a late activation never shows a destroyed window.
    fn watch(self: &Rc<Self>, window: &Rc<Window>) {
        let weak = Rc::downgrade(self);
        window.window.connect_destroy(move |closed| {
            with(&weak, |app| {
                if &app.window().window == closed {
                    app.application.quit();
                }
            })
        });
    }
}
