//! End-to-end smoke tests for `oxy compile` (Phase 1.6a).
//!
//! Both tests are `#[ignore]` because they require a running
//! PostgreSQL with the workspaces table seeded (a row with id =
//! `Uuid::nil()`). Run locally after `oxy serve --local` or `oxy
//! start` has primed the DB:
//!
//! ```text
//! cargo nextest run -p oxy-server --test integration -E 'test(compile::)' --run-ignored only --no-fail-fast
//! ```
//!
//! These are intentionally light — the heavy lifting is in
//! `oxy-compile`'s own unit tests, which run unconditionally.

use assert_cmd::assert::OutputAssertExt;
use serial_test::serial;
use std::process::Command;

fn setup_command() -> Command {
    let mut cmd = Command::new(oxy_test_utils::get_oxy_binary());
    cmd.current_dir(oxy_test_utils::oxy_example_fixture_dir())
        .arg("compile");
    cmd
}

#[test]
#[serial]
#[ignore = "requires a running PostgreSQL database with workspaces table seeded"]
fn test_compile_succeeds_against_oxy_example() {
    let mut cmd = setup_command();
    cmd.assert().success();
}

#[test]
#[serial]
#[ignore = "requires a running PostgreSQL database with workspaces table seeded"]
fn test_compile_json_output_is_parseable() {
    let mut cmd = setup_command();
    let output = cmd.arg("--json").output().expect("run compile");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value =
        serde_json::from_str(&stdout).expect("--json output should be valid JSON");
    assert!(
        json.get("revision_id").is_some(),
        "expected revision_id in --json output"
    );
    assert!(
        json.get("status").is_some(),
        "expected status in --json output"
    );
}
