//! App preferences in `settings.ini`, which the Python version of the app writes with Qt's `QSettings` in INI format.
//! Only the top-level keys below are read or changed; every other line stays as it is.
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::profile::{self, private_directory};
use crate::{Failure, Result};

/// The language choices; `auto` follows the desktop.
pub const LANGUAGES: [&str; 3] = ["auto", "en", "es"];
const LANGUAGE: &str = "language";
const UNLOCK: &str = "unlock_automatically";
const SIGNED_IN: &str = "apple_signed_in_at";

pub struct Settings {
    path: PathBuf,
}

impl Settings {
    /// The preferences in `path`, normally `Paths::settings()`.
    pub fn new(path: PathBuf) -> Settings {
        Settings { path }
    }

    /// The saved language choice; anything else counts as `auto`.
    pub fn language(&self) -> &'static str {
        let saved = self.value(LANGUAGE);
        LANGUAGES
            .into_iter()
            .find(|choice| saved.as_deref() == Some(*choice))
            .unwrap_or("auto")
    }

    pub fn set_language(&self, choice: &str) -> Result<()> {
        if !LANGUAGES.contains(&choice) {
            return Err(Failure::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unknown language",
            )));
        }
        self.change(&[(LANGUAGE, Some(choice))])
    }

    /// Whether to connect with the key saved in the desktop keyring. As in the Python app, only an empty value or
    /// `false` in any letter case is off.
    pub fn unlock_automatically(&self) -> bool {
        self.value(UNLOCK)
            .is_some_and(|value| !value.is_empty() && !value.eq_ignore_ascii_case("false"))
    }

    pub fn set_unlock_automatically(&self, enabled: bool) -> Result<()> {
        self.change(&[(UNLOCK, Some(if enabled { "true" } else { "false" }))])
    }

    /// When the app last signed in to the Apple account, in seconds since 1970, if it was recorded.
    pub fn apple_signed_in_at(&self) -> Option<i64> {
        self.value(SIGNED_IN)?.parse().ok().filter(|seconds| *seconds != 0)
    }

    pub fn set_apple_signed_in_at(&self, seconds: i64) -> Result<()> {
        self.change(&[(SIGNED_IN, Some(&seconds.to_string()))])
    }

    /// Forget the sign-in time and automatic unlock, as signing out does; the language stays.
    pub fn forget_account(&self) -> Result<()> {
        self.change(&[(SIGNED_IN, None), (UNLOCK, None)])
    }

    /// The last top-level value of `key`, as Qt reads it.
    fn value(&self, key: &str) -> Option<String> {
        let text = fs::read_to_string(&self.path).ok()?;
        let mut top_level = true;
        let mut found = None;
        for line in text.lines() {
            if let Some(general) = section(line) {
                top_level = general;
            } else if let Some((_, raw)) = pair(line).filter(|(name, _)| top_level && *name == key) {
                found = Some(unquote(raw));
            }
        }
        found
    }

    /// Set or remove (`None`) top-level keys, then replace the file atomically with a private copy.
    fn change(&self, changes: &[(&str, Option<&str>)]) -> Result<()> {
        let text = match fs::read_to_string(&self.path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
            result => result?,
        };
        let mut kept = Vec::new();
        let mut top_level = true;
        let mut header = None;
        for line in text.split_inclusive('\n') {
            if let Some(general) = section(line) {
                top_level = general;
                if general && header.is_none() {
                    header = Some(kept.len());
                }
            } else if top_level && pair(line).is_some_and(|(name, _)| changes.iter().any(|(key, _)| *key == name)) {
                continue;
            }
            kept.push(line);
        }
        let added: String = changes
            .iter()
            .filter_map(|(key, value)| Some(format!("{key}={}\n", (*value)?)))
            .collect();
        let mut output = String::with_capacity(text.len() + added.len() + 11);
        if header.is_none() && !added.is_empty() {
            // Qt writes top-level keys under [General]; keys already above any section stay top-level below it.
            output.push_str("[General]\n");
            output.push_str(&added);
            if !kept.is_empty() {
                output.push('\n');
            }
        }
        for (index, line) in kept.iter().enumerate() {
            output.push_str(line);
            if header == Some(index) {
                if !line.ends_with('\n') {
                    output.push('\n');
                }
                output.push_str(&added);
            }
        }
        if output == text {
            return Ok(());
        }
        private_directory(self.path.parent().unwrap_or(Path::new("/")))?;
        Ok(profile::replace(&self.path, output.as_bytes(), 0o600)?)
    }
}

/// For a section header, whether it holds the top-level keys, which Qt writes under `[General]`.
fn section(line: &str) -> Option<bool> {
    let (name, _) = line.trim_start().strip_prefix('[')?.split_once(']')?;
    Some(name.eq_ignore_ascii_case("general"))
}

/// The key and raw value of a `key=value` line.
fn pair(line: &str) -> Option<(&str, &str)> {
    let (key, value) = line.split_once('=')?;
    Some((key.trim(), value))
}

/// A simple value as Qt reads it: trimmed, without its quotes or a `;` comment after it.
fn unquote(raw: &str) -> String {
    let raw = raw.trim();
    match raw.strip_prefix('"').and_then(|inner| inner.split_once('"')) {
        Some((inner, _)) => inner.to_owned(),
        None => raw.split(';').next().unwrap_or_default().trim_end().to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    /// Written by PySide6's `QSettings` in INI format, with keys of other features and a group named General.
    const QT: &str = "[General]\napple_signed_in_at=1794130392\nlanguage=es\nnote=\"a, b; c = d\"\n\
                      unlock_automatically=true\n\n[%General]\ninner=x\n\n[window]\nsize=560x600\n";

    fn fixture(text: Option<&str>) -> (tempfile::TempDir, Settings) {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("icloud-linux/settings.ini");
        if let Some(text) = text {
            fs::create_dir(path.parent().unwrap()).unwrap();
            fs::write(&path, text).unwrap();
        }
        (root, Settings::new(path))
    }

    fn saved(settings: &Settings) -> String {
        fs::read_to_string(&settings.path).unwrap()
    }

    #[test]
    fn a_file_written_by_qt_is_read_and_keeps_its_other_keys() {
        let (root, settings) = fixture(Some(QT));
        assert_eq!(settings.language(), "es");
        assert!(settings.unlock_automatically());
        assert_eq!(settings.apple_signed_in_at(), Some(1_794_130_392));
        settings.set_language("en").unwrap();
        let expected = QT
            .replace("language=es\n", "")
            .replace("[General]\n", "[General]\nlanguage=en\n");
        assert_eq!(saved(&settings), expected);
        assert_eq!(fs::metadata(&settings.path).unwrap().mode() & 0o777, 0o600);
        assert_eq!(fs::read_dir(root.path().join("icloud-linux")).unwrap().count(), 1);
        settings.set_language("en").unwrap();
        assert_eq!(saved(&settings), expected);
    }

    #[test]
    fn a_missing_file_has_defaults_and_is_created_private() {
        let (root, settings) = fixture(None);
        assert_eq!(
            (
                settings.language(),
                settings.unlock_automatically(),
                settings.apple_signed_in_at()
            ),
            ("auto", false, None)
        );
        settings.forget_account().unwrap();
        assert!(!settings.path.exists());
        settings.set_unlock_automatically(true).unwrap();
        assert_eq!(saved(&settings), "[General]\nunlock_automatically=true\n");
        assert_eq!(fs::metadata(&settings.path).unwrap().mode() & 0o777, 0o600);
        let folder = root.path().join("icloud-linux");
        assert_eq!(fs::metadata(folder).unwrap().mode() & 0o777, 0o700);
    }

    #[test]
    fn values_are_read_like_the_python_app_reads_them() {
        let (_root, settings) = fixture(Some(
            "language=en\n[General]\nunlock_automatically=0\n[general]\napple_signed_in_at = 5 ; noted\n\
             [Other]\nlanguage=es\n",
        ));
        assert_eq!(settings.language(), "en");
        // PySide6 reads every value other than an empty one or `false` as true, `0` included.
        assert!(settings.unlock_automatically());
        assert_eq!(settings.apple_signed_in_at(), Some(5));
        settings.forget_account().unwrap();
        assert_eq!(
            saved(&settings),
            "language=en\n[General]\n[general]\n[Other]\nlanguage=es\n"
        );
        settings.set_language("es").unwrap();
        assert_eq!(
            saved(&settings),
            "[General]\nlanguage=es\n[general]\n[Other]\nlanguage=es\n"
        );
        for (value, enabled) in [("FALSE", false), ("", false), ("\"true\"", true)] {
            fs::write(&settings.path, format!("[General]\nunlock_automatically={value}\n")).unwrap();
            assert_eq!(settings.unlock_automatically(), enabled, "{value}");
        }
        fs::write(&settings.path, "[General]\nlanguage=fr\napple_signed_in_at=soon\n").unwrap();
        assert_eq!((settings.language(), settings.apple_signed_in_at()), ("auto", None));
        assert!(settings.set_language("fr\n[General]").is_err());
    }
}
