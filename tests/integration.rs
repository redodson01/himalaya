use std::time::Duration;

use assert_cmd::{cargo::cargo_bin_cmd, Command};
use predicates::prelude::*;
use serial_test::serial;

const CONFIG: &str = "tests/test_config.toml";
const TIMEOUT: Duration = Duration::from_secs(30);

fn enabled() -> bool {
    std::env::var("HIMALAYA_INTEGRATION_TEST").is_ok()
}

fn himalaya() -> Command {
    let mut cmd: Command = cargo_bin_cmd!("himalaya").into();
    cmd.args(["--config", CONFIG]);
    cmd.timeout(TIMEOUT);
    cmd
}

/// Extract the ID of the first envelope from JSON output.
fn first_envelope_id(json: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(json).expect("valid JSON");
    let id = &v
        .as_array()
        .expect("expected JSON array")
        .first()
        .expect("expected at least one envelope")["id"];
    match id {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        other => panic!("unexpected id type: {other}"),
    }
}

// ---------------------------------------------------------------------------
// Folder tests
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn folder_list_contains_inbox() {
    if !enabled() {
        return;
    }
    himalaya()
        .args(["folder", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("INBOX"));
}

#[test]
#[serial]
fn folder_create_and_delete() {
    if !enabled() {
        return;
    }
    himalaya()
        .args(["folder", "create", "TestFolder"])
        .assert()
        .success();

    himalaya()
        .args(["folder", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("TestFolder"));

    himalaya()
        .args(["folder", "delete", "--yes", "TestFolder"])
        .assert()
        .success();
}

// ---------------------------------------------------------------------------
// Message send/receive tests
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn message_send_and_read() {
    if !enabled() {
        return;
    }

    let raw_msg = concat!(
        "From: user@localhost\r\n",
        "To: user@localhost\r\n",
        "Subject: Integration Test\r\n",
        "\r\n",
        "Hello from integration test!\r\n",
    );

    himalaya()
        .args(["template", "send"])
        .write_stdin(raw_msg)
        .assert()
        .success();

    std::thread::sleep(Duration::from_secs(2));

    let output = himalaya()
        .args(["envelope", "list", "--output", "json"])
        .output()
        .expect("failed to run envelope list");
    assert!(output.status.success());

    let json = String::from_utf8_lossy(&output.stdout);
    assert!(
        json.contains("Integration Test"),
        "expected to find the sent message in INBOX"
    );

    let id = first_envelope_id(&json);

    himalaya()
        .args(["message", "read", &id])
        .assert()
        .success()
        .stdout(predicate::str::contains("Hello from integration test!"));

    himalaya()
        .args(["message", "delete", &id])
        .assert()
        .success();
}

// ---------------------------------------------------------------------------
// Flag operations
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn flag_operations() {
    if !enabled() {
        return;
    }

    let raw_msg = concat!(
        "From: user@localhost\r\n",
        "To: user@localhost\r\n",
        "Subject: Flag Test\r\n",
        "\r\n",
        "Testing flags\r\n",
    );

    himalaya()
        .args(["template", "send"])
        .write_stdin(raw_msg)
        .assert()
        .success();

    std::thread::sleep(Duration::from_secs(2));

    let output = himalaya()
        .args(["envelope", "list", "--output", "json"])
        .output()
        .expect("failed to list envelopes");
    let json = String::from_utf8_lossy(&output.stdout);
    let id = first_envelope_id(&json);

    himalaya()
        .args(["flag", "add", &id, "\\Seen"])
        .assert()
        .success();

    himalaya()
        .args(["flag", "set", &id, "\\Flagged"])
        .assert()
        .success();

    himalaya()
        .args(["flag", "remove", &id, "\\Flagged"])
        .assert()
        .success();

    himalaya()
        .args(["message", "delete", &id])
        .assert()
        .success();
}

// ---------------------------------------------------------------------------
// Copy/move tests
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn copy_and_move_message() {
    if !enabled() {
        return;
    }

    // Create Archive folder
    himalaya()
        .args(["folder", "create", "Archive"])
        .assert()
        .success();

    let raw_msg = concat!(
        "From: user@localhost\r\n",
        "To: user@localhost\r\n",
        "Subject: Copy Move Test\r\n",
        "\r\n",
        "Testing copy and move\r\n",
    );

    himalaya()
        .args(["template", "send"])
        .write_stdin(raw_msg)
        .assert()
        .success();

    std::thread::sleep(Duration::from_secs(2));

    let output = himalaya()
        .args(["envelope", "list", "--output", "json"])
        .output()
        .expect("failed to list envelopes");
    let json = String::from_utf8_lossy(&output.stdout);
    let id = first_envelope_id(&json);

    // Copy to Archive
    himalaya()
        .args(["message", "copy", "Archive", &id])
        .assert()
        .success();

    // Verify in Archive
    himalaya()
        .args(["envelope", "list", "--folder", "Archive"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Copy Move Test"));

    // Move from INBOX to Archive
    himalaya()
        .args(["message", "move", "Archive", &id])
        .assert()
        .success();

    // Clean up — skip folder delete since GreenMail container is ephemeral
}
