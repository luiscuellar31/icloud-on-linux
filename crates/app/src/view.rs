//! What the window shows for a given connection state, kept apart from the widgets so it can be tested without a
//! display.
use icloud_core::session::State;
use icloud_core::tr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Ok,
    Busy,
    Error,
    Off,
}

impl Tone {
    /// The libadwaita style class for the status dot.
    pub fn style(self) -> &'static str {
        match self {
            Tone::Ok => "success",
            Tone::Busy => "accent",
            Tone::Error => "error",
            Tone::Off => "dim-label",
        }
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
}

#[derive(Debug, PartialEq, Eq)]
pub struct Screen {
    pub status: String,
    pub tone: Tone,
    pub detail: Option<String>,
    pub progress: bool,
    pub key_panel: bool,
    pub connect: bool,
    pub open_visible: bool,
    pub open: bool,
    pub cancel: bool,
    pub disconnect: bool,
}

pub fn screen(facts: &Facts) -> Screen {
    let busy = matches!(facts.state, State::Connecting | State::Disconnecting);
    let available = facts.state == State::Available;
    let can_connect = !busy && !facts.mounted && !facts.client;
    let (status, tone) = match facts.state {
        State::Available => (tr!("Connected"), Tone::Ok),
        State::Error => (tr!("Needs attention"), Tone::Error),
        State::Connecting => (tr!("Connecting..."), Tone::Busy),
        State::Disconnecting => (tr!("Disconnecting..."), Tone::Busy),
        State::Disconnected => (tr!("Disconnected"), Tone::Off),
    };
    // The status line already says a normal connection is ready; the message explains anything else.
    let detail = (!facts.message.is_empty() && !(available && facts.owns_mount)).then(|| facts.message.clone());
    Screen {
        status,
        tone,
        detail,
        progress: busy,
        key_panel: can_connect,
        connect: can_connect && facts.key_typed,
        open_visible: available,
        open: available && facts.mounted && facts.compatible,
        cancel: facts.state == State::Connecting,
        disconnect: facts.state != State::Disconnecting && (facts.mounted || facts.checking || facts.client),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(state: State) -> Facts {
        Facts {
            state,
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
}
