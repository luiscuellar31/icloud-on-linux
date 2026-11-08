#![cfg(target_os = "linux")]
//! The real rclone iCloud setup questions, answered with fictional values up to the answer that would contact
//! Apple.
mod support;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use icloud_core::auth::Authorization;
use icloud_core::paths::Paths;
use icloud_core::process::without_rclone_variables;
use icloud_core::profile::check_profile;
use icloud_core::session::Session;
use serde_json::Value;

const ACCOUNT: &str = "fixture@example.invalid";
const PASSWORD: &str = "fictional-password";

fn helper() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/session.sh")
}

/// Run one sign-in step of the session script on `config`, as the app does.
fn step(config: &Path, action: &str, state: &str, answer: &str) -> Value {
    let mut command = Command::new("bash");
    without_rclone_variables(&mut command)
        .arg(helper())
        .arg(action)
        .env("RCLONE_CONFIG", config)
        .env("RCLONE_CONFIG_PASS", support::KEY)
        .env("RCLONE_STATE", state)
        .env("RCLONE_RESULT", answer);
    let output = command.output().unwrap();
    assert!(output.status.success(), "{action} failed");
    check_profile(config).unwrap();
    if action == "auth-init" {
        Value::Null
    } else {
        serde_json::from_slice(&output.stdout).unwrap()
    }
}

fn rclone(config: &Path, arguments: &[&str]) -> String {
    let mut command = Command::new("rclone");
    without_rclone_variables(&mut command)
        .env("RCLONE_CONFIG", config)
        .env("RCLONE_CONFIG_PASS", support::KEY)
        .arg("--ask-password=false")
        .args(arguments);
    let output = command.output().unwrap();
    assert!(output.status.success(), "rclone {arguments:?} failed");
    String::from_utf8(output.stdout).unwrap()
}

/// The saved `icloud` remote, read through rclone.
fn remote(config: &Path) -> Value {
    serde_json::from_str::<Value>(&rclone(config, &["config", "dump"])).unwrap()["icloud"].clone()
}

/// Answer the questions before the advanced-options one, whose answer makes rclone contact Apple.
fn sign_in_until_apple(config: &Path) {
    step(config, "auth-init", "", "");
    let mut response = step(config, "auth-create", "", "");
    for (question, answer) in [("service", "drive"), ("apple_id", ACCOUNT), ("password", PASSWORD)] {
        assert_eq!(response["Option"]["Name"], question);
        let action = if question == "password" {
            "auth-password"
        } else {
            "auth-answer"
        };
        response = step(config, action, response["State"].as_str().unwrap(), answer);
    }
    assert_eq!(response["Option"]["Name"], "config_fs_advanced");
}

#[test]
fn real_rclone_saves_private_answers_with_the_password_obscured_before_any_apple_request() {
    let workspace = tempfile::tempdir().unwrap();
    let config = workspace.path().join("rclone.conf");
    sign_in_until_apple(&config);
    let encrypted = fs::read(&config).unwrap();
    let text = String::from_utf8_lossy(&encrypted);
    assert!(!text.contains(ACCOUNT) && !text.contains(PASSWORD));
    // The backend reveals the saved password before contacting Apple, so a clear answer could not sign in.
    let stored = remote(&config)["password"].as_str().unwrap().to_owned();
    assert_ne!(stored, PASSWORD);
    // An obscured value can start with a dash, so `--` ends rclone's options.
    assert_eq!(rclone(&config, &["reveal", "--", &stored]).trim_end(), PASSWORD);
}

#[test]
fn renewal_reads_the_saved_account_from_real_rclone_and_refuses_another_one() {
    let workspace = tempfile::tempdir().unwrap();
    let config = workspace.path().join("rclone.conf");
    sign_in_until_apple(&config);
    let encrypted = fs::read(&config).unwrap();
    // The app answers the advanced-options question with `false`; this helper stops there instead and keeps the
    // remote that the private copy reached.
    let reached = workspace.path().join("reached.json");
    let wrapper = workspace.path().join("helper.sh");
    fs::write(
        &wrapper,
        format!(
            "if [ \"$1\" = auth-answer ] && [ \"$RCLONE_RESULT\" = false ]; then\n    \
             rclone --ask-password=false config dump > '{}'\n    exit 1\nfi\nexec bash '{}' \"$@\"\n",
            reached.display(),
            helper().display()
        ),
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
    let session = Session::new(Paths {
        config: config.clone(),
        mount: workspace.path().join("mount"),
        cache: workspace.path().join("cache"),
        runtime: workspace.path().join("runtime"),
        helper: wrapper,
    });
    let authorization = Authorization::new(session);

    authorization.start(support::KEY, "", "another@example.invalid", PASSWORD);
    support::wait_until(60, || authorization.tick(), || !authorization.active());
    assert!(
        authorization.message().contains("same Apple account"),
        "{}",
        authorization.message()
    );
    assert!(!reached.exists());
    assert_eq!(fs::read(&config).unwrap(), encrypted);

    // Letter case does not change the account; the remote is recreated with the new answers.
    authorization.start(support::KEY, "", "Fixture@Example.invalid", PASSWORD);
    support::wait_until(60, || authorization.tick(), || !authorization.active());
    let dump: Value =
        serde_json::from_slice(&fs::read(&reached).expect("the sign-in did not reach Apple's step")).unwrap();
    let copy = &dump["icloud"];
    assert_eq!(copy["type"], "iclouddrive");
    assert_eq!(copy["apple_id"], "Fixture@Example.invalid");
    assert_eq!(copy["service"].as_str().unwrap_or("drive"), "drive");
    let password = copy["password"].as_str().unwrap();
    assert_eq!(rclone(&config, &["reveal", "--", password]).trim_end(), PASSWORD);
    assert_eq!(fs::read(&config).unwrap(), encrypted);
    let copies = fs::read_dir(workspace.path())
        .unwrap()
        .filter(|entry| {
            entry
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".signin-")
        })
        .count();
    assert_eq!(copies, 0, "the private sign-in copy was left behind");
}
