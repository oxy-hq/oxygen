//! A GitHub `push` delivery queues a commit compile
//! (`oxy_app::server::compile_reconcile::push`, behind `POST /api/webhooks/github`).
//!
//! A parsed delivery is handed to `push::handle` against a real database, as the
//! receiver hands it once the signature checks out: what matters is which
//! workspaces get a `compile_git` task and, as much, which do not — another
//! repository, another branch, another subdirectory, a duplicate delivery.
//!
//! The receiver itself — the signature, `ping`, other events, a malformed
//! payload, an unconfigured secret — is `oxy-api-github`'s `webhook` since
//! 2026-11-15, and its tests are there (in-source, plus `webhook_push` in its
//! integration group for the hand-off to this module). `oxy-app` cannot depend
//! on that crate, so this file drives the half that stayed.
//!
//! The last test drives the worker queue with a fake GitHub that is down for the
//! first attempt, to show that a task the webhook queued retries on the queue's
//! own policy and ends promoted.
//!
//! Run with:
//! `cargo nextest run -p oxy-app --test platform -E 'test(compile_push)'`

use std::time::Duration;

use chrono::Utc;
use entity::{revisions, workspaces};
use oxy_app::server::compile_reconcile::push::{self, PushEvent};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ConnectionTrait, DatabaseBackend, DatabaseConnection,
    EntityTrait, Statement,
};
use serde_json::{Value, json};
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::compile_from_git::{drive_worker_queue, seed_workspace, workspace_tree};
use crate::common::{Schema, test_db_with};

const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";

struct Fx {
    db: DatabaseConnection,
    /// The workspace on `acme/analytics`, branch `main`, at the repository root.
    ws: Uuid,
    github: MockServer,
}

impl Fx {
    async fn new() -> Self {
        let db = test_db_with(Schema::All).await;
        let github = MockServer::start().await;
        // SAFETY: nextest runs each test in its own process (`test_db_with`
        // asserts it), and nothing has read these yet.
        unsafe {
            std::env::set_var("GITHUB_API_URL", github.uri());
            std::env::remove_var("OXY_COMPILE_BLOB_S3_BUCKET");
        }
        let ws = seed_workspace(&db, None).await;
        Self { db, ws, github }
    }

    /// A second workspace sharing this one's connection, with its own remote,
    /// branch and subdirectory.
    async fn another(&self, remote: &str, branch: &str, subdir: Option<&str>) -> Uuid {
        let id = seed_workspace(&self.db, subdir).await;
        let mut ws: workspaces::ActiveModel = workspaces::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .unwrap()
            .unwrap()
            .into();
        ws.git_remote_url = ActiveValue::Set(Some(remote.into()));
        ws.default_branch = ActiveValue::Set(Some(branch.into()));
        ws.update(&self.db).await.expect("repoint workspace");
        id
    }

    /// Hand `body` to `push::handle` as the receiver does once its signature
    /// checks out, and answer with the line the receiver puts in its `202`.
    async fn push(&self, body: &Value) -> String {
        let push: PushEvent = serde_json::from_value(body.clone()).expect("a push payload");
        push::handle(&self.db, &push)
            .await
            .expect("push handled")
            .describe()
    }

    /// `(workspace, git_sha, source_type, from_git, has a retry policy)` of
    /// every compile task, whatever its state.
    async fn tasks(&self) -> Vec<(Uuid, String, String, bool, bool)> {
        let rows = self
            .db
            .query_all_raw(Statement::from_string(
                DatabaseBackend::Postgres,
                "SELECT q.spec->>'workspace_id', q.spec->>'git_sha', r.source_type, \
                        COALESCE((q.spec->>'from_git')::boolean, false), \
                        q.policy->'retry' IS NOT NULL \
                 FROM agentic_task_queue q JOIN agentic_runs r ON r.id = q.run_id \
                 WHERE q.spec->>'type' = 'compile' ORDER BY q.created_at",
            ))
            .await
            .expect("read the queue");
        rows.iter()
            .map(|r| {
                let ws: String = r.try_get_by_index(0).unwrap();
                (
                    ws.parse().unwrap(),
                    r.try_get_by_index(1).unwrap(),
                    r.try_get_by_index(2).unwrap(),
                    r.try_get_by_index(3).unwrap(),
                    r.try_get_by_index(4).unwrap(),
                )
            })
            .collect()
    }

    async fn serve_ready_revision(&self, ws: Uuid, git_sha: &str) {
        self.ready_revision_started(ws, git_sha, Utc::now()).await;
    }

    /// A ready main revision whose compile started at `started`. Returns its id.
    async fn ready_revision_started(
        &self,
        ws: Uuid,
        git_sha: &str,
        started: chrono::DateTime<Utc>,
    ) -> Uuid {
        let now = started.fixed_offset();
        let id = Uuid::new_v4();
        revisions::ActiveModel {
            revision_id: ActiveValue::Set(id),
            workspace_id: ActiveValue::Set(ws),
            git_sha: ActiveValue::Set(git_sha.into()),
            branch: ActiveValue::Set(Some("main".into())),
            schema_version: ActiveValue::Set(1),
            status: ActiveValue::Set("ready".into()),
            kind: ActiveValue::Set("main".into()),
            owner_user_id: ActiveValue::Set(None),
            compiler_version: ActiveValue::Set("test".into()),
            started_at: ActiveValue::Set(now),
            finished_at: ActiveValue::Set(Some(now)),
            file_count_seen: ActiveValue::Set(1),
            file_count_compiled: ActiveValue::Set(1),
            file_count_failed: ActiveValue::Set(0),
            error_summary: ActiveValue::Set(None),
        }
        .insert(&self.db)
        .await
        .expect("seed revision");
        id
    }
}

fn push_body(git_ref: &str, repository: &str, after: &str, paths: &[&str]) -> Value {
    json!({
        "ref": git_ref,
        "after": after,
        "deleted": after.bytes().all(|b| b == b'0'),
        "repository": { "full_name": repository },
        "commits": [{ "added": [], "removed": [], "modified": paths }],
    })
}

fn main_push(paths: &[&str]) -> Value {
    push_body("refs/heads/main", "acme/analytics", HEAD, paths)
}

#[tokio::test]
async fn a_push_queues_one_commit_compile_for_the_matching_workspace_and_no_other() {
    let fx = Fx::new().await;
    let other_repo = fx
        .another("https://github.com/acme/other.git", "main", None)
        .await;
    let other_branch = fx
        .another("https://github.com/acme/analytics.git", "develop", None)
        .await;
    let untouched_subdir = fx
        .another(
            "https://github.com/acme/analytics.git",
            "main",
            Some("reports"),
        )
        .await;
    let touched_subdir = fx
        .another(
            "https://github.com/acme/analytics.git",
            "main",
            Some("analytics"),
        )
        .await;
    let _ = (other_repo, other_branch, untouched_subdir);

    let body = fx.push(&main_push(&["analytics/config.yml"])).await;
    assert_eq!(body, "2 workspace(s) match; 2 handed to the compile queue");

    let mut tasks = fx.tasks().await;
    tasks.sort_by_key(|t| t.0);
    let mut expected = vec![
        (
            fx.ws,
            HEAD.to_string(),
            "compile_git".to_string(),
            true,
            true,
        ),
        (
            touched_subdir,
            HEAD.to_string(),
            "compile_git".to_string(),
            true,
            true,
        ),
    ];
    expected.sort_by_key(|t| t.0);
    assert_eq!(
        tasks, expected,
        "one commit compile, with a retry policy, for each workspace the push concerns"
    );
}

#[tokio::test]
async fn a_duplicate_delivery_queues_nothing_new() {
    let fx = Fx::new().await;
    let body = main_push(&["config.yml"]);

    fx.push(&body).await;
    assert_eq!(fx.tasks().await.len(), 1);

    // The same delivery again while the first is still queued.
    fx.push(&body).await;
    // And a different push to the same branch: the one compile in flight is not
    // queued behind, and the periodic check picks the newer head up.
    let newer = push_body(
        "refs/heads/main",
        "acme/analytics",
        "89abcdef0123456789abcdef0123456789abcdef",
        &["config.yml"],
    );
    fx.push(&newer).await;
    assert_eq!(fx.tasks().await.len(), 1, "{:?}", fx.tasks().await);
}

#[tokio::test]
async fn a_head_that_is_already_compiled_or_served_queues_nothing() {
    let fx = Fx::new().await;
    fx.serve_ready_revision(fx.ws, HEAD).await;
    let body = fx.push(&main_push(&["config.yml"])).await;
    assert_eq!(body, "1 workspace(s) match; 0 handed to the compile queue");
    assert!(fx.tasks().await.is_empty());
}

#[tokio::test]
async fn pushes_that_are_not_to_a_branch_head_do_nothing() {
    let fx = Fx::new().await;
    for body in [
        push_body("refs/heads/feature/x", "acme/analytics", HEAD, &[]),
        push_body("refs/tags/v1", "acme/analytics", HEAD, &[]),
        push_body("refs/heads/main", "acme/analytics", &"0".repeat(40), &[]),
        push_body("refs/heads/main", "someone/analytics", HEAD, &[]),
    ] {
        let outcome = fx.push(&body).await;
        assert!(
            outcome.starts_with("ignored: ") || outcome.starts_with("0 "),
            "{body}: {outcome}"
        );
    }
    assert!(fx.tasks().await.is_empty(), "{:?}", fx.tasks().await);
}

/// The task the webhook queued retries on the queue's own policy: GitHub is
/// down for the first attempt, the same row is claimed again after its backoff,
/// and the compile promotes.
#[tokio::test]
async fn a_transient_github_failure_is_requeued_and_the_commit_is_promoted() {
    let fx = Fx::new().await;
    // SAFETY: single test per process.
    unsafe { std::env::set_var("OXY_ROLE", "worker") };
    oxy_app::server::role_manifest::init_process_role_from_env();

    let archive = format!("/repos/acme/analytics/tarball/{HEAD}");
    Mock::given(method("GET"))
        .and(path(archive.clone()))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&fx.github)
        .await;
    Mock::given(method("GET"))
        .and(path(archive))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(workspace_tree()))
        .with_priority(2)
        .mount(&fx.github)
        .await;

    fx.push(&main_push(&["config.yml"])).await;
    assert_eq!(fx.tasks().await.len(), 1);

    // The real backoff is 15 s; what is under test is that the row is retried,
    // not how long it waits, so let the wait pass as soon as one is recorded.
    let db = fx.db.clone();
    let skip_the_wait = tokio::spawn(async move {
        loop {
            let _ = db
                .execute_unprepared(
                    "UPDATE agentic_task_queue SET available_at = now() \
                     WHERE queue_status = 'queued' AND retry_attempt > 0",
                )
                .await;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });

    let promoted = drive_worker_queue(&fx.db, fx.ws, || async {
        workspaces::Entity::find_by_id(fx.ws)
            .one(&fx.db)
            .await
            .unwrap()
            .unwrap()
            .current_revision_id
            .is_some()
    })
    .await;
    skip_the_wait.abort();
    assert!(promoted, "the retried compile never promoted");

    let row = fx
        .db
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT queue_status, retry_attempt FROM agentic_task_queue \
             WHERE spec->>'type' = 'compile'",
        ))
        .await
        .unwrap()
        .expect("the one compile task");
    assert_eq!(
        row.try_get_by_index::<i32>(1).unwrap(),
        1,
        "one retry spent"
    );
    assert_eq!(
        fx.tasks().await.len(),
        1,
        "a retry is the same row, not a new task"
    );
    assert_eq!(
        fx.github
            .received_requests()
            .await
            .unwrap_or_default()
            .len(),
        2,
        "one failed fetch, one successful"
    );
}

/// A retried commit compile runs minutes after it was asked for. A compile that
/// started after the ask and is being served (the Compile button's, say) is
/// newer than it in the only sense the promotion rule knows, so the retry must
/// not take over from it.
#[tokio::test]
async fn a_retried_commit_compile_does_not_take_over_from_one_that_started_after_it_was_asked_for()
{
    let fx = Fx::new().await;
    // SAFETY: single test per process.
    unsafe { std::env::set_var("OXY_ROLE", "worker") };
    oxy_app::server::role_manifest::init_process_role_from_env();

    let archive = format!("/repos/acme/analytics/tarball/{HEAD}");
    Mock::given(method("GET"))
        .and(path(archive.clone()))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&fx.github)
        .await;
    Mock::given(method("GET"))
        .and(path(archive))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(workspace_tree()))
        .with_priority(2)
        .mount(&fx.github)
        .await;

    fx.push(&main_push(&["config.yml"])).await;
    assert_eq!(fx.tasks().await.len(), 1);

    // While the push's compile waits for GitHub, another compile starts later
    // than the push was queued, and is promoted.
    let newer = fx
        .ready_revision_started(
            fx.ws,
            "89abcdef0123456789abcdef0123456789abcdef",
            // After the push queued its compile, before the retry starts.
            Utc::now(),
        )
        .await;
    fx.db
        .execute_unprepared(&format!(
            "UPDATE workspaces SET current_revision_id = '{newer}' WHERE id = '{}'",
            fx.ws
        ))
        .await
        .unwrap();

    let db = fx.db.clone();
    let skip_the_wait = tokio::spawn(async move {
        loop {
            let _ = db
                .execute_unprepared(
                    "UPDATE agentic_task_queue SET available_at = now() \
                     WHERE queue_status = 'queued' AND retry_attempt > 0",
                )
                .await;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });
    let compiled = drive_worker_queue(&fx.db, fx.ws, || async {
        let rows = revisions::Entity::find().all(&fx.db).await.unwrap();
        rows.iter()
            .any(|r| r.git_sha == HEAD && r.status == "ready")
    })
    .await;
    skip_the_wait.abort();
    assert!(compiled, "the retried compile never ran");

    let served = workspaces::Entity::find_by_id(fx.ws)
        .one(&fx.db)
        .await
        .unwrap()
        .unwrap()
        .current_revision_id;
    assert_eq!(
        served,
        Some(newer),
        "the retry kept its revision but must not have moved the pointer"
    );
}
