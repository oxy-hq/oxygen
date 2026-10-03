//! `/admin/compiles/*` — the compile boundary's operator surface. A revision belongs
//! to a workspace and a workspace to an org, and two of these routes WRITE: they
//! enqueue a compile, or repoint which revision a tenant's workspace serves.

use axum::Json;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use chrono::Utc;
use entity::{revisions, workspaces};
use oxy_app::server::api::admin::compiles::backfill::backfill_uncompiled;
use oxy_app::server::api::admin::compiles::batch::{
    BatchPromoteRequest, BatchRunRequest, batch_promote, batch_run_compile,
};
use oxy_app::server::api::admin::compiles::crud::{
    ListQuery, RunCompileRequest, get_compile, list_compiles, promote_to_revision, run_compile_now,
};
use oxy_app::server::api::admin::compiles::workspaces::{WorkspacesQuery, list_workspaces};
use sea_orm::{
    ActiveModelTrait, ActiveValue, DatabaseBackend, DatabaseConnection, EntityTrait,
    FromQueryResult, Statement,
};
use uuid::Uuid;

use super::fixture::{World, as_actor, reply, world};

/// A `ready` `main` revision of `workspace` — promotable.
async fn revision(db: &DatabaseConnection, workspace: Uuid) -> Uuid {
    let id = Uuid::new_v4();
    let now = Utc::now().fixed_offset();
    revisions::ActiveModel {
        revision_id: ActiveValue::Set(id),
        workspace_id: ActiveValue::Set(workspace),
        git_sha: ActiveValue::Set("0123abc".into()),
        branch: ActiveValue::Set(Some("main".into())),
        schema_version: ActiveValue::Set(1),
        status: ActiveValue::Set("ready".into()),
        kind: ActiveValue::Set("main".into()),
        owner_user_id: ActiveValue::Set(None),
        compiler_version: ActiveValue::Set("test".into()),
        started_at: ActiveValue::Set(now),
        finished_at: ActiveValue::Set(Some(now)),
        file_count_seen: ActiveValue::Set(0),
        file_count_compiled: ActiveValue::Set(0),
        file_count_failed: ActiveValue::Set(0),
        error_summary: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("seed revision");
    id
}

struct Revisions {
    a: Uuid,
    b: Uuid,
    orphan: Uuid,
}

async fn seed_revisions(w: &World) -> Revisions {
    Revisions {
        a: revision(&w.db, w.ws_a).await,
        b: revision(&w.db, w.ws_b).await,
        orphan: revision(&w.db, w.ws_orphan).await,
    }
}

async fn current_revision(db: &DatabaseConnection, workspace: Uuid) -> Option<Uuid> {
    workspaces::Entity::find_by_id(workspace)
        .one(db)
        .await
        .expect("read workspace")
        .expect("workspace exists")
        .current_revision_id
}

#[derive(FromQueryResult)]
struct Count {
    n: i64,
}

/// Compile runs enqueued for `workspace` — what a `run` or a backfill leaves behind.
async fn compiles_enqueued(db: &DatabaseConnection, workspace: Uuid) -> i64 {
    Count::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT COUNT(*) AS n FROM agentic_runs \
         WHERE workspace_id = $1 AND source_type = 'compile'",
        [workspace.into()],
    ))
    .one(db)
    .await
    .expect("count compile runs")
    .map_or(0, |c| c.n)
}

#[tokio::test]
async fn a_bounded_grant_lists_and_reads_only_its_own_orgs_revisions() {
    let w = world().await;
    let rev = seed_revisions(&w).await;

    let listed =
        reply(list_compiles(as_actor(&w.bounded), Query(ListQuery::default())).await).await;
    assert_eq!(listed.status, StatusCode::OK);
    assert_eq!(
        listed.column(Some("rows"), "revision_id"),
        vec![rev.a.to_string()],
        "another tenant's compile history is listed"
    );
    for query in [
        ListQuery {
            org_id: Some(w.org_b),
            ..ListQuery::default()
        },
        ListQuery {
            workspace_id: Some(w.ws_b),
            ..ListQuery::default()
        },
    ] {
        let asked = reply(list_compiles(as_actor(&w.bounded), Query(query)).await).await;
        assert!(
            asked.column(Some("rows"), "revision_id").is_empty(),
            "a filter naming org B returned org B's revisions"
        );
    }

    let overview =
        reply(list_workspaces(as_actor(&w.bounded), Query(WorkspacesQuery::default())).await).await;
    assert_eq!(
        overview.column(Some("rows"), "workspace_id"),
        vec![w.ws_a.to_string()]
    );

    // By id: the same 404 a revision that does not exist gets.
    let missing = reply(get_compile(as_actor(&w.bounded), Path(Uuid::new_v4())).await).await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    for id in [rev.b, rev.orphan] {
        let read = reply(get_compile(as_actor(&w.bounded), Path(id)).await).await;
        assert_eq!(read.status, StatusCode::NOT_FOUND, "read revision {id}");
        assert_eq!(read.body["code"], missing.body["code"]);
    }
    let mine = reply(get_compile(as_actor(&w.bounded), Path(rev.a)).await).await;
    assert_eq!(mine.status, StatusCode::OK, "{}", mine.body);
}

/// The two writes. Repointing a workspace changes what a tenant's users are served.
#[tokio::test]
async fn a_bounded_grant_cannot_compile_or_repoint_another_orgs_workspace() {
    let w = world().await;
    let rev = seed_revisions(&w).await;

    for ws in [w.ws_b, w.ws_orphan] {
        let run = reply(
            run_compile_now(
                as_actor(&w.bounded),
                Json(RunCompileRequest {
                    workspace_id: ws,
                    git_sha: None,
                    branch: None,
                    promote: true,
                }),
            )
            .await,
        )
        .await;
        assert_eq!(run.status, StatusCode::NOT_FOUND, "compiled {ws}");
        assert_eq!(compiles_enqueued(&w.db, ws).await, 0);
    }
    for (id, ws) in [(rev.b, w.ws_b), (rev.orphan, w.ws_orphan)] {
        let promoted = reply(promote_to_revision(as_actor(&w.bounded), Path(id)).await).await;
        assert_eq!(promoted.status, StatusCode::NOT_FOUND, "promoted {id}");
        assert_eq!(promoted.body["code"], "revision_not_found");
        assert_eq!(
            current_revision(&w.db, ws).await,
            None,
            "{ws} was repointed"
        );
    }

    // Ids in a body, where no path guard can see them: fenced per item, and an
    // out-of-scope id reads as not found.
    let batch = reply(
        batch_run_compile(
            as_actor(&w.bounded),
            Json(BatchRunRequest {
                workspace_ids: vec![w.ws_a, w.ws_b, w.ws_orphan],
                promote: false,
            }),
        )
        .await,
    )
    .await;
    assert_eq!(batch.body["enqueued"], 1, "{}", batch.body);
    assert_eq!(compiles_enqueued(&w.db, w.ws_a).await, 1);
    assert_eq!(compiles_enqueued(&w.db, w.ws_b).await, 0);
    assert_eq!(
        batch.body["results"][1]["error"],
        format!("workspace {} not found", w.ws_b)
    );

    let batch = reply(
        batch_promote(
            as_actor(&w.bounded),
            Json(BatchPromoteRequest {
                revision_ids: vec![rev.a, rev.b, rev.orphan],
            }),
        )
        .await,
    )
    .await;
    assert_eq!(batch.body["promoted"], 1, "{}", batch.body);
    assert_eq!(current_revision(&w.db, w.ws_a).await, Some(rev.a));
    assert_eq!(current_revision(&w.db, w.ws_b).await, None);
    assert_eq!(current_revision(&w.db, w.ws_orphan).await, None);
}

/// A bounded grant backfills its own orgs' uncompiled workspaces, not the fleet's.
#[tokio::test]
async fn a_bounded_grants_backfill_compiles_only_its_own_orgs_workspaces() {
    let w = world().await;

    let backfill = reply(backfill_uncompiled(as_actor(&w.bounded)).await).await;
    assert_eq!(backfill.status, StatusCode::OK);
    assert_eq!(backfill.body["enqueued"], 1, "{}", backfill.body);
    assert_eq!(compiles_enqueued(&w.db, w.ws_a).await, 1);
    assert_eq!(compiles_enqueued(&w.db, w.ws_b).await, 0);
    assert_eq!(compiles_enqueued(&w.db, w.ws_orphan).await, 0);

    // Unchanged for an all-orgs grant: every uncompiled workspace on the deployment.
    let backfill = reply(backfill_uncompiled(as_actor(&w.unbounded)).await).await;
    assert_eq!(backfill.body["enqueued"], 3, "{}", backfill.body);
    assert_eq!(compiles_enqueued(&w.db, w.ws_b).await, 1);
}

/// Control: unbounded staff list, read and repoint every tenant's revisions.
#[tokio::test]
async fn unbounded_staff_operate_every_tenants_compiles() {
    let w = world().await;
    let rev = seed_revisions(&w).await;

    for (who, actor) in w.everything_readers() {
        let listed = reply(list_compiles(as_actor(actor), Query(ListQuery::default())).await).await;
        let ids = listed.column(Some("rows"), "revision_id");
        for id in [rev.a, rev.b, rev.orphan] {
            assert!(ids.contains(&id.to_string()), "{who} lost revision {id}");
        }
        let overview =
            reply(list_workspaces(as_actor(actor), Query(WorkspacesQuery::default())).await).await;
        assert_eq!(
            overview.column(Some("rows"), "workspace_id").len(),
            3,
            "{who}"
        );
        let read = reply(get_compile(as_actor(actor), Path(rev.b)).await).await;
        assert_eq!(read.status, StatusCode::OK, "{who}: {}", read.body);
    }

    let promoted = reply(promote_to_revision(as_actor(&w.unbounded), Path(rev.b)).await).await;
    assert_eq!(promoted.status, StatusCode::OK, "{}", promoted.body);
    assert_eq!(current_revision(&w.db, w.ws_b).await, Some(rev.b));
    let promoted = reply(promote_to_revision(as_actor(&w.owner), Path(rev.orphan)).await).await;
    assert_eq!(promoted.status, StatusCode::OK, "{}", promoted.body);
}
