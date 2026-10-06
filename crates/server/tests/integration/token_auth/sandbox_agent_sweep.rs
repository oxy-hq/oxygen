//! `token.expired_sandboxes_queued` across orgs (sandbox agent credential
//! design §2 "Audit"): a token granted apps of two orgs leaves a sandbox in
//! each, and the event's row on one org's chain holds nothing of the other
//! org's — not its app id, its sandbox's name, or its teardown's run id.

use agentic_runtime::migration::RuntimeMigrator;
use axum::http::StatusCode;
use chrono::{DateTime, Duration, Utc};
use migration::MigratorTrait;
use oxy_app::server::api::custom_apps_sandboxes::maintenance::sweep;
use oxy_app::server::api::custom_apps_sandboxes::token_ended;
use sea_orm::EntityTrait;
use serde_json::json;
use uuid::Uuid;

use super::sandbox_agent::{minted, staff_with_app};
use super::stack::{flat_api, published_app};
use super::{audit_rows, call, pat_row, seed_org, seed_workspace_in};

async fn create_sandbox(secret: &str, app: Uuid, name: &str) {
    let bearer = format!("Bearer {secret}");
    let uri = format!("/customer-apps/{app}/environments");
    let headers = [("authorization", bearer.as_str())];
    let (status, body) = call(
        flat_api(),
        "POST",
        &uri,
        &headers,
        Some(json!({ "name": name })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create {name}: {body}");
}

/// The pass logs a token whose sandboxes it could not queue and carries on,
/// so a wrong count says nothing by itself. Show its warnings with a failure.
fn show_warnings() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_test_writer()
        .try_init();
}

#[tokio::test]
async fn a_token_ended_event_shows_each_org_only_its_own_sandboxes() {
    show_warnings();
    let (fx, here) = staff_with_app().await;
    // A teardown is a run on the task queue, and those tables are the
    // runtime's: this group's database carries the central migrations alone.
    RuntimeMigrator::up(&fx.db, None)
        .await
        .expect("the runtime's migrations");
    let other_org = seed_org(&fx.db).await;
    let other_workspace = seed_workspace_in(&fx.db, other_org).await;
    let there = published_app(&fx.db, other_org, other_workspace).await;
    let (id, secret) = minted(&fx, &[here.id, there.id]).await;
    create_sandbox(&secret, here.id, "dev-alpha").await;
    create_sandbox(&secret, there.id, "dev-bravo").await;

    let bearer = format!("Bearer {secret}");
    let headers = [("authorization", bearer.as_str())];
    let (status, body) = call(flat_api(), "DELETE", "/auth/token", &headers, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let revoked_at = pat_row(&fx.db, id).await.revoked_at.expect("revoked");
    let due = DateTime::<Utc>::from(revoked_at) + token_ended::grace() + Duration::seconds(1);
    let queued = sweep(&fx.db, due).await.expect("sweep");
    let left = entity::app_environments::Entity::find()
        .all(&fx.db)
        .await
        .expect("read the sandboxes");
    assert_eq!(queued, 2, "the sandboxes after the pass: {left:#?}");

    let rows = audit_rows(&fx.db, "token.expired_sandboxes_queued").await;
    assert_eq!(rows.len(), 2, "one row per granted app's org: {rows:?}");
    assert_eq!(
        rows[0].metadata["event_id"], rows[1].metadata["event_id"],
        "one event"
    );
    let row_of = |org: Uuid| {
        rows.iter()
            .find(|row| row.org_id == Some(org))
            .unwrap_or_else(|| panic!("a row on org {org}"))
    };
    let run_of = |org: Uuid| {
        row_of(org).metadata["sandboxes"][0]["run_id"]
            .as_str()
            .expect("a run id")
            .to_string()
    };
    let sides = [
        (fx.org_id, here.id, "dev-alpha"),
        (other_org, there.id, "dev-bravo"),
    ];
    for ((org, app, name), (foreign_org, foreign_app, foreign_name)) in
        [(sides[0], sides[1]), (sides[1], sides[0])]
    {
        let row = row_of(org);
        assert_eq!(row.target_id, Some(id.to_string()));
        let listed = row.metadata["sandboxes"].as_array().expect("sandboxes");
        assert_eq!(listed.len(), 1, "{}", row.metadata);
        assert_eq!(listed[0]["app_id"], json!(app));
        assert_eq!(listed[0]["environment"], name);
        // The row exactly as it is stored and chained.
        let stored = serde_json::to_string(row).expect("serialise the row");
        for leaked in [
            foreign_org.to_string(),
            foreign_app.to_string(),
            foreign_name.to_string(),
            run_of(foreign_org),
        ] {
            assert!(
                !stored.contains(&leaked),
                "the row on org {org} holds {leaked} of org {foreign_org}"
            );
        }
    }
}
