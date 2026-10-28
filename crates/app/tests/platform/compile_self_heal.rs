//! A lazy self-heal recompiles the commit the workspace serves
//! (`oxy_app::server::compile_git::self_heal`), and the recompile replaces the
//! revision an older compiler wrote for that commit
//! (`oxy_compile::writer::supersede_older_compiler`).
//!
//! The second half is what makes the first one work: the unique index on
//! `(workspace_id, git_sha)` lets one ready revision of a commit exist, so
//! before it a newer compiler's compile of a served commit lost to its own
//! predecessor and reported itself superseded by it.
//!
//! Run with:
//! `cargo nextest run -p oxy-app --test platform -E 'test(compile_self_heal)'`

use chrono::Utc;
use entity::{revisions, workspaces};
use oxy_app::server::api::middlewares::workspace_context::enqueue_lazy_compile;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, DatabaseBackend,
    DatabaseConnection, EntityTrait, QueryFilter, Statement,
};
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use oxy_app::server::compile_git;
use oxy_compile::RevisionKind;

use super::compile_from_git::{drive_worker_queue, seed_workspace, workspace_tree};
use crate::common::{Schema, test_db_with};

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

struct Fx {
    db: DatabaseConnection,
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
            std::env::set_var("OXY_ROLE", "worker");
            std::env::set_var("GITHUB_API_URL", github.uri());
            std::env::remove_var("OXY_COMPILE_BLOB_S3_BUCKET");
        }
        oxy_app::server::role_manifest::init_process_role_from_env();
        let ws = seed_workspace(&db, None).await;
        Self { db, ws, github }
    }

    async fn github_has_the_commit(&self) {
        Mock::given(method("GET"))
            .and(path(format!("/repos/acme/analytics/tarball/{SHA}")))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(workspace_tree()))
            .mount(&self.github)
            .await;
    }

    /// Serve a ready main revision of `git_sha` stamped `compiler`.
    async fn serving(&self, git_sha: &str, compiler: &str) -> Uuid {
        let now = Utc::now().fixed_offset();
        let id = Uuid::new_v4();
        revisions::ActiveModel {
            revision_id: ActiveValue::Set(id),
            workspace_id: ActiveValue::Set(self.ws),
            git_sha: ActiveValue::Set(git_sha.into()),
            branch: ActiveValue::Set(Some("main".into())),
            schema_version: ActiveValue::Set(oxy_compile::CURRENT_SCHEMA_VERSION),
            status: ActiveValue::Set("ready".into()),
            kind: ActiveValue::Set("main".into()),
            owner_user_id: ActiveValue::Set(None),
            compiler_version: ActiveValue::Set(compiler.into()),
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
        let mut ws: workspaces::ActiveModel = workspaces::Entity::find_by_id(self.ws)
            .one(&self.db)
            .await
            .unwrap()
            .unwrap()
            .into();
        ws.current_revision_id = ActiveValue::Set(Some(id));
        ws.update(&self.db).await.expect("promote");
        id
    }

    /// `(run source_type, spec git_sha)` of every compile task.
    async fn tasks(&self) -> Vec<(String, Option<String>)> {
        let rows = self
            .db
            .query_all_raw(Statement::from_string(
                DatabaseBackend::Postgres,
                "SELECT r.source_type, q.spec->>'git_sha' FROM agentic_task_queue q \
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

    async fn served(&self) -> Option<Uuid> {
        workspaces::Entity::find_by_id(self.ws)
            .one(&self.db)
            .await
            .unwrap()
            .unwrap()
            .current_revision_id
    }
}

#[tokio::test]
async fn a_workspace_serving_a_commit_recompiles_that_commit_over_the_older_compilers_revision() {
    let fx = Fx::new().await;
    fx.github_has_the_commit().await;
    let old = fx.serving(SHA, "oxy-compile/0.0.1").await;

    enqueue_lazy_compile(&fx.db, fx.ws).await;
    assert_eq!(
        fx.tasks().await,
        [("compile_git".to_string(), Some(SHA.to_string()))],
        "a commit compile of the served commit, not a working-copy compile"
    );

    let done = drive_worker_queue(&fx.db, fx.ws, || async { fx.served().await != Some(old) }).await;
    assert!(done, "the recompile never promoted");

    let old_row = revisions::Entity::find_by_id(old)
        .one(&fx.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        old_row.status, "superseded",
        "the older compiler's revision steps aside"
    );
    let ready = revisions::Entity::find()
        .filter(revisions::Column::WorkspaceId.eq(fx.ws))
        .filter(revisions::Column::Status.eq("ready"))
        .all(&fx.db)
        .await
        .unwrap();
    assert_eq!(ready.len(), 1, "{ready:?}");
    assert_eq!(ready[0].git_sha, SHA);
    assert_eq!(ready[0].compiler_version, oxy_compile::compiler_version());
    assert_eq!(fx.served().await, Some(ready[0].revision_id));
}

/// A commit compile of a commit whose served revision `stamp` wrote.
async fn commit_compile_over(stamp: &str) -> (Fx, Uuid) {
    let fx = Fx::new().await;
    fx.github_has_the_commit().await;
    let kept = fx.serving(SHA, stamp).await;
    compile_git::enqueue(&fx.db, fx.ws, SHA, Some("main"), RevisionKind::Main, true)
        .await
        .expect("enqueue");
    let finished = drive_worker_queue(&fx.db, fx.ws, || async {
        fx.db
            .query_one_raw(Statement::from_string(
                DatabaseBackend::Postgres,
                "SELECT 1 FROM agentic_task_queue WHERE spec->>'type' = 'compile' \
                 AND queue_status IN ('completed', 'failed', 'dead')",
            ))
            .await
            .unwrap()
            .is_some()
    })
    .await;
    assert!(finished, "the compile never finished");
    (fx, kept)
}

/// The writer never steps aside a revision of a newer or an equal compiler. The
/// self-heal does not ask for such a recompile (see the tests below), but a
/// person or a push can.
#[tokio::test]
async fn a_newer_compilers_revision_is_not_stepped_aside_by_a_commit_compile() {
    let (fx, kept) = commit_compile_over("oxy-compile/999.0.0").await;
    let row = revisions::Entity::find_by_id(kept)
        .one(&fx.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.status, "ready");
    assert_eq!(fx.served().await, Some(kept));
}

#[tokio::test]
async fn an_equal_compilers_revision_is_not_stepped_aside_by_a_commit_compile() {
    let (fx, kept) = commit_compile_over(&oxy_compile::compiler_version()).await;
    let row = revisions::Entity::find_by_id(kept)
        .one(&fx.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.status, "ready");
    assert_eq!(fx.served().await, Some(kept));
}

/// A recompile of the served commit by this build could not replace the served
/// revision when its compiler is not older (the unique index keeps the first),
/// and would be asked for again by every request that finds nothing usable. Those
/// keep the working-copy compile, which always writes a fresh revision.
#[tokio::test]
async fn a_served_revision_the_recompile_could_not_replace_keeps_the_working_copy_self_heal() {
    let fx = Fx::new().await;
    fx.serving(SHA, &oxy_compile::compiler_version()).await;
    enqueue_lazy_compile(&fx.db, fx.ws).await;
    assert_eq!(fx.tasks().await, [("compile".to_string(), None)]);
}

#[tokio::test]
async fn a_served_revision_from_a_newer_build_keeps_the_working_copy_self_heal() {
    let fx = Fx::new().await;
    fx.serving(SHA, "oxy-compile/999.0.0").await;
    enqueue_lazy_compile(&fx.db, fx.ws).await;
    assert_eq!(fx.tasks().await, [("compile".to_string(), None)]);
}

/// A build that cannot read the new compiler's output falls back to the last
/// revision whose config loads. The older revision of the same commit is the
/// one that does, and being stepped aside must not take it out of that walk;
/// a revision that merely lost a race (also `superseded`) must stay out of it.
#[tokio::test]
async fn a_stepped_aside_revision_is_still_the_last_known_good() {
    let fx = Fx::new().await;
    // A ready revision of the commit, then stepped aside the way the writer
    // does it, by a revision whose config this build cannot read.
    let old = fx.serving(SHA, "oxy-compile/0.0.1").await;
    fx.db
        .execute_unprepared(&format!(
            "UPDATE revisions SET status = 'superseded', \
               error_summary = '{{\"reason\": \"{}\"}}'::jsonb WHERE revision_id = '{old}'",
            oxy_compile::SUPERSEDED_BY_NEWER_COMPILER,
        ))
        .await
        .unwrap();
    let new = fx.serving(SHA, "oxy-compile/999.0.0").await;
    let loser = fx
        .serving("89abcdef0123456789abcdef0123456789abcdef", "x")
        .await;
    fx.db
        .execute_unprepared(&format!(
            "UPDATE revisions SET status = 'superseded', \
               error_summary = '{{\"superseded_by\": \"{new}\"}}'::jsonb WHERE revision_id = '{loser}'; \
             UPDATE workspaces SET current_revision_id = '{new}' WHERE id = '{}'; \
             INSERT INTO workspace_compiled_configs (revision_id, databases, other) \
               VALUES ('{new}', '[]'::jsonb, '{{\"a_key_only_a_newer_build_writes\": 1}}'::jsonb)",
            fx.ws
        ))
        .await
        .unwrap();
    let resolved = oxy_app_core::compiled_reader::resolve_request_revision(fx.ws, None).await;
    assert_eq!(resolved, Some(old), "old={old} loser={loser} new={new}");
}

#[tokio::test]
async fn a_workspace_that_cannot_be_recompiled_from_git_keeps_the_working_copy_self_heal() {
    // Nothing promoted yet.
    let fx = Fx::new().await;
    enqueue_lazy_compile(&fx.db, fx.ws).await;
    assert_eq!(fx.tasks().await, [("compile".to_string(), None)]);
}

#[tokio::test]
async fn a_local_revision_keeps_the_working_copy_self_heal() {
    let fx = Fx::new().await;
    fx.serving(
        "local-6f1c2c1a-0000-4000-8000-000000000000",
        "oxy-compile/0.0.1",
    )
    .await;
    enqueue_lazy_compile(&fx.db, fx.ws).await;
    assert_eq!(fx.tasks().await, [("compile".to_string(), None)]);
}

#[tokio::test]
async fn a_workspace_with_no_github_connection_keeps_the_working_copy_self_heal() {
    let fx = Fx::new().await;
    fx.serving(SHA, "oxy-compile/0.0.1").await;
    fx.db
        .execute_unprepared(&format!(
            "UPDATE workspaces SET git_namespace_id = NULL WHERE id = '{}'",
            fx.ws
        ))
        .await
        .unwrap();
    enqueue_lazy_compile(&fx.db, fx.ws).await;
    assert_eq!(fx.tasks().await, [("compile".to_string(), None)]);
}

/// The periodic check closes the loop for a workspace whose branch has not
/// moved: it serves the head, but an older compiler wrote it. The check queues
/// a commit compile of that head, the worker's compile replaces the revision,
/// and the next check finds nothing to do.
#[tokio::test]
async fn a_compiler_bump_reaches_a_workspace_whose_head_has_not_moved() {
    use oxy_app::server::compile_reconcile::tick;

    let fx = Fx::new().await;
    // SAFETY: single test per process.
    unsafe { std::env::set_var("OXY_COMPILE_RECONCILE", "1") };
    fx.github_has_the_commit().await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/analytics/commits/heads/main"))
        .respond_with(ResponseTemplate::new(200).set_body_string(SHA))
        .mount(&fx.github)
        .await;
    let old = fx.serving(SHA, "oxy-compile/0.0.1").await;

    let make_due = || async {
        fx.db
            .execute_unprepared(
                "UPDATE workspace_compile_checks SET next_check_at = now() - interval '1 second'",
            )
            .await
            .unwrap();
    };
    tick(&fx.db).await; // gives the workspace its check row
    make_due().await;
    let ran = tick(&fx.db).await;
    assert_eq!((ran.checked, ran.enqueued), (1, 1), "{ran:?}");

    let done = drive_worker_queue(&fx.db, fx.ws, || async { fx.served().await != Some(old) }).await;
    assert!(done, "the recompile never replaced the served revision");
    let served = revisions::Entity::find_by_id(fx.served().await.unwrap())
        .one(&fx.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(served.git_sha, SHA);
    assert_eq!(served.compiler_version, oxy_compile::compiler_version());

    make_due().await;
    let again = tick(&fx.db).await;
    assert_eq!((again.checked, again.enqueued), (1, 0), "{again:?}");
}
