//! The uploads section: the cloud folder for new files, the queue with its decisions, and the file dialog.
use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;
use icloud_core::connection::Connection;
use icloud_core::tr;
use icloud_core::uploads::Upload;

use crate::app::Parts;
use crate::view::{self, Queue};
use crate::window::with;

/// A queue row on screen.
struct Row {
    identifier: String,
    row: adw::ActionRow,
    state: gtk::Label,
    progress: gtk::Label,
}

impl Row {
    fn new(item: &Upload) -> Row {
        // File names are shown as typed, never as markup.
        let row = adw::ActionRow::builder().use_markup(false).build();
        let state = gtk::Label::new(None);
        let progress = gtk::Label::builder()
            .width_chars(4)
            .xalign(1.0)
            .css_classes(["numeric", "dim-label"])
            .build();
        row.add_suffix(&state);
        row.add_suffix(&progress);
        let row = Row {
            identifier: item.identifier.clone(),
            row,
            state,
            progress,
        };
        row.show(item);
        row
    }

    fn show(&self, item: &Upload) {
        self.row.set_title(&item.name);
        self.row.set_subtitle(&item.destination);
        self.state.set_label(&view::upload_state(item.state));
        self.progress.set_label(&format!("{}%", item.progress));
    }
}

pub struct Uploads {
    parts: Rc<Parts>,
    root: gtk::Box,
    expander: gtk::Expander,
    destination: adw::EntryRow,
    add: gtk::Button,
    scroller: gtk::ScrolledWindow,
    list: gtk::ListBox,
    rows: RefCell<Vec<Row>>,
    detail: gtk::Label,
    resume: gtk::Button,
    pause: gtk::Button,
    skip: gtk::Button,
    keep: gtk::Button,
    /// The window's 'Upload files...' action.
    pub action: gio::SimpleAction,
}

fn button(label: String) -> gtk::Button {
    gtk::Button::builder().label(label).build()
}

impl Uploads {
    /// The section for the app's queue; when the queue could not be opened, it only shows a warning.
    pub fn new(parts: &Rc<Parts>) -> Rc<Uploads> {
        let warning = gtk::Label::builder()
            .label(tr!(
                "The saved upload queue could not be opened. Your pending files were preserved."
            ))
            .wrap(true)
            .xalign(0.0)
            .css_classes(["error"])
            .visible(parts.transfers.is_none())
            .build();

        // Qt marks the access key with `&` and GTK with `_`; the text stays the one the translations know.
        let destination = adw::EntryRow::builder()
            .title(tr!("&Upload to").replace('&', "_"))
            .use_underline(true)
            .text("iCloud on Linux uploads")
            .build();
        destination
            .upcast_ref::<gtk::Widget>()
            .update_property(&[gtk::accessible::Property::Description(&tr!(
                "Upload folder relative to iCloud Drive"
            ))]);
        let fields = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .hexpand(true)
            .build();
        fields.append(&destination);
        let add = button(tr!("Add files..."));
        add.set_valign(gtk::Align::Center);
        let destination_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        destination_row.append(&fields);
        destination_row.append(&add);

        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .css_classes(["boxed-list"])
            .build();
        list.update_property(&[gtk::accessible::Property::Label(&tr!("Upload queue"))]);
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(240)
            .child(&list)
            .build();
        let detail = gtk::Label::builder().xalign(0.0).wrap(true).build();
        let (resume, pause) = (button(tr!("Resume uploads")), button(tr!("Pause uploads")));
        let (skip, keep) = (button(tr!("Skip")), button(tr!("Keep both")));
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        for control in [&resume, &pause, &skip, &keep] {
            buttons.append(control);
        }

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .margin_top(8)
            .build();
        for part in [
            destination_row.upcast_ref::<gtk::Widget>(),
            scroller.upcast_ref(),
            detail.upcast_ref(),
            buttons.upcast_ref(),
        ] {
            content.append(part);
        }
        let expander = gtk::Expander::builder()
            .label(tr!("Uploads"))
            .child(&content)
            .visible(false)
            .build();
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        root.append(&warning);
        root.append(&expander);

        let this = Rc::new(Uploads {
            parts: parts.clone(),
            root,
            expander,
            destination,
            add,
            scroller,
            list,
            rows: RefCell::default(),
            detail,
            resume,
            pause,
            skip,
            keep,
            action: gio::SimpleAction::new("upload", None),
        });
        this.connect_signals();
        this
    }

    fn connect_signals(self: &Rc<Self>) {
        let Some(transfers) = self.parts.transfers.clone() else {
            return;
        };
        let weak = Rc::downgrade(self);
        self.add
            .connect_clicked(move |_| with(&weak, |uploads| uploads.choose()));
        let weak = Rc::downgrade(self);
        self.action.connect_activate(move |_, _| {
            with(&weak, |uploads| {
                uploads.expander.set_expanded(true);
                uploads.choose();
            })
        });
        let weak = Rc::downgrade(self);
        self.list
            .connect_row_selected(move |_, _| with(&weak, |uploads| uploads.render_controls()));
        let queue = Rc::downgrade(&transfers);
        self.resume
            .connect_clicked(move |_| with(&queue, |transfers| transfers.resume()));
        let queue = Rc::downgrade(&transfers);
        self.pause
            .connect_clicked(move |_| with(&queue, |transfers| transfers.pause()));
        let weak = Rc::downgrade(self);
        self.skip
            .connect_clicked(move |_| with(&weak, |uploads| uploads.resolve(false)));
        let weak = Rc::downgrade(self);
        self.keep
            .connect_clicked(move |_| with(&weak, |uploads| uploads.resolve(true)));
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    pub fn status(&self) -> view::Uploads {
        self.parts.transfers.as_ref().map_or(view::Uploads::Idle, |transfers| {
            view::upload_status(&transfers.items(), transfers.storage_error(), transfers.busy())
        })
    }

    /// Queue `paths` for the folder in 'Upload to', as the file dialog does.
    pub fn add(&self, paths: Vec<PathBuf>) {
        if let Some(transfers) = &self.parts.transfers
            && !paths.is_empty()
            && !self.parts.disconnect_pending.get()
        {
            transfers.add(&paths, &self.destination.text());
        }
    }

    fn choose(self: &Rc<Self>) {
        if !self.add.is_sensitive() {
            return;
        }
        let parent = self.root.root().and_downcast::<gtk::Window>();
        let dialog = gtk::FileDialog::builder()
            .title(tr!("Choose files to upload"))
            .modal(true)
            .build();
        let weak = Rc::downgrade(self);
        dialog.open_multiple(parent.as_ref(), gio::Cancellable::NONE, move |result| {
            // Cancelling the dialog, or files without a local path, add nothing.
            let Ok(files) = result else {
                return;
            };
            let paths = (0..files.n_items())
                .filter_map(|index| files.item(index).and_downcast::<gio::File>()?.path())
                .collect();
            with(&weak, |uploads| uploads.add(paths));
        });
    }

    fn selected(&self) -> Option<String> {
        let index = usize::try_from(self.list.selected_row()?.index()).ok()?;
        Some(self.rows.borrow().get(index)?.identifier.clone())
    }

    fn resolve(&self, keep_both: bool) {
        if let (Some(transfers), Some(identifier)) = (&self.parts.transfers, self.selected()) {
            transfers.resolve(&identifier, keep_both);
        }
    }

    /// Keep what the user set in `old`, the section of the window this one replaces.
    pub fn take_over(&self, old: &Uploads) {
        self.destination.set_text(&old.destination.text());
        self.expander.set_expanded(old.expander.is_expanded());
    }

    /// Show the queue. While a disconnect waits for it, no new work starts. Returns whether an upload newly waits
    /// for a decision or saving the queue newly failed.
    pub fn render(&self) -> bool {
        let Some(transfers) = &self.parts.transfers else {
            self.action.set_enabled(false);
            return false;
        };
        let items = transfers.items();
        let signed_in = fs::symlink_metadata(&self.parts.session.paths().config).is_ok();
        self.expander.set_visible(signed_in || !items.is_empty());
        self.expander.set_label(Some(&view::uploads_title(&items)));
        self.show_rows(&items);
        let detail = [transfers.message(), transfers.recovery_warning()]
            .into_iter()
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        self.detail.set_label(&detail);
        self.detail.set_visible(!detail.is_empty());
        // Show the queue once for each new decision or storage failure, without reopening it after the user closes it.
        let alerts = view::upload_alerts(&items, transfers.storage_error());
        let new = !alerts.is_subset(&self.parts.told.uploads.borrow());
        if new {
            self.expander.set_expanded(true);
        }
        *self.parts.told.uploads.borrow_mut() = alerts;
        self.render_controls();
        new
    }

    /// Update the rows in place, or rebuild them when rows were added or replaced.
    fn show_rows(&self, items: &[Upload]) {
        let same = {
            let rows = self.rows.borrow();
            rows.len() == items.len()
                && rows
                    .iter()
                    .zip(items)
                    .all(|(row, item)| row.identifier == item.identifier)
        };
        if !same {
            let selected = self.selected();
            let rows: Vec<Row> = items.iter().map(Row::new).collect();
            // The list announces selection changes while it changes, so the rows are not borrowed meanwhile.
            self.list.remove_all();
            for row in &rows {
                self.list.append(&row.row);
            }
            *self.rows.borrow_mut() = rows;
            let index = selected.and_then(|identifier| items.iter().position(|item| item.identifier == identifier));
            if let Some(row) = index.and_then(|index| self.list.row_at_index(i32::try_from(index).ok()?)) {
                self.list.select_row(Some(&row));
            }
        }
        for (row, item) in self.rows.borrow().iter().zip(items) {
            row.show(item);
        }
        self.scroller.set_visible(!items.is_empty());
    }

    fn render_controls(&self) {
        let Some(transfers) = &self.parts.transfers else {
            return;
        };
        let items = transfers.items();
        let selected = self.selected();
        let profile = self.parts.session.profile_id();
        let controls = view::upload_controls(&Queue {
            items: &items,
            selected: selected.as_deref(),
            profile: &profile,
            unlocked: self.parts.session.unlocked() && !self.parts.disconnect_pending.get(),
            accepts_files: transfers.can_add(),
            busy: transfers.busy(),
            storage_error: transfers.storage_error(),
        });
        self.add.set_sensitive(controls.add);
        self.action.set_enabled(controls.add);
        self.resume.set_sensitive(controls.resume);
        self.pause.set_sensitive(controls.pause);
        self.skip.set_sensitive(controls.skip);
        self.keep.set_sensitive(controls.keep);
    }
}
