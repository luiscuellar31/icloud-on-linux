//! A plain-text report for support. It never includes keys, tokens, file contents, or file and folder names.
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

use regex::Regex;

use crate::process::without_rclone_variables;

/// The computer the app runs on.
pub struct System {
    pub distribution: String,
    pub desktop: String,
    pub rclone: String,
    pub fuse: bool,
    pub installation: &'static str,
}

impl System {
    /// Read the system's facts. `program` is the app's own path, which tells how it was installed.
    pub fn detect(program: &Path) -> System {
        let distribution = fs::read_to_string("/etc/os-release")
            .ok()
            .and_then(|text| {
                text.lines()
                    .find_map(|line| line.strip_prefix("PRETTY_NAME="))
                    .map(|name| name.trim_matches('"').to_owned())
            })
            .unwrap_or_else(|| env::consts::OS.to_owned());
        let variable = |name: &str| env::var(name).unwrap_or_else(|_| "unknown".to_owned());
        let mut rclone = Command::new("rclone");
        without_rclone_variables(&mut rclone).arg("version");
        let rclone = rclone
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| {
                String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .next()
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "unavailable".to_owned());
        let fuse = Path::new("/dev/fuse").exists()
            && env::split_paths(&env::var_os("PATH").unwrap_or_default())
                .any(|folder| folder.join("fusermount3").is_file());
        System {
            distribution,
            desktop: format!("{} ({})", variable("XDG_CURRENT_DESKTOP"), variable("XDG_SESSION_TYPE")),
            rclone,
            fuse,
            installation: installation(program, &crate::paths::home()),
        }
    }
}

fn installation(program: &Path, home: &Path) -> &'static str {
    if program.starts_with("/usr") {
        "package"
    } else if program.starts_with(home.join(".local/bin")) {
        "user installation"
    } else {
        "source checkout"
    }
}

/// The connection, as the session reports it.
pub struct Connection {
    pub saved_session: bool,
    pub state: &'static str,
    pub message: String,
    pub owns_mount: bool,
    pub reconnecting: bool,
}

/// One synced folder: its English state name and counts, never its name.
pub struct Folder {
    pub state: &'static str,
    pub items: Option<usize>,
    pub pending: usize,
    pub conflicts: usize,
    pub review: Option<&'static str>,
    pub message: String,
}

pub struct Folders {
    pub folders: Vec<Folder>,
    pub message: String,
}

/// The upload queue: the English state of each row, never its file or folder.
pub struct Uploads {
    pub states: Vec<&'static str>,
    pub storage_problem: bool,
    pub messages: Vec<String>,
}

static QUOTED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""[^"]*""#).unwrap());

/// Hide names a message may quote, such as two files whose names differ only in letter case.
fn scrub(text: &str, home: &str) -> String {
    let text = QUOTED.replace_all(text, "\"(name)\"");
    if home.is_empty() {
        text.into_owned()
    } else {
        text.replace(home, "~")
    }
}

/// A UTC time as `YYYY-MM-DD HH:MM`.
fn utc_minutes(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let (hour, minute) = (seconds % 86_400 / 3_600, seconds % 3_600 / 60);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}")
}

/// Describe versions, settings, and states. `details` are (label, value) pairs that the window adds; `home` is
/// replaced with `~` in messages.
pub fn report(
    system: &System,
    details: &[(String, String)],
    connection: &Connection,
    folders: Option<&Folders>,
    uploads: Option<&Uploads>,
    home: &str,
) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|time| time.as_secs())
        .unwrap_or_default();
    let yes_no = |value: bool| if value { "yes" } else { "no" };
    let mut lines = vec![
        "iCloud on Linux diagnostic report".to_owned(),
        format!("Created: {} UTC", utc_minutes(now)),
        "It has no keys, tokens, file contents, or file and folder names.".to_owned(),
        String::new(),
        "System".to_owned(),
        format!("  Distribution: {}", system.distribution),
        format!("  Desktop: {}", system.desktop),
        format!("  rclone: {}", system.rclone),
        format!("  FUSE 3: {}", if system.fuse { "available" } else { "not available" }),
        format!("  Installed from: {}", system.installation),
        String::new(),
        "App".to_owned(),
    ];
    lines.extend(details.iter().map(|(label, value)| format!("  {label}: {value}")));
    lines.extend([
        String::new(),
        "Connection".to_owned(),
        format!(
            "  Saved session: {}",
            if connection.saved_session { "present" } else { "none" }
        ),
        format!("  State: {}", connection.state),
        format!("  Message: {}", scrub(&connection.message, home)),
        format!("  Connection owned by the app: {}", yes_no(connection.owns_mount)),
        format!("  Automatic reconnection waiting: {}", yes_no(connection.reconnecting)),
        String::new(),
    ]);
    match folders {
        None => lines.push("Synced folders: the list could not be opened".to_owned()),
        Some(folders) => {
            lines.push(format!("Synced folders: {}", folders.folders.len()));
            if !folders.message.is_empty() {
                lines.push(format!("  Message: {}", scrub(&folders.message, home)));
            }
            for (number, folder) in folders.folders.iter().enumerate() {
                let items = folder.items.map_or("unknown".to_owned(), |items| items.to_string());
                lines.push(format!(
                    "  Folder {}: {}; items {items}; pending changes {}; conflict copies {}; review {}",
                    number + 1,
                    folder.state,
                    folder.pending,
                    folder.conflicts,
                    folder.review.unwrap_or("none")
                ));
                if !folder.message.is_empty() {
                    lines.push(format!("    Message: {}", scrub(&folder.message, home)));
                }
            }
        }
    }
    lines.push(String::new());
    match uploads {
        None => lines.push("Uploads: the queue could not be opened".to_owned()),
        Some(uploads) => {
            let mut states = BTreeMap::new();
            for state in &uploads.states {
                *states.entry(*state).or_insert(0) += 1;
            }
            let counts: Vec<String> = states.iter().map(|(state, count)| format!("{state} {count}")).collect();
            let summary = if counts.is_empty() {
                "none".to_owned()
            } else {
                counts.join(", ")
            };
            lines.push(format!("Uploads: {summary}"));
            lines.push(format!("  Storage problem: {}", yes_no(uploads.storage_problem)));
            for message in uploads.messages.iter().filter(|message| !message.is_empty()) {
                lines.push(format!("  Message: {}", scrub(message, home)));
            }
        }
    }
    lines.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn system() -> System {
        System {
            distribution: "Fixture Linux 1".into(),
            desktop: "GNOME (wayland)".into(),
            rclone: "rclone v1.75.2".into(),
            fuse: true,
            installation: "package",
        }
    }

    #[test]
    fn the_report_describes_states_without_names_keys_or_contents() {
        let home = "/home/fixture-user";
        let connection = Connection {
            saved_session: true,
            state: "Available",
            message: "iCloud Drive is ready to open in your file manager.".into(),
            owns_mount: true,
            reconnecting: false,
        };
        let folders = Folders {
            folders: vec![Folder {
                state: "Error",
                items: Some(1),
                pending: 2,
                conflicts: 1,
                review: None,
                message: "\"Plan.txt\" and \"plan.txt\" differ only in letter case, which iCloud Drive treats as the same name. See /home/fixture-user/iCloud Sync/Secret Project.".into(),
            }],
            message: String::new(),
        };
        let uploads = Uploads {
            states: vec!["Paused", "Paused"],
            storage_problem: false,
            messages: vec![
                "Uploads paused. Pending local copies are retained.".into(),
                String::new(),
            ],
        };
        let details = [("Language".to_owned(), "auto (showing es)".to_owned())];
        let text = report(&system(), &details, &connection, Some(&folders), Some(&uploads), home);
        for private in ["Plan.txt", "plan.txt", home] {
            assert!(!text.contains(private), "{private} appears in the report");
        }
        for expected in [
            "Saved session: present",
            "State: Available",
            "Folder 1: Error; items 1; pending changes 2; conflict copies 1; review none",
            "\"(name)\" and \"(name)\" differ only in letter case",
            "~/iCloud Sync",
            "Uploads: Paused 2",
            "Language: auto (showing es)",
            "rclone: rclone v1.75.2",
            "Installed from: package",
        ] {
            assert!(text.contains(expected), "{expected} is missing from:\n{text}");
        }
    }

    #[test]
    fn unavailable_parts_are_reported_instead_of_failing() {
        let connection = Connection {
            saved_session: false,
            state: "Disconnected",
            message: String::new(),
            owns_mount: false,
            reconnecting: true,
        };
        let text = report(&system(), &[], &connection, None, None, "");
        assert!(text.contains("Synced folders: the list could not be opened"));
        assert!(text.contains("Uploads: the queue could not be opened"));
        assert!(text.contains("Automatic reconnection waiting: yes"));
        assert!(text.contains("Saved session: none"));
    }

    #[test]
    fn times_and_installations_are_named_plainly() {
        assert_eq!(utc_minutes(0), "1970-01-01 00:00");
        assert_eq!(utc_minutes(1_794_128_400), "2026-11-08 09:00");
        assert_eq!(utc_minutes(951_782_400), "2000-02-29 00:00");
        let home = Path::new("/home/fixture-user");
        assert_eq!(installation(Path::new("/usr/bin/icloud-on-linux"), home), "package");
        assert_eq!(
            installation(Path::new("/home/fixture-user/.local/bin/icloud-on-linux"), home),
            "user installation"
        );
        assert_eq!(
            installation(Path::new("/home/fixture-user/src/target/debug/icloud-on-linux"), home),
            "source checkout"
        );
    }
}
