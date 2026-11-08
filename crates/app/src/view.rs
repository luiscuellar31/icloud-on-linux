//! What the window shows for a given connection, upload queue, and synced folders, kept apart from the widgets so it
//! can be tested without a display.
use std::collections::BTreeSet;
use std::path::Path;

use icloud_core::session::State;
use icloud_core::sync::{Review, State as FolderState, SyncFolder};
use icloud_core::tr;
use icloud_core::uploads::{State as UploadState, Upload};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Ok,
    Busy,
    Warning,
    Error,
    Off,
}

impl Tone {
    /// The libadwaita style class for the status dot.
    pub fn style(self) -> &'static str {
        match self {
            Tone::Ok => "success",
            Tone::Busy => "accent",
            Tone::Warning => "warning",
            Tone::Error => "error",
            Tone::Off => "dim-label",
        }
    }
}

/// What the upload queue adds to the status of an open connection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Uploads {
    #[default]
    Idle,
    /// A file is being prepared or transferred.
    Busy,
    /// Saving the queue failed, or a row stopped with an error.
    Problem,
    /// A conflict waits for Skip or Keep both.
    Review,
}

/// The most urgent upload state: a decision, then a problem, then activity.
pub fn upload_status(items: &[Upload], storage_error: bool, busy: bool) -> Uploads {
    if items.iter().any(|item| item.state == UploadState::Conflict) {
        Uploads::Review
    } else if storage_error || items.iter().any(|item| item.state == UploadState::Error) {
        Uploads::Problem
    } else if busy {
        Uploads::Busy
    } else {
        Uploads::Idle
    }
}

/// What the synced folders add to the status of an open connection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Folders {
    /// No folder is synced.
    #[default]
    None,
    UpToDate,
    Syncing,
    /// A folder stopped with an error.
    Problem,
    /// A folder waits for Review.
    Review,
}

/// The most urgent folder state: a review, then a problem, then a running sync.
pub fn folder_status(folders: &[SyncFolder]) -> Folders {
    let any = |state: FolderState| folders.iter().any(|folder| folder.state == state);
    if folders.iter().any(|folder| folder.attention.is_some()) {
        Folders::Review
    } else if any(FolderState::Error) {
        Folders::Problem
    } else if any(FolderState::Syncing) {
        Folders::Syncing
    } else if folders.is_empty() {
        Folders::None
    } else {
        Folders::UpToDate
    }
}

/// The connection facts the window reads before rendering.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    pub state: State,
    pub message: String,
    pub mounted: bool,
    pub compatible: bool,
    pub client: bool,
    pub checking: bool,
    pub owns_mount: bool,
    /// A connection that dropped waits to reconnect on its own.
    pub reconnecting: bool,
    pub key_typed: bool,
    pub signed_in: bool,
    pub renewing: bool,
    pub uploads: Uploads,
    pub folders: Folders,
    /// The window waits for uploads and sync to stop before disconnecting.
    pub disconnect_pending: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Screen {
    pub status: String,
    pub tone: Tone,
    pub detail: Option<String>,
    pub progress: bool,
    pub welcome: bool,
    pub key_panel: bool,
    pub connect: bool,
    pub open_visible: bool,
    pub open: bool,
    pub cancel: bool,
    pub cancel_enabled: bool,
    pub disconnect: bool,
    pub renew: bool,
}

pub fn screen(facts: &Facts) -> Screen {
    let busy = matches!(facts.state, State::Connecting | State::Disconnecting);
    let available = facts.state == State::Available;
    let can_connect = !busy && !facts.mounted && !facts.client;
    let connected = |summary: String| format!("{} · {summary}", tr!("Connected"));
    let (status, tone) = match facts.state {
        State::Available if facts.folders == Folders::Review || facts.uploads == Uploads::Review => {
            (connected(tr!("Review needed")), Tone::Warning)
        }
        State::Available if facts.folders == Folders::Problem => (connected(tr!("Sync problem")), Tone::Error),
        State::Available if facts.uploads == Uploads::Problem => (connected(tr!("Upload problem")), Tone::Error),
        State::Available if facts.folders == Folders::Syncing => (connected(tr!("Syncing...")), Tone::Busy),
        State::Available if facts.uploads == Uploads::Busy => (connected(tr!("Uploading...")), Tone::Busy),
        State::Available if facts.folders == Folders::UpToDate => (connected(tr!("Up to date")), Tone::Ok),
        State::Available => (tr!("Connected"), Tone::Ok),
        _ if facts.reconnecting => (tr!("Reconnecting..."), Tone::Busy),
        State::Error => (tr!("Needs attention"), Tone::Error),
        State::Connecting => (tr!("Connecting..."), Tone::Busy),
        State::Disconnecting => (tr!("Disconnecting..."), Tone::Busy),
        State::Disconnected if facts.signed_in => (tr!("Disconnected"), Tone::Off),
        State::Disconnected => (tr!("Not signed in"), Tone::Off),
    };
    // The status line already says a normal connection is ready; the message explains anything else.
    let detail = (!facts.message.is_empty()
        && (facts.signed_in || facts.state == State::Error)
        && !(available && facts.owns_mount))
        .then(|| facts.message.clone());
    Screen {
        status,
        tone,
        detail,
        progress: busy,
        welcome: can_connect && !facts.signed_in,
        key_panel: can_connect && facts.signed_in && !facts.reconnecting,
        connect: can_connect && facts.key_typed,
        open_visible: available,
        open: available && facts.mounted && facts.compatible,
        // Cancel also stops waiting for a reconnection.
        cancel: facts.state == State::Connecting || facts.reconnecting,
        cancel_enabled: !facts.disconnect_pending,
        disconnect: !facts.disconnect_pending
            && facts.state != State::Disconnecting
            && (facts.mounted || facts.checking || facts.client),
        renew: facts.signed_in && !facts.renewing && (can_connect || available),
    }
}

/// Apple sessions last about 30 days; the window warns in the last 5.
const SESSION_DAYS: f64 = 30.0;
const WARNING_DAYS: f64 = 5.0;

/// The warning that the Apple session ends soon, from when this app last signed in, in seconds since 1970. A session
/// signed in from the terminal has no recorded time, so it gets no estimate.
pub fn expiry(signed_in_at: Option<i64>, now: i64) -> Option<String> {
    let days = SESSION_DAYS - (now - signed_in_at?) as f64 / 86_400.0;
    if days > WARNING_DAYS {
        None
    } else if days <= 0.0 {
        Some(tr!("Your Apple session may have ended. Renew it to keep syncing."))
    } else if days < 2.0 {
        Some(tr!(
            "Your Apple session ends in less than 2 days. Renew it to keep syncing."
        ))
    } else {
        Some(tr!(
            "Your Apple session ends in about {days} days. Renew it to keep syncing.",
            days = days.ceil() as i64
        ))
    }
}

/// What the upload controls read before rendering.
pub struct Queue<'a> {
    pub items: &'a [Upload],
    /// The identifier of the selected row.
    pub selected: Option<&'a str>,
    /// The profile identity of the open connection.
    pub profile: &'a str,
    /// The connection is unlocked and no disconnect is pending.
    pub unlocked: bool,
    /// The queue takes files: it is connected, saving works, and no preparation runs.
    pub accepts_files: bool,
    pub busy: bool,
    pub storage_error: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Controls {
    pub add: bool,
    pub resume: bool,
    pub pause: bool,
    pub skip: bool,
    pub keep: bool,
}

pub fn upload_controls(queue: &Queue) -> Controls {
    let any = |states: &[UploadState]| queue.items.iter().any(|item| states.contains(&item.state));
    let selected = queue
        .selected
        .and_then(|identifier| queue.items.iter().find(|item| item.identifier == identifier));
    let conflict =
        selected.is_some_and(|item| item.state == UploadState::Conflict) && !queue.busy && !queue.storage_error;
    Controls {
        add: queue.unlocked && queue.accepts_files,
        resume: queue.unlocked
            && !queue.busy
            && (queue.storage_error || any(&[UploadState::Paused, UploadState::Error])),
        pause: queue.busy || any(&[UploadState::Queued]),
        skip: conflict,
        // Keep both uploads again, so only the profile that added the row may choose it.
        keep: conflict && queue.unlocked && selected.is_some_and(|item| item.profile == queue.profile),
    }
}

/// The section title, with the rows not yet completed or skipped.
pub fn uploads_title(items: &[Upload]) -> String {
    let pending = items
        .iter()
        .filter(|item| !matches!(item.state, UploadState::Completed | UploadState::Skipped))
        .count();
    if pending > 0 {
        tr!("Uploads ({count} pending)", count = pending)
    } else {
        tr!("Uploads")
    }
}

pub fn upload_state(state: UploadState) -> String {
    match state {
        UploadState::Queued => tr!("Queued"),
        UploadState::Checking => tr!("Checking"),
        UploadState::Uploading => tr!("Uploading"),
        UploadState::Verifying => tr!("Verifying"),
        UploadState::Completed => tr!("Completed"),
        UploadState::Paused => tr!("Paused"),
        UploadState::Conflict => tr!("Conflict"),
        UploadState::Skipped => tr!("Skipped"),
        UploadState::Error => tr!("Error"),
    }
}

/// What the synced folder controls read before rendering.
pub struct FolderList<'a> {
    pub folders: &'a [SyncFolder],
    /// The identifier of the selected folder.
    pub selected: Option<&'a str>,
    /// The connection is unlocked and no disconnect is pending.
    pub unlocked: bool,
    pub busy: bool,
    /// The folder that is syncing now.
    pub current: Option<&'a str>,
    /// Whether the selected folder's local copy and its Backups exist.
    pub local_exists: bool,
    pub backups_exist: bool,
    /// Why the last change to the list failed, or nothing.
    pub message: &'a str,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct FolderControls {
    pub add: bool,
    pub open: bool,
    pub sync_now: bool,
    pub backups: bool,
    pub stop: bool,
    /// The folder that Review continues, when one waits for it.
    pub review: Option<String>,
    pub review_enabled: bool,
    pub detail: String,
}

/// The name of a synced folder's local copy.
pub fn folder_name(folder: &SyncFolder) -> String {
    folder
        .local
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
}

pub fn folder_controls(list: &FolderList) -> FolderControls {
    let selected = list
        .selected
        .and_then(|identifier| list.folders.iter().find(|folder| folder.identifier == identifier));
    // The selected folder, or else the first one waiting for a review, is the one the details and Review act on.
    let shown = selected.or_else(|| list.folders.iter().find(|folder| folder.attention.is_some()));
    let review = shown.filter(|folder| folder.attention.is_some());
    let mut detail = vec![list.message.to_owned()];
    if let Some(folder) = shown.filter(|folder| !folder.message.is_empty()) {
        detail.push(format!("{}: {}", folder_name(folder), folder.message));
    }
    if list.folders.is_empty() {
        detail.push(tr!(
            "Add an iCloud Drive folder to edit it in Files or Dolphin; changes sync both ways."
        ));
    }
    FolderControls {
        add: list.unlocked,
        open: selected.is_some() && list.local_exists,
        sync_now: list.unlocked && selected.is_some() && !list.busy,
        backups: selected.is_some() && list.backups_exist,
        stop: selected.is_some_and(|folder| list.current != Some(folder.identifier.as_str())),
        review: review.map(|folder| folder.identifier.clone()),
        review_enabled: list.unlocked && review.is_some(),
        detail: detail
            .into_iter()
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(" "),
    }
}

/// The question Review asks before it continues a folder.
pub fn review_text(review: Review, name: &str) -> String {
    match review {
        Review::Confirm => tr!(
            "Apply the deletions or changes found in \"{name}\" to the other side? Files deleted from iCloud go to Recently Deleted; replaced or deleted local files are kept in Backups.",
            name = name
        ),
        Review::Repair => tr!(
            "Copy files between \"{name}\" and iCloud to restore syncing? Nothing is deleted. Where both sides differ, the iCloud version is used and the local copy is kept in Backups.",
            name = name
        ),
    }
}

/// The iCloud Drive folder for a folder chosen in the mount; the whole drive or a folder elsewhere gives nothing,
/// which adding refuses with an explanation.
pub fn chosen_folder(mount: &Path, chosen: &Path) -> String {
    match chosen.strip_prefix(mount) {
        Ok(relative) if !relative.as_os_str().is_empty() => relative.to_string_lossy().into_owned(),
        _ => String::new(),
    }
}

/// The conflicts and storage failure that need the user. Each new one shows the queue once.
pub fn upload_alerts(items: &[Upload], storage_error: bool) -> BTreeSet<String> {
    let conflicts = items.iter().filter(|item| item.state == UploadState::Conflict);
    let mut alerts: BTreeSet<String> = conflicts.map(|item| item.identifier.clone()).collect();
    if storage_error {
        alerts.insert(String::new());
    }
    alerts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(state: State) -> Facts {
        Facts {
            state,
            signed_in: true,
            message: "Fixture message".into(),
            ..Facts::default()
        }
    }

    #[test]
    fn a_disconnected_session_asks_for_the_key() {
        let mut shown = facts(State::Disconnected);
        assert!(screen(&shown).key_panel && !screen(&shown).connect);
        shown.key_typed = true;
        let result = screen(&shown);
        assert!(result.connect && !result.open_visible && !result.disconnect);
        assert_eq!((result.status.as_str(), result.tone), ("Disconnected", Tone::Off));
        assert_eq!(result.detail.as_deref(), Some("Fixture message"));
    }

    #[test]
    fn connecting_shows_progress_and_can_be_cancelled() {
        let result = screen(&Facts {
            checking: true,
            ..facts(State::Connecting)
        });
        assert!(result.progress && result.cancel && result.disconnect && !result.key_panel);
    }

    #[test]
    fn a_dropped_connection_waits_to_reconnect_and_can_be_cancelled() {
        let result = screen(&Facts {
            reconnecting: true,
            ..facts(State::Error)
        });
        assert_eq!((result.status.as_str(), result.tone), ("Reconnecting...", Tone::Busy));
        assert!(result.cancel && result.cancel_enabled && !result.key_panel && !result.welcome);
        assert_eq!(result.detail.as_deref(), Some("Fixture message"));
    }

    #[test]
    fn the_apple_session_warning_starts_five_days_before_it_ends() {
        let (day, now) = (86_400, 1_800_000_000);
        let warning = |age: i64| expiry(Some(now - age), now);
        assert_eq!(warning(10 * day), None);
        assert_eq!(warning(24 * day), None);
        let about = |days: &str| format!("Your Apple session ends in about {days} days. Renew it to keep syncing.");
        assert_eq!(warning(25 * day), Some(about("5")));
        assert_eq!(warning(27 * day + day / 2), Some(about("3")));
        assert_eq!(
            warning(29 * day).as_deref(),
            Some("Your Apple session ends in less than 2 days. Renew it to keep syncing.")
        );
        assert_eq!(
            warning(31 * day).as_deref(),
            Some("Your Apple session may have ended. Renew it to keep syncing.")
        );
        assert_eq!(expiry(None, now), None);
    }

    #[test]
    fn an_owned_connection_opens_files_without_repeating_its_message() {
        let connected = Facts {
            mounted: true,
            compatible: true,
            client: true,
            owns_mount: true,
            ..facts(State::Available)
        };
        let result = screen(&connected);
        assert_eq!((result.status.as_str(), result.tone), ("Connected", Tone::Ok));
        assert!(result.open && result.disconnect && result.detail.is_none() && !result.key_panel);
        let adopted = Facts {
            owns_mount: false,
            client: false,
            ..connected
        };
        assert!(screen(&adopted).detail.is_some());
    }

    fn row(identifier: &str, state: UploadState) -> Upload {
        Upload {
            identifier: identifier.into(),
            name: format!("{identifier}.bin"),
            destination: "Uploads".into(),
            size: 1,
            digest: "d".repeat(64),
            profile: "a".repeat(64),
            state,
            progress: 0,
            source_name: format!("{identifier}.bin"),
        }
    }

    #[test]
    fn the_connected_status_names_the_most_urgent_upload_state() {
        let connected = |uploads| {
            let result = screen(&Facts {
                uploads,
                ..facts(State::Available)
            });
            (result.status, result.tone)
        };
        assert_eq!(connected(Uploads::Idle), ("Connected".into(), Tone::Ok));
        assert_eq!(
            connected(Uploads::Busy),
            ("Connected · Uploading...".into(), Tone::Busy)
        );
        assert_eq!(
            connected(Uploads::Problem),
            ("Connected · Upload problem".into(), Tone::Error)
        );
        assert_eq!(
            connected(Uploads::Review),
            ("Connected · Review needed".into(), Tone::Warning)
        );
        let disconnected = Facts {
            uploads: Uploads::Review,
            ..facts(State::Disconnected)
        };
        assert_eq!(screen(&disconnected).status, "Disconnected");
        let items = [row("a", UploadState::Error), row("b", UploadState::Conflict)];
        assert_eq!(upload_status(&items, false, true), Uploads::Review);
        assert_eq!(upload_status(&items[..1], false, true), Uploads::Problem);
        assert_eq!(upload_status(&[], true, false), Uploads::Problem);
        assert_eq!(upload_status(&[], false, true), Uploads::Busy);
    }

    #[test]
    fn a_pending_disconnect_disables_cancel_and_disconnect() {
        let waiting = Facts {
            checking: true,
            disconnect_pending: true,
            ..facts(State::Connecting)
        };
        let result = screen(&waiting);
        assert!(result.cancel && !result.cancel_enabled && !result.disconnect);
    }

    #[test]
    fn upload_controls_follow_the_queue_and_the_selection() {
        let profile = "a".repeat(64);
        let items = [
            row("paused", UploadState::Paused),
            row("conflict", UploadState::Conflict),
        ];
        let queue = |selected, unlocked, busy| Queue {
            items: &items,
            selected,
            profile: &profile,
            unlocked,
            accepts_files: true,
            busy,
            storage_error: false,
        };
        let idle = upload_controls(&queue(Some("conflict"), true, false));
        assert_eq!(
            idle,
            Controls {
                add: true,
                resume: true,
                pause: false,
                skip: true,
                keep: true
            }
        );
        let busy = upload_controls(&queue(Some("conflict"), true, true));
        assert!(busy.pause && !busy.resume && !busy.skip && !busy.keep);
        let locked = upload_controls(&queue(Some("conflict"), false, false));
        assert!(locked.skip && !locked.keep && !locked.resume && !locked.add);
        assert!(!upload_controls(&queue(Some("paused"), true, false)).skip);
        let other = Queue {
            profile: "b",
            ..queue(Some("conflict"), true, false)
        };
        assert!(upload_controls(&other).skip && !upload_controls(&other).keep);
        let failed = Queue {
            storage_error: true,
            items: &[],
            ..queue(None, true, false)
        };
        assert!(upload_controls(&failed).resume);
        let queued = [row("queued", UploadState::Queued)];
        assert!(
            upload_controls(&Queue {
                items: &queued,
                ..queue(None, false, false)
            })
            .pause
        );
    }

    #[test]
    fn the_uploads_title_counts_pending_rows_and_alerts_name_new_decisions() {
        let items = [
            row("done", UploadState::Completed),
            row("skipped", UploadState::Skipped),
            row("conflict", UploadState::Conflict),
            row("paused", UploadState::Paused),
        ];
        assert_eq!(uploads_title(&items), "Uploads (2 pending)");
        assert_eq!(uploads_title(&items[..2]), "Uploads");
        assert_eq!(upload_state(UploadState::Verifying), "Verifying");
        assert_eq!(upload_alerts(&items, false), BTreeSet::from(["conflict".to_owned()]));
        assert_eq!(upload_alerts(&[], true), BTreeSet::from([String::new()]));
    }

    #[test]
    fn a_failed_disconnect_keeps_disconnect_available() {
        let failed = Facts {
            mounted: true,
            compatible: true,
            client: true,
            ..facts(State::Error)
        };
        assert!(screen(&failed).disconnect && !screen(&failed).key_panel);
        assert!(!screen(&facts(State::Disconnecting)).disconnect);
    }

    #[test]
    fn without_a_saved_session_the_window_offers_sign_in_instead_of_the_key() {
        let result = screen(&Facts {
            signed_in: false,
            ..facts(State::Disconnected)
        });
        assert_eq!(result.status, "Not signed in");
        assert!(result.welcome && !result.key_panel && !result.renew && result.detail.is_none());
    }

    #[test]
    fn renewal_is_offered_while_disconnected_or_connected_but_not_twice() {
        assert!(screen(&facts(State::Disconnected)).renew);
        let connected = Facts {
            mounted: true,
            compatible: true,
            client: true,
            owns_mount: true,
            ..facts(State::Available)
        };
        assert!(screen(&connected).renew);
        assert!(
            !screen(&Facts {
                renewing: true,
                ..connected
            })
            .renew
        );
        assert!(!screen(&facts(State::Connecting)).renew);
    }

    fn folder(identifier: &str, state: FolderState) -> SyncFolder {
        let mut folder = SyncFolder::default();
        folder.identifier = identifier.into();
        folder.local = format!("/home/fixture/iCloud Sync/{identifier}").into();
        folder.remote = identifier.into();
        folder.state = state;
        folder
    }

    #[test]
    fn the_connected_status_puts_reviews_and_problems_before_activity() {
        let status = |uploads, folders| {
            let result = screen(&Facts {
                uploads,
                folders,
                ..facts(State::Available)
            });
            (result.status, result.tone)
        };
        let review = ("Connected · Review needed".to_owned(), Tone::Warning);
        assert_eq!(status(Uploads::Busy, Folders::Review), review);
        assert_eq!(status(Uploads::Review, Folders::Problem), review);
        assert_eq!(
            status(Uploads::Problem, Folders::Problem),
            ("Connected · Sync problem".into(), Tone::Error)
        );
        assert_eq!(
            status(Uploads::Problem, Folders::Syncing),
            ("Connected · Upload problem".into(), Tone::Error)
        );
        assert_eq!(
            status(Uploads::Busy, Folders::Syncing),
            ("Connected · Syncing...".into(), Tone::Busy)
        );
        assert_eq!(
            status(Uploads::Busy, Folders::UpToDate),
            ("Connected · Uploading...".into(), Tone::Busy)
        );
        assert_eq!(
            status(Uploads::Idle, Folders::UpToDate),
            ("Connected · Up to date".into(), Tone::Ok)
        );
        assert_eq!(status(Uploads::Idle, Folders::None), ("Connected".into(), Tone::Ok));
        let mut folders = vec![folder("a", FolderState::UpToDate), folder("b", FolderState::Syncing)];
        assert_eq!(folder_status(&folders), Folders::Syncing);
        folders.push(folder("c", FolderState::Error));
        assert_eq!(folder_status(&folders), Folders::Problem);
        folders[0].attention = Some(Review::Confirm);
        assert_eq!(folder_status(&folders), Folders::Review);
        assert_eq!(folder_status(&[]), Folders::None);
    }

    #[test]
    fn folder_controls_follow_the_selection_and_the_connection() {
        let mut folders = vec![
            folder("Docs", FolderState::UpToDate),
            folder("Work", FolderState::NeedsReview),
        ];
        folders[0].message = "Up to date.".into();
        folders[1].attention = Some(Review::Repair);
        folders[1].message = "Fixture review.".into();
        let list = |selected, unlocked| FolderList {
            folders: &folders,
            selected,
            unlocked,
            busy: false,
            current: None,
            local_exists: true,
            backups_exist: false,
            message: "",
        };
        // Without a selection, Review and the details concern the folder that waits.
        let waiting = folder_controls(&list(None, true));
        assert_eq!(waiting.review.as_deref(), Some("Work"));
        assert!(waiting.add && waiting.review_enabled && !waiting.open && !waiting.stop && !waiting.sync_now);
        assert_eq!(waiting.detail, "Work: Fixture review.");
        let selected = folder_controls(&list(Some("Docs"), true));
        assert!(selected.open && selected.sync_now && selected.stop && !selected.backups);
        assert_eq!((selected.review, selected.detail.as_str()), (None, "Docs: Up to date."));
        let locked = folder_controls(&list(Some("Work"), false));
        assert!(!locked.add && !locked.sync_now && !locked.review_enabled && locked.review.is_some());
        let running = FolderList {
            busy: true,
            current: Some("Docs"),
            message: "The synced folder list could not be saved.",
            ..list(Some("Docs"), true)
        };
        let running = folder_controls(&running);
        assert!(!running.stop && !running.sync_now);
        assert!(
            running
                .detail
                .starts_with("The synced folder list could not be saved. Docs:")
        );
        let empty = FolderList {
            folders: &[],
            ..list(None, true)
        };
        assert_eq!(
            folder_controls(&empty).detail,
            "Add an iCloud Drive folder to edit it in Files or Dolphin; changes sync both ways."
        );
    }

    #[test]
    fn review_texts_name_the_folder_and_chosen_folders_are_relative_to_the_mount() {
        assert!(review_text(Review::Confirm, "Docs").starts_with("Apply the deletions or changes found in \"Docs\""));
        assert!(review_text(Review::Repair, "Docs").starts_with("Copy files between \"Docs\" and iCloud"));
        let mount = Path::new("/home/fixture/iCloud");
        assert_eq!(chosen_folder(mount, &mount.join("Documents/Work")), "Documents/Work");
        assert_eq!(chosen_folder(mount, mount), "");
        assert_eq!(chosen_folder(mount, Path::new("/home/fixture/Documents")), "");
        assert_eq!(folder_name(&folder("Docs", FolderState::Waiting)), "Docs");
    }
}
