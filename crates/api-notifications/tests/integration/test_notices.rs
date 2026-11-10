//! `oxy.test` against a real Oxy database: a test notice reaches the caller
//! alone, renders in the inbox with no app, ignores mutes, and is limited to
//! one per person per 10 seconds — including two sent at once.
//!
//! Run with:
//! `cargo nextest run -p oxy-api-notifications --test integration -E 'test(test_notices)'`

use chrono::{Duration, Utc};
use entity::org_members::OrgRole;
use entity::workspaces::WorkspaceStatus;
use entity::{
    notification_deliveries, notification_sends, notifications, org_frontline_members, org_members,
    organizations, users, workspaces,
};
use oxy_api_notifications::delivery::Channel;
use oxy_api_notifications::inbox_view;
use oxy_api_notifications::platform::kinds::TEST;
use oxy_api_notifications::platform::preferences;
use oxy_api_notifications::platform::self_test::{TestSent, send_test};
use oxy_api_notifications::settings::model::Node;
use oxy_api_notifications::settings::store;
use oxy_api_notifications::test_notice::org_for;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::ActiveValue::Set;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter,
    Statement,
};
use uuid::Uuid;

use crate::common::{Schema, fresh_db};

async fn seed_user(db: &DatabaseConnection) -> users::Model {
    let id = Uuid::new_v4();
    users::ActiveModel {
        id: Set(id),
        email: Set(Some(format!("t-{}@example.com", id.simple()))),
        name: Set(format!("Tester {}", &id.simple().to_string()[..6])),
        picture: Set(None),
        email_verified: Set(true),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed user")
}

async fn seed_org(db: &DatabaseConnection) -> (Uuid, String) {
    let id = Uuid::new_v4();
    let slug = format!("tests-{}", id.simple());
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
    (id, slug)
}

async fn join(db: &DatabaseConnection, org: Uuid, user: Uuid, days_ago: i64) {
    let at = (Utc::now() - Duration::days(days_ago)).fixed_offset();
    org_members::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org),
        user_id: Set(user),
        role: Set(OrgRole::Member),
        created_at: Set(at),
        updated_at: Set(at),
    }
    .insert(db)
    .await
    .expect("join org");
}

async fn seed_workspace(db: &DatabaseConnection, org: Uuid) {
    let now = Utc::now().fixed_offset();
    workspaces::ActiveModel {
        id: Set(Uuid::new_v4()),
        name: Set("Default".into()),
        created_at: Set(now),
        updated_at: Set(now),
        org_id: Set(Some(org)),
        status: Set(WorkspaceStatus::Ready),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed workspace");
}

async fn signal(db: &DatabaseConnection, id: Uuid) -> notifications::Model {
    notifications::Entity::find_by_id(id)
        .one(db)
        .await
        .expect("query signal")
        .expect("the test notice's inbox row")
}

async fn tests_for(db: &DatabaseConnection, user: Uuid) -> usize {
    notifications::Entity::find()
        .filter(notifications::Column::UserId.eq(user))
        .filter(notifications::Column::Kind.eq(TEST))
        .all(db)
        .await
        .expect("query tests")
        .len()
}

fn sent_id(outcome: TestSent) -> Uuid {
    match outcome {
        TestSent::Sent { id } => id,
        other => panic!("expected a send, got {other:?}"),
    }
}

/// The caller and nobody else, push only, queued under the org's workspace;
/// a second within 10 seconds is refused, someone else's is not, and once the
/// window passes the caller may send again.
#[tokio::test]
async fn test_notices_reach_only_the_caller_once_per_ten_seconds() {
    let (db, _url) = fresh_db(Schema::All).await;
    let (org, _) = seed_org(&db).await;
    let caller = seed_user(&db).await;
    let colleague = seed_user(&db).await;
    join(&db, org, caller.id, 1).await;
    join(&db, org, colleague.id, 1).await;
    seed_workspace(&db, org).await;

    let id = sent_id(send_test(&db, caller.id, org, None).await.unwrap());
    let row = signal(&db, id).await;
    assert_eq!((row.user_id, row.org_id), (caller.id, org));
    assert_eq!(row.kind, TEST);
    assert!(row.app_id.is_none() && row.body.is_none() && row.title.is_empty());
    assert_eq!(tests_for(&db, colleague.id).await, 0, "the caller only");
    let send = notification_sends::Entity::find_by_id(row.message_id.unwrap())
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert!(!send.email && !send.slack && !send.critical, "push only");
    assert!(send.run_id.is_some(), "the normal path: a queued delivery");

    match send_test(&db, caller.id, org, None).await.unwrap() {
        TestSent::RateLimited { retry_after_secs } => {
            assert!((1..=10).contains(&retry_after_secs), "{retry_after_secs}")
        }
        other => panic!("a second test within 10 seconds must be refused: {other:?}"),
    }
    sent_id(send_test(&db, colleague.id, org, None).await.unwrap());

    db.execute_raw(Statement::from_sql_and_values(
        db.get_database_backend(),
        "UPDATE notifications SET created_at = created_at - interval '11 seconds' WHERE user_id = $1",
        [caller.id.into()],
    ))
    .await
    .unwrap();
    sent_id(send_test(&db, caller.id, org, None).await.unwrap());
    assert_eq!(tests_for(&db, caller.id).await, 2);
}

/// Two taps at once — two replicas — send one notice between them.
#[tokio::test]
async fn test_notices_sent_at_once_send_one() {
    let (db, _url) = fresh_db(Schema::All).await;
    let (org, _) = seed_org(&db).await;
    let caller = seed_user(&db).await;
    join(&db, org, caller.id, 1).await;
    seed_workspace(&db, org).await;

    let (a, b) = tokio::join!(
        send_test(&db, caller.id, org, None),
        send_test(&db, caller.id, org, None)
    );
    let outcomes = [a.unwrap(), b.unwrap()];
    let sent = outcomes
        .iter()
        .filter(|o| matches!(o, TestSent::Sent { .. }))
        .count();
    assert_eq!(sent, 1, "{outcomes:?}");
    assert_eq!(tests_for(&db, caller.id).await, 1);
}

/// It reads like any other inbox item: title and body, no app, no app link,
/// a link to the notification settings — and its row stores none of that
/// text, only the moment it was sent.
#[tokio::test]
async fn test_notices_render_in_the_inbox_without_an_app() {
    let (db, _url) = fresh_db(Schema::All).await;
    let (org, slug) = seed_org(&db).await;
    let caller = seed_user(&db).await;
    join(&db, org, caller.id, 1).await;
    seed_workspace(&db, org).await;

    let plain = sent_id(send_test(&db, caller.id, org, None).await.unwrap());
    let items = inbox_view::items(&db, vec![signal(&db, plain).await])
        .await
        .unwrap();
    assert_eq!(items[0].title.as_deref(), Some("Test notification"));
    let body = items[0].body.as_deref().unwrap();
    assert!(body.starts_with("Sent from Oxygen at "), "{body}");
    assert!(body.contains(" UTC on "), "{body}");
    assert_eq!(
        items[0].href.as_deref(),
        Some(format!("/{slug}?settings=account.notifications").as_str())
    );
    assert!(items[0].app.is_none() && items[0].link.is_none());

    let row = signal(&db, plain).await;
    assert!(row.title.is_empty() && row.body.is_none() && row.link.is_none());
    assert_eq!(row.subject_kind.as_deref(), Some("test"));
    let stored: serde_json::Value = serde_json::from_str(row.subject_id.as_deref().unwrap())
        .expect("the subject is the test itself");
    assert_eq!(
        stored
            .as_object()
            .map(|o| o.keys().cloned().collect::<Vec<_>>()),
        Some(vec!["at".to_string()]),
        "a session's test stores the moment and nothing else: {stored}"
    );
}

/// No mute silences a test, and settings offer no switch for one. An org with
/// no workspace still delivers it, inline.
#[tokio::test]
async fn test_notices_ignore_mutes_and_deliver_without_a_workspace() {
    // What `oxy serve` and `oxy worker` install at boot (`seam().install`): the
    // transport the write path in `oxy_app_core` delivers a workspace-less send
    // through. nextest runs each test in its own process.
    oxy_app_core::notifications::inline::install(oxy_api_notifications::delivery::deliver_inline);
    let (db, _url) = fresh_db(Schema::All).await;
    let (org, _) = seed_org(&db).await;
    let caller = seed_user(&db).await;
    join(&db, org, caller.id, 1).await;

    assert_eq!(
        preferences::set(&db, caller.id, TEST, Channel::Push, true).await,
        Err(axum::http::StatusCode::BAD_REQUEST)
    );
    let test = Node::Kind(TEST.into());
    store::set_channel(&db, caller.id, None, &test, Channel::Push, Some(false))
        .await
        .expect("seed a mute");

    let id = sent_id(send_test(&db, caller.id, org, None).await.unwrap());
    let row = signal(&db, id).await;
    let send = notification_sends::Entity::find_by_id(row.message_id.unwrap())
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert!(send.run_id.is_none(), "no workspace to queue under");
    let push = notification_deliveries::Entity::find_by_id((id, "push".to_string()))
        .one(&db)
        .await
        .unwrap()
        .expect("delivered inline after the commit");
    assert_ne!(push.reason.as_deref(), Some("muted"));

    let kinds = preferences::kinds(&db, caller.id).await.unwrap();
    assert!(kinds.iter().all(|k| k.kind != TEST), "{kinds:?}");
}

/// Filed under the caller's oldest org unless they name another of theirs; a
/// frontline worker's under the org they crew for; nobody else's.
#[tokio::test]
async fn test_notices_file_under_the_callers_own_org() {
    let (db, _url) = fresh_db(Schema::All).await;
    let (older, _) = seed_org(&db).await;
    let (newer, _) = seed_org(&db).await;
    let (foreign, _) = seed_org(&db).await;
    let member = seed_user(&db).await;
    join(&db, newer, member.id, 1).await;
    join(&db, older, member.id, 30).await;
    let user = AuthenticatedUser::from(member);
    assert_eq!(org_for(&db, &user, None).await.unwrap(), Some(older));
    assert_eq!(org_for(&db, &user, Some(newer)).await.unwrap(), Some(newer));
    assert_eq!(org_for(&db, &user, Some(foreign)).await.unwrap(), None);

    let crew = seed_user(&db).await;
    org_frontline_members::ActiveModel {
        org_id: Set(foreign),
        user_id: Set(crew.id),
        status: Set(org_frontline_members::STATUS_ACTIVE.into()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("enrol crew");
    let crew = AuthenticatedUser::from(crew);
    assert_eq!(org_for(&db, &crew, None).await.unwrap(), Some(foreign));

    let nobody = AuthenticatedUser::from(seed_user(&db).await);
    assert_eq!(org_for(&db, &nobody, None).await.unwrap(), None);
}
