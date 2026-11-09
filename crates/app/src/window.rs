//! The main window: the connection status, the local key, opening iCloud Drive in the file manager, synced folders,
//! and uploads. It shows the app's parts, which keep running when a window in another language replaces it, and while
//! the window is closed to the tray.
use std::cell::RefCell;
use std::path::Path;
use std::rc::{Rc, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{env, fs, io};

use adw::prelude::*;
use gtk::{gio, glib};
use icloud_core::connection::Connection;
use icloud_core::diagnostics::{self, System};
use icloud_core::session::{Session, State};
use icloud_core::sync::Review;
use icloud_core::uploads::State as UploadState;
use icloud_core::{i18n, paths, profile, settings, tr};

use crate::app::{App, Parts, SignOut};
use crate::folders::Folders;
use crate::signin::SignIn;
use crate::uploads::Uploads;
use crate::view::{self, AccountData, Facts, Tone};

pub struct Window {
    pub window: adw::ApplicationWindow,
    pub folders: Rc<Folders>,
    pub uploads: Rc<Uploads>,
    parts: Rc<Parts>,
    session: Rc<Session>,
    app: Weak<App>,
    status_row: gtk::Box,
    dot: gtk::Label,
    status: gtk::Label,
    detail: gtk::Label,
    progress: gtk::ProgressBar,
    /// The warning that the Apple session ends soon, with Renew.
    expiry: adw::Banner,
    welcome: adw::StatusPage,
    sign_in_button: gtk::Button,
    key_panel: gtk::Box,
    key: adw::PasswordEntryRow,
    /// Unlock automatically, beside the key; the menu item shares its action.
    unlock_row: adw::SwitchRow,
    connect: gtk::Button,
    open: gtk::Button,
    cancel: gtk::Button,
    renew: gio::SimpleAction,
    change_account: gio::SimpleAction,
    sign_out: gio::SimpleAction,
    login: gio::SimpleAction,
    unlock: gio::SimpleAction,
    language: gio::SimpleAction,
    report: gio::SimpleAction,
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

/// The current time in seconds since 1970, as `settings.ini` records sign-ins.
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.as_secs() as i64)
}

fn label(css: &[&str]) -> gtk::Label {
    let label = gtk::Label::builder().xalign(0.0).wrap(true).build();
    for class in css {
        label.add_css_class(class);
    }
    label
}

impl Window {
    /// A window in the current language that shows `parts`; `app` renders it once it holds the window, and replaces
    /// it when another language is chosen.
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
        let unlock_row = adw::SwitchRow::builder()
            .title(tr!("Unlock automatically on this computer"))
            .subtitle(if parts.keyring.borrow().is_some() {
                tr!("Saves the local key in your desktop keyring, so your Linux login protects your iCloud Drive instead.")
            } else {
                tr!("Install secret-tool (libsecret-tools) to save the key in your desktop keyring.")
            })
            .use_markup(false)
            .build();
        let keys = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .build();
        keys.append(&key);
        keys.append(&unlock_row);
        let hint = label(&["dim-label", "caption"]);
        hint.set_label(&tr!(
            "The key you chose when signing in. It is not your Apple password."
        ));
        let connect = gtk::Button::builder()
            .label(tr!("Connect"))
            .css_classes(["suggested-action", "pill"])
            .halign(gtk::Align::Start)
            .build();
        let sign_in_button = gtk::Button::builder()
            .label(tr!("Sign in to iCloud"))
            .css_classes(["suggested-action", "pill"])
            .halign(gtk::Align::Center)
            .build();
        // The description is markup, so the translated text is escaped.
        let welcome = adw::StatusPage::builder()
            .icon_name("folder-remote-symbolic")
            .title(tr!("Welcome"))
            .description(glib::markup_escape_text(&tr!(
                "Sign in with your Apple account to open iCloud Drive in your file manager and keep folders in sync on this computer."
            )))
            .child(&sign_in_button)
            .vexpand(true)
            .build();
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
        actions.append(Some(&tr!("Change Apple account...")), Some("win.change-account"));
        actions.append(Some(&tr!("Sign out of this computer...")), Some("win.sign-out"));
        menu.append_section(None, &actions);
        // Each language is shown by its own name, so it can be found in any language.
        let languages = gio::Menu::new();
        languages.append(Some(&tr!("Automatic")), Some("win.language::auto"));
        for (code, name) in i18n::LANGUAGES {
            languages.append(Some(name), Some(&format!("win.language::{code}")));
        }
        let preferences = gio::Menu::new();
        preferences.append(Some(&tr!("Start at login")), Some("win.login"));
        preferences.append(Some(&tr!("Unlock automatically on this computer")), Some("win.unlock"));
        preferences.append_submenu(Some(&tr!("Language")), &languages);
        preferences.append(Some(&tr!("Save diagnostic report...")), Some("win.report"));
        menu.append_section(None, &preferences);
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
        let expiry = adw::Banner::builder()
            .button_label(tr!("Renew"))
            .action_name("win.renew")
            .use_markup(false)
            .build();
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.add_top_bar(&expiry);
        // The content scrolls, so a long list of folders or uploads never makes the window taller than the screen.
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&adw::Clamp::builder().maximum_size(560).child(&content).build())
            .build();
        toolbar.set_content(Some(&scroller));

        let window = adw::ApplicationWindow::builder()
            .application(application)
            .title("iCloud on Linux")
            .icon_name("folder-remote")
            .default_width(560)
            .default_height(600)
            .content(&toolbar)
            .build();
        let renew = gio::SimpleAction::new("renew", None);
        let change_account = gio::SimpleAction::new("change-account", None);
        let sign_out = gio::SimpleAction::new("sign-out", None);
        for action in [&renew, &change_account, &sign_out] {
            window.add_action(action);
        }
        // The menu items and the unlock switch show and change these on and off states.
        let login = gio::SimpleAction::new_stateful("login", None, &false.to_variant());
        let unlock = gio::SimpleAction::new_stateful("unlock", None, &false.to_variant());
        window.add_action(&login);
        window.add_action(&unlock);
        let language = gio::SimpleAction::new_stateful(
            "language",
            Some(glib::VariantTy::STRING),
            &parts.language.get().to_variant(),
        );
        window.add_action(&language);
        let report = gio::SimpleAction::new("report", None);
        let disconnect = gio::SimpleAction::new("disconnect", None);
        let quit = gio::SimpleAction::new("quit", None);
        for action in [&report, &disconnect, &quit] {
            window.add_action(action);
        }
        window.add_action(&uploads.action);

        let this = Rc::new(Window {
            window,
            folders,
            uploads,
            parts: parts.clone(),
            session: parts.session.clone(),
            app,
            status_row,
            dot,
            status,
            detail,
            progress,
            expiry,
            welcome,
            sign_in_button,
            key_panel,
            key,
            unlock_row,
            connect,
            open,
            cancel,
            renew,
            change_account,
            sign_out,
            login,
            unlock,
            language,
            report,
            disconnect,
            quit,
            sign_in: RefCell::new(Weak::new()),
        });
        this.connect_signals();
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
        let weak = Rc::downgrade(self);
        self.change_account
            .connect_activate(move |_, _| with(&weak, |window| window.confirm_sign_out(true)));
        let weak = Rc::downgrade(self);
        self.sign_out
            .connect_activate(move |_, _| with(&weak, |window| window.confirm_sign_out(false)));
        let weak = Rc::downgrade(self);
        self.login.connect_change_state(move |_, state| {
            let enabled = state.and_then(|state| state.get::<bool>());
            with(&weak, |window| window.start_at_login(enabled == Some(true)));
        });
        let weak = Rc::downgrade(self);
        self.unlock.connect_change_state(move |_, state| {
            let enabled = state.and_then(|state| state.get::<bool>());
            with(&weak, |window| window.set_auto_unlock(enabled == Some(true)));
        });
        let weak = Rc::downgrade(self);
        self.unlock_row.connect_active_notify(move |row| {
            with(&weak, |window| {
                // Rendering sets the switch to the saved choice; only a change by the user asks for another.
                if row.is_active() != window.parts.unlocks_automatically() {
                    window.unlock.change_state(&row.is_active().to_variant());
                }
            })
        });
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
        self.report
            .connect_activate(move |_, _| with(&weak, |window| window.save_report()));
        let weak = Rc::downgrade(self);
        self.disconnect
            .connect_activate(move |_, _| with(&weak, |window| window.disconnect_session()));
        let weak = Rc::downgrade(self);
        self.quit
            .connect_activate(move |_, _| with(&weak, |window| window.quit()));
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
            signed_in: parts.signed_in(),
            renewing: parts.renew_pending.get(),
            uploads: self.uploads.status(),
            folders: self.folders.status(),
            disconnect_pending: parts.disconnect_pending.get(),
            queued: parts
                .transfers
                .as_ref()
                .is_some_and(|transfers| !transfers.items().is_empty()),
            signing_out: parts.sign_out.get().is_some(),
        };
        let screen = view::screen(&facts);
        self.status_row.set_visible(screen.show_status);
        self.status.set_label(&screen.status);
        for tone in Tone::ALL {
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
        let review = self.folders.render(screen.open_visible, facts.signed_in);
        let decision = self.uploads.render();
        self.renew.set_enabled(screen.renew);
        self.change_account.set_enabled(screen.change_account);
        self.sign_out.set_enabled(screen.sign_out);
        // A window that is closing is not replaced.
        self.language.set_enabled(!parts.closing.get());
        let expiry = facts
            .signed_in
            .then(|| view::expiry(parts.settings.apple_signed_in_at(), now()))
            .flatten();
        self.expiry.set_title(expiry.as_deref().unwrap_or_default());
        self.expiry.set_revealed(expiry.is_some());
        self.render_toggles();
        self.remember_typed_key(facts.state);
        self.tell(&facts, review, decision, expiry.as_deref());
        let sync_now = parts.sync.as_ref().is_some_and(|sync| {
            !sync.folders().is_empty() && self.session.unlocked() && !sync.busy() && !parts.disconnect_pending.get()
        });
        with(&self.app, |app| app.show_in_tray(&screen.status, screen.open, sync_now));
        if parts.renew_pending.get() {
            if self.session.state() == State::Error {
                parts.renew_pending.set(false);
            } else if screen.key_panel || screen.welcome {
                parts.renew_pending.set(false);
                self.sign_in();
            }
        }
        if let Some(sign_out) = parts.sign_out.get() {
            let stopped = !self.session.busy()
                && mount.is_none()
                && !facts.client
                && !parts.busy()
                && !parts.disconnect_pending.get();
            if stopped {
                parts.sign_out.set(None);
                // Removing the data changes the parts, which render again, so it waits for this render to end.
                with(&self.app, |app| {
                    let weak = Rc::downgrade(app);
                    glib::idle_add_local_once(move || with(&weak, |app| app.window().forget(sign_out)));
                });
            } else if facts.state == State::Error && !parts.disconnect_pending.get() {
                // Disconnecting failed; the status explains why, and nothing was removed.
                parts.sign_out.set(None);
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

    /// Show the state of Start at login and of Unlock automatically, and whether they can change.
    fn render_toggles(&self) {
        {
            let autostart = self.parts.autostart.borrow();
            // A source checkout has no installed entry for the login session to start.
            self.login
                .set_enabled(autostart.enabled() || autostart.installed().is_some());
            self.login.set_state(&autostart.enabled().to_variant());
        }
        let unlocks = self.parts.unlocks_automatically();
        self.unlock.set_enabled(self.parts.keyring.borrow().is_some());
        self.unlock.set_state(&unlocks.to_variant());
        // The switch returns to the saved choice when its confirmation is declined.
        self.unlock_row.set_sensitive(self.unlock.is_enabled());
        self.unlock_row.set_active(unlocks);
    }

    /// Save the key in the keyring once the key the user typed unlocks the connection, so a mistyped one never is.
    fn remember_typed_key(&self, state: State) {
        let parts = &self.parts;
        if parts.remember_key.get()
            && let Some(key) = self.session.unlocked_key()
        {
            parts.remember_key.set(false);
            with(&self.app, |app| app.store_key(&key));
        }
        if state != State::Connecting {
            parts.remember_key.set(false);
        }
    }

    /// Announce what newly needs the user while the window is hidden: a connection problem, a folder waiting for a
    /// review, an upload waiting for a decision, and the end of the Apple session.
    fn tell(&self, facts: &Facts, review: Option<String>, decision: bool, expiry: Option<&str>) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        let told = &self.parts.told;
        let error = facts.state == State::Error && !facts.reconnecting;
        if error && *told.error.borrow() != facts.message {
            app.announce("attention", &tr!("iCloud Drive needs attention"), &facts.message);
        }
        *told.error.borrow_mut() = match facts.state {
            State::Error => facts.message.clone(),
            _ => String::new(),
        };
        if let Some(review) = review {
            app.announce("review", &tr!("A synced folder needs your review"), &review);
        }
        if decision && let Some(transfers) = &self.parts.transfers {
            app.announce("uploads", &tr!("Uploads need your decision"), &transfers.message());
        }
        if let Some(expiry) = expiry
            && !told.expiry.replace(true)
        {
            app.announce("expiry", &tr!("Renew your Apple session"), expiry);
        }
    }

    fn sign_in(&self) {
        // A waiting reconnection would start a connection during sign-in; signing in replaces it.
        if self.session.reconnecting() {
            self.session.stop();
        }
        let parts = self.parts.clone();
        let dialog = SignIn::present(&self.window, self.session.clone(), move || {
            // Apple sessions last about 30 days from now; the window warns before they end.
            let _ = parts.settings.set_apple_signed_in_at(now());
            parts.told.expiry.set(false);
            parts
                .session
                .set_status(State::Disconnected, tr!("Signed in. Enter your local key to connect."));
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

    /// Ask before signing out of this computer, listing what goes away; with `change`, sign-in opens afterwards for
    /// another Apple account.
    fn confirm_sign_out(self: &Rc<Self>, change: bool) {
        let parts = &self.parts;
        let folders = parts.sync.as_ref().map(|sync| sync.folders()).unwrap_or_default();
        let pending_uploads = parts.transfers.as_ref().map_or(0, |transfers| {
            let done = [UploadState::Completed, UploadState::Skipped];
            transfers
                .items()
                .iter()
                .filter(|item| !done.contains(&item.state))
                .count()
        });
        let data = AccountData {
            key_saved: parts.unlocks_automatically(),
            pending_uploads,
            folders: folders.len(),
            cache: profile::folder_size(&self.session.paths().cache.join("vfs")),
        };
        let (heading, accept) = if change {
            (tr!("Change Apple account"), tr!("Sign out and continue"))
        } else {
            (tr!("Sign out of this computer"), tr!("Sign out"))
        };
        let dialog = adw::AlertDialog::builder()
            .heading(heading)
            .body(view::sign_out_text(&data, change))
            .close_response("cancel")
            .default_response("cancel")
            .build();
        dialog.add_responses(&[("cancel", &tr!("Cancel")), ("sign-out", &accept)]);
        dialog.set_response_appearance("sign-out", adw::ResponseAppearance::Destructive);
        let trash = (!folders.is_empty()).then(|| {
            let size = folders.iter().map(|folder| profile::folder_size(&folder.local)).sum();
            gtk::CheckButton::with_label(&tr!(
                "Also move the local copies in iCloud Sync to the Trash ({size})",
                size = glib::format_size(size)
            ))
        });
        dialog.set_extra_child(trash.as_ref());
        let weak = Rc::downgrade(self);
        dialog.choose(Some(&self.window), gio::Cancellable::NONE, move |response| {
            if response != "sign-out" {
                return;
            }
            with(&weak, |window| {
                let trash = trash.as_ref().is_some_and(|check| check.is_active());
                window.parts.sign_out.set(Some(SignOut { trash, change }));
                // `render` removes the data once uploads, sync, and the connection have stopped.
                let session = &window.session;
                if window.active() || session.current_mount().is_some() || session.reconnecting() {
                    window.disconnect_session();
                }
                window.render();
            })
        });
    }

    /// Remove this computer's account data, now that uploads, sync, and the connection have stopped. Nothing is
    /// removed from iCloud.
    pub(crate) fn forget(self: &Rc<Self>, sign_out: SignOut) {
        let parts = &self.parts;
        let paths = self.session.paths();
        let mut problems = Vec::new();
        if sign_out.trash
            && let Some(sync) = &parts.sync
        {
            let trash = parts.trash.borrow();
            for folder in sync.folders() {
                // The Trash keeps local changes that had not synced yet recoverable.
                let copy = fs::symlink_metadata(&folder.local).is_ok_and(|info| info.is_dir());
                if copy && !trash(&folder.local) {
                    problems.push(tr!("local copies"));
                }
            }
        }
        if profile::forget(paths).is_err() {
            problems.push(tr!("the saved session"));
        }
        let uploads = match &parts.transfers {
            Some(transfers) => transfers.forget().is_ok(),
            None => profile::delete_path(&paths.cache.join("uploads")).is_ok(),
        };
        if !uploads {
            problems.push(tr!("pending uploads"));
        }
        let sync = match &parts.sync {
            Some(sync) => sync.forget().is_ok(),
            None => profile::delete_path(&paths.cache.join("sync")).is_ok(),
        };
        if !sync {
            problems.push(tr!("sync state"));
        }
        with(&self.app, |app| app.clear_key());
        let _ = parts.settings.forget_account();
        if sign_out.trash
            && let Some(sync) = &parts.sync
            && fs::remove_dir(sync.local_root()).is_ok()
        {
            // Other files left in iCloud Sync would keep the folder and its sidebar place.
            let _ = parts.places.borrow().remove(sync.local_root());
        }
        parts.told.expiry.set(false);
        self.session.set_status(
            State::Disconnected,
            tr!("Signed out of this computer. Nothing was removed from iCloud."),
        );
        problems.dedup();
        if problems.is_empty() {
            if sign_out.change {
                self.sign_in();
            }
            return;
        }
        let dialog = adw::AlertDialog::builder()
            .heading(tr!("Sign out of this computer"))
            .body(tr!(
                "Some data could not be removed: {items}. Check the permissions of your home folder and try again.",
                items = problems.join(", ")
            ))
            .build();
        dialog.add_responses(&[("close", &tr!("Close"))]);
        let weak = Rc::downgrade(self);
        dialog.choose(Some(&self.window), gio::Cancellable::NONE, move |_| {
            if sign_out.change {
                with(&weak, |window| window.sign_in());
            }
        });
    }

    /// Ask where to save the diagnostic report, then save it there and say whether that worked.
    fn save_report(self: &Rc<Self>) {
        let name = match glib::DateTime::now_local().and_then(|now| now.format("%Y-%m-%d")) {
            Ok(date) => format!("icloud-on-linux-report-{date}.txt"),
            Err(_) => "icloud-on-linux-report.txt".to_owned(),
        };
        let text = gtk::FileFilter::new();
        text.set_name(Some(&tr!("Text files (*.txt)")));
        text.add_pattern("*.txt");
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&text);
        let dialog = gtk::FileDialog::builder()
            .title(tr!("Save diagnostic report"))
            .initial_folder(&gio::File::for_path(paths::home()))
            .initial_name(name)
            .filters(&filters)
            .default_filter(&text)
            .modal(true)
            .build();
        let weak = Rc::downgrade(self);
        dialog.save(Some(&self.window), gio::Cancellable::NONE, move |result| {
            // Cancelling the dialog, or a place without a local path, saves nothing.
            let Some(path) = result.ok().and_then(|file| file.path()) else {
                return;
            };
            with(&weak, |window| {
                let body = match window.write_report(&path) {
                    Ok(()) => tr!(
                        "Report saved. It has no keys, tokens, file contents, or file and folder names, so you can share it."
                    ),
                    Err(_) => tr!("The report could not be saved. Choose another folder."),
                };
                let note = adw::AlertDialog::builder()
                    .heading(tr!("Save diagnostic report"))
                    .body(body)
                    .build();
                note.add_responses(&[("close", &tr!("Close"))]);
                note.present(Some(&window.window));
            })
        });
    }

    /// Save the diagnostic report to `path`: versions, settings, and states, without keys, tokens, file contents, or
    /// file and folder names.
    pub fn write_report(&self, path: &Path) -> io::Result<()> {
        let (parts, session) = (&self.parts, &self.session);
        let words = |on: bool, yes: &str, no: &str| if on { yes } else { no }.to_owned();
        let tray = self.app.upgrade().is_some_and(|app| app.tray_shown());
        let age = parts.settings.apple_signed_in_at().map_or_else(
            || "unknown".to_owned(),
            |at| format!("{:.0} days", (now() - at) as f64 / 86_400.0),
        );
        let details = [
            ("Version", env!("CARGO_PKG_VERSION").to_owned()),
            (
                "GTK",
                format!(
                    "{}.{}.{}",
                    gtk::major_version(),
                    gtk::minor_version(),
                    gtk::micro_version()
                ),
            ),
            (
                "libadwaita",
                format!(
                    "{}.{}.{}",
                    adw::major_version(),
                    adw::minor_version(),
                    adw::micro_version()
                ),
            ),
            (
                "Language",
                format!("{} (showing {})", parts.language.get(), i18n::current()),
            ),
            ("Tray icon", words(tray, "available", "not available")),
            ("Start at login", words(parts.autostart.borrow().enabled(), "on", "off")),
            (
                "Unlock automatically",
                words(parts.unlocks_automatically(), "on", "off"),
            ),
            (
                "Keyring tool",
                words(parts.keyring.borrow().is_some(), "available", "not installed"),
            ),
            ("Apple session age", age),
        ]
        .map(|(label, value)| (label.to_owned(), value));
        let connection = diagnostics::Connection {
            saved_session: parts.signed_in(),
            state: session.state().name(),
            message: session.message(),
            owns_mount: session.owns_mount(),
            reconnecting: session.reconnecting(),
        };
        let folders = parts.sync.as_ref().map(|sync| diagnostics::Folders {
            folders: sync
                .folders()
                .iter()
                .map(|folder| diagnostics::Folder {
                    state: folder.state.name(),
                    items: folder.items(),
                    pending: folder.pending,
                    conflicts: folder.conflicts,
                    review: folder.attention.map(Review::name),
                    message: folder.message.clone(),
                })
                .collect(),
            message: sync.message(),
        });
        let uploads = parts.transfers.as_ref().map(|transfers| diagnostics::Uploads {
            states: transfers.items().iter().map(|item| item.state.name()).collect(),
            storage_problem: transfers.storage_error(),
            messages: vec![transfers.message(), transfers.recovery_warning()],
        });
        let system = System::detect(&env::current_exe().unwrap_or_default());
        let home = paths::home();
        fs::write(
            path,
            diagnostics::report(
                &system,
                &details,
                &connection,
                folders.as_ref(),
                uploads.as_ref(),
                &home.to_string_lossy(),
            ),
        )
    }

    fn connect_session(&self) {
        if !self.connect.is_sensitive() {
            return;
        }
        let key = self.key.text();
        self.key.set_text("");
        self.parts.remember_key.set(self.parts.unlocks_automatically());
        self.session.start(&key);
    }

    pub(crate) fn open_folder(self: &Rc<Self>) {
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

    /// Closing the window leaves the app running in the tray while it works or is signed in. Without a tray icon,
    /// the window asks whether to keep syncing while the app works, and closes the app otherwise.
    fn close_request(self: &Rc<Self>) -> glib::Propagation {
        if self.parts.closing.get() {
            return glib::Propagation::Stop;
        }
        let tray = self.app.upgrade().is_some_and(|app| app.tray_shown());
        if tray && (self.active() || self.parts.signed_in()) {
            self.hide_to_tray();
        } else if self.active() {
            self.ask_keep_syncing();
        } else {
            return glib::Propagation::Proceed;
        }
        glib::Propagation::Stop
    }

    /// Hide the window and keep running in the tray; the first time, say how to come back.
    fn hide_to_tray(&self) {
        self.window.set_visible(false);
        if !self.parts.told.background.replace(true) {
            with(&self.app, |app| {
                app.announce(
                    "background",
                    &tr!("iCloud on Linux is still running"),
                    &tr!("Syncing continues in the background. Use its icon to open the window or quit."),
                )
            });
        }
    }

    fn ask_keep_syncing(self: &Rc<Self>) {
        let dialog = adw::AlertDialog::builder()
            .heading(tr!("Keep syncing?"))
            .body(tr!(
                "iCloud on Linux can keep syncing after you close this window. To see it again, open iCloud on Linux from your applications."
            ))
            .close_response("cancel")
            .default_response("keep")
            .build();
        dialog.add_responses(&[
            ("cancel", &tr!("Cancel")),
            ("quit", &tr!("Quit")),
            ("keep", &tr!("Keep syncing")),
        ]);
        dialog.set_response_appearance("keep", adw::ResponseAppearance::Suggested);
        let weak = Rc::downgrade(self);
        dialog.choose(Some(&self.window), gio::Cancellable::NONE, move |response| {
            with(&weak, |window| match response.as_str() {
                "keep" => window.window.set_visible(false),
                "quit" => window.shut_down(),
                _ => {}
            })
        });
    }

    /// Quit the app, after the user confirms while it works.
    pub(crate) fn quit(self: &Rc<Self>) {
        if !self.active() {
            self.shut_down();
            return;
        }
        // Quit from the tray asks too, so the window opens for the question.
        with(&self.app, |app| app.present());
        let dialog = adw::AlertDialog::builder()
            .heading(tr!("Quit iCloud on Linux"))
            .body(tr!(
                "Quitting pauses uploads, stops syncing, and disconnects iCloud Drive. Quit now?"
            ))
            .close_response("cancel")
            .default_response("cancel")
            .build();
        dialog.add_responses(&[("cancel", &tr!("Cancel")), ("quit", &tr!("Quit"))]);
        dialog.set_response_appearance("quit", adw::ResponseAppearance::Destructive);
        let weak = Rc::downgrade(self);
        dialog.choose(Some(&self.window), gio::Cancellable::NONE, move |response| {
            if response == "quit" {
                with(&weak, |window| window.shut_down());
            }
        });
    }

    fn start_at_login(&self, enabled: bool) {
        let autostart = self.parts.autostart.borrow();
        // When this fails, the entry stays as it was, and so does the menu item.
        let _ = if enabled {
            autostart.enable()
        } else {
            autostart.disable()
        };
        self.login.set_state(&autostart.enabled().to_variant());
    }

    /// Turn automatic unlock on once the user confirms, saving the key of an unlocked connection, or turn it off and
    /// remove the saved key.
    fn set_auto_unlock(self: &Rc<Self>, enabled: bool) {
        let parts = &self.parts;
        if enabled && !parts.unlocks_automatically() {
            let dialog = adw::AlertDialog::builder()
                .heading(tr!("Unlock automatically"))
                .body(tr!(
                    "Your local key will be saved in the desktop keyring. Anyone who can use your Linux account will be able to open your iCloud Drive without typing it. Turn this on?"
                ))
                .close_response("cancel")
                .default_response("cancel")
                .build();
            dialog.add_responses(&[("cancel", &tr!("Cancel")), ("on", &tr!("Turn on"))]);
            let weak = Rc::downgrade(self);
            dialog.choose(Some(&self.window), gio::Cancellable::NONE, move |response| {
                with(&weak, |window| {
                    if response == "on"
                        && window.parts.settings.set_unlock_automatically(true).is_ok()
                        && let Some(key) = window.session.unlocked_key()
                    {
                        with(&window.app, |app| app.store_key(&key));
                    }
                    window.render();
                })
            });
        } else if !enabled && parts.unlocks_automatically() {
            with(&self.app, |app| app.clear_key());
            let _ = parts.settings.set_unlock_automatically(false);
            self.render();
        }
    }

    /// Say that the key could not be saved, so automatic unlock is off again.
    pub(crate) fn key_not_saved(&self) {
        let dialog = adw::AlertDialog::builder()
            .heading(tr!("Unlock automatically"))
            .body(tr!(
                "The key could not be saved in the desktop keyring. Type it the next time you connect."
            ))
            .build();
        dialog.add_responses(&[("close", &tr!("Close"))]);
        dialog.present(Some(&self.window));
        self.render();
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
