//! The `source_type` a compile is stamped with, and the payload it travels as.
//!
//! Selection keys on `source_type` alone, so these two strings are the whole
//! difference between a compile only the node with a working copy may take
//! and one any node may.

use agentic_core::delegation::TaskSpec;

use super::{COMPILE_GIT_SOURCE_TYPE, COMPILE_SOURCE_TYPE, source_type_for_spec};

fn compile(from_git: bool) -> TaskSpec {
    TaskSpec::Compile {
        workspace_id: uuid::Uuid::nil(),
        git_sha: Some("0123456789abcdef0123456789abcdef01234567".into()),
        branch: Some("main".into()),
        promote: true,
        kind: Some("main".into()),
        owner_user_id: None,
        from_git,
    }
}

#[test]
fn a_commit_compile_is_its_own_source_type() {
    assert_eq!(source_type_for_spec(&compile(false)), COMPILE_SOURCE_TYPE);
    assert_eq!(
        source_type_for_spec(&compile(true)),
        COMPILE_GIT_SOURCE_TYPE
    );
    assert_ne!(COMPILE_SOURCE_TYPE, COMPILE_GIT_SOURCE_TYPE);
}

/// Rows are queued by one build and claimed by another. A compile queued
/// before `from_git` existed carries no such key and must stay what it was.
#[test]
fn a_compile_queued_before_the_key_existed_is_a_working_copy_compile() {
    let queued = serde_json::json!({
        "type": "compile",
        "workspace_id": uuid::Uuid::nil(),
        "git_sha": "local-1",
        "promote": true,
    });
    let spec: TaskSpec = serde_json::from_value(queued).expect("an old row still parses");
    assert!(matches!(
        spec,
        TaskSpec::Compile {
            from_git: false,
            ..
        }
    ));
    assert_eq!(source_type_for_spec(&spec), COMPILE_SOURCE_TYPE);
}

/// Both kinds keep the `compile` tag — the dedupe and backoff queries find a
/// workspace's compiles by `spec->>'type'` — and a working-copy compile is
/// written exactly as before, so a build that predates the key reads it the
/// same way.
#[test]
fn both_kinds_travel_under_the_compile_tag() {
    let working_copy = serde_json::to_value(compile(false)).unwrap();
    assert_eq!(working_copy["type"], "compile");
    assert!(working_copy.get("from_git").is_none(), "{working_copy}");

    let from_git = serde_json::to_value(compile(true)).unwrap();
    assert_eq!(from_git["type"], "compile");
    assert_eq!(from_git["from_git"], true);
}
