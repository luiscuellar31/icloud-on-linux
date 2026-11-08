//! The main window: the connection status, the local key, opening iCloud Drive in the file manager, synced folders,
//! and uploads. It shows the app's parts, which keep running when a window in another language replaces it.
use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::time::Duration;

use adw::prelude::*;
use gtk::{gio, glib};
use icloud_core::connection::Connection;
use icloud_core::session::{Session, State};
use icloud_core::{i18n, settings, tr};

use crate::app::{App, Parts};
use crate::folders::Folders;
use crate::signin::SignIn;
use crate::uploads::Uploads;
use crate::view::{self, Facts, Tone};

const TONES: [Tone; 5] = [Tone::Ok, Tone::Busy, Tone::Warning, Tone::Error, Tone::Off];

pub struct Window {
    pub window: adw::ApplicationWindow,
    pub folders: Rc<Folders>,
    pub uploads: Rc<Uploads>,
    parts: Rc<Parts>,
    session: Rc<Session>,
    app: Weak<App>,
    dot: gtk::Label,
    status: gtk::Label,
    detail: gtk::Label,
    progress: gtk::ProgressBar,
    welcome: gtk::Box,
    sign_in_button: gtk::Button,
    key_panel: gtk::Box,
    key: adw::PasswordEntryRow,
    connect: gtk::Button,
    open: gtk::Button,
    cancel: gtk::Button,
    renew: gio::SimpleAction,
    language: gio::SimpleAction,
    disconnect: gio::SimpleAction,
    quit: gio::SimpleAction,
    sign_in: RefCell<Weak<SignIn>>,
}

/// Run `action` if the part still exists; closures keep only weak references so the window can be freed.
pub(crate) fn with<T>(part: &Weak<T>, action: impl FnOnce(&Rc<T>)) {
    if let Some(part) = part.upgrade() {
        action(&part);
    }
}

/// Whether a field holds text, read without copying it: the field may hold the local key.
fn typed(field: &impl IsA<gtk::Editable>) -> bool {
    match field.delegate().and_then(|text| text.downcast::<gtk::Text>().ok()) {
        Some(text) => text.text_length() > 0,
        None => !field.text().is_empty(),
    }
}

fn label(css: &[&str]) -> gtk::Label {
    let label = gtk::Label::builder().xalign(0.0).wrap(true).build();
    for class in css {
        label.add_css_class(class);
    }
    label
}

impl Window {
    /// A window in the current language that shows `parts`; `app` replaces it when another language is chosen.
    pub(crate) fn new(application: &adw::Application, parts: &Rc<Parts>, app: Weak<App>) -> Rc<Window> {
        let dot = gtk::Label::new(Some("\u{25cf}"));
        let status = label(&["heading"]);
        status.set_hexpand(true);
        let status_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        status_row.append(&dot);
        status_row.append(&status);
        let detail = label(&[]);
        let progress = gtk::ProgressBar::new();

        let key = adw::PasswordEntryRow::builder().title(tr!("Local key")).build();
        let keys = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .build();
        keys.append(&key);
        let hint = label(&["dim-label", "caption"]);
        hint.set_label(&tr!(
            "The key you chose when signing in. It is not your Apple password."
        ));
        let connect = gtk::Button::builder()
            .label(tr!("Connect"))
            .css_classes(["suggested-action", "pill"])
            .halign(gtk::Align::Start)
            .build();
        let welcome_title = label(&["title-3"]);
        welcome_title.set_label(&tr!("Welcome"));
        let welcome_text = label(&[]);
        welcome_text.set_label(&tr!(
            "Sign in with your Apple account to open iCloud Drive in your file manager and keep folders in sync on this computer."
        ));
        let sign_in_button = gtk::Button::builder()
            .label(tr!("Sign in to iCloud"))
            .css_classes(["suggested-action", "pill"])
            .halign(gtk::Align::Start)
            .build();
        let welcome = gtk::Box::new(gtk::Orientation::Vertical, 8);
        welcome.append(&welcome_title);
        welcome.append(&welcome_text);
        welcome.append(&sign_in_button);
        let key_panel = gtk::Box::new(gtk::Orientation::Vertical, 8);
        key_panel.append(&keys);
        key_panel.append(&hint);
        key_panel.append(&connect);

        let open = gtk::Button::builder()
            .label(tr!("Open iCloud Drive"))
            .css_classes(["suggested-action", "pill"])
            .build();
        let cancel = gtk::Button::builder()
            .label(tr!("Cancel"))
            .css_classes(["pill"])
            .build();
        let folders = Folders::new(parts);
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        actions.append(&open);
        actions.append(&folders.add);
        actions.append(&cancel);
        let uploads = Uploads::new(parts);

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(18)
            .margin_bottom(18)
            .margin_start(18)
            .margin_end(18)
            .build();
        for part in [
            status_row.upcast_ref::<gtk::Widget>(),
            detail.upcast_ref(),
            progress.upcast_ref(),
            welcome.upcast_ref(),
            key_panel.upcast_ref(),
            actions.upcast_ref(),
            folders.widget().upcast_ref(),
            uploads.widget().upcast_ref(),
        ] {
            content.append(part);
        }

        let menu = gio::Menu::new();
        let actions = gio::Menu::new();
        if parts.transfers.is_some() {
            actions.append(Some(&tr!("Upload files...")), Some("win.upload"));
        }
        actions.append(Some(&tr!("Renew Apple session...")), Some("win.renew"));
        menu.append_section(None, &actions);
        // Each language is shown by its own name, so it can be found in any language.
        let languages = gio::Menu::new();
        languages.append(Some(&tr!("Automatic")), Some("win.language::auto"));
        for (code, name) in i18n::LANGUAGES {
            languages.append(Some(name), Some(&format!("win.language::{code}")));
        }
        let language = gio::Menu::new();
        language.append_submenu(Some(&tr!("Language")), &languages);
        menu.append_section(None, &language);
        let connection = gio::Menu::new();
        connection.append(Some(&tr!("Disconnect")), Some("win.disconnect"));
        connection.append(Some(&tr!("Quit")), Some("win.quit"));
        menu.append_section(None, &connection);
        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&menu)
            .tooltip_text(tr!("Menu"))
            .build();
        let header = adw::HeaderBar::builder()
            .title_widget(&adw::WindowTitle::new("iCloud Drive", ""))
            .build();
        header.pack_end(&menu_button);
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&adw::Clamp::builder().maximum_size(560).child(&content).build()));

        let window = adw::ApplicationWindow::builder()
            .application(application)
            .title("iCloud on Linux")
            .icon_name("folder-remote")
            .default_width(560)
            .default_height(600)
            .content(&toolbar)
            .build();
        let renew = gio::SimpleAction::new("renew", None);
        window.add_action(&renew);
        let language = gio::SimpleAction::new_stateful(
            "language",
            Some(glib::VariantTy::STRING),
            &parts.language.get().to_variant(),
        );
        window.add_action(&language);
        let disconnect = gio::SimpleAction::new("disconnect", None);
        let quit = gio::SimpleAction::new("quit", None);
        window.add_action(&disconnect);
        window.add_action(&quit);
        window.add_action(&uploads.action);

        let this = Rc::new(Window {
            window,
            folders,
            uploads,
            parts: parts.clone(),
            session: parts.session.clone(),
            app,
            dot,
            status,
            detail,
            progress,
            welcome,
            sign_in_button,
            key_panel,
            key,
            connect,
            open,
            cancel,
            renew,
            language,
            disconnect,
            quit,
            sign_in: RefCell::new(Weak::new()),
        });
        this.connect_signals();
        this.render();
        this
    }

    fn connect_signals(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.key.connect_changed(move |_| with(&weak, |window| window.render()));
        let weak = Rc::downgrade(self);
        self.key
            .connect_entry_activated(move |_| with(&weak, |window| window.connect_session()));
        let weak = Rc::downgrade(self);
        self.connect
            .connect_clicked(move |_| with(&weak, |window| window.connect_session()));
        let weak = Rc::downgrade(self);
        self.open
            .connect_clicked(move |_| with(&weak, |window| window.open_folder()));
        let weak = Rc::downgrade(self);
        self.sign_in_button
            .connect_clicked(move |_| with(&weak, |window| window.sign_in()));
        let weak = Rc::downgrade(self);
        self.renew
            .connect_activate(move |_, _| with(&weak, |window| window.renew()));
        let app = self.app.clone();
        self.language.connect_change_state(move |_, state| {
            let chosen = state.and_then(|state| state.str());
            if let Some(choice) = settings::LANGUAGES.into_iter().find(|choice| Some(*choice) == chosen) {
                with(&app, |app| app.choose_language(choice));
            }
        });
        let weak = Rc::downgrade(self);
        self.cancel
            .connect_clicked(move |_| with(&weak, |window| window.disconnect_session()));
        let weak = Rc::downgrade(self);
        self.disconnect
            .connect_activate(move |_, _| with(&weak, |window| window.disconnect_session()));
        let weak = Rc::downgrade(self);
        self.quit
            .connect_activate(move |_, _| with(&weak, |window| window.window.close()));
        let weak = Rc::downgrade(self);
        self.window.connect_close_request(move |_| match weak.upgrade() {
            Some(window) => window.close_request(),
            None => glib::Propagation::Proceed,
        });
        let weak = Rc::downgrade(self);
        glib::timeout_add_local(Duration::from_millis(200), move || match weak.upgrade() {
            Some(window) => {
                // A cancelled sign-in ends outside the session, so closing checks on its own.
                if window.parts.closing.get() {
                    window.render();
                }
                if window.progress.is_visible() {
                    window.progress.pulse();
                }
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
    }

    /// Keep the size and the uploads settings of `old`, the window this one replaces.
    pub(crate) fn take_over(&self, old: &Window) {
        let (width, height) = old.window.default_size();
        self.window.set_default_size(width, height);
        self.window.set_maximized(old.window.is_maximized());
        self.uploads.take_over(&old.uploads);
    }

    pub(crate) fn render(&self) {
        let parts = &self.parts;
        let mount = self.session.current_mount();
        let facts = Facts {
            state: self.session.state(),
            message: self.session.message(),
            mounted: mount.is_some(),
            compatible: mount.as_ref().is_some_and(|mount| mount.compatible()),
            client: self.session.client_running(),
            checking: self.session.checking(),
            owns_mount: self.session.owns_mount(),
            reconnecting: self.session.reconnecting(),
            key_typed: typed(&self.key),
            signed_in: std::fs::symlink_metadata(&self.session.paths().config).is_ok(),
            renewing: parts.renew_pending.get(),
            uploads: self.uploads.status(),
            folders: self.folders.status(),
            disconnect_pending: parts.disconnect_pending.get(),
        };
        let screen = view::screen(&facts);
        self.status.set_label(&screen.status);
        for tone in TONES {
            self.dot.remove_css_class(tone.style());
        }
        self.dot.add_css_class(screen.tone.style());
        self.detail.set_label(screen.detail.as_deref().unwrap_or_default());
        self.detail.set_visible(screen.detail.is_some());
        self.progress.set_visible(screen.progress);
        self.welcome.set_visible(screen.welcome);
        self.key_panel.set_visible(screen.key_panel);
        self.connect.set_sensitive(screen.connect);
        self.open.set_visible(screen.open_visible);
        self.open.set_sensitive(screen.open);
        self.cancel.set_visible(screen.cancel);
        self.cancel.set_sensitive(screen.cancel_enabled);
        self.disconnect.set_enabled(screen.disconnect);
        self.folders.render(screen.open_visible, facts.signed_in);
        self.uploads.render();
        self.renew.set_enabled(screen.renew);
        // A window that is closing is not replaced.
        self.language.set_enabled(!parts.closing.get());
        if parts.renew_pending.get() {
            if self.session.state() == State::Error {
                parts.renew_pending.set(false);
            } else if screen.key_panel || screen.welcome {
                parts.renew_pending.set(false);
                self.sign_in();
            }
        }
        if parts.closing.get() && !self.session.busy() && !parts.disconnect_pending.get() && !self.signing_in() {
            if self.session.running() || mount.is_some() {
                // Disconnecting failed, for example because a file is open; the status explains it.
                parts.closing.set(false);
            } else {
                self.window.destroy();
            }
        }
    }

    fn sign_in(&self) {
        // A waiting reconnection would start a connection during sign-in; signing in replaces it.
        if self.session.reconnecting() {
            self.session.stop();
        }
        let session = self.session.clone();
        let dialog = SignIn::present(&self.window, self.session.clone(), move || {
            session.set_status(State::Disconnected, tr!("Signed in. Enter your local key to connect."));
        });
        *self.sign_in.borrow_mut() = Rc::downgrade(&dialog);
    }

    fn signing_in(&self) -> bool {
        self.sign_in.borrow().upgrade().is_some_and(|dialog| dialog.active())
    }

    /// Renew the Apple session; a connected app disconnects first and then opens sign-in.
    fn renew(self: &Rc<Self>) {
        if self.session.state() != State::Available {
            self.sign_in();
            return;
        }
        let dialog = adw::AlertDialog::builder()
            .heading(tr!("Renew Apple session"))
            .body(tr!(
                "Renewing disconnects iCloud Drive and pauses syncing until you connect again. Continue?"
            ))
            .close_response("cancel")
            .default_response("cancel")
            .build();
        dialog.add_responses(&[("cancel", &tr!("Cancel")), ("renew", &tr!("Disconnect and renew"))]);
        let weak = Rc::downgrade(self);
        dialog.choose(Some(&self.window), gio::Cancellable::NONE, move |response| {
            if response == "renew" {
                with(&weak, |window| {
                    window.parts.renew_pending.set(true);
                    window.disconnect_session();
                    window.render();
                });
            }
        });
    }

    fn connect_session(&self) {
        if !self.connect.is_sensitive() {
            return;
        }
        let key = self.key.text();
        self.key.set_text("");
        self.session.start(&key);
    }

    fn open_folder(self: &Rc<Self>) {
        let Some(mount) = self.session.current_mount() else {
            return;
        };
        if self.session.state() != State::Available || !mount.compatible() {
            return;
        }
        let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(&mount.path)));
        let weak = Rc::downgrade(self);
        launcher.launch(Some(&self.window), gio::Cancellable::NONE, move |result| {
            if result.is_err() {
                with(&weak, |window| {
                    window.session.set_status(
                        State::Error,
                        tr!("The file manager could not be opened. You can still disconnect safely."),
                    )
                });
            }
        });
    }

    /// Pause uploads and sync, then disconnect once they have stopped.
    fn disconnect_session(&self) {
        self.parts.disconnect();
        self.render();
    }

    fn active(&self) -> bool {
        self.signing_in() || self.session.running() || self.session.owns_mount() || self.parts.busy()
    }

    fn close_request(self: &Rc<Self>) -> glib::Propagation {
        if !self.active() {
            return glib::Propagation::Proceed;
        }
        if !self.parts.closing.get() {
            let dialog = adw::AlertDialog::builder()
                .heading(tr!("Close iCloud on Linux"))
                .body(tr!("Disconnect from iCloud Drive and close the app?"))
                .close_response("cancel")
                .default_response("cancel")
                .build();
            dialog.add_responses(&[("cancel", &tr!("Cancel")), ("close", &tr!("Disconnect and close"))]);
            dialog.set_response_appearance("close", adw::ResponseAppearance::Destructive);
            let weak = Rc::downgrade(self);
            dialog.choose(Some(&self.window), gio::Cancellable::NONE, move |response| {
                if response == "close" {
                    with(&weak, |window| window.shut_down());
                }
            });
        }
        glib::Propagation::Stop
    }

    /// Pause uploads and sync and disconnect if needed, then close; `render` finishes once everything has stopped.
    pub(crate) fn shut_down(&self) {
        let dialog = self.sign_in.borrow().upgrade();
        if let Some(dialog) = dialog {
            dialog.close();
        }
        if self.active() {
            self.parts.closing.set(true);
            self.disconnect_session();
        } else {
            self.window.destroy();
        }
    }
}
