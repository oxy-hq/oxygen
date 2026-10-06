//! The periodic check that keeps a remote-backed workspace serving what its
//! default branch has (`oxy_app::server::compile_reconcile`).
//!
//! What is worth a real database and a fake GitHub here is what the loop must
//! *not* do: ask GitHub more than once per workspace per interval however
//! many replicas tick, queue a second compile behind the first, keep asking
//! about a repository GitHub will not answer for, or do anything at all with
//! its gate off.
//!
//! GitHub is a `wiremock` server reached through `GITHUB_API_URL`; the
//! workspace and its PAT connection come from [`super::compile_from_git`]. A
//! check is made due by moving `next_check_at` into the past, which is what
//! the passage of an interval does.
//!
//! Run with:
//! `cargo nextest run -p oxy-app --test platform -E 'test(compile_reconcile)'`

use chrono::Utc;
use entity::{revisions, workspace_compile_checks, workspaces};
use oxy_app::server::compile_reconcile::{CHECK_INTERVAL, Tick, tick};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ConnectionTrait, DatabaseBackend, DatabaseConnection,
    EntityTrait, Statement,
};
use uuid::Uuid;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::compile_from_git::{TOKEN, seed_workspace};
use crate::common::{Schema, test_db_with};

const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";
const BRANCH_HEAD_PATH: &str = "/repos/acme/analytics/commits/heads/main";

struct Fx {
    db: DatabaseConnection,
    ws: Uuid,
    github: MockServer,
}

impl Fx {
    /// The gate on, a fake GitHub, and one remote-backed workspace whose
    /// recorded default branch is `main`.
    async fn new() -> Self {
        let db = test_db_with(Schema::All).await;
        let github = MockServer::start().await;
        // SAFETY: nextest runs each test in its own process (`test_db_with`
        // asserts it), and nothing has read these yet.
        unsafe {
            std::env::set_var("GITHUB_API_URL", github.uri());
            std::env::set_var("OXY_COMPILE_RECONCILE", "1");
        }
        let ws = seed_workspace(&db, None).await;
        Self { db, ws, github }
    }

    /// GitHub's answer to "where is `main`", for a caller carrying the token.
    async fn github_answers(&self, response: ResponseTemplate) {
        self.github.reset().await;
        Mock::given(method("GET"))
            .and(path(BRANCH_HEAD_PATH))
            .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
            .respond_with(response)
            .mount(&self.github)
            .await;
    }

    async fn branch_head_is(&self, sha: &str) {
        self.github_answers(ResponseTemplate::new(200).set_body_string(sha))
            .await;
    }

    /// Let an interval pass: every check the loop knows of is due.
    async fn an_interval_passes(&self) {
        // The first tick only gives each workspace a row, at a random point
        // in the coming interval.
        let seeded = tick(&self.db).await;
        assert_eq!(seeded.checked, 0, "a freshly seeded check is not due yet");
        self.db
            .execute_unprepared(
                "UPDATE workspace_compile_checks SET next_check_at = now() - interval '1 second'",
            )
            .await
            .expect("make checks due");
    }

    /// Serve a ready main revision compiled from `git_sha`.
    async fn serving(&self, git_sha: &str) -> Uuid {
        let revision = self.ready_revision(git_sha).await;
        let mut ws: workspaces::ActiveModel = workspaces::Entity::find_by_id(self.ws)
            .one(&self.db)
            .await
            .unwrap()
            .unwrap()
            .into();
        ws.current_revision_id = ActiveValue::Set(Some(revision));
        ws.update(&self.db).await.expect("promote");
        revision
    }

    async fn ready_revision(&self, git_sha: &str) -> Uuid {
        let now = Utc::now().fixed_offset();
        let id = Uuid::new_v4();
        revisions::ActiveModel {
            revision_id: ActiveValue::Set(id),
            workspace_id: ActiveValue::Set(self.ws),
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

    async fn requests(&self) -> usize {
        self.github
            .received_requests()
            .await
            .unwrap_or_default()
            .len()
    }

    async fn check_row(&self, ws: Uuid) -> workspace_compile_checks::Model {
        workspace_compile_checks::Entity::find_by_id(ws)
            .one(&self.db)
            .await
            .expect("read check")
            .expect("the workspace has a check row")
    }

    /// `(run source_type, spec git_sha, spec from_git)` of every compile task
    /// queued for any workspace.
    async fn compile_tasks(&self) -> Vec<(String, String, bool)> {
        let rows = self
            .db
            .query_all_raw(Statement::from_string(
                DatabaseBackend::Postgres,
                "SELECT r.source_type, q.spec->>'git_sha', \
                        COALESCE((q.spec->>'from_git')::boolean, false) \
                 FROM agentic_task_queue q JOIN agentic_runs r ON r.id = q.run_id \
                 WHERE q.spec->>'type' = 'compile'",
            ))
            .await
            .expect("read the queue");
        rows.iter()
            .map(|r| {
                (
                    r.try_get_by_index(0).unwrap(),
                    r.try_get_by_index(1).unwrap(),
                    r.try_get_by_index(2).unwrap(),
                )
            })
            .collect()
    }
}

/// How far in the future a check's next run is, in seconds.
fn due_in(row: &workspace_compile_checks::Model) -> i64 {
    (row.next_check_at.with_timezone(&Utc) - Utc::now()).num_seconds()
}

#[tokio::test]
async fn with_the_gate_off_the_loop_does_nothing() {
    let fx = Fx::new().await;
    fx.branch_head_is(HEAD).await;
    // SAFETY: process-per-test; the loop reads this on every tick.
    unsafe { std::env::remove_var("OXY_COMPILE_RECONCILE") };

    for _ in 0..2 {
        assert_eq!(tick(&fx.db).await, Tick::default());
    }

    let rows = workspace_compile_checks::Entity::find()
        .all(&fx.db)
        .await
        .expect("read checks");
    assert!(rows.is_empty(), "no check may even be scheduled: {rows:?}");
    assert_eq!(fx.requests().await, 0);
    assert!(fx.compile_tasks().await.is_empty());
}

#[tokio::test]
async fn a_branch_head_that_is_being_served_queues_nothing() {
    let fx = Fx::new().await;
    fx.serving(HEAD).await;
    fx.branch_head_is(HEAD).await;
    fx.an_interval_passes().await;

    let ran = tick(&fx.db).await;

    assert_eq!((ran.checked, ran.enqueued), (1, 0));
    assert!(fx.compile_tasks().await.is_empty());
    let row = fx.check_row(fx.ws).await;
    assert_eq!(row.last_outcome.as_deref(), Some("up_to_date"));
    assert_eq!(row.last_head_sha.as_deref(), Some(HEAD));
    let interval = CHECK_INTERVAL.as_secs() as i64;
    assert!(
        (interval - 30..=interval).contains(&due_in(&row)),
        "the next check is one interval out, got {}s",
        due_in(&row)
    );
}

/// The case this loop exists for: the workspace serves a snapshot of a disk,
/// and its branch has a commit nobody compiled.
#[tokio::test]
async fn a_moved_branch_head_queues_one_commit_compile_however_many_replicas_tick() {
    let fx = Fx::new().await;
    fx.serving("local-ce83e24f").await;
    fx.branch_head_is(HEAD).await;
    fx.an_interval_passes().await;

    let replicas = futures::future::join_all((0..6).map(|_| tick(&fx.db))).await;

    let checked: usize = replicas.iter().map(|t| t.checked).sum();
    let enqueued: usize = replicas.iter().map(|t| t.enqueued).sum();
    assert_eq!((checked, enqueued), (1, 1), "{replicas:?}");
    assert_eq!(fx.requests().await, 1, "one replica asks GitHub");
    let expected = vec![("compile_git".to_string(), HEAD.to_string(), true)];
    assert_eq!(fx.compile_tasks().await, expected);

    // The next interval: the compile is still queued, so nothing is added.
    fx.an_interval_passes().await;
    let again = tick(&fx.db).await;
    assert_eq!(again.checked, 1);
    assert_eq!(fx.compile_tasks().await, expected);
}

/// The node with the working copy compiled this head and then something
/// newer, which is what is being served. Queueing the head again would fetch
/// it only for the promote to be declined — every interval, for good.
#[tokio::test]
async fn a_head_that_is_already_compiled_is_left_alone() {
    let fx = Fx::new().await;
    fx.ready_revision(HEAD).await;
    fx.serving("89abcdef0123456789abcdef0123456789abcdef").await;
    fx.branch_head_is(HEAD).await;
    fx.an_interval_passes().await;

    let ran = tick(&fx.db).await;

    assert_eq!((ran.checked, ran.enqueued), (1, 0));
    assert!(fx.compile_tasks().await.is_empty());
    assert_eq!(
        fx.check_row(fx.ws).await.last_outcome.as_deref(),
        Some("already_compiled")
    );
}

#[tokio::test]
async fn a_branch_github_will_not_answer_for_queues_nothing_and_backs_off() {
    let fx = Fx::new().await;
    fx.serving("local-ce83e24f").await;

    for (status, outcome) in [(404, "not_found"), (403, "denied")] {
        fx.github_answers(ResponseTemplate::new(status)).await;
        fx.an_interval_passes().await;

        let ran = tick(&fx.db).await;

        assert_eq!((ran.checked, ran.enqueued, ran.rate_limited), (1, 0, false));
        assert!(fx.compile_tasks().await.is_empty(), "HTTP {status}");
        let row = fx.check_row(fx.ws).await;
        assert_eq!(row.last_outcome.as_deref(), Some(outcome));
        assert!(
            due_in(&row) > 50 * 60,
            "HTTP {status}: left alone for about an hour, got {}s",
            due_in(&row)
        );
    }
}

/// A rate limit belongs to the token, not to the workspace that happened to
/// be asked: the loop stops, for every workspace, and stays stopped.
#[tokio::test]
async fn a_rate_limit_stops_the_whole_loop() {
    let fx = Fx::new().await;
    let other = seed_workspace(&fx.db, None).await;
    fx.github_answers(ResponseTemplate::new(429).insert_header("retry-after", "60"))
        .await;
    fx.an_interval_passes().await;

    let ran = tick(&fx.db).await;

    assert_eq!((ran.checked, ran.rate_limited), (1, true));
    assert_eq!(fx.requests().await, 1, "the second workspace is not asked");
    let mut outcomes = vec![
        fx.check_row(fx.ws).await.last_outcome,
        fx.check_row(other).await.last_outcome,
    ];
    outcomes.sort();
    assert_eq!(outcomes, [None, Some("rate_limited".to_string())]);

    // Still paused on the next tick, with a check still due.
    assert_eq!(tick(&fx.db).await, Tick::default());
    assert_eq!(fx.requests().await, 1);
    assert!(fx.compile_tasks().await.is_empty());
}

/// The tick runs inline on the periodic driver, ahead of schedules and health
/// checks. When GitHub does not answer, the next workspace would most likely
/// wait out the same timeout, so the tick stops — but it is not a rate limit,
/// and the following tick tries again.
#[tokio::test]
async fn an_unanswered_check_ends_the_tick_without_pausing_the_loop() {
    let fx = Fx::new().await;
    seed_workspace(&fx.db, None).await;
    fx.github_answers(ResponseTemplate::new(503)).await;
    fx.an_interval_passes().await;

    let first = tick(&fx.db).await;

    assert_eq!((first.checked, first.rate_limited), (1, false));
    assert_eq!(
        fx.requests().await,
        1,
        "the second workspace is left for later"
    );

    // Not paused: the workspace that was not reached is still due, and the
    // next tick asks about it.
    let second = tick(&fx.db).await;
    assert_eq!(second.checked, 1);
    assert_eq!(fx.requests().await, 2);
    assert!(fx.compile_tasks().await.is_empty());
}

#[tokio::test]
async fn a_workspace_with_no_github_connection_is_skipped_for_hours() {
    let fx = Fx::new().await;
    fx.branch_head_is(HEAD).await;
    let mut unlinked: workspaces::ActiveModel = workspaces::Entity::find_by_id(fx.ws)
        .one(&fx.db)
        .await
        .unwrap()
        .unwrap()
        .into();
    unlinked.git_namespace_id = ActiveValue::Set(None);
    unlinked.update(&fx.db).await.expect("unlink");
    fx.an_interval_passes().await;

    let ran = tick(&fx.db).await;

    assert_eq!((ran.checked, ran.enqueued), (1, 0));
    assert_eq!(fx.requests().await, 0, "there is no token to ask with");
    assert!(fx.compile_tasks().await.is_empty());
    let row = fx.check_row(fx.ws).await;
    assert_eq!(row.last_outcome.as_deref(), Some("no_token"));
    assert!(due_in(&row) > 5 * 60 * 60, "got {}s", due_in(&row));
}
