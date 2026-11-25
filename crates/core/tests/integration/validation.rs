//! `oxy validate` is a one-release stub: it prints the `oxyc validate` command
//! and exits 2 (`crates/server/src/cli/commands/validate.rs`).
//!
//! These were the binary's validation tests — `ok_on_valid_config` over the
//! example fixture and `failed_on_invalid_config` over
//! `crates/core/tests/fixtures/invalid_config`. Both assertions moved with the
//! checks, to `sdk/cli/src/commands/validate-parity.test.ts`, which runs
//! `oxyc validate` over the same two fixtures and pins its verdict against the
//! one this binary recorded before it became a stub. What is left to pin here
//! is the stub: an old invocation, on a valid workspace or an invalid one, is
//! told what to run and exits as a usage error — never 0, which a CI step
//! would read as "valid".

use std::path::PathBuf;
use std::process::Command;

fn invalid_config_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/invalid_config")
}

fn run(dir: PathBuf, args: &[&str]) -> (Option<i32>, String) {
    let out = Command::new(oxy_test_utils::get_oxy_binary())
        .current_dir(dir)
        .arg("validate")
        .args(args)
        .output()
        .expect("the oxy binary runs");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn a_valid_workspace_is_told_to_run_oxyc_validate() {
    let (code, stderr) = run(oxy_test_utils::oxy_example_fixture_dir(), &[]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains("oxyc validate"), "{stderr}");
}

#[test]
fn an_invalid_workspace_gets_the_same_answer_not_a_verdict() {
    let (code, stderr) = run(invalid_config_dir(), &["--file", "config.yml"]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(
        stderr.contains("oxyc validate --file config.yml"),
        "{stderr}"
    );
}
