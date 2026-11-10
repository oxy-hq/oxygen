//! The staff console's workspace-health routes under a bounded grant. Moved from
//! `oxy-app`'s `tests/platform/admin_staff_scope/directories.rs` with the
//! handlers; the fixture is that suite's, included by path so the two cannot
//! drift.

use axum::extract::{Path, Query};
use axum::http::StatusCode;
use chrono::Utc;
use entity::workspace_health_state;
use oxy_api_workspace_health::handlers::{
    TriggerEvalParams, list_workspace_health, trigger_workspace_health_eval,
};
use oxy_auth::types::AuthenticatedUser;
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection};
use serde_json::json;
use uuid::Uuid;

#[path = "../../../app/tests/platform/admin_staff_scope/fixture.rs"]
#[allow(dead_code)]
mod fixture;
use fixture::{Reply, as_actor, reply, world};

async fn health_row(db: &DatabaseConnection, workspace: Uuid) {
    let now = Utc::now().fixed_offset();
    workspace_health_state::ActiveModel {
        workspace_id: ActiveValue::Set(workspace),
        status: ActiveValue::Set("unhealthy".into()),
        reasons: ActiveValue::Set(json!(["probe"])),
        changed_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
        payload: ActiveValue::Set(None),
        last_smoke_at: ActiveValue::Set(None),
        last_alerted_at: ActiveValue::Set(None),
        alerted_failures: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("seed health row");
}

async fn eval_as(actor: &AuthenticatedUser, workspace: Uuid) -> Reply {
    reply(
        trigger_workspace_health_eval(
            as_actor(actor),
            Path(workspace),
            Query(TriggerEvalParams::default()),
        )
        .await,
    )
    .await
}

#[tokio::test]
async fn a_bounded_grant_reads_and_evaluates_only_its_own_orgs_workspace_health() {
    let w = world().await;
    for ws in [w.ws_a, w.ws_b, w.ws_orphan] {
        health_row(&w.db, ws).await;
    }

    let rollup = reply(list_workspace_health(as_actor(&w.bounded)).await).await;
    assert_eq!(rollup.status, StatusCode::OK);
    assert_eq!(
        rollup.column(Some("workspaces"), "workspace_id"),
        vec![w.ws_a.to_string()],
        "another tenant's health rollup is listed"
    );

    // Enqueuing an eval can bill a tenant's warehouse and agent tokens. Out of
    // scope answers what a workspace that does not exist answers.
    let missing = eval_as(&w.bounded, Uuid::new_v4()).await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    for ws in [w.ws_b, w.ws_orphan] {
        let refused = eval_as(&w.bounded, ws).await;
        assert_eq!(refused.status, StatusCode::NOT_FOUND, "evaluated {ws}");
        assert_eq!(refused.body, missing.body);
    }
    assert_eq!(
        eval_as(&w.bounded, w.ws_a).await.status,
        StatusCode::ACCEPTED
    );

    for (who, actor) in w.everything_readers() {
        let rollup = reply(list_workspace_health(as_actor(actor)).await).await;
        assert_eq!(
            rollup.column(Some("workspaces"), "workspace_id").len(),
            3,
            "{who}"
        );
        assert_eq!(
            eval_as(actor, w.ws_b).await.status,
            StatusCode::ACCEPTED,
            "{who}"
        );
    }
}

/// A health row outlives a hard-deleted workspace (no FK). It can never be
/// re-evaluated, so listing it shows a frozen verdict as "Unknown workspace".
#[tokio::test]
async fn the_health_rollup_leaves_out_a_deleted_workspaces_last_verdict() {
    let w = world().await;
    let deleted = Uuid::new_v4();
    for ws in [w.ws_a, deleted] {
        health_row(&w.db, ws).await;
    }

    for (who, actor) in w.everything_readers() {
        let rollup = reply(list_workspace_health(as_actor(actor)).await).await;
        assert_eq!(
            rollup.column(Some("workspaces"), "workspace_id"),
            vec![w.ws_a.to_string()],
            "{who} sees a deleted workspace's health row"
        );
    }
}
