//! The sign-in dialog: the local key, the Apple account and password, then the verification code or phone.
use std::cell::Cell;
use std::rc::{Rc, Weak};
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use icloud_core::auth::Authorization;
use icloud_core::connection::Connection;
use icloud_core::session::Session;
use icloud_core::tr;

pub struct SignIn {
    dialog: adw::Dialog,
    authorization: Rc<Authorization>,
    form: gtk::ListBox,
    key: adw::PasswordEntryRow,
    confirmation: adw::PasswordEntryRow,
    account: adw::EntryRow,
    password: adw::PasswordEntryRow,
    questions: gtk::ListBox,
    verification: adw::EntryRow,
    phones: adw::ComboRow,
    phone_list: gtk::StringList,
    message: gtk::Label,
    submit: gtk::Button,
    sms: gtk::Button,
    close: gtk::Button,
    saved: Cell<bool>,
    closing: Cell<bool>,
}

fn with(sign_in: &Weak<SignIn>, action: impl FnOnce(&Rc<SignIn>)) {
    if let Some(sign_in) = sign_in.upgrade() {
        action(&sign_in);
    }
}

fn boxed_list() -> gtk::ListBox {
    let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
    list.add_css_class("boxed-list");
    list
}

impl SignIn {
    /// Show the dialog over `parent`; `saved` runs once a new profile is published.
    pub fn present(parent: &impl IsA<gtk::Widget>, session: Rc<Session>, saved: impl Fn() + 'static) -> Rc<SignIn> {
        let existing = std::fs::symlink_metadata(&session.paths().config).is_ok();
        let authorization = Authorization::new(session);
        let description = gtk::Label::builder()
            .label(tr!(
                "Use your Apple account password and verification code. Your local key encrypts the saved session."
            ))
            .wrap(true)
            .xalign(0.0)
            .build();
        let key = adw::PasswordEntryRow::builder().title(tr!("Local key")).build();
        let confirmation = adw::PasswordEntryRow::builder()
            .title(tr!("Confirm new local key"))
            .visible(!existing)
            .build();
        let account = adw::EntryRow::builder().title(tr!("Apple account")).build();
        let password = adw::PasswordEntryRow::builder().title(tr!("Apple password")).build();
        let form = boxed_list();
        for row in [
            key.upcast_ref::<gtk::Widget>(),
            confirmation.upcast_ref(),
            account.upcast_ref(),
            password.upcast_ref(),
        ] {
            form.append(row);
        }
        let verification = adw::EntryRow::builder()
            .title(tr!("6-digit verification code"))
            .input_purpose(gtk::InputPurpose::Digits)
            .build();
        let phone_list = gtk::StringList::new(&[]);
        let phones = adw::ComboRow::builder()
            .title(tr!("Trusted phone number"))
            .model(&phone_list)
            .build();
        let questions = boxed_list();
        questions.append(&verification);
        questions.append(&phones);
        let message = gtk::Label::builder().wrap(true).xalign(0.0).build();
        let submit = gtk::Button::builder()
            .label(tr!("Sign in"))
            .css_classes(["suggested-action", "pill"])
            .build();
        let sms = gtk::Button::builder()
            .label(tr!("Send SMS"))
            .css_classes(["pill"])
            .build();
        let close = gtk::Button::builder()
            .label(tr!("Cancel"))
            .css_classes(["pill"])
            .build();
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        for button in [&submit, &sms, &close] {
            buttons.append(button);
        }
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(12)
            .margin_bottom(18)
            .margin_start(18)
            .margin_end(18)
            .build();
        for part in [
            description.upcast_ref::<gtk::Widget>(),
            form.upcast_ref(),
            questions.upcast_ref(),
            message.upcast_ref(),
            buttons.upcast_ref(),
        ] {
            content.append(part);
        }
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&adw::HeaderBar::new());
        toolbar.set_content(Some(&content));
        let dialog = adw::Dialog::builder()
            .title(tr!("Sign in to iCloud"))
            .content_width(440)
            .child(&toolbar)
            .build();

        let this = Rc::new(SignIn {
            dialog,
            authorization,
            form,
            key,
            confirmation,
            account,
            password,
            questions,
            verification,
            phones,
            phone_list,
            message,
            submit,
            sms,
            close,
            saved: Cell::new(false),
            closing: Cell::new(false),
        });
        this.connect_signals(saved);
        this.render();
        this.dialog.present(Some(parent));
        this
    }

    /// Whether a sign-in attempt is under way.
    pub fn active(&self) -> bool {
        self.authorization.active()
    }

    /// Close the dialog, first stopping a running step so no copy of the profile is left behind.
    pub fn close(&self) {
        self.close_attempt();
    }

    fn connect_signals(self: &Rc<Self>, saved: impl Fn() + 'static) {
        let weak = Rc::downgrade(self);
        self.authorization
            .changed()
            .connect(move || with(&weak, |sign_in| sign_in.render()));
        let weak = Rc::downgrade(self);
        self.authorization.saved().connect(move || {
            with(&weak, |sign_in| {
                sign_in.saved.set(true);
                sign_in.account.set_text("");
                sign_in.render();
            });
            saved();
        });
        let weak = Rc::downgrade(self);
        self.submit
            .connect_clicked(move |_| with(&weak, |sign_in| sign_in.continue_sign_in()));
        let weak = Rc::downgrade(self);
        self.verification
            .connect_entry_activated(move |_| with(&weak, |sign_in| sign_in.continue_sign_in()));
        let weak = Rc::downgrade(self);
        self.sms
            .connect_clicked(move |_| with(&weak, |sign_in| sign_in.authorization.answer("sms")));
        let weak = Rc::downgrade(self);
        self.close
            .connect_clicked(move |_| with(&weak, |sign_in| sign_in.close_attempt()));
        let weak = Rc::downgrade(self);
        self.dialog
            .connect_close_attempt(move |_| with(&weak, |sign_in| sign_in.close_attempt()));
        // The dialog keeps itself alive through this timer until it closes.
        let this = self.clone();
        let timer = glib::timeout_add_local(Duration::from_millis(100), move || {
            this.authorization.tick();
            glib::ControlFlow::Continue
        });
        let timer = Cell::new(Some(timer));
        self.dialog.connect_closed(move |_| {
            if let Some(timer) = timer.take() {
                timer.remove();
            }
        });
    }

    fn continue_sign_in(&self) {
        let auth = &self.authorization;
        if auth.active() {
            let choices = auth.choices();
            let value = if choices.is_empty() {
                self.verification.text().to_string()
            } else {
                choices
                    .get(self.phones.selected() as usize)
                    .map(|(value, _)| value.clone())
                    .unwrap_or_default()
            };
            self.verification.set_text("");
            auth.answer(&value);
        } else {
            let (key, confirmation) = (self.key.text(), self.confirmation.text());
            let (account, password) = (self.account.text(), self.password.text());
            for field in [&self.key, &self.confirmation, &self.password] {
                field.set_text("");
            }
            auth.start(&key, &confirmation, &account, &password);
        }
    }

    fn close_attempt(&self) {
        for field in [&self.key, &self.confirmation, &self.password] {
            field.set_text("");
        }
        self.verification.set_text("");
        self.account.set_text("");
        if self.authorization.active() {
            // Closing waits for the running step to stop, so no copy of the profile is left behind.
            self.closing.set(true);
            self.authorization.cancel();
        } else {
            self.dialog.force_close();
        }
    }

    fn render(&self) {
        let auth = &self.authorization;
        let (active, question, choices) = (auth.active(), auth.question(), auth.choices());
        self.dialog.set_can_close(!active);
        self.form.set_visible(!active && !self.saved.get());
        self.verification
            .set_visible(question == "config_2fa" || question == "config_2fa_sms");
        self.phones.set_visible(!choices.is_empty());
        self.questions.set_visible(!question.is_empty());
        if !choices.is_empty() && self.phone_list.n_items() == 0 {
            for (_, label) in &choices {
                self.phone_list.append(label);
            }
        }
        if choices.is_empty() {
            self.phone_list.splice(0, self.phone_list.n_items(), &[]);
        }
        self.sms.set_visible(question == "config_2fa");
        self.submit.set_visible(!self.saved.get());
        self.submit
            .set_label(&if active { tr!("Continue") } else { tr!("Sign in") });
        self.submit.set_sensitive(!auth.running());
        self.message.set_label(&auth.message());
        self.close
            .set_label(&if self.saved.get() { tr!("Close") } else { tr!("Cancel") });
        if !question.is_empty() {
            self.verification.grab_focus();
        }
        if self.closing.get() && !active {
            self.dialog.force_close();
        }
    }
}
