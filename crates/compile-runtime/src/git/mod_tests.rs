use std::fs;

use oxy::github::TarballError;
use uuid::Uuid;

use super::{CommitSource, CompileGitError, UnpackError, commit_sha, workspace_root_in};

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

#[test]
fn a_workspace_at_the_repository_root_compiles_the_tree_itself() {
    let tree = tempfile::tempdir().unwrap();
    for at_root in [None, Some("")] {
        assert_eq!(
            workspace_root_in(tree.path(), at_root).unwrap(),
            tree.path()
        );
    }
}

#[test]
fn a_workspace_in_a_subdirectory_compiles_that_directory() {
    let tree = tempfile::tempdir().unwrap();
    fs::create_dir_all(tree.path().join("data/oxy")).unwrap();

    let root = workspace_root_in(tree.path(), Some("data/oxy")).unwrap();

    assert_eq!(root, tree.path().canonicalize().unwrap().join("data/oxy"));
}

#[test]
fn a_subdirectory_the_commit_does_not_have_is_a_typed_failure() {
    let tree = tempfile::tempdir().unwrap();
    fs::write(tree.path().join("a-file"), b"x").unwrap();

    for subdir in ["analytics", "a-file", "../outside", "/etc", "./analytics"] {
        assert!(
            matches!(
                workspace_root_in(tree.path(), Some(subdir)),
                Err(CompileGitError::SubdirMissing(ref s)) if s == subdir
            ),
            "{subdir}"
        );
    }
}

/// The name is in the commit, but as a link to somewhere else on the host.
#[cfg(unix)]
#[test]
fn a_subdirectory_that_resolves_outside_the_tree_is_refused() {
    let outside = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), tree.path().join("analytics")).unwrap();

    assert!(matches!(
        workspace_root_in(tree.path(), Some("analytics")),
        Err(CompileGitError::SubdirMissing(_))
    ));
}

#[test]
fn only_a_full_commit_id_names_a_commit() {
    assert_eq!(commit_sha(Some(SHA)).unwrap(), SHA);
    assert_eq!(
        commit_sha(Some(&SHA.to_uppercase())).unwrap(),
        SHA,
        "one commit, one spelling: the dedupe compares strings"
    );
    for not_a_commit in [None, Some(""), Some("main"), Some("0123abc"), Some("local")] {
        assert!(
            matches!(
                commit_sha(not_a_commit),
                Err(CompileGitError::NotACommit(_))
            ),
            "{not_a_commit:?}"
        );
    }
    let synthetic = format!("local-{}", Uuid::new_v4());
    assert!(commit_sha(Some(&synthetic)).is_err());
}

/// After GitHub's redirect the URL is the archive host's and, for a private
/// repository, carries a download token. A transport error names the URL it
/// failed on, and that text ends up in a task's failure message.
#[tokio::test]
async fn a_transport_failure_is_reported_without_the_url_it_failed_on() {
    let error = reqwest::Client::new()
        .get("http://127.0.0.1:1/archive?token=download-secret")
        .send()
        .await
        .expect_err("nothing listens on port 1");
    assert!(
        error.to_string().contains("download-secret"),
        "the raw error is what would have leaked: {error}"
    );

    let reported = oxy::github::transport_error(error);

    assert!(!reported.contains("download-secret"), "{reported}");
    assert!(!reported.contains("127.0.0.1"), "{reported}");
}

/// A `CommitSource` reaches logs through `?`; its token must not.
#[test]
fn formatting_a_commit_source_does_not_print_its_token() {
    let source = CommitSource {
        workspace_id: Uuid::nil(),
        owner: "acme".into(),
        repo: "analytics".into(),
        sha: SHA.into(),
        token: "ghs_a_real_looking_secret".into(),
        repo_subdir: None,
    };
    let printed = format!("{source:?}");
    assert!(!printed.contains("ghs_a_real_looking_secret"), "{printed}");
    assert!(printed.contains("acme") && printed.contains("<redacted>"));
}

fn github(e: TarballError) -> CompileGitError {
    CompileGitError::GitHub(e)
}

#[test]
fn only_an_outage_is_retryable_and_the_message_says_which() {
    let unavailable = github(TarballError::Unavailable("HTTP 503".into()));
    assert_eq!(unavailable.code(), "github_unavailable");
    assert!(unavailable.is_retryable());
    assert!(
        unavailable
            .task_failure()
            .starts_with("compile from git failed [github_unavailable, retryable]: ")
    );

    let not_found = github(TarballError::NotFound {
        owner: "acme".into(),
        repo: "analytics".into(),
        sha: SHA.into(),
    });
    assert_eq!(not_found.code(), "commit_not_found");
    assert!(!not_found.is_retryable());
    assert!(
        not_found
            .task_failure()
            .starts_with("compile from git failed [commit_not_found]: ")
    );
}

#[test]
fn every_failure_a_person_has_to_fix_is_not_retryable() {
    let ws = Uuid::new_v4();
    let needs_a_person = [
        (CompileGitError::NoRemote(ws), "no_remote"),
        (CompileGitError::NoToken(ws), "no_token"),
        (
            CompileGitError::UnsupportedRemote("https://gitlab.com/a/b".into()),
            "unsupported_remote",
        ),
        (
            CompileGitError::ArchiveTooLarge { limit: 1 },
            "archive_too_large",
        ),
        (
            CompileGitError::Unpack(UnpackError::TooLarge { limit: 1 }),
            "tree_too_large",
        ),
        (
            CompileGitError::Unpack(UnpackError::TooManyFiles { limit: 1 }),
            "tree_too_large",
        ),
        (
            CompileGitError::Unpack(UnpackError::UnsafePath("../x".into())),
            "bad_archive",
        ),
        (
            github(TarballError::Denied {
                owner: "acme".into(),
                repo: "analytics".into(),
                status: 403,
            }),
            "github_denied",
        ),
        (
            CompileGitError::DuckDbDataMissing {
                database: "sales".into(),
                path: ".db".into(),
            },
            "duckdb_data_missing",
        ),
        (
            CompileGitError::DuckDbPathOutsideTree {
                database: "sales".into(),
                path: "/proc/self/environ".into(),
            },
            "duckdb_path_outside_tree",
        ),
        (
            CompileGitError::DuckDbFileDatabase {
                database: "sales".into(),
            },
            "duckdb_file_database",
        ),
    ];
    for (error, code) in needs_a_person {
        assert_eq!(error.code(), code);
        assert!(!error.is_retryable(), "{code}");
    }
}

/// The coordinator logs these refusals at WARN rather than ERROR, because only
/// the workspace's owner can act on them and `fetch_for_task` has already
/// logged each with its `workspace_id`. It cannot import this crate
/// (`agentic-runtime` depends on `agentic-core` only), so it matches the
/// leading `compile from git failed [<code>]: ` as text —
/// `is_workspace_config_refusal` in
/// `crates/agentic/runtime/src/orchestrator/coordinator/outcomes.rs`. Renaming
/// one of these codes without editing that list would silently put the
/// refusal back in Sentry's error events, which is what this pins.
#[test]
fn the_codes_the_coordinator_warns_on_keep_their_names() {
    let warned = [
        CompileGitError::DuckDbFileDatabase {
            database: "sales".into(),
        },
        CompileGitError::DuckDbDataMissing {
            database: "sales".into(),
            path: ".db".into(),
        },
        CompileGitError::DuckDbPathOutsideTree {
            database: "sales".into(),
            path: "/proc/self/environ".into(),
        },
        CompileGitError::NoConfig,
        CompileGitError::SubdirMissing("warehouse".into()),
        github(TarballError::NotFound {
            owner: "acme".into(),
            repo: "analytics".into(),
            sha: SHA.into(),
        }),
        CompileGitError::Unpack(UnpackError::TooLarge { limit: 1 }),
    ];
    let mut codes: Vec<&str> = warned
        .iter()
        .map(|error| {
            let code = error.code();
            let message = error.task_failure();
            assert!(
                message.starts_with(&format!("compile from git failed [{code}]: ")),
                "{message}"
            );
            code
        })
        .collect();
    codes.sort_unstable();
    assert_eq!(
        codes,
        [
            "commit_not_found",
            "duckdb_data_missing",
            "duckdb_file_database",
            "duckdb_path_outside_tree",
            "no_config",
            "subdir_missing",
            "tree_too_large",
        ]
    );
}
