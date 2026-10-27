//! Platform notices (`oxy.*` kinds) against a real Oxy database: who each
//! source tells, that each person is told once per subject, that the words are
//! rendered from Oxy's records with no org OLTP store, that platform mutes
//! apply per kind and channel, that billing cannot be muted, and that a
//! failing pipeline tells once per streak, then daily (`run_failed_streak`).
//!
//! Run with:
//! `cargo nextest run -p oxy-app --test platform -E 'test(platform_notices)'`

use chrono::{TimeZone, Utc};
use entity::org_members::OrgRole;
use entity::workspaces::WorkspaceStatus;
use entity::{
    notification_deliveries, notification_sends, notifications, org_members, organizations, users,
    workspaces,
};
use oxy_app::server::api::billing::notify::past_due_notice;
use oxy_app::server::api::notifications::delivery::{Channel, deliver_send};
use oxy_app::server::api::notifications::inbox_view;
use oxy_app::server::api::notifications::platform::kinds::{
    BILLING_PAST_DUE, RUN_FAILED, WORKSPACE_READY,
};
use oxy_app::server::api::notifications::platform::preferences;
use oxy_app::server::api::notifications::platform::run_failed::notify_run_failed;
use oxy_app::server::api::notifications::platform::workspace_ready::notify_workspace_ready;
use oxy_app::server::api::notifications::settings::model::Node;
use oxy_app::server::api::notifications::settings::store;
use sea_orm::ActiveValue::Set;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter,
    Statement,
};
use serde_json::json;
use uuid::Uuid;

use crate::common::{Schema, fresh_db};

mod ask_finished;
mod ask_waiting;
mod run_failed_streak;

struct Org {
    id: Uuid,
    slug: String,
    owner: Uuid,
    second_owner: Uuid,
    admin: Uuid,
    member: Uuid,
}

async fn seed_user(db: &DatabaseConnection) -> Uuid {
    let id = Uuid::new_v4();
    users::ActiveModel {
        id: Set(id),
        email: Set(Some(format!("p-{}@example.com", id.simple()))),
        name: Set(format!("Person {}", &id.simple().to_string()[..6])),
        picture: Set(None),
        email_verified: Set(true),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed user");
    id
}

async fn join(db: &DatabaseConnection, org: Uuid, user: Uuid, role: OrgRole) {
    let now = Utc::now().fixed_offset();
    org_members::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org),
        user_id: Set(user),
        role: Set(role),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .expect("join org");
}

/// An org with two owners, an admin and a member — and no OLTP store, which a
/// platform notice must not need.
async fn seed_org(db: &DatabaseConnection) -> Org {
    let id = Uuid::new_v4();
    let slug = format!("notices-{}", id.simple());
    let now = Utc::now().fixed_offset();
    organizations::ActiveModel {
        id: Set(id),
        name: Set("Acme".into()),
        slug: Set(slug.clone()),
        created_at: Set(now),
        updated_at: Set(now),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed org");
    let org = Org {
        id,
        slug,
        owner: seed_user(db).await,
        second_owner: seed_user(db).await,
        admin: seed_user(db).await,
        member: seed_user(db).await,
    };
    join(db, id, org.owner, OrgRole::Owner).await;
    join(db, id, org.second_owner, OrgRole::Owner).await;
    join(db, id, org.admin, OrgRole::Admin).await;
    join(db, id, org.member, OrgRole::Member).await;
    org
}

async fn seed_workspace(
    db: &DatabaseConnection,
    org: Uuid,
    name: &str,
    status: WorkspaceStatus,
    created_by: Option<Uuid>,
) -> Uuid {
    let id = Uuid::new_v4();
    let now = Utc::now().fixed_offset();
    workspaces::ActiveModel {
        id: Set(id),
        name: Set(name.into()),
        created_at: Set(now),
        updated_at: Set(now),
        org_id: Set(Some(org)),
        status: Set(status),
        created_by: Set(created_by),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed workspace");
    id
}

async fn told(db: &DatabaseConnection, kind: &str, subject_id: &str) -> Vec<notifications::Model> {
    notifications::Entity::find()
        .filter(notifications::Column::Kind.eq(kind))
        .filter(notifications::Column::SubjectId.eq(subject_id))
        .all(db)
        .await
        .expect("query notices")
}

fn people(rows: &[notifications::Model]) -> Vec<Uuid> {
    let mut ids: Vec<Uuid> = rows.iter().map(|r| r.user_id).collect();
    ids.sort();
    ids
}

fn sorted(mut ids: Vec<Uuid>) -> Vec<Uuid> {
    ids.sort();
    ids
}

async fn push_outcome(db: &DatabaseConnection, signal: Uuid) -> (String, Option<String>) {
    let row = notification_deliveries::Entity::find_by_id((signal, "push".to_string()))
        .one(db)
        .await
        .expect("query delivery")
        .expect("a push delivery was recorded");
    (row.status, row.reason)
}

/// Billing tells every owner, once per episode, needs no OLTP store, and is
/// critical: a mute row cannot silence it, and the settings route refuses to
/// write one.
#[tokio::test]
async fn platform_notices_billing_reaches_every_owner_once_and_cannot_be_muted() {
    let (db, _url) = fresh_db(Schema::All).await;
    let org = seed_org(&db).await;
    seed_workspace(&db, org.id, "Default", WorkspaceStatus::Ready, None).await;
    let grace = Utc.with_ymd_and_hms(2026, 10, 21, 9, 0, 0).unwrap();

    let sent = past_due_notice(&db, org.id, grace).await.expect("notice");
    assert_eq!(sent.recipients, 2);
    let again = past_due_notice(&db, org.id, grace).await.expect("replay");
    assert_eq!(
        (again.recipients, again.already_told),
        (0, 2),
        "a replayed webhook tells nobody twice"
    );

    let rows = told(&db, BILLING_PAST_DUE, "2026-10-21T09:00:00Z").await;
    assert_eq!(people(&rows), sorted(vec![org.owner, org.second_owner]));
    let send = notification_sends::Entity::find_by_id(sent.id.unwrap())
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert!(
        send.critical && !send.email,
        "critical, and the owner email is billing's own"
    );
    assert!(
        send.run_id.is_some(),
        "queued under the org's workspace, not delivered inline"
    );

    // The settings route will not write the mute; a row written anyway is
    // ignored at delivery.
    assert_eq!(
        preferences::set(&db, org.owner, BILLING_PAST_DUE, Channel::Push, true).await,
        Err(axum::http::StatusCode::BAD_REQUEST)
    );
    let billing = Node::Kind(BILLING_PAST_DUE.into());
    store::set_channel(&db, org.owner, None, &billing, Channel::Push, Some(false))
        .await
        .expect("seed a mute");
    deliver_send(&db, sent.id.unwrap()).await;
    let owners_signal = rows.iter().find(|r| r.user_id == org.owner).unwrap();
    assert_ne!(
        push_outcome(&db, owners_signal.id).await.1.as_deref(),
        Some("muted"),
        "a critical kind ignores mutes"
    );

    let items = inbox_view::items(&db, vec![owners_signal.clone()])
        .await
        .expect("inbox");
    assert_eq!(items[0].title.as_deref(), Some("Payment past due for Acme"));
    assert!(
        items[0]
            .body
            .as_deref()
            .unwrap()
            .contains("October 21, 2026")
    );
    assert_eq!(
        items[0].href.as_deref(),
        Some(format!("/{}?settings=organization.billing", org.slug).as_str())
    );
    assert!(items[0].app.is_none() && items[0].link.is_none());
}

/// Billing's subject is the grace period's end, which two orgs can share to the
/// second. A person who owns both must hear about each: "already told" is per
/// org, not per subject alone.
#[tokio::test]
async fn platform_notices_billing_tells_an_owner_of_two_orgs_about_each() {
    let (db, _url) = fresh_db(Schema::All).await;
    let first = seed_org(&db).await;
    let second = seed_org(&db).await;
    join(&db, second.id, first.owner, OrgRole::Owner).await;
    seed_workspace(&db, first.id, "Default", WorkspaceStatus::Ready, None).await;
    seed_workspace(&db, second.id, "Default", WorkspaceStatus::Ready, None).await;
    let grace = Utc.with_ymd_and_hms(2026, 10, 21, 9, 0, 0).unwrap();

    past_due_notice(&db, first.id, grace)
        .await
        .expect("first org");
    let sent = past_due_notice(&db, second.id, grace)
        .await
        .expect("second org");
    assert_eq!(
        (sent.recipients, sent.already_told),
        (3, 0),
        "the shared owner is told about the second org too"
    );

    let shared_owner_orgs: Vec<Uuid> = told(&db, BILLING_PAST_DUE, "2026-10-21T09:00:00Z")
        .await
        .into_iter()
        .filter(|r| r.user_id == first.owner)
        .map(|r| r.org_id)
        .collect();
    assert_eq!(sorted(shared_owner_orgs), sorted(vec![first.id, second.id]));
}

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
