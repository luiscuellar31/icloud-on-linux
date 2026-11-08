use std::env;
use std::path::PathBuf;

/// Where the app keeps the encrypted profile, the mount point, caches, runtime files, and the session helper.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    pub config: PathBuf,
    pub mount: PathBuf,
    pub cache: PathBuf,
    pub runtime: PathBuf,
    pub helper: PathBuf,
}

impl Paths {
    /// The current user's locations; `helper` is the `scripts/session.sh` shipped with the app.
    pub fn for_user(helper: PathBuf) -> Paths {
        let home = home();
        Paths {
            config: home.join(".config/icloud-linux/rclone.conf"),
            mount: home.join("iCloud"),
            cache: variable("XDG_CACHE_HOME")
                .unwrap_or_else(|| home.join(".cache"))
                .join("icloud-on-linux"),
            runtime: variable("XDG_RUNTIME_DIR")
                .unwrap_or_else(|| PathBuf::from(format!("/run/user/{}", nix::unistd::getuid())))
                .join("icloud-on-linux"),
            helper,
        }
    }

    /// App preferences, beside the profile.
    pub fn settings(&self) -> PathBuf {
        self.config.with_file_name("settings.ini")
    }
}

pub fn home() -> PathBuf {
    // Since Rust 1.86 this reads HOME and then the password database; 1.85 only warns about it.
    #[allow(deprecated)]
    env::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

fn variable(name: &str) -> Option<PathBuf> {
    env::var_os(name).filter(|value| !value.is_empty()).map(PathBuf::from)
}
