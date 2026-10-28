//! `oxy serve` and `oxy worker` refuse to start, naming the setting, when they
//! are not the single process and the state every node must share is missing.
//!
//! The unit tests in `server/shared_state_check.rs` pin the rule against a
//! fake environment. These run the real binary, because what the operator
//! meets is the process exiting, and the check is only worth anything if it is
//! wired into both entry points and runs before they do anything else.

use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

const SETTINGS: [&str; 3] = [
    "OXY_ENCRYPTION_KEY",
    "OXY_CUSTOMER_APPS_S3_BUCKET",
    "OXY_CUSTOMER_APPS_STORAGE_S3_BUCKET",
];

/// The binary with none of the shared-state settings, in a state dir of its
/// own so a key file from an earlier run cannot matter.
fn oxy(subcommand: &str, role: Option<&str>, state_dir: &std::path::Path) -> Output {
    let mut cmd = Command::new(oxy_test_utils::get_oxy_binary());
    cmd.arg(subcommand)
        .env("OXY_STATE_DIR", state_dir)
        // Present but never reached: the check runs before the database is used.
        .env(
            "OXY_DATABASE_URL",
            "postgresql://nobody:nothing@127.0.0.1:1/none",
        )
        .env_remove("OXY_ROLE");
    for setting in SETTINGS {
        cmd.env_remove(setting);
    }
    if let Some(role) = role {
        cmd.env("OXY_ROLE", role);
    }
    // A process that was not refused goes on to start for real; it must not be
    // allowed to hold the test, so it is stopped after a few seconds and
    // whatever it had said by then is what is read.
    cmd.stdout(Stdio::null()).stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("run the oxy binary");
    let started = Instant::now();
    while child.try_wait().expect("poll").is_none() {
        if started.elapsed() > Duration::from_secs(20) {
            let _ = child.kill();
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    child.wait_with_output().expect("collect output")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn serve_in_every_fleet_role_refuses_and_names_each_missing_setting() {
    for role in ["ide", "serve", "worker"] {
        let dir = tempfile::tempdir().unwrap();
        let out = oxy("serve", Some(role), dir.path());
        let err = stderr(&out);
        assert!(!out.status.success(), "{role}: should have refused\n{err}");
        assert!(
            err.contains(&format!("OXY_ROLE={role} needs")),
            "{role}: {err}"
        );
        for setting in SETTINGS {
            assert!(err.contains(setting), "{role}: {setting} not named\n{err}");
        }
        // It refused before it could write a key of its own.
        assert!(
            !dir.path().join("encryption_key.txt").exists(),
            "{role}: a refused start must not mint a key file"
        );
    }
}

#[test]
fn worker_refuses_by_default_because_running_it_is_the_role() {
    let dir = tempfile::tempdir().unwrap();
    let out = oxy("worker", None, dir.path());
    let err = stderr(&out);
    assert!(!out.status.success(), "should have refused\n{err}");
    assert!(err.contains("OXY_ROLE=worker needs"), "{err}");
}

#[test]
fn the_single_process_is_not_asked_for_them() {
    // `all` goes on to fail later, for want of a reachable database; what
    // matters is that it was not stopped for the shared-state settings.
    let dir = tempfile::tempdir().unwrap();
    for role in [None, Some("all")] {
        let out = oxy("serve", role, dir.path());
        let err = stderr(&out);
        assert!(
            !err.contains("needs state that every node can read"),
            "role {role:?} was refused: {err}"
        );
    }
}
