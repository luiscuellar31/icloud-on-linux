//! The synced folders section: iCloud Drive folders with local copies to edit in Files or Dolphin, their states, and
//! the reviews they need, with Add folder beside Open.
use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use icloud_core::connection::Connection;
use icloud_core::session::Session;
use icloud_core::sync::{State as FolderState, SyncFolder, SyncedFolders};
use icloud_core::tr;

use crate::view::{self, FolderList};
use crate::window::with;

/// A folder row on screen.
struct Row {
    identifier: String,
    row: adw::ActionRow,
    state: gtk::Label,
}

impl Row {
    fn new(folder: &SyncFolder) -> Row {
        // Folder names are shown as typed, never as markup.
        let row = adw::ActionRow::builder().use_markup(false).build();
        let state = gtk::Label::new(None);
        row.add_suffix(&state);
        let row = Row {
            identifier: folder.identifier.clone(),
            row,
            state,
        };
        row.show(folder);
        row
    }

    fn show(&self, folder: &SyncFolder) {
        self.row.set_title(&view::folder_name(folder));
        let place = format!("{}\niCloud Drive/{}", folder.local.display(), folder.remote);
        self.row.set_tooltip_text(Some(&place));
        self.state.set_label(&folder.state.label());
        let message = (!folder.message.is_empty()).then_some(folder.message.as_str());
        self.state.set_tooltip_text(message);
        // A folder that waits for a review or failed stands out.
        if folder.attention.is_some() || folder.state == FolderState::Error {
            self.state.add_css_class("heading");
        } else {
            self.state.remove_css_class("heading");
        }
    }
}

pub struct Folders {
    sync: Option<Rc<SyncedFolders>>,
    session: Rc<Session>,
    root: gtk::Box,
    section: gtk::Box,
    /// 'Add folder', which the window shows beside Open.
    pub add: gtk::Button,
    list: gtk::ListBox,
    rows: RefCell<Vec<Row>>,
    detail: gtk::Label,
    review: gtk::Button,
    menu: gtk::PopoverMenu,
    open: gio::SimpleAction,
    sync_now: gio::SimpleAction,
    backups: gio::SimpleAction,
    stop: gio::SimpleAction,
    disconnect_pending: Cell<bool>,
}

impl Folders {
    /// Open the saved folder list; when it cannot be opened, the section only shows a warning.
    pub fn new(session: &Rc<Session>) -> Rc<Folders> {
        let sync = SyncedFolders::new(session.clone()).ok();
        let warning = gtk::Label::builder()
            .label(tr!(
                "The synced folder list could not be opened. Synced folders were left unchanged."
            ))
            .wrap(true)
            .xalign(0.0)
            .css_classes(["error"])
            .visible(sync.is_none())
            .build();
        let heading = gtk::Label::builder()
            .label(tr!("Folders on this computer"))
            .xalign(0.0)
            .css_classes(["heading"])
            .build();
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .activate_on_single_click(false)
            .css_classes(["boxed-list"])
            .build();
        list.update_property(&[gtk::accessible::Property::Label(&tr!("Synced folders"))]);
        let detail = gtk::Label::builder().xalign(0.0).wrap(true).hexpand(true).build();
        let review = gtk::Button::builder()
            .label(tr!("Review..."))
            .valign(gtk::Align::Start)
            .build();
        let review_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        review_row.append(&detail);
        review_row.append(&review);
        let section = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .visible(false)
            .build();
        section.append(&heading);
        section.append(&list);
        section.append(&review_row);
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        root.append(&warning);
        root.append(&section);

        let model = gio::Menu::new();
        let items = gio::Menu::new();
        items.append(Some(&tr!("Open")), Some("folder.open"));
        items.append(Some(&tr!("Sync now")), Some("folder.sync"));
        items.append(Some(&tr!("Backups")), Some("folder.backups"));
        model.append_section(None, &items);
        let stopping = gio::Menu::new();
        stopping.append(Some(&tr!("Stop syncing")), Some("folder.stop"));
        model.append_section(None, &stopping);
        let menu = gtk::PopoverMenu::from_model(Some(&model));
        menu.set_has_arrow(false);
        // The box removes the menu with its other children when the window closes.
        menu.set_parent(&root);
        let actions = gio::SimpleActionGroup::new();
        let action = |name: &str| {
            let action = gio::SimpleAction::new(name, None);
            actions.add_action(&action);
            action
        };
        let (open, sync_now, backups, stop) = (action("open"), action("sync"), action("backups"), action("stop"));
        root.insert_action_group("folder", Some(&actions));
        let add = gtk::Button::builder()
            .label(tr!("Add folder"))
            .css_classes(["pill"])
            .visible(false)
            .build();

        let this = Rc::new(Folders {
            sync,
            session: session.clone(),
            root,
            section,
            add,
            list,
            rows: RefCell::default(),
            detail,
            review,
            menu,
            open,
            sync_now,
            backups,
            stop,
            disconnect_pending: Cell::new(false),
        });
        this.connect_signals();
        this
    }

    fn connect_signals(self: &Rc<Self>) {
        let Some(sync) = self.sync.clone() else {
            return;
        };
        // Sync ticks for as long as the app runs, also while disconnected, to keep the folder states current.
        let weak = Rc::downgrade(&sync);
        glib::timeout_add_local(Duration::from_secs(1), move || match weak.upgrade() {
            Some(sync) => {
                sync.tick();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
        let weak = Rc::downgrade(self);
        self.add
            .connect_clicked(move |_| with(&weak, |folders| folders.choose()));
        let weak = Rc::downgrade(self);
        self.review
            .connect_clicked(move |_| with(&weak, |folders| folders.confirm_review()));
        let weak = Rc::downgrade(self);
        self.list
            .connect_row_selected(move |_, _| with(&weak, |folders| folders.render_controls()));
        let weak = Rc::downgrade(self);
        self.list
            .connect_row_activated(move |_, _| with(&weak, |folders| folders.open_selected()));
        let gesture = gtk::GestureClick::builder().button(gdk::BUTTON_SECONDARY).build();
        let weak = Rc::downgrade(self);
        gesture.connect_pressed(move |gesture, _, _, y| {
            with(&weak, |folders| {
                if let Some(row) = folders.list.row_at_y(y as i32) {
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                    folders.list.select_row(Some(&row));
                    folders.show_menu();
                }
            })
        });
        self.list.add_controller(gesture);
        let weak = Rc::downgrade(self);
        let shortcut = gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("<Shift>F10|Menu"),
            Some(gtk::CallbackAction::new(move |_, _| {
                with(&weak, |folders| folders.show_menu());
                glib::Propagation::Stop
            })),
        );
        let shortcuts = gtk::ShortcutController::new();
        shortcuts.add_shortcut(shortcut);
        self.list.add_controller(shortcuts);
        let weak = Rc::downgrade(self);
        self.open
            .connect_activate(move |_, _| with(&weak, |folders| folders.open_selected()));
        let weak = Rc::downgrade(self);
        self.sync_now.connect_activate(move |_, _| {
            with(&weak, |folders| {
                if let (Some(sync), Some(identifier)) = (&folders.sync, folders.selected()) {
                    sync.sync_now(Some(&identifier));
                }
            })
        });
        let weak = Rc::downgrade(self);
        self.backups.connect_activate(move |_, _| {
            with(&weak, |folders| {
                if let (Some(sync), Some(identifier)) = (&folders.sync, folders.selected()) {
                    folders.open_path(&sync.backups(&identifier));
                }
            })
        });
        let weak = Rc::downgrade(self);
        self.stop
            .connect_activate(move |_, _| with(&weak, |folders| folders.confirm_stop()));
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    /// The synced folders, unless their list could not be opened.
    pub fn synced(&self) -> Option<&Rc<SyncedFolders>> {
        self.sync.as_ref()
    }

    /// Whether bisync runs; a disconnect waits until it ends.
    pub fn busy(&self) -> bool {
        self.sync.as_ref().is_some_and(|sync| sync.busy())
    }

    pub fn pause(&self) {
        if let Some(sync) = &self.sync {
            sync.pause();
        }
    }

    pub fn status(&self) -> view::Folders {
        self.sync
            .as_ref()
            .map_or(view::Folders::None, |sync| view::folder_status(&sync.folders()))
    }

    /// Sync the iCloud Drive folder chosen in the mount, as the folder dialog does.
    pub fn add_chosen(&self, chosen: &Path) {
        if let Some(sync) = &self.sync {
            sync.add(&view::chosen_folder(&self.session.paths().mount, chosen));
        }
    }

    fn parent(&self) -> Option<gtk::Window> {
        self.root.root().and_downcast::<gtk::Window>()
    }

    fn choose(self: &Rc<Self>) {
        if !self.add.is_sensitive() {
            return;
        }
        let mount = gio::File::for_path(&self.session.paths().mount);
        let dialog = gtk::FileDialog::builder()
            .title(tr!("Choose an iCloud Drive folder to sync"))
            .initial_folder(&mount)
            .modal(true)
            .build();
        let weak = Rc::downgrade(self);
        dialog.select_folder(self.parent().as_ref(), gio::Cancellable::NONE, move |result| {
            // Cancelling the dialog, or a folder without a local path, adds nothing.
            if let Some(chosen) = result.ok().and_then(|folder| folder.path()) {
                with(&weak, |folders| folders.add_chosen(&chosen));
            }
        });
    }

    fn selected(&self) -> Option<String> {
        let index = usize::try_from(self.list.selected_row()?.index()).ok()?;
        Some(self.rows.borrow().get(index)?.identifier.clone())
    }

    fn open_selected(&self) {
        let folder = self
            .selected()
            .and_then(|identifier| self.sync.as_ref()?.folder(&identifier));
        if let Some(folder) = folder {
            self.open_path(&folder.local);
        }
    }

    fn open_path(&self, path: &Path) {
        if path.is_dir() {
            let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(path)));
            launcher.launch(self.parent().as_ref(), gio::Cancellable::NONE, |_| {});
        }
    }

    /// Point the context menu at the selected folder.
    fn show_menu(&self) {
        let Some(bounds) = self.list.selected_row().and_then(|row| row.compute_bounds(&self.root)) else {
            return;
        };
        let (x, y) = (bounds.x() as i32, bounds.y() as i32);
        let area = gdk::Rectangle::new(x, y, bounds.width() as i32, bounds.height() as i32);
        self.menu.set_pointing_to(Some(&area));
        self.menu.popup();
    }

    fn confirm_review(self: &Rc<Self>) {
        let Some(sync) = self.sync.clone() else {
            return;
        };
        let review = self
            .controls()
            .and_then(|controls| controls.review)
            .and_then(|identifier| sync.folder(&identifier));
        let Some((folder, kind)) = review.and_then(|folder| {
            let kind = folder.attention?;
            Some((folder, kind))
        }) else {
            return;
        };
        let dialog = adw::AlertDialog::builder()
            .heading(tr!("Synced folder"))
            .body(view::review_text(kind, &view::folder_name(&folder)))
            .close_response("cancel")
            .default_response("cancel")
            .build();
        dialog.add_responses(&[("cancel", &tr!("Cancel")), ("continue", &tr!("Continue"))]);
        let weak = Rc::downgrade(&sync);
        dialog.choose(Some(&self.root), gio::Cancellable::NONE, move |response| {
            if response == "continue" {
                with(&weak, |sync| sync.approve(&folder.identifier));
            }
        });
    }

    fn confirm_stop(self: &Rc<Self>) {
        let (Some(sync), Some(identifier)) = (self.sync.clone(), self.selected()) else {
            return;
        };
        let Some(folder) = sync.folder(&identifier) else {
            return;
        };
        let dialog = adw::AlertDialog::builder()
            .heading(tr!("Stop syncing"))
            .body(tr!(
                "Stop syncing \"{name}\"? The local copy and the iCloud folder are kept.",
                name = view::folder_name(&folder)
            ))
            .close_response("cancel")
            .default_response("cancel")
            .build();
        dialog.add_responses(&[("cancel", &tr!("Cancel")), ("stop", &tr!("Stop syncing"))]);
        dialog.set_response_appearance("stop", adw::ResponseAppearance::Destructive);
        let weak = Rc::downgrade(&sync);
        dialog.choose(Some(&self.root), gio::Cancellable::NONE, move |response| {
            if response == "stop" {
                with(&weak, |sync| sync.remove(&identifier));
            }
        });
    }

    /// Show the folders. Add folder appears while connected; while a disconnect waits for sync, nothing new starts.
    pub fn render(&self, disconnect_pending: bool, available: bool, signed_in: bool) {
        self.disconnect_pending.set(disconnect_pending);
        let Some(sync) = &self.sync else {
            return;
        };
        let folders = sync.folders();
        self.add.set_visible(available);
        self.section.set_visible(signed_in || !folders.is_empty());
        self.show_rows(&folders);
        self.render_controls();
    }

    /// Update the rows in place, or rebuild them when folders were added or removed.
    fn show_rows(&self, folders: &[SyncFolder]) {
        let same = {
            let rows = self.rows.borrow();
            rows.len() == folders.len()
                && rows
                    .iter()
                    .zip(folders)
                    .all(|(row, folder)| row.identifier == folder.identifier)
        };
        if !same {
            let selected = self.selected();
            let rows: Vec<Row> = folders.iter().map(Row::new).collect();
            // The list announces selection changes while it changes, so the rows are not borrowed meanwhile.
            self.list.remove_all();
            for row in &rows {
                self.list.append(&row.row);
            }
            *self.rows.borrow_mut() = rows;
            let index =
                selected.and_then(|identifier| folders.iter().position(|folder| folder.identifier == identifier));
            if let Some(row) = index.and_then(|index| self.list.row_at_index(i32::try_from(index).ok()?)) {
                self.list.select_row(Some(&row));
            }
        }
        for (row, folder) in self.rows.borrow().iter().zip(folders) {
            row.show(folder);
        }
        self.list.set_visible(!folders.is_empty());
    }

    fn controls(&self) -> Option<view::FolderControls> {
        let sync = self.sync.as_ref()?;
        let folders = sync.folders();
        let selected = self.selected();
        let chosen = selected
            .as_deref()
            .and_then(|identifier| folders.iter().find(|folder| folder.identifier == identifier));
        let (current, message) = (sync.current(), sync.message());
        Some(view::folder_controls(&FolderList {
            folders: &folders,
            selected: selected.as_deref(),
            unlocked: self.session.unlocked() && !self.disconnect_pending.get(),
            busy: sync.busy(),
            current: current.as_deref(),
            local_exists: chosen.is_some_and(|folder| folder.local.is_dir()),
            backups_exist: chosen.is_some_and(|folder| sync.backups(&folder.identifier).is_dir()),
            message: &message,
        }))
    }

    fn render_controls(&self) {
        let Some(controls) = self.controls() else {
            return;
        };
        self.add.set_sensitive(controls.add);
        self.open.set_enabled(controls.open);
        self.sync_now.set_enabled(controls.sync_now);
        self.backups.set_enabled(controls.backups);
        self.stop.set_enabled(controls.stop);
        self.review.set_visible(controls.review.is_some());
        self.review.set_sensitive(controls.review_enabled);
        self.detail.set_label(&controls.detail);
        self.detail.set_visible(!controls.detail.is_empty());
    }
}
