//! Platform notices (`oxy.*` kinds) against a real Oxy database: who each
//! source tells, that each person is told once per subject, that the words are
//! rendered from Oxy's records with no org OLTP store, that platform mutes
//! apply per kind and channel, and that a failing pipeline tells once per
//! streak, then daily (`run_failed_streak`). Billing's notice (every owner,
//! once per episode, and that it cannot be muted) is driven by
//! `oxy-api-billing`, so its cases are `oxy-server`'s
//! (`billing_past_due_notice`, decompose B2); they share [`fixture`].
//!
//! Run with:
//! `cargo nextest run -p oxy-api-notifications --test integration -E 'test(platform_notices)'`

use entity::notification_sends;
use entity::workspaces::WorkspaceStatus;
use oxy_api_notifications::delivery::{Channel, deliver_send};
use oxy_api_notifications::inbox_view;
use oxy_api_notifications::platform::kinds::{RUN_FAILED, WORKSPACE_READY};
use oxy_api_notifications::platform::preferences;
use oxy_api_notifications::platform::run_failed::notify_run_failed;
use oxy_api_notifications::platform::workspace_ready::notify_workspace_ready;
use sea_orm::{ConnectionTrait, DatabaseConnection, EntityTrait, Statement};
use serde_json::json;
use uuid::Uuid;

use crate::common::{Schema, fresh_db};

pub(crate) mod fixture;
use fixture::{people, push_outcome, seed_org, seed_user, seed_workspace, sorted, told};

mod anomaly;
mod ask_finished;
mod ask_waiting;
mod run_failed_streak;

/// The first workspace anyone can open is news for every member; a later one
/// for its creator and the org's owners and admins. A workspace that is not
/// ready tells nobody.
#[tokio::test]
async fn platform_notices_workspace_ready_tells_who_is_waiting() {
    let (db, _url) = fresh_db(Schema::All).await;
    let org = seed_org(&db).await;
    let cloning = seed_workspace(&db, org.id, "Later", WorkspaceStatus::Cloning, None).await;
    assert!(
        notify_workspace_ready(&db, cloning)
            .await
            .unwrap()
            .is_none()
    );

    let first = seed_workspace(
        &db,
        org.id,
        "Sales",
        WorkspaceStatus::Ready,
        Some(org.admin),
    )
    .await;
    notify_workspace_ready(&db, first)
        .await
        .unwrap()
        .expect("sent");
    assert_eq!(
        people(&told(&db, WORKSPACE_READY, &first.to_string()).await),
        sorted(vec![org.owner, org.second_owner, org.admin, org.member]),
        "the first navigable workspace tells everyone on the set-up screen"
    );

    let second = seed_workspace(&db, org.id, "Ops", WorkspaceStatus::Ready, Some(org.member)).await;
    notify_workspace_ready(&db, second)
        .await
        .unwrap()
        .expect("sent");
    let rows = told(&db, WORKSPACE_READY, &second.to_string()).await;
    assert_eq!(
        people(&rows),
        sorted(vec![org.owner, org.second_owner, org.admin, org.member]),
        "its creator, plus owners and admins"
    );
    let stranger = seed_user(&db).await;
    let third = seed_workspace(
        &db,
        org.id,
        "Finance",
        WorkspaceStatus::Ready,
        Some(stranger),
    )
    .await;
    notify_workspace_ready(&db, third)
        .await
        .unwrap()
        .expect("sent");
    assert_eq!(
        people(&told(&db, WORKSPACE_READY, &third.to_string()).await),
        sorted(vec![org.owner, org.second_owner, org.admin]),
        "a creator who is not a member is not told; members are not, on a later one"
    );

    let mine = rows.into_iter().find(|r| r.user_id == org.member).unwrap();
    let items = inbox_view::items(&db, vec![mine]).await.expect("inbox");
    assert_eq!(items[0].title.as_deref(), Some("Ops is ready"));
    assert_eq!(
        items[0].href.as_deref(),
        Some(format!("/{}/workspaces/{second}", org.slug).as_str())
    );
}

/// A platform mute applies to its kind and channel only, and the settings
/// route lists the kind with its mute.
#[tokio::test]
async fn platform_notices_mutes_apply_per_kind_and_channel() {
    let (db, _url) = fresh_db(Schema::All).await;
    let org = seed_org(&db).await;
    seed_workspace(&db, org.id, "Default", WorkspaceStatus::Ready, None).await;
    assert_eq!(
        preferences::set(&db, org.member, "oxy.not_a_kind", Channel::Push, true).await,
        Err(axum::http::StatusCode::BAD_REQUEST),
        "the platform's kinds are a closed list"
    );
    assert_eq!(
        preferences::set(&db, org.member, WORKSPACE_READY, Channel::Slack, true).await,
        Err(axum::http::StatusCode::BAD_REQUEST),
        "a ready workspace never goes to Slack, so there is nothing to mute"
    );
    preferences::set(&db, org.member, WORKSPACE_READY, Channel::Push, true)
        .await
        .expect("mute");

    let ws = seed_workspace(&db, org.id, "Ops", WorkspaceStatus::Ready, Some(org.member)).await;
    let sent = notify_workspace_ready(&db, ws).await.unwrap().unwrap();
    deliver_send(&db, sent.id.unwrap()).await;
    let rows = told(&db, WORKSPACE_READY, &ws.to_string()).await;
    let member = rows.iter().find(|r| r.user_id == org.member).unwrap();
    let owner = rows.iter().find(|r| r.user_id == org.owner).unwrap();
    assert_eq!(
        push_outcome(&db, member.id).await.1.as_deref(),
        Some("muted")
    );
    assert_ne!(
        push_outcome(&db, owner.id).await.1.as_deref(),
        Some("muted")
    );

    let kinds = preferences::kinds(&db, org.member).await.expect("kinds");
    assert_eq!(kinds.len(), 1, "{kinds:?}");
    assert_eq!(kinds[0].kind, WORKSPACE_READY);
    assert_eq!(kinds[0].muted, vec!["push"]);
    assert_eq!(
        kinds[0].channels,
        vec!["push"],
        "settings offer only the channels the kind is delivered on"
    );
    assert!(!kinds[0].critical);
}

async fn seed_run(
    db: &DatabaseConnection,
    workspace: Uuid,
    source: &str,
    metadata: serde_json::Value,
    schedule: Option<&str>,
    cancelled: bool,
) -> String {
    let id = Uuid::new_v4().to_string();
    db.execute_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "INSERT INTO agentic_runs (id, question, source_type, metadata, schedule_id, task_status, \
           error_message, workspace_id, cancel_requested_at, attempt, created_at, updated_at) \
         VALUES ($1, 'q', $2, $3, $4, 'failed', 'step failed', $5, \
           CASE WHEN $6 THEN now() END, 0, now(), now())",
        [
            id.clone().into(),
            source.into(),
            metadata.into(),
            schedule.map(str::to_string).into(),
            workspace.into(),
            cancelled.into(),
        ],
    ))
    .await
    .expect("seed run");
    id
}

/// A failed run tells whoever started it while they are a member; a
/// scheduled one, the workspace's admins. A cancel tells nobody.
#[tokio::test]
async fn platform_notices_run_failed_tells_the_starter_or_the_admins() {
    let (db, _url) = fresh_db(Schema::All).await;
    let org = seed_org(&db).await;
    let ws = seed_workspace(&db, org.id, "Ops", WorkspaceStatus::Ready, None).await;

    let manual = seed_run(
        &db,
        ws,
        "workflow",
        json!({ "workflow_ref": "automations/nightly.automation.yml", "started_by": org.member.to_string() }),
        None,
        false,
    )
    .await;
    let sent = notify_run_failed(&db, &manual)
        .await
        .unwrap()
        .expect("sent");
    assert_eq!(sent.recipients, 1);
    let rows = told(&db, RUN_FAILED, &manual).await;
    assert_eq!(people(&rows), vec![org.member]);
    let send = notification_sends::Entity::find_by_id(sent.id.unwrap())
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert!(send.email, "a failed run is emailed");
    let items = inbox_view::items(&db, rows).await.expect("inbox");
    assert_eq!(items[0].title.as_deref(), Some("nightly failed"));
    let body = items[0].body.as_deref().unwrap();
    assert!(
        body.starts_with("The run that started ") && body.contains(" UTC failed"),
        "two failures of one automation read apart by their start: {body}"
    );
    assert!(
        items[0].href.as_deref().unwrap().ends_with(&format!(
            "/automations/YXV0b21hdGlvbnMvbmlnaHRseS5hdXRvbWF0aW9uLnltbA==?run={manual}"
        )),
        "{:?}",
        items[0].href
    );
    assert_eq!(
        notify_run_failed(&db, &manual)
            .await
            .unwrap()
            .unwrap()
            .recipients,
        0,
        "the same run's failure is told once"
    );

    let scheduled = seed_run(
        &db,
        ws,
        "airway",
        json!({ "pipeline_ref": "toast.airway.yml", "pipeline_name": "Toast" }),
        Some("schedule-1"),
        false,
    )
    .await;
    notify_run_failed(&db, &scheduled)
        .await
        .unwrap()
        .expect("sent");
    assert_eq!(
        people(&told(&db, RUN_FAILED, &scheduled).await),
        sorted(vec![org.owner, org.second_owner, org.admin]),
        "no workspace admins named: the org's owners and admins"
    );

    let cancelled = seed_run(
        &db,
        ws,
        "workflow",
        json!({ "workflow_ref": "a.automation.yml", "started_by": org.member.to_string() }),
        None,
        true,
    )
    .await;
    assert!(notify_run_failed(&db, &cancelled).await.unwrap().is_none());
    assert!(told(&db, RUN_FAILED, &cancelled).await.is_empty());
}
