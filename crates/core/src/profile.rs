//! The encrypted rclone profile, private folders, and the kernel's view of mounts.
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions, Permissions};
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use nix::fcntl::OFlag;
use regex::Regex;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{Failure, Result, tr};

static DEVICE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^icloud-on-linux-[0-9a-f]{32}$").unwrap());
static OCTAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\([0-7]{3})").unwrap());
static DIGEST: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-f0-9]{64}$").unwrap());

/// One line of /proc/self/mountinfo for the mount point.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mount {
    pub identifier: u64,
    pub path: PathBuf,
    pub source: String,
    pub filesystem: String,
    pub options: BTreeSet<String>,
}

impl Mount {
    /// A read-only rclone mount of this user, from the app or the terminal lab.
    pub fn compatible(&self) -> bool {
        (self.source == "icloud:" || DEVICE.is_match(&self.source))
            && self.filesystem == "fuse.rclone"
            && self.options.contains("ro")
            && self.options.contains(&format!("user_id={}", nix::unistd::getuid()))
    }
}

fn unescape(value: &str) -> String {
    OCTAL
        .replace_all(value, |captures: &regex::Captures| {
            char::from(u8::from_str_radix(&captures[1], 8).unwrap_or(b'?')).to_string()
        })
        .into_owned()
}

/// The newest mount at `path` in a mountinfo text.
pub fn parse_mount(path: &Path, text: &str) -> Option<Mount> {
    let target = path.to_string_lossy();
    text.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let separator = fields.iter().position(|field| *field == "-")?;
            if fields.len() < separator + 4 || fields.len() < 6 || unescape(fields[4]) != target {
                return None;
            }
            let options = fields[5].split(',').chain(fields[separator + 3].split(','));
            Some(Mount {
                identifier: fields[0].parse().ok()?,
                path: path.to_path_buf(),
                source: unescape(fields[separator + 2]),
                filesystem: fields[separator + 1].to_owned(),
                options: options.map(str::to_owned).collect(),
            })
        })
        .next_back()
}

/// The newest mount at `path`, as the kernel reports it now.
pub fn read_mount(path: &Path) -> Option<Mount> {
    parse_mount(path, &fs::read_to_string("/proc/self/mountinfo").ok()?)
}

/// Create `path` if needed and require a folder owned by this user, readable only by it.
pub fn private_directory(path: &Path) -> Result<()> {
    fs::DirBuilder::new().recursive(true).mode(0o700).create(path)?;
    let info = fs::symlink_metadata(path)?;
    if !info.is_dir() || info.uid() != nix::unistd::getuid().as_raw() {
        return Err(Failure::Invalid(tr!("A private application folder is unavailable.")));
    }
    fs::set_permissions(path, Permissions::from_mode(0o700))?;
    Ok(())
}

fn private_file(path: &Path) -> Result<bool> {
    let info = fs::symlink_metadata(path)?;
    Ok(info.is_file() && info.uid() == nix::unistd::getuid().as_raw() && info.mode() & 0o077 == 0)
}

/// Require a private, encrypted rclone profile.
pub fn check_profile(path: &Path) -> Result<()> {
    if !private_file(path)? {
        return Err(Failure::Invalid(tr!(
            "The saved session must be a private file owned by your user."
        )));
    }
    let mut lines = BufReader::new(File::open(path)?).split(b'\n');
    let encrypted = (0..4)
        .filter_map(|_| lines.next()?.ok())
        .any(|line| line.trim_ascii() == b"RCLONE_ENCRYPT_V0:");
    if !encrypted {
        return Err(Failure::Invalid(tr!(
            "The saved session must be encrypted before using this app."
        )));
    }
    Ok(())
}

/// The most of a command's error output kept, in memory only, to recognize a cause.
pub const ERROR_TAIL: usize = 16 * 1024;

/// Keep only the last `ERROR_TAIL` bytes of `errors` after adding `more`.
pub fn keep_error_tail(errors: &mut Vec<u8>, more: &[u8]) {
    errors.extend_from_slice(more);
    let excess = errors.len().saturating_sub(ERROR_TAIL);
    errors.drain(..excess);
}

/// Whether rclone failed to get the extra cookies that Advanced Data Protection accounts need.
pub fn web_access_blocked(output: &[u8]) -> bool {
    let text = String::from_utf8_lossy(output);
    text.contains("requestPCS") || text.contains("Missing PCS cookies")
}

pub fn web_access_message() -> String {
    tr!(
        "Your Apple account uses Advanced Data Protection, and Apple refused this access. On your iPhone, iPad, or Mac, turn on 'Access iCloud Data on the Web' in iCloud settings. Then sign in again here and approve the request on your trusted device."
    )
}

pub fn sha256(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// 128 random bits as hexadecimal, for private names that must not collide.
pub fn random_hex() -> std::io::Result<String> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes)?;
    Ok(hex(&bytes))
}

/// Replace `path` with `value` as JSON in a private file, atomically and durably.
pub fn write_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    replace(path, &serde_json::to_vec(value)?, 0o600)
}

/// Replace `path` with `bytes`, atomically and durably: write a temporary file with permissions `mode` beside it,
/// flush it, rename it over `path`, and flush the folder. A failure leaves the previous file and no temporary one.
pub(crate) fn replace(path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    let mut name = path.file_name().unwrap_or_default().to_owned();
    name.push(format!("-{}", random_hex()?));
    let temporary = path.with_file_name(name);
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        // Set explicitly, so the app's private umask never narrows a desktop file that others may read.
        file.set_permissions(Permissions::from_mode(mode))?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        flush_directory(
            path.parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )
    })();
    let _ = fs::remove_file(&temporary);
    result
}

/// Delete a file or a folder tree without following links; a missing path is fine.
pub fn delete_path(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(info) if info.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Flush a folder, so that files created or renamed in it survive a crash.
pub fn flush_directory(path: &Path) -> io::Result<()> {
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(OFlag::O_DIRECTORY.bits())
        .open(path)?;
    Ok(nix::unistd::fsync(&directory)?)
}

/// The identity of a profile: its SHA-256, or the identity it inherited when a renewal replaced it.
pub fn profile_identity(path: &Path, digest: Option<String>) -> Result<String> {
    let digest = match digest {
        Some(digest) => digest,
        None => sha256(&fs::read(path)?),
    };
    let links = path.with_file_name("profile-links.json");
    if fs::symlink_metadata(&links).is_err() {
        return Ok(digest);
    }
    if !private_file(&links)? {
        return Err(Failure::Invalid(tr!("The saved profile identity must be private.")));
    }
    let invalid = || Failure::Invalid(tr!("The saved profile identity is invalid."));
    let value: serde_json::Value = serde_json::from_slice(&fs::read(&links)?).map_err(|_| invalid())?;
    if value.get("version") != Some(&serde_json::Value::from(1)) {
        return Err(invalid());
    }
    let profiles: BTreeMap<String, String> =
        serde_json::from_value(value.get("profiles").cloned().ok_or_else(invalid)?).map_err(|_| invalid())?;
    if !profiles
        .iter()
        .all(|(key, identity)| DIGEST.is_match(key) && DIGEST.is_match(identity))
    {
        return Err(invalid());
    }
    Ok(profiles.get(&digest).cloned().unwrap_or(digest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const LINE: &str = "41 29 0:52 / /home/fixture/iCloud rw,nosuid,nodev,relatime shared:1 - fuse.rclone icloud: \
                        ro,user_id=UID,group_id=1000\n";

    fn line(source: &str, target: &str) -> String {
        LINE.replace("icloud:", source)
            .replace("/home/fixture/iCloud", target)
            .replace("UID", &nix::unistd::getuid().to_string())
    }

    #[test]
    fn the_newest_matching_mount_is_read_with_escaped_paths() {
        let path = Path::new("/home/fixture/iCloud Drive");
        let text = line("other:", "/home/fixture/iCloud\\040Drive")
            + &line(
                "icloud-on-linux-0123456789abcdef0123456789abcdef",
                "/home/fixture/iCloud\\040Drive",
            )
            + &line("icloud:", "/elsewhere");
        let mount = parse_mount(path, &text).unwrap();
        assert_eq!(mount.source, "icloud-on-linux-0123456789abcdef0123456789abcdef");
        assert_eq!(mount.identifier, 41);
        assert!(mount.compatible());
        assert!(parse_mount(Path::new("/missing"), &text).is_none());
    }

    #[test]
    fn writable_or_foreign_mounts_are_not_compatible() {
        let path = Path::new("/home/fixture/iCloud");
        let writable = line("icloud:", "/home/fixture/iCloud").replace(" ro,", " rw,");
        assert!(!parse_mount(path, &writable).unwrap().compatible());
        let other = line("drive:", "/home/fixture/iCloud");
        assert!(!parse_mount(path, &other).unwrap().compatible());
    }

    #[test]
    fn profiles_must_be_private_and_encrypted() {
        let root = tempfile::tempdir().unwrap();
        let profile = root.path().join("rclone.conf");
        fs::write(&profile, "[icloud]\ntype = iclouddrive\n").unwrap();
        fs::set_permissions(&profile, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(check_profile(&profile).unwrap_err().to_string().contains("encrypted"));
        fs::write(
            &profile,
            "# Encrypted rclone configuration File\n\nRCLONE_ENCRYPT_V0:\nfixture\n",
        )
        .unwrap();
        check_profile(&profile).unwrap();
        fs::set_permissions(&profile, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(check_profile(&profile).unwrap_err().to_string().contains("private"));
    }

    #[test]
    fn a_renewed_profile_keeps_its_linked_identity() {
        let root = tempfile::tempdir().unwrap();
        let profile = root.path().join("rclone.conf");
        fs::write(&profile, "RCLONE_ENCRYPT_V0:\nrenewed\n").unwrap();
        let digest = sha256(b"RCLONE_ENCRYPT_V0:\nrenewed\n");
        assert_eq!(profile_identity(&profile, None).unwrap(), digest);
        let original = "a".repeat(64);
        let links = root.path().join("profile-links.json");
        fs::write(
            &links,
            format!(r#"{{"version": 1, "profiles": {{"{digest}": "{original}"}}}}"#),
        )
        .unwrap();
        fs::set_permissions(&links, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(profile_identity(&profile, None).unwrap(), original);
        fs::write(&links, r#"{"version": 1, "profiles": {"short": "value"}}"#).unwrap();
        assert!(profile_identity(&profile, None).is_err());
    }

    #[test]
    fn json_replaces_a_file_privately_and_a_failure_leaves_no_temporary_file() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("state.json");
        write_json(&path, &serde_json::json!({"version": 1})).unwrap();
        write_json(&path, &serde_json::json!({"version": 2})).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), r#"{"version":2}"#);
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        // A folder in place of the file makes the rename fail after the temporary file was written.
        let blocked = root.path().join("blocked.json");
        fs::create_dir(&blocked).unwrap();
        assert!(write_json(&blocked, &serde_json::json!({})).is_err());
        let mut names: Vec<_> = fs::read_dir(root.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort();
        assert_eq!(names, ["blocked.json", "state.json"]);
    }

    #[test]
    fn private_folders_are_created_for_this_user_only() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("a/b");
        private_directory(&folder).unwrap();
        assert_eq!(fs::metadata(&folder).unwrap().mode() & 0o777, 0o700);
    }
}
