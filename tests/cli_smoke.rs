use assert_cmd::{cargo::cargo_bin_cmd, Command};
use predicates::prelude::*;

fn himalaya() -> Command {
    cargo_bin_cmd!("himalaya").into()
}

#[test]
fn help_flag() {
    himalaya()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("CLI to manage emails"));
}

#[test]
fn version_flag() {
    himalaya()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn completion_bash() {
    himalaya()
        .args(["completion", "bash"])
        .assert()
        .success()
        .stdout(predicate::str::is_empty().not());
}

#[test]
fn completion_zsh() {
    himalaya()
        .args(["completion", "zsh"])
        .assert()
        .success()
        .stdout(predicate::str::is_empty().not());
}

#[test]
fn completion_fish() {
    himalaya()
        .args(["completion", "fish"])
        .assert()
        .success()
        .stdout(predicate::str::is_empty().not());
}

#[test]
fn manual_generates_pages() {
    let dir = tempfile::tempdir().unwrap();
    himalaya()
        .args(["manual", dir.path().to_str().unwrap()])
        .assert()
        .success();

    let entries: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map_or(false, |ext| ext == "1"))
        .collect();
    assert!(
        !entries.is_empty(),
        "expected at least one .1 man page file"
    );
}

#[test]
fn invalid_config_path() {
    himalaya()
        .args([
            "--config",
            "/nonexistent/path/config.toml",
            "envelope",
            "list",
        ])
        .assert()
        .failure();
}

#[test]
fn folder_help() {
    himalaya()
        .args(["folder", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("folder"));
}

#[test]
fn all_flag_in_help() {
    himalaya()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("--all"));
}

#[test]
fn all_with_account_fails() {
    himalaya()
        .args(["--all", "envelope", "list", "--account", "foo"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "--all and --account are mutually exclusive",
        ));
}

#[test]
fn all_with_subcommand_message_read_fails() {
    himalaya()
        .args(["--all", "message", "read", "1"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "--all can only be used with listing commands",
        ));
}

#[test]
fn all_with_mutating_command_fails() {
    himalaya()
        .args(["--all", "folder", "add", "test-folder"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "--all can only be used with listing commands",
        ));
}
