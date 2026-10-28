use super::*;

const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";
const REMOTE: &str = "https://github.com/acme/analytics.git";

fn push(git_ref: &str, full_name: &str, commits: Vec<PushCommit>) -> PushEvent {
    PushEvent {
        git_ref: git_ref.into(),
        deleted: false,
        after: HEAD.into(),
        repository: PushRepository {
            full_name: full_name.into(),
        },
        commits,
    }
}

fn commit(added: &[&str], modified: &[&str], removed: &[&str]) -> PushCommit {
    let owned = |v: &[&str]| v.iter().map(|s| s.to_string()).collect();
    PushCommit {
        added: owned(added),
        modified: owned(modified),
        removed: owned(removed),
    }
}

fn main_push() -> PushEvent {
    push("refs/heads/main", "acme/analytics", vec![])
}

#[test]
fn a_push_to_the_recorded_default_branch_of_the_repository_matches() {
    assert!(concerns(&main_push(), REMOTE, Some("main"), None));
}

#[test]
fn the_repository_is_compared_by_owner_and_name_ignoring_case() {
    let shouted = push("refs/heads/main", "Acme/Analytics", vec![]);
    assert!(concerns(&shouted, REMOTE, Some("main"), None));
    for remote in [
        "git@github.com:acme/analytics.git",
        "https://x-access-token:ghs_abc@github.com/acme/analytics",
        "ssh://git@github.com/acme/analytics.git",
    ] {
        assert!(
            concerns(&main_push(), remote, Some("main"), None),
            "{remote}"
        );
    }
    for other in [
        // A repository whose name only starts or ends the same way.
        "https://github.com/acme/analytics-old.git",
        "https://github.com/acme/old-analytics.git",
        "https://github.com/other/analytics.git",
        // Not github.com, and not a remote at all.
        "https://gitlab.com/acme/analytics.git",
        "",
    ] {
        assert!(
            !concerns(&main_push(), other, Some("main"), None),
            "{other}"
        );
    }
}

#[test]
fn only_a_push_to_the_recorded_default_branch_matches() {
    let feature = push("refs/heads/feature/x", "acme/analytics", vec![]);
    assert!(!concerns(&feature, REMOTE, Some("main"), None));
    // The branch is compared as git does, case-sensitively.
    assert!(!concerns(&main_push(), REMOTE, Some("Main"), None));
    // A workspace with no recorded branch cannot be matched.
    assert!(!concerns(&main_push(), REMOTE, None, None));
    // A workspace whose default branch is not `main`.
    assert!(concerns(&feature, REMOTE, Some("feature/x"), None));
}

#[test]
fn a_tag_is_not_a_branch() {
    let tag = push("refs/tags/main", "acme/analytics", vec![]);
    assert_eq!(tag.branch(), None);
    assert!(!concerns(&tag, REMOTE, Some("main"), None));
    assert_eq!(push("refs/heads/", "acme/analytics", vec![]).branch(), None);
}

#[test]
fn a_deletion_or_a_push_naming_no_commit_has_no_head() {
    let mut deleted = main_push();
    deleted.deleted = true;
    assert_eq!(deleted.head(), None);

    let mut zeros = main_push();
    zeros.after = "0".repeat(40);
    assert_eq!(zeros.head(), None);

    for after in [
        "",
        "main",
        "0123456",
        "not-a-sha-not-a-sha-not-a-sha-not-a-sha!",
    ] {
        let mut p = main_push();
        p.after = after.into();
        assert_eq!(p.head(), None, "{after:?}");
    }
    // Upper case is normalised, as every other commit id is.
    let mut upper = main_push();
    upper.after = HEAD.to_ascii_uppercase();
    assert_eq!(upper.head().as_deref(), Some(HEAD));
}

#[test]
fn a_workspace_at_the_root_or_with_no_recorded_subdirectory_matches_any_push() {
    let elsewhere = push(
        "refs/heads/main",
        "acme/analytics",
        vec![commit(&[], &["docs/readme.md"], &[])],
    );
    for subdir in [None, Some(""), Some("/")] {
        assert!(
            concerns(&elsewhere, REMOTE, Some("main"), subdir),
            "{subdir:?}"
        );
    }
}

#[test]
fn a_workspace_in_a_subdirectory_is_skipped_when_the_push_changed_nothing_there() {
    let docs_only = push(
        "refs/heads/main",
        "acme/analytics",
        vec![commit(&["docs/a.md"], &["docs/b.md"], &["docs/c.md"])],
    );
    assert!(!concerns(
        &docs_only,
        REMOTE,
        Some("main"),
        Some("analytics")
    ));

    // Added, modified and removed all count.
    for changed in [
        commit(&["analytics/new.yml"], &[], &[]),
        commit(&[], &["analytics/config.yml"], &[]),
        commit(&[], &[], &["analytics/old.yml"]),
    ] {
        let p = push("refs/heads/main", "acme/analytics", vec![changed]);
        assert!(concerns(&p, REMOTE, Some("main"), Some("analytics")));
        // A trailing slash on the recorded value means the same thing.
        assert!(concerns(&p, REMOTE, Some("main"), Some("analytics/")));
    }
    // Any commit in the push counts, not just the last.
    let two = push(
        "refs/heads/main",
        "acme/analytics",
        vec![
            commit(&[], &["analytics/a.yml"], &[]),
            commit(&[], &["docs/b.md"], &[]),
        ],
    );
    assert!(concerns(&two, REMOTE, Some("main"), Some("analytics")));
}

#[test]
fn a_directory_that_only_shares_a_prefix_is_not_the_subdirectory() {
    let p = push(
        "refs/heads/main",
        "acme/analytics",
        vec![commit(
            &[],
            &["analytics-old/config.yml", "analytics.md"],
            &[],
        )],
    );
    assert!(!concerns(&p, REMOTE, Some("main"), Some("analytics")));
    // A nested subdirectory is matched by its full path.
    let nested = push(
        "refs/heads/main",
        "acme/analytics",
        vec![commit(&[], &["data/oxy/config.yml"], &[])],
    );
    assert!(concerns(&nested, REMOTE, Some("main"), Some("data/oxy")));
    assert!(!concerns(&nested, REMOTE, Some("main"), Some("data/other")));
}

#[test]
fn a_push_whose_file_lists_may_be_incomplete_is_never_narrowed() {
    // No commits: a branch created at an existing commit, or a force push.
    let none = push("refs/heads/main", "acme/analytics", vec![]);
    assert!(none.changed_paths().is_none());
    assert!(concerns(&none, REMOTE, Some("main"), Some("analytics")));

    // As many commits as GitHub will list.
    let capped = push(
        "refs/heads/main",
        "acme/analytics",
        (0..COMPLETE_COMMIT_LIST_BELOW)
            .map(|_| commit(&[], &["docs/x.md"], &[]))
            .collect(),
    );
    assert!(concerns(&capped, REMOTE, Some("main"), Some("analytics")));

    // One commit with a very long file list.
    let big: Vec<String> = (0..COMPLETE_PATH_LIST_BELOW)
        .map(|i| format!("docs/{i}.md"))
        .collect();
    let big_refs: Vec<&str> = big.iter().map(String::as_str).collect();
    let huge = push(
        "refs/heads/main",
        "acme/analytics",
        vec![commit(&[], &big_refs, &[])],
    );
    assert!(concerns(&huge, REMOTE, Some("main"), Some("analytics")));
}

#[test]
fn the_payload_github_sends_parses_into_what_is_read() {
    let raw = serde_json::json!({
        "ref": "refs/heads/main",
        "before": "1".repeat(40),
        "after": HEAD,
        "created": false,
        "deleted": false,
        "forced": false,
        "repository": { "id": 1, "name": "analytics", "full_name": "acme/analytics",
                        "default_branch": "main", "owner": { "login": "acme" } },
        "pusher": { "name": "octocat" },
        "head_commit": { "id": HEAD },
        "commits": [{ "id": HEAD, "added": [], "removed": [], "modified": ["config.yml"] }]
    });
    let parsed: PushEvent = serde_json::from_value(raw).unwrap();
    assert_eq!(parsed.branch(), Some("main"));
    assert_eq!(parsed.head().as_deref(), Some(HEAD));
    assert!(parsed.is_for("acme", "analytics"));
    assert_eq!(
        parsed.changed_paths().unwrap().collect::<Vec<_>>(),
        ["config.yml"]
    );

    // A deletion arrives with no commits and an `after` of zeros.
    let deletion: PushEvent = serde_json::from_value(serde_json::json!({
        "ref": "refs/heads/main", "after": "0".repeat(40), "deleted": true,
        "repository": { "full_name": "acme/analytics" }, "commits": []
    }))
    .unwrap();
    assert_eq!(deletion.head(), None);
}
