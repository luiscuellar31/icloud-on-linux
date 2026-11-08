//! What the window shows for a given connection and upload queue, kept apart from the widgets so it can be tested
//! without a display.
use std::collections::BTreeSet;

use icloud_core::session::State;
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
    pub key_typed: bool,
    pub signed_in: bool,
    pub renewing: bool,
    pub uploads: Uploads,
    /// The window waits for uploads to stop before disconnecting.
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
        State::Available => match facts.uploads {
            Uploads::Review => (connected(tr!("Review needed")), Tone::Warning),
            Uploads::Problem => (connected(tr!("Upload problem")), Tone::Error),
            Uploads::Busy => (connected(tr!("Uploading...")), Tone::Busy),
            Uploads::Idle => (tr!("Connected"), Tone::Ok),
        },
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
        key_panel: can_connect && facts.signed_in,
        connect: can_connect && facts.key_typed,
        open_visible: available,
        open: available && facts.mounted && facts.compatible,
        cancel: facts.state == State::Connecting,
        cancel_enabled: !facts.disconnect_pending,
        disconnect: !facts.disconnect_pending
            && facts.state != State::Disconnecting
            && (facts.mounted || facts.checking || facts.client),
        renew: facts.signed_in && !facts.renewing && (can_connect || available),
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
}
