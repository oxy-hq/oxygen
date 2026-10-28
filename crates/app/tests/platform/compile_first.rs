//! The first compile of a workspace that has nothing compiled, on a worker.
//!
//! Until now a workspace with no promoted revision could only be compiled by
//! the Factory: a request that found it queued a working-copy compile, and the
//! worker's driver refused to build a context for it (nothing compiled, no
//! working copy), so even a commit compile that was queued for it sat unclaimed.
//! Worse, the working-copy compile nobody could run counted as "a compile in
//! flight" and kept every commit compile out of the queue.
//!
//! What is under test here is each step the first compile now takes:
//!
//! 1. a request finds the workspace empty and, with the branch check on,
//!    brings that check forward instead of queueing a working-copy compile;
//! 2. the check queues a commit compile of the branch head;
//! 3. a context built the way the worker builds one for a workspace with
//!    nothing compiled (its path is not on this node) drives that compile, and
//!    drives nothing else (`DrivePolicy::Exactly`).
//!
//! `tick_cloud`'s own glue between 2 and 3 is private to the router and not
//! driven here.
//!
//! Run with:
//! `cargo nextest run -p oxy-app --test platform -E 'test(compile_first)'`

use std::sync::Arc;

use agentic_pipeline::platform::PlatformContext;
use agentic_pipeline::recovery::{DrivePolicy, recover_pending_global_runs};
use agentic_runtime::coordinator::COMPILE_GIT_SOURCE_TYPE;
use agentic_runtime::state::RuntimeState;
use entity::{workspace_compile_checks, workspaces};
use oxy::adapters::workspace::builder::WorkspaceBuilder;
use oxy_app::agentic_wiring::OxyProjectContext;
use oxy_app::server::api::middlewares::workspace_context::enqueue_lazy_compile;
use oxy_app::server::compile_reconcile::{request_check, tick};
use oxy_app::server::previews::runtime::PreviewRunResolver;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, EntityTrait, Statement};
use uuid::Uuid;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::compile_from_git::{TOKEN, seed_workspace, workspace_tree};
use crate::common::{Schema, test_db_with};

const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";

struct Fx {
    db: DatabaseConnection,
    ws: Uuid,
    github: MockServer,
}

impl Fx {
    async fn new(gate_on: bool) -> Self {
        let db = test_db_with(Schema::All).await;
        let github = MockServer::start().await;
        // SAFETY: nextest runs each test in its own process (`test_db_with`
        // asserts it), and nothing has read these yet.
        unsafe {
            std::env::set_var("OXY_ROLE", "worker");
            std::env::set_var("GITHUB_API_URL", github.uri());
            std::env::remove_var("OXY_COMPILE_BLOB_S3_BUCKET");
            if gate_on {
                std::env::set_var("OXY_COMPILE_RECONCILE", "1");
            } else {
                std::env::remove_var("OXY_COMPILE_RECONCILE");
            }
        }
        oxy_app::server::role_manifest::init_process_role_from_env();
        let ws = seed_workspace(&db, None).await;
        Self { db, ws, github }
    }

    async fn github_has_the_branch(&self) {
        Mock::given(method("GET"))
            .and(path("/repos/acme/analytics/commits/heads/main"))
            .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_string(HEAD))
            .mount(&self.github)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/repos/acme/analytics/tarball/{HEAD}")))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(workspace_tree()))
            .mount(&self.github)
            .await;
    }

    /// `(run source_type, queue_status)` of every compile task.
    async fn tasks(&self) -> Vec<(String, String)> {
        let rows = self
            .db
            .query_all_raw(Statement::from_string(
                DatabaseBackend::Postgres,
                "SELECT r.source_type, q.queue_status FROM agentic_task_queue q \
                 JOIN agentic_runs r ON r.id = q.run_id WHERE q.spec->>'type' = 'compile'",
            ))
            .await
            .expect("read the queue");
        rows.iter()
            .map(|r| {
                (
                    r.try_get_by_index(0).unwrap(),
                    r.try_get_by_index(1).unwrap(),
                )
            })
            .collect()
    }

    async fn check(&self) -> Option<workspace_compile_checks::Model> {
        workspace_compile_checks::Entity::find_by_id(self.ws)
            .one(&self.db)
            .await
            .unwrap()
    }

    async fn promoted(&self) -> Option<Uuid> {
        workspaces::Entity::find_by_id(self.ws)
            .one(&self.db)
            .await
            .unwrap()
            .unwrap()
            .current_revision_id
    }
}

#[tokio::test]
async fn with_the_check_on_a_request_for_an_empty_workspace_asks_for_the_check_not_a_compile() {
    let fx = Fx::new(true).await;
    enqueue_lazy_compile(&fx.db, fx.ws).await;
    assert!(
        fx.tasks().await.is_empty(),
        "a working-copy compile nobody can run would block the commit compile"
    );
    let check = fx.check().await.expect("the workspace has a check");
    assert!(check.next_check_at <= chrono::Utc::now().fixed_offset());
}

/// When no check will look at the workspace, waiting for one is waiting for
/// nothing: the working-copy compile is the only way to a first revision.
#[tokio::test]
async fn a_workspace_the_check_will_not_look_at_still_gets_the_working_copy_compile() {
    let fx = Fx::new(true).await;
    // Its last check found something only a person can fix.
    fx.db
        .execute_unprepared(&format!(
            "INSERT INTO workspace_compile_checks (workspace_id, next_check_at, last_checked_at, last_outcome) \
             VALUES ('{}', now() + interval '1 hour', now() - interval '10 minutes', 'not_found')",
            fx.ws
        ))
        .await
        .unwrap();
    enqueue_lazy_compile(&fx.db, fx.ws).await;
    assert_eq!(
        fx.tasks().await,
        [("compile".to_string(), "queued".to_string())]
    );
}

#[tokio::test]
async fn a_workspace_that_is_not_ready_still_gets_the_working_copy_compile() {
    let fx = Fx::new(true).await;
    fx.db
        .execute_unprepared(&format!(
            "UPDATE workspaces SET status = 'cloning' WHERE id = '{}'",
            fx.ws
        ))
        .await
        .unwrap();
    enqueue_lazy_compile(&fx.db, fx.ws).await;
    assert_eq!(
        fx.tasks().await,
        [("compile".to_string(), "queued".to_string())]
    );
    assert!(fx.check().await.is_none());
}

#[tokio::test]
async fn with_the_check_off_it_is_still_a_working_copy_compile() {
    let fx = Fx::new(false).await;
    enqueue_lazy_compile(&fx.db, fx.ws).await;
    assert_eq!(
        fx.tasks().await,
        [("compile".to_string(), "queued".to_string())]
    );
    assert!(fx.check().await.is_none());
}

#[tokio::test]
async fn asking_again_straight_after_a_check_does_not_bring_the_next_one_forward() {
    let fx = Fx::new(true).await;
    fx.github_has_the_branch().await;
    request_check(&fx.db, fx.ws).await;
    let first = tick(&fx.db).await;
    assert_eq!(first.checked, 1, "{first:?}");

    // The check has just run and claimed its next one an interval away.
    let after = fx.check().await.unwrap();
    request_check(&fx.db, fx.ws).await;
    request_check(&fx.db, fx.ws).await;
    assert_eq!(
        fx.check().await.unwrap().next_check_at,
        after.next_check_at,
        "a page that keeps retrying must not turn into a request a tick"
    );
    assert_eq!(tick(&fx.db).await.checked, 0);
}

#[tokio::test]
async fn an_empty_workspace_is_compiled_from_its_branch_by_a_context_with_no_files() {
    let fx = Fx::new(true).await;
    fx.github_has_the_branch().await;

    enqueue_lazy_compile(&fx.db, fx.ws).await;
    let ticked = tick(&fx.db).await;
    assert_eq!((ticked.checked, ticked.enqueued), (1, 1), "{ticked:?}");
    assert_eq!(
        fx.tasks().await,
        [(COMPILE_GIT_SOURCE_TYPE.to_string(), "queued".to_string())]
    );

    // Something else pending for the same workspace, which a context built
    // for a commit compile must not run.
    let other = format!("airway-{}", Uuid::new_v4());
    agentic_runtime::crud::insert_run(&fx.db, &other, "Q", None, "airway", None, fx.ws)
        .await
        .unwrap();
    agentic_runtime::crud::enqueue_task(
        &fx.db,
        &other,
        &other,
        None,
        &agentic_core::delegation::TaskSpec::Custom {
            kind: "airway".into(),
            payload: serde_json::json!({}),
        },
        None,
        agentic_runtime::crud::TaskScope::Global,
    )
    .await
    .unwrap();

    // The context the worker builds when nothing is compiled: the workspace's
    // path, which is not on this node, and no compiled config.
    let path = workspaces::Entity::find_by_id(fx.ws)
        .one(&fx.db)
        .await
        .unwrap()
        .unwrap()
        .path
        .unwrap();
    assert!(!std::path::Path::new(&path).exists());
    let manager = WorkspaceBuilder::new(fx.ws)
        .with_working_copy(path.as_str(), None, oxy::config::OnMissing::Empty)
        .await
        .expect("a context for a path that is not here")
        .build()
        .await
        .expect("a manager");
    let ctx: Arc<dyn PlatformContext> =
        Arc::new(OxyProjectContext::new(manager).with_db(Arc::new(fx.db.clone())));

    let state = Arc::new(RuntimeState::new());
    let resolver = PreviewRunResolver::shared(&fx.db);
    let mut done = false;
    for _ in 0..200 {
        recover_pending_global_runs(
            fx.db.clone(),
            state.clone(),
            ctx.clone(),
            resolver.clone(),
            None,
            None,
            None,
            None,
            Arc::new(agentic_runtime::router::NoopTaskRouter),
            Some(fx.ws),
            None,
            DrivePolicy::Exactly(&[COMPILE_GIT_SOURCE_TYPE]),
        )
        .await;
        if fx.promoted().await.is_some() {
            done = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        done,
        "the first compile never promoted: {:?}",
        fx.tasks().await
    );

    let untouched = fx
        .db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT queue_status FROM agentic_task_queue WHERE task_id = $1",
            [other.into()],
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        untouched.try_get_by_index::<String>(0).unwrap(),
        "queued",
        "only the commit compile is driven under a context that has nothing compiled"
    );
}
