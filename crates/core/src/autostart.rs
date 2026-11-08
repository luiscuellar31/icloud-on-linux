//! Start the app at login through an XDG autostart entry copied from the installed application entry.
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::paths::home;
use crate::places;

pub const NAME: &str = "io.github.luiscuellar31.IcloudOnLinux.desktop";
/// The entry that the Python version of the app writes.
const LEGACY: &str = "icloud-drive.desktop";
/// The command of the packaged app, which both versions install.
const COMMAND: &str = "icloud-on-linux";

/// Where the autostart entry goes and where the installed application entry is looked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Autostart {
    pub entry: PathBuf,
    /// The Python app's entry. It starts this app when it runs `launcher` or the packaged command, which this app
    /// replaced, so it counts as this app's entry.
    pub legacy: PathBuf,
    /// The launcher of the user installation, in `~/.local/bin`.
    pub launcher: PathBuf,
    /// The `applications` folders of `XDG_DATA_HOME` and then `XDG_DATA_DIRS`.
    pub applications: Vec<PathBuf>,
}

impl Autostart {
    pub fn for_user() -> Autostart {
        let variable = |name| env::var_os(name).filter(|value| !value.is_empty());
        let config = variable("XDG_CONFIG_HOME").map_or_else(|| home().join(".config"), PathBuf::from);
        let data = variable("XDG_DATA_HOME").map_or_else(|| home().join(".local/share"), PathBuf::from);
        let system = variable("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into());
        let folders = env::split_paths(&system).filter(|folder| !folder.as_os_str().is_empty());
        Autostart {
            entry: config.join("autostart").join(NAME),
            legacy: config.join("autostart").join(LEGACY),
            launcher: home().join(".local/bin").join(COMMAND),
            applications: std::iter::once(data)
                .chain(folders)
                .map(|folder| folder.join("applications"))
                .collect(),
        }
    }

    /// The installed application entry, or `None` when the app runs from a source checkout.
    pub fn installed(&self) -> Option<PathBuf> {
        self.applications
            .iter()
            .map(|folder| folder.join(NAME))
            .find(|path| path.is_file())
    }

    pub fn enabled(&self) -> bool {
        self.entry.is_file() || self.legacy_is_ours()
    }

    /// Copy the installed entry so the login session starts the app with `--background`.
    pub fn enable(&self) -> io::Result<()> {
        let source = self
            .installed()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, NAME))?;
        // The app decides at startup whether it can stay in the background without its window.
        let text: String = fs::read_to_string(source)?
            .lines()
            .map(|line| {
                let background = if line.starts_with("Exec=") { " --background" } else { "" };
                format!("{line}{background}\n")
            })
            .collect();
        if let Some(folder) = self.entry.parent() {
            fs::create_dir_all(folder)?;
        }
        places::replace(&self.entry, &text)
    }

    /// Remove the entry, and the Python app's entry when it starts this app.
    pub fn disable(&self) -> io::Result<()> {
        if self.legacy_is_ours() {
            remove(&self.legacy)?;
        }
        remove(&self.entry)
    }

    /// Whether the Python app's entry runs this app: its command is the user launcher or the packaged command, quoted
    /// or not.
    fn legacy_is_ours(&self) -> bool {
        let Ok(text) = fs::read_to_string(&self.legacy) else {
            return false;
        };
        let launcher = self.launcher.to_string_lossy();
        let programs = [format!("\"{launcher}\""), launcher.into_owned(), COMMAND.to_owned()];
        text.lines().filter_map(|line| line.strip_prefix("Exec=")).any(|exec| {
            programs.iter().any(|program| {
                exec.strip_prefix(program.as_str())
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with(' '))
            })
        })
    }
}

fn remove(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENTRY: &str =
        "[Desktop Entry]\nType=Application\nName=iCloud on Linux\nExec=\"/home/fixture/.local/bin/icloud-on-linux\"\n";

    fn fixture() -> (tempfile::TempDir, Autostart) {
        let root = tempfile::tempdir().unwrap();
        let autostart = Autostart {
            entry: root.path().join("config/autostart").join(NAME),
            legacy: root.path().join("config/autostart").join(LEGACY),
            launcher: PathBuf::from("/home/fixture/.local/bin/icloud-on-linux"),
            applications: vec![
                root.path().join("data/applications"),
                root.path().join("system/applications"),
            ],
        };
        (root, autostart)
    }

    fn write(root: &Path, folder: &str, text: &str) {
        let path = root.join(folder).join("applications").join(NAME);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn the_user_entry_is_preferred_and_started_in_the_background() {
        let (root, autostart) = fixture();
        let packaged = ENTRY.replace("\"/home/fixture/.local/bin/icloud-on-linux\"", "icloud-on-linux");
        write(root.path(), "system", &packaged);
        write(root.path(), "data", ENTRY);
        assert!(!autostart.enabled());
        autostart.enable().unwrap();
        assert!(autostart.enabled());
        let text = fs::read_to_string(&autostart.entry).unwrap();
        assert!(text.contains("Exec=\"/home/fixture/.local/bin/icloud-on-linux\" --background\n"));
        assert!(text.contains("Name=iCloud on Linux\n"));
        autostart.disable().unwrap();
        assert!(!autostart.enabled());
        autostart.disable().unwrap();
    }

    #[test]
    fn a_packaged_entry_is_used_when_there_is_no_user_entry() {
        let (root, autostart) = fixture();
        let packaged = ENTRY.replace("\"/home/fixture/.local/bin/icloud-on-linux\"", "icloud-on-linux");
        write(root.path(), "system", &packaged);
        autostart.enable().unwrap();
        let text = fs::read_to_string(&autostart.entry).unwrap();
        assert!(text.contains("Exec=icloud-on-linux --background\n"));
    }

    #[test]
    fn a_source_checkout_cannot_start_at_login() {
        let (_root, autostart) = fixture();
        assert_eq!(autostart.installed(), None);
        assert_eq!(autostart.enable().unwrap_err().kind(), io::ErrorKind::NotFound);
        assert!(!autostart.enabled());
    }

    #[test]
    fn the_python_entry_counts_as_this_app_when_it_runs_this_app() {
        let (root, autostart) = fixture();
        fs::create_dir_all(root.path().join("config/autostart")).unwrap();
        for exec in [
            "\"/home/fixture/.local/bin/icloud-on-linux\" --background",
            "/home/fixture/.local/bin/icloud-on-linux --background",
            "icloud-on-linux --background",
            "icloud-on-linux",
        ] {
            fs::write(
                &autostart.legacy,
                ENTRY.replace("\"/home/fixture/.local/bin/icloud-on-linux\"", exec),
            )
            .unwrap();
            assert!(autostart.enabled(), "{exec}");
            autostart.disable().unwrap();
            assert!(!autostart.legacy.exists() && !autostart.enabled(), "{exec}");
        }
    }

    #[test]
    fn another_programs_entry_with_the_python_name_is_kept() {
        let (root, autostart) = fixture();
        fs::create_dir_all(root.path().join("config/autostart")).unwrap();
        for exec in [
            "/opt/other/icloud-on-linux --background",
            "icloud-on-linux-helper",
            "env icloud-on-linux",
        ] {
            fs::write(
                &autostart.legacy,
                ENTRY.replace("\"/home/fixture/.local/bin/icloud-on-linux\"", exec),
            )
            .unwrap();
            assert!(!autostart.enabled(), "{exec}");
            autostart.disable().unwrap();
            assert!(autostart.legacy.exists(), "{exec}");
        }
    }
}
