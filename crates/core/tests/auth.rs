//! Sign-in and renewal against a stand-in for rclone's setup questions; flags are files, not environment variables.
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use icloud_core::auth::Authorization;
use icloud_core::paths::Paths;
use icloud_core::profile::{profile_identity, sha256};
use icloud_core::session::Session;

const KEY: &str = "native-fixture-key";
const ACCOUNT: &str = "fixture@example.invalid";
const PASSWORD: &str = "fictional-apple-password";
const PROFILE: &[u8] = b"RCLONE_ENCRYPT_V0:\nfictional-config\n";
const STUB: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "@ROOT@/calls.log"
[ "$RCLONE_CONFIG_PASS" = native-fixture-key ] || exit 1
question() { printf '{"State": "%s", "Option": {"Name": "%s"%s}, "Error": ""}\n' "$1" "$2" "$3"; }
case " $* " in
    *" encryption "*) printf 'RCLONE_ENCRYPT_V0:\nempty-fixture\n' > "$RCLONE_CONFIG" ;;
    *" show "*)
        if [ -e "@ROOT@/no-remote" ]; then printf '[icloud]\n# no type for "icloud"\n'
        else printf '[icloud]\ntype = iclouddrive\nservice = drive\napple_id = fixture@example.invalid\n'; fi ;;
    *" obscure "*)
        read -r line
        printf 'obscured:%s\n' "$(printf %s "$line" | { sha256sum 2>/dev/null || shasum -a 256; } | cut -d' ' -f1)" ;;
    *" create "*) question service service ;;
    *" update "*)
        [ -e "@ROOT@/hold" ] && sleep 20
        case "$RCLONE_STATE:$RCLONE_RESULT" in
            service:drive) question account apple_id ;;
            account:fixture@example.invalid) question password password ;;
            password:obscured:@OBSCURED@) question advanced config_fs_advanced ;;
            advanced:false) question code config_2fa ;;
            code:sms) question phone config_2fa_phone ', "Examples": [{"Value": "1_sms", "Help": "Trusted phone ending in 01"}]' ;;
            phone:1_sms) question sms config_2fa_sms ;;
            *:123456)
                if [ -e "@ROOT@/pcs-fails" ]; then
                    echo 'requestPCS(fixture): server returned success but cookies still missing' >&2
                    exit 1
                fi
                printf 'RCLONE_ENCRYPT_V0:\nnew-signed-in-fixture\n' > "$RCLONE_CONFIG"
                printf '{"State": "", "Option": null, "Error": ""}\n' ;;
            *) echo 'Fictional backend token must not be shown' >&2; exit 1 ;;
        esac ;;
    *" lsjson "*) [ -e "@ROOT@/probe-fails" ] && exit 1; echo '{}' ;;
    *) exit 1 ;;
esac
"#;

struct Bench {
    root: tempfile::TempDir,
    config: PathBuf,
    auth: Rc<Authorization>,
}

impl Bench {
    fn new() -> Bench {
        let root = tempfile::Builder::new().prefix("icloud-auth-").tempdir().unwrap();
        let bin = root.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let script = STUB
            .replace("@ROOT@", root.path().to_str().unwrap())
            .replace("@OBSCURED@", &sha256(PASSWORD.as_bytes()));
        fs::write(bin.join("rclone"), script).unwrap();
        fs::set_permissions(bin.join("rclone"), fs::Permissions::from_mode(0o755)).unwrap();
        let folder = root.path().join("config");
        fs::create_dir(&folder).unwrap();
        let config = folder.join("rclone.conf");
        fs::write(&config, PROFILE).unwrap();
        fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();
        let session = Session::new(Paths {
            config: config.clone(),
            mount: root.path().join("mount"),
            cache: root.path().join("cache"),
            runtime: root.path().join("runtime"),
            helper: Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/session.sh"),
        });
        let search = std::env::join_paths(
            [bin]
                .into_iter()
                .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        );
        session.set_search_path(Some(search.unwrap()));
        session.set_mount_reader(|_| None);
        Bench {
            root,
            config,
            auth: Authorization::new(session),
        }
    }

    fn flag(&self, name: &str) {
        fs::write(self.root.path().join(name), "").unwrap();
    }

    fn log(&self) -> String {
        fs::read_to_string(self.root.path().join("calls.log")).unwrap_or_default()
    }

    fn wait(&self, done: impl Fn(&Authorization) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done(&self.auth) {
            assert!(Instant::now() < deadline, "timed out: {}", self.auth.message());
            self.auth.tick();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn begin(&self) {
        self.auth.start(KEY, KEY, ACCOUNT, PASSWORD);
        self.wait(|auth| auth.question() == "config_2fa" || !auth.active());
    }

    fn leftovers(&self) -> usize {
        fs::read_dir(self.config.parent().unwrap())
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".signin-")
            })
            .count()
    }
}

#[test]
fn renewal_keeps_the_pending_profile_identity_and_puts_no_secret_in_arguments() {
    let bench = Bench::new();
    let identity = profile_identity(&bench.config, None).unwrap();
    bench.begin();
    assert_eq!(bench.auth.question(), "config_2fa");
    bench.auth.answer("123456");
    bench.wait(|auth| !auth.active());
    assert!(
        bench.auth.message().starts_with("Signed in."),
        "{}",
        bench.auth.message()
    );
    assert_ne!(fs::read(&bench.config).unwrap(), PROFILE);
    assert_eq!(profile_identity(&bench.config, None).unwrap(), identity);
    assert_eq!(fs::metadata(&bench.config).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(bench.leftovers(), 0);
    let log = bench.log();
    for secret in [KEY, PASSWORD, "123456"] {
        assert!(!log.contains(secret), "{secret} reached a command line");
    }
}

#[test]
fn a_new_sign_in_requires_matching_local_keys_and_stays_encrypted() {
    let bench = Bench::new();
    fs::remove_file(&bench.config).unwrap();
    bench.auth.start(KEY, "different-key", ACCOUNT, PASSWORD);
    assert!(bench.auth.message().contains("do not match") && !bench.auth.active());
    bench.begin();
    bench.auth.answer("123456");
    bench.wait(|auth| !auth.active());
    assert!(fs::read(&bench.config).unwrap().starts_with(b"RCLONE_ENCRYPT_V0:"));
}

#[test]
fn a_text_message_code_can_go_to_a_chosen_trusted_phone() {
    let bench = Bench::new();
    bench.begin();
    bench.auth.answer("sms");
    bench.wait(|auth| auth.question() == "config_2fa_phone");
    assert_eq!(
        bench.auth.choices(),
        vec![("1_sms".to_owned(), "Trusted phone ending in 01".to_owned())]
    );
    bench.auth.answer("2_sms");
    assert!(bench.auth.message().contains("listed trusted phone"));
    bench.auth.answer("1_sms");
    bench.wait(|auth| auth.question() == "config_2fa_sms");
    bench.auth.answer("123456");
    bench.wait(|auth| !auth.active());
    assert_ne!(fs::read(&bench.config).unwrap(), PROFILE);
}

#[test]
fn a_wrong_key_a_wrong_code_and_a_failed_probe_keep_the_saved_profile() {
    let bench = Bench::new();
    bench.auth.start("wrong-key", "", ACCOUNT, PASSWORD);
    bench.wait(|auth| !auth.active());
    assert_eq!(fs::read(&bench.config).unwrap(), PROFILE);
    bench.begin();
    bench.auth.answer("12345");
    assert!(bench.auth.message().contains("6-digit"));
    bench.auth.answer("000000");
    bench.wait(|auth| !auth.active());
    assert_eq!(fs::read(&bench.config).unwrap(), PROFILE);
    assert!(!bench.auth.message().contains("backend token"));
    bench.flag("probe-fails");
    bench.begin();
    bench.auth.answer("123456");
    bench.wait(|auth| !auth.active());
    assert_eq!(fs::read(&bench.config).unwrap(), PROFILE);
    assert_eq!(bench.leftovers(), 0);
}

#[test]
fn unusable_credentials_or_a_missing_rclone_start_no_process() {
    let bench = Bench::new();
    bench.auth.start(KEY, "", ACCOUNT, "first line\nsecond line");
    assert!(!bench.auth.active() && bench.auth.message().contains("Enter your local key"));
    assert_eq!(bench.log(), "");
}

#[test]
fn a_profile_without_the_remote_receives_the_new_sign_in() {
    let bench = Bench::new();
    let identity = profile_identity(&bench.config, None).unwrap();
    bench.flag("no-remote");
    bench.begin();
    bench.auth.answer("123456");
    bench.wait(|auth| !auth.active());
    assert_ne!(fs::read(&bench.config).unwrap(), PROFILE);
    assert_eq!(profile_identity(&bench.config, None).unwrap(), identity);
}

#[test]
fn another_account_cannot_replace_the_saved_profile() {
    let bench = Bench::new();
    bench.auth.start(KEY, "", "other@example.invalid", PASSWORD);
    bench.wait(|auth| !auth.active());
    assert_eq!(fs::read(&bench.config).unwrap(), PROFILE);
    assert!(bench.auth.message().contains("same Apple account"));
}

#[test]
fn cancelling_while_waiting_or_running_keeps_the_profile() {
    let bench = Bench::new();
    bench.begin();
    bench.auth.cancel();
    assert!(!bench.auth.active());
    bench.flag("hold");
    bench.auth.start(KEY, "", ACCOUNT, PASSWORD);
    bench.wait(|auth| auth.running() && auth.message().contains("Signing in"));
    bench.wait(|auth| auth.running());
    bench.auth.cancel();
    bench.wait(|auth| !auth.active());
    assert!(bench.auth.message().contains("cancelled"));
    assert_eq!(fs::read(&bench.config).unwrap(), PROFILE);
}

#[test]
fn a_profile_changed_meanwhile_or_an_unwritable_folder_keeps_the_saved_bytes() {
    let bench = Bench::new();
    bench.begin();
    let changed = b"RCLONE_ENCRYPT_V0:\nChanged outside the app\n";
    fs::write(&bench.config, changed).unwrap();
    bench.auth.answer("123456");
    bench.wait(|auth| !auth.active());
    assert_eq!(fs::read(&bench.config).unwrap(), changed);
    fs::write(&bench.config, PROFILE).unwrap();
    bench.begin();
    let folder = bench.config.parent().unwrap().to_owned();
    fs::set_permissions(&folder, fs::Permissions::from_mode(0o500)).unwrap();
    bench.auth.answer("123456");
    bench.wait(|auth| !auth.active());
    fs::set_permissions(&folder, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(fs::read(&bench.config).unwrap(), PROFILE);
}

#[test]
fn a_new_sign_in_removes_copies_left_by_interrupted_attempts() {
    let bench = Bench::new();
    let folder = bench.config.parent().unwrap().to_path_buf();
    for name in [".signin-interrupted", ".signin-recent"] {
        fs::create_dir(folder.join(name)).unwrap();
        fs::write(folder.join(name).join("rclone.conf"), "encrypted fixture copy").unwrap();
    }
    let two_hours_ago = std::time::SystemTime::now() - Duration::from_secs(7200);
    fs::File::open(folder.join(".signin-interrupted"))
        .unwrap()
        .set_modified(two_hours_ago)
        .unwrap();
    bench.begin();
    assert!(!folder.join(".signin-interrupted").exists());
    assert!(folder.join(".signin-recent").exists());
    bench.auth.cancel();
    bench.wait(|auth| !auth.active());
}

#[test]
fn an_advanced_data_protection_refusal_explains_web_access() {
    let bench = Bench::new();
    bench.flag("pcs-fails");
    bench.begin();
    bench.auth.answer("123456");
    bench.wait(|auth| !auth.active());
    let message = bench.auth.message();
    assert!(message.contains("Advanced Data Protection"), "{message}");
    assert!(!message.contains("requestPCS"));
    assert_eq!(fs::read(&bench.config).unwrap(), PROFILE);
    assert_eq!(bench.leftovers(), 0);
}
