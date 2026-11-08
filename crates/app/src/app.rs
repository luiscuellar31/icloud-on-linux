//! The running app: the connection, the upload queue, and synced folders, which keep running for as long as the app
//! does, and the window that shows them. Choosing another language replaces the window and leaves the parts running.
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use icloud_core::connection::Connection;
use icloud_core::i18n;
use icloud_core::places::Places;
use icloud_core::session::Session;
use icloud_core::settings::Settings;
use icloud_core::sync::SyncedFolders;
use icloud_core::transfers::Transfers;

use crate::window::{Window, with};

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
    /// Where Add folder puts the iCloud Sync place.
    pub places: RefCell<Places>,
    /// Disconnect waits for uploads and sync to stop, because they use the unlocked connection.
    pub disconnect_pending: Cell<bool>,
    /// The app closes once uploads, sync, and the connection have stopped.
    pub closing: Cell<bool>,
    /// Sign-in opens once a renewal has disconnected.
    pub renew_pending: Cell<bool>,
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
        let parts = Rc::new(Parts {
            sync: SyncedFolders::new(session.clone()).ok(),
            transfers: Transfers::new(session.clone()).ok(),
            language: Cell::new(settings.language()),
            settings,
            session,
            places: RefCell::new(Places::for_user()),
            disconnect_pending: Cell::new(false),
            closing: Cell::new(false),
            renew_pending: Cell::new(false),
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

/// The running app: its parts and the window that shows them.
pub struct App {
    application: adw::Application,
    pub parts: Rc<Parts>,
    window: RefCell<Rc<Window>>,
}

impl App {
    /// Open the parts of `session` and show them in a new window.
    pub fn new(application: &adw::Application, session: Rc<Session>) -> Rc<App> {
        let parts = Parts::new(session);
        let app = Rc::new_cyclic(|app: &Weak<App>| App {
            application: application.clone(),
            parts: parts.clone(),
            window: RefCell::new(Window::new(application, &parts, app.clone())),
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
        app.present();
        app
    }

    /// The window that shows the app now.
    pub fn window(&self) -> Rc<Window> {
        self.window.borrow().clone()
    }

    /// Show the window, as opening the app again does.
    pub fn present(&self) {
        self.window().window.present();
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
        let old = self.window();
        let window = Window::new(&self.application, &self.parts, Rc::downgrade(self));
        window.take_over(&old);
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
