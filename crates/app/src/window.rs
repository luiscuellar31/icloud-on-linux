//! The main window: the connection status, the local key, and opening iCloud Drive in the file manager.
use std::cell::Cell;
use std::rc::{Rc, Weak};
use std::time::Duration;

use adw::prelude::*;
use gtk::{gio, glib};
use icloud_core::connection::Connection;
use icloud_core::session::{Session, State};
use icloud_core::tr;

use crate::view::{self, Facts, Tone};

const TONES: [Tone; 4] = [Tone::Ok, Tone::Busy, Tone::Error, Tone::Off];

pub struct Window {
    pub window: adw::ApplicationWindow,
    session: Rc<Session>,
    dot: gtk::Label,
    status: gtk::Label,
    detail: gtk::Label,
    progress: gtk::ProgressBar,
    key_panel: gtk::Box,
    key: adw::PasswordEntryRow,
    connect: gtk::Button,
    open: gtk::Button,
    cancel: gtk::Button,
    disconnect: gio::SimpleAction,
    quit: gio::SimpleAction,
    closing: Cell<bool>,
}

/// Run `action` if the window still exists; closures keep only weak references so the window can be freed.
fn with(window: &Weak<Window>, action: impl FnOnce(&Rc<Window>)) {
    if let Some(window) = window.upgrade() {
        action(&window);
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
    pub fn new(application: &adw::Application, session: Rc<Session>) -> Rc<Window> {
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
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        actions.append(&open);
        actions.append(&cancel);

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
            key_panel.upcast_ref(),
            actions.upcast_ref(),
        ] {
            content.append(part);
        }

        let menu = gio::Menu::new();
        menu.append(Some(&tr!("Disconnect")), Some("win.disconnect"));
        menu.append(Some(&tr!("Quit")), Some("win.quit"));
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
        let disconnect = gio::SimpleAction::new("disconnect", None);
        let quit = gio::SimpleAction::new("quit", None);
        window.add_action(&disconnect);
        window.add_action(&quit);

        let this = Rc::new(Window {
            window,
            session,
            dot,
            status,
            detail,
            progress,
            key_panel,
            key,
            connect,
            open,
            cancel,
            disconnect,
            quit,
            closing: Cell::new(false),
        });
        this.connect_signals();
        this.render();
        this
    }

    fn connect_signals(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.session
            .changed()
            .connect(move || with(&weak, |window| window.render()));
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
        self.cancel
            .connect_clicked(move |_| with(&weak, |window| window.session.stop()));
        let weak = Rc::downgrade(self);
        self.disconnect
            .connect_activate(move |_, _| with(&weak, |window| window.session.stop()));
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
                window.session.tick();
                if window.progress.is_visible() {
                    window.progress.pulse();
                }
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
        // The end of the desktop session disconnects without asking.
        for signal in [nix::libc::SIGTERM, nix::libc::SIGINT] {
            let weak = Rc::downgrade(self);
            glib_unix::unix_signal_add_local(signal, move || {
                with(&weak, |window| window.shut_down());
                glib::ControlFlow::Continue
            });
        }
    }

    fn render(&self) {
        let mount = self.session.current_mount();
        let facts = Facts {
            state: self.session.state(),
            message: self.session.message(),
            mounted: mount.is_some(),
            compatible: mount.as_ref().is_some_and(|mount| mount.compatible()),
            client: self.session.client_running(),
            checking: self.session.checking(),
            owns_mount: self.session.owns_mount(),
            key_typed: typed(&self.key),
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
        self.key_panel.set_visible(screen.key_panel);
        self.connect.set_sensitive(screen.connect);
        self.open.set_visible(screen.open_visible);
        self.open.set_sensitive(screen.open);
        self.cancel.set_visible(screen.cancel);
        self.disconnect.set_enabled(screen.disconnect);
        if self.closing.get() && !self.session.busy() {
            if self.session.running() || mount.is_some() {
                // Disconnecting failed, for example because a file is open; the status explains it.
                self.closing.set(false);
            } else {
                self.window.destroy();
            }
        }
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

    fn active(&self) -> bool {
        self.session.running() || self.session.owns_mount()
    }

    fn close_request(self: &Rc<Self>) -> glib::Propagation {
        if !self.active() {
            return glib::Propagation::Proceed;
        }
        if !self.closing.get() {
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

    /// Disconnect if needed, then close; `render` finishes once the connection has stopped.
    fn shut_down(&self) {
        if self.active() {
            self.closing.set(true);
            self.session.stop();
            self.render();
        } else {
            self.window.destroy();
        }
    }
}
