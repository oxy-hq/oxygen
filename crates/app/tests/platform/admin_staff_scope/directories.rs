//! The directories and rollups: `/admin/orgs-meta`, `/admin/workspaces-meta`,
//! `/admin/metrics/llm-usage`, `/admin/workspace-health`, and the one org-owned
//! write on the users router. None was in the review; the audit of the rest of the
//! console found them.

use axum::extract::{OriginalUri, Path, Query};
use axum::http::{StatusCode, Uri};
use chrono::{Duration, Utc};
use entity::org_invitations::{self, InviteStatus};
use entity::org_members::OrgRole;
use entity::workspace_health_state;
use oxy_app::server::api::admin::metrics::{UsageQuery, llm_usage};
use oxy_app::server::api::admin::orgs_admin::{ListMetaQuery, list_orgs_meta};
use oxy_app::server::api::admin::scope::list_scope;
use oxy_app::server::api::admin::users_admin::revoke_user_invitation;
use oxy_app::server::api::admin::workspaces_admin::{ListWorkspacesQuery, list_workspaces};
use oxy_app::server::api::admin::{
    TriggerEvalParams, list_workspace_health, trigger_workspace_health_eval,
};
use oxy_auth::types::AuthenticatedUser;
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection, EntityTrait};
use serde_json::json;
use uuid::Uuid;

use super::fixture::{Reply, World, as_actor, grant, reply, seed_user, world};

fn uri(path: &'static str) -> OriginalUri {
    OriginalUri(Uri::from_static(path))
}

/// The fact every listing is narrowed by, read through the loader and the model.
#[tokio::test]
async fn list_scope_is_the_grants_orgs_and_nothing_for_no_standing() {
    let w = world().await;
    assert_eq!(list_scope(&w.db, &w.bounded).await, Ok(Some(vec![w.org_a])));
    assert_eq!(list_scope(&w.db, &w.unbounded).await, Ok(None));
    assert_eq!(list_scope(&w.db, &w.owner).await, Ok(None));

    // A grant bounded to nothing reaches nothing — never "unbounded by omission".
    let empty = seed_user(&w.db, "empty@staff-scope.test").await;
    grant(&w.db, "empty@staff-scope.test", Some(&[])).await;
    assert_eq!(list_scope(&w.db, &empty).await, Ok(Some(Vec::new())));

    // No standing at all: unreachable behind `platform_cap_guard`, and if that ever
    // changes the listing comes back empty rather than whole.
    let stranger = seed_user(&w.db, "stranger@tenant.test").await;
    assert_eq!(list_scope(&w.db, &stranger).await, Ok(Some(Vec::new())));
}

async fn orgs_as(actor: &AuthenticatedUser, page_size: Option<u64>) -> Reply {
    reply(
        list_orgs_meta(
            as_actor(actor),
            uri("/api/admin/orgs-meta"),
            Query(ListMetaQuery {
                search: None,
                page: None,
                page_size,
            }),
        )
        .await,
    )
    .await
}

async fn workspaces_as(actor: &AuthenticatedUser, org_id: Option<Uuid>) -> Reply {
    reply(
        list_workspaces(
            as_actor(actor),
            uri("/api/admin/workspaces-meta"),
            Query(ListWorkspacesQuery {
                search: None,
                status: None,
                org_id,
                page: None,
                page_size: None,
            }),
        )
        .await,
    )
    .await
}

/// The directory that undid the 404 policy: every by-id route on these routers
/// already refused an out-of-scope org, and the listing named all of them.
#[tokio::test]
async fn a_bounded_grant_lists_only_its_own_orgs_and_their_workspaces() {
    let w = world().await;

    let orgs = orgs_as(&w.bounded, None).await;
    assert_eq!(orgs.status, StatusCode::OK);
    assert_eq!(orgs.column(None, "id"), vec![w.org_a.to_string()]);
    // Org A sorts first by name, so a page of one shows it either way — what the
    // scope must change is whether the response claims a second page.
    let first = orgs_as(&w.bounded, Some(1)).await;
    assert_eq!(first.column(None, "id"), vec![w.org_a.to_string()]);
    assert!(
        !first.has_next(),
        "`rel=\"next\"` tells a bounded grant another org exists"
    );

    let workspaces = workspaces_as(&w.bounded, None).await;
    assert_eq!(workspaces.status, StatusCode::OK);
    assert_eq!(
        workspaces.column(None, "id"),
        vec![w.ws_a.to_string()],
        "org B's workspace, or the org-less one, is listed"
    );
    assert!(
        workspaces_as(&w.bounded, Some(w.org_b))
            .await
            .column(None, "id")
            .is_empty(),
        "`?org_id=<org B>` listed org B's workspaces"
    );

    for (who, actor) in w.everything_readers() {
        let orgs = orgs_as(actor, None).await.column(None, "id");
        assert!(orgs.contains(&w.org_a.to_string()) && orgs.contains(&w.org_b.to_string()));
        let workspaces = workspaces_as(actor, None).await.column(None, "id");
        for ws in [w.ws_a, w.ws_b, w.ws_orphan] {
            assert!(workspaces.contains(&ws.to_string()), "{who} lost {ws}");
        }
    }
}

/// One run with one LLM call, in `workspace`.
async fn llm_run(db: &DatabaseConnection, workspace: Uuid) {
    let id = format!("run-{}", Uuid::new_v4());
    agentic_runtime::crud::runs::insert_run(db, &id, "q", None, "analytics", None, workspace)
        .await
        .expect("seed run");
    let events = [
        ("llm_start", json!({ "prompt_tokens": 100 })),
        (
            "llm_end",
            json!({ "model": "staff-scope-model", "output_tokens": 10 }),
        ),
    ];
    for (seq, (kind, payload)) in events.iter().enumerate() {
        agentic_runtime::crud::events::insert_event(db, &id, seq as i64, kind, payload, 0)
            .await
            .expect("seed run event");
    }
}

/// Token totals and the per-org leaderboard are every tenant's spend.
#[tokio::test]
async fn a_bounded_grants_llm_usage_counts_only_its_own_orgs_runs() {
    let w = world().await;
    llm_run(&w.db, w.ws_a).await;
    llm_run(&w.db, w.ws_b).await;
    llm_run(&w.db, w.ws_b).await;
    llm_run(&w.db, w.ws_orphan).await;
    let query = || Query(UsageQuery::default());

    let usage = reply(llm_usage(as_actor(&w.bounded), query()).await).await;
    assert_eq!(usage.status, StatusCode::OK);
    assert_eq!(
        usage.body["total"]["run_count"], 1,
        "{}",
        usage.body["total"]
    );
    assert_eq!(usage.body["total"]["input_tokens"], 100);
    assert_eq!(
        usage.column(Some("by_org"), "org_id"),
        vec![w.org_a.to_string()],
        "org B's LLM spend is on a bounded grant's leaderboard"
    );

    for (who, actor) in w.everything_readers() {
        let usage = reply(llm_usage(as_actor(actor), query()).await).await;
        assert_eq!(usage.body["total"]["run_count"], 4, "{who}");
        assert_eq!(usage.column(Some("by_org"), "org_id").len(), 2, "{who}");
    }
}

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

async fn invitation(w: &World, org: Uuid, invitee: &AuthenticatedUser) -> Uuid {
    let id = Uuid::new_v4();
    org_invitations::ActiveModel {
        id: ActiveValue::Set(id),
        org_id: ActiveValue::Set(org),
        email: ActiveValue::Set(invitee.email.clone().unwrap_or_default()),
        role: ActiveValue::Set(OrgRole::Member),
        invited_by: ActiveValue::Set(w.unbounded.id),
        token: ActiveValue::Set(format!("tok-{id}")),
        status: ActiveValue::Set(InviteStatus::Pending),
        expires_at: ActiveValue::Set((Utc::now() + Duration::days(7)).fixed_offset()),
        created_at: ActiveValue::NotSet,
    }
    .insert(&w.db)
    .await
    .expect("seed invitation");
    id
}

/// An invitation belongs to the org that issued it; the route names only the
/// invitee, so this was a cross-tenant write by id.
#[tokio::test]
async fn a_bounded_grant_cannot_revoke_another_orgs_invitation() {
    let w = world().await;
    let invitee = seed_user(&w.db, "invitee@tenant.test").await;
    let theirs = invitation(&w, w.org_b, &invitee).await;
    let mine = invitation(&w, w.org_a, &invitee).await;

    let refused = revoke_user_invitation(as_actor(&w.bounded), Path((invitee.id, theirs))).await;
    assert_eq!(refused, Err(StatusCode::NOT_FOUND));
    assert!(
        org_invitations::Entity::find_by_id(theirs)
            .one(&w.db)
            .await
            .expect("read invitation")
            .is_some(),
        "org B's invitation was revoked by a grant bounded to org A"
    );

    let revoked = revoke_user_invitation(as_actor(&w.bounded), Path((invitee.id, mine))).await;
    assert_eq!(revoked, Ok(StatusCode::NO_CONTENT));
    let revoked = revoke_user_invitation(as_actor(&w.owner), Path((invitee.id, theirs))).await;
    assert_eq!(revoked, Ok(StatusCode::NO_CONTENT));
}
