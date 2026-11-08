//! Sidebar places in GTK file managers and Dolphin, for the editable copies of synced folders.
use std::env;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use regex::Regex;

use crate::paths::home;
use crate::profile;

const ICON: &str = "  <info>\n   <metadata owner=\"http://freedesktop.org\">\n    <bookmark:icon name=\"folder-cloud\"/>\n   \
                    </metadata>\n  </info>\n";

/// The desktop settings files that hold sidebar places.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Places {
    /// GTK bookmarks, shown by Files and other GTK file managers.
    pub gtk: PathBuf,
    /// Dolphin places, extended only after Dolphin created them.
    pub kde: PathBuf,
}

impl Places {
    /// The current user's files.
    pub fn for_user() -> Places {
        let base = |name: &str, fallback: &str| {
            env::var_os(name)
                .filter(|value| !value.is_empty())
                .map_or_else(|| home().join(fallback), PathBuf::from)
        };
        Places {
            gtk: base("XDG_CONFIG_HOME", ".config").join("gtk-3.0/bookmarks"),
            kde: base("XDG_DATA_HOME", ".local/share").join("user-places.xbel"),
        }
    }

    /// Add a place for `folder` unless one exists.
    pub fn add(&self, folder: &Path, label: &str) -> io::Result<()> {
        let uri = uri(folder)?;
        let text = read(&self.gtk)?;
        if !text.lines().any(|line| line.split(' ').next() == Some(&uri)) {
            if let Some(parent) = self.gtk.parent() {
                fs::create_dir_all(parent)?;
            }
            let separator = if text.is_empty() || text.ends_with('\n') {
                ""
            } else {
                "\n"
            };
            replace(&self.gtk, &format!("{text}{separator}{uri} {label}\n"))?;
        }
        // Dolphin creates its default places with this file; creating it here would hide them.
        if self.kde.exists() {
            let text = fs::read_to_string(&self.kde)?;
            let href = format!("href=\"{uri}\"");
            if let Some(end) = text.rfind("</xbel>").filter(|_| !text.contains(&href)) {
                let icon = if text.contains("xmlns:bookmark=") { ICON } else { "" };
                let entry = format!(
                    " <bookmark {href}>\n  <title>{}</title>\n{icon} </bookmark>\n",
                    escape(label)
                );
                replace(&self.kde, &format!("{}{entry}{}", &text[..end], &text[end..]))?;
            }
        }
        Ok(())
    }

    /// Remove the place for `folder`, keeping every other entry.
    pub fn remove(&self, folder: &Path) -> io::Result<()> {
        let uri = uri(folder)?;
        let text = read(&self.gtk)?;
        let kept: String = text
            .split_inclusive('\n')
            .filter(|line| line.split(' ').next().map(str::trim) != Some(&uri))
            .collect();
        if kept != text {
            replace(&self.gtk, &kept)?;
        }
        let text = read(&self.kde)?;
        let href = regex::escape(&format!("href=\"{uri}\""));
        let entry =
            Regex::new(&format!(r"(?s)[ \t]*<bookmark {href}>.*?</bookmark>[ \t]*\n?")).map_err(io::Error::other)?;
        let changed = entry.replace(&text, "");
        if changed != text {
            replace(&self.kde, &changed)?;
        }
        Ok(())
    }
}

/// The `file://` URI of an absolute path, percent-encoded like Python's `Path.as_uri()` so existing entries match.
fn uri(folder: &Path) -> io::Result<String> {
    if !folder.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a place needs an absolute path",
        ));
    }
    let mut uri = String::from("file://");
    for &byte in folder.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"/_.-~".contains(&byte) {
            uri.push(char::from(byte));
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    Ok(uri)
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The text of a settings file, or nothing when it does not exist.
fn read(path: &Path) -> io::Result<String> {
    match fs::read_to_string(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        result => result,
    }
}

/// Replace a desktop settings file atomically, keeping its permissions.
fn replace(path: &Path, text: &str) -> io::Result<()> {
    let mode = fs::metadata(path).map_or(0o600, |info| info.permissions().mode() & 0o777);
    profile::replace(path, text.as_bytes(), mode)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::Permissions;

    const KDE: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE xbel>\n\
                       <xbel xmlns:bookmark=\"http://www.freedesktop.org/standards/desktop-bookmarks\">\n \
                       <bookmark href=\"file:///home/fixture\">\n  <title>Home</title>\n </bookmark>\n</xbel>\n";

    fn fixture() -> (tempfile::TempDir, Places, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let places = Places {
            gtk: root.path().join("config/gtk-3.0/bookmarks"),
            kde: root.path().join("data/user-places.xbel"),
        };
        let folder = root.path().join("iCloud Sync & more");
        (root, places, folder)
    }

    #[test]
    fn uris_are_encoded_like_python_paths() {
        let encoded = uri(Path::new("/tmp/iCloud Sync & more/é~_.-x")).unwrap();
        assert_eq!(encoded, "file:///tmp/iCloud%20Sync%20%26%20more/%C3%A9~_.-x");
        assert!(uri(Path::new("relative")).is_err());
    }

    #[test]
    fn gtk_bookmark_is_added_once_and_keeps_existing_entries_and_mode() {
        let (_root, places, folder) = fixture();
        fs::create_dir_all(places.gtk.parent().unwrap()).unwrap();
        fs::write(&places.gtk, "file:///home/fixture/iCloud iCloud Drive").unwrap();
        fs::set_permissions(&places.gtk, Permissions::from_mode(0o600)).unwrap();
        places.add(&folder, "iCloud Sync").unwrap();
        places.add(&folder, "iCloud Sync").unwrap();
        let expected = format!(
            "file:///home/fixture/iCloud iCloud Drive\n{} iCloud Sync\n",
            uri(&folder).unwrap()
        );
        assert_eq!(fs::read_to_string(&places.gtk).unwrap(), expected);
        assert_eq!(fs::metadata(&places.gtk).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn dolphin_places_are_only_extended_when_dolphin_already_created_them() {
        let (_root, places, folder) = fixture();
        places.add(&folder, "iCloud Sync").unwrap();
        assert!(!places.kde.exists());
        fs::create_dir_all(places.kde.parent().unwrap()).unwrap();
        fs::write(&places.kde, KDE).unwrap();
        places.add(&folder, "iCloud Sync").unwrap();
        places.add(&folder, "iCloud Sync").unwrap();
        let entry = format!(
            " <bookmark href=\"{}\">\n  <title>iCloud Sync</title>\n{ICON} </bookmark>\n",
            uri(&folder).unwrap()
        );
        let expected = KDE.replace("</xbel>", &format!("{entry}</xbel>"));
        assert_eq!(fs::read_to_string(&places.kde).unwrap(), expected);
    }

    #[test]
    fn removing_a_place_keeps_every_other_entry() {
        let (_root, places, folder) = fixture();
        fs::create_dir_all(places.gtk.parent().unwrap()).unwrap();
        fs::write(&places.gtk, "file:///home/fixture/iCloud iCloud Drive\n").unwrap();
        fs::create_dir_all(places.kde.parent().unwrap()).unwrap();
        fs::write(&places.kde, KDE).unwrap();
        places.add(&folder, "iCloud Sync").unwrap();
        places.remove(&folder).unwrap();
        places.remove(&folder).unwrap();
        assert_eq!(
            fs::read_to_string(&places.gtk).unwrap(),
            "file:///home/fixture/iCloud iCloud Drive\n"
        );
        assert_eq!(fs::read_to_string(&places.kde).unwrap(), KDE);
    }
}
