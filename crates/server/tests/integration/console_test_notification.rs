//! The console's "Test notifications" against a real Oxy database and a real
//! org OLTP store: one notification of an app's kind, sent as the app to the
//! caller alone, through `ctx.notify`'s send path.
//!
//! What only a live schema can show: the signal is the caller's and nobody
//! else's, the caller's mute of that kind holds while a critical kind ignores
//! it, a second click inside ten seconds sends nothing (also when two land at
//! once, which the idempotency index decides), and someone the app cannot
//! reach, or a kind it does not declare, writes nothing.
//!
//! Run with:
//! `cargo nextest run -p oxy-server --test integration -E 'test(console_test_notification)'`

use chrono::{Duration, TimeZone, Utc};
use entity::{
    app_builds, apps, notification_deliveries, notification_sends, notifications, org_members,
    users, workspaces,
};
use oxy_api_custom_apps::admin::apps::test_notification::{TestOutcome, send_test, window};
use oxy_api_custom_apps::custom_apps_environments::{EnvAction, record_move};
use oxy_api_notifications::delivery::{Channel, deliver_send};
use oxy_api_notifications::settings::model::Node;
use oxy_api_notifications::settings::store;
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
};
use serde_json::json;
use uuid::Uuid;

use crate::common::Schema;

/// The catalogue port `oxy-server` layers onto the console's routes.
fn live_kinds() -> &'static dyn oxy_app_core::notifications::catalog::live_kinds::LiveKinds {
    static LIVE: std::sync::OnceLock<
        std::sync::Arc<dyn oxy_app_core::notifications::catalog::live_kinds::LiveKinds>,
    > = std::sync::OnceLock::new();
    LIVE.get_or_init(oxy_api_notifications::live_kinds).as_ref()
}
use crate::oltp_fx::with_fx_on;

struct World {
    app: apps::Model,
    member: Uuid,
    teammate: Uuid,
    stranger: Uuid,
}

async fn seed_user(db: &DatabaseConnection) -> Uuid {
    let id = Uuid::new_v4();
    users::ActiveModel {
        id: Set(id),
        email: Set(Some(format!("p-{id}@example.com"))),
        name: Set("Person".into()),
        picture: Set(None),
        email_verified: Set(true),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed user");
    id
}

async fn join(db: &DatabaseConnection, org: Uuid, user: Uuid) {
    let now = Utc::now().fixed_offset();
    org_members::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org),
        user_id: Set(user),
        role: Set(org_members::OrgRole::Member),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .expect("join org");
}

/// An app whose live build declares a time-sensitive kind and a critical one;
/// two members of its org, and a person outside it.
async fn seed_world(db: &DatabaseConnection, org: Uuid) -> World {
    let workspace = Uuid::new_v4();
    workspaces::ActiveModel {
        id: Set(workspace),
        org_id: Set(Some(org)),
        name: Set("ops".into()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed workspace");
    let app = apps::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org),
        project_id: Set(workspace),
        slug: Set("store-ops".into()),
        name: Set("Store Ops".into()),
        branch: Set("main".into()),
        source_repo: Set("git@example.com:acme/app.git".into()),
        status: Set("active".into()),
        source_type: Set("s3".into()),
        source_config: Set(json!({})),
        visibility: Set("org".into()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed app");
    publish(db, &app).await;
    let (member, teammate, stranger) = (
        seed_user(db).await,
        seed_user(db).await,
        seed_user(db).await,
    );
    join(db, org, member).await;
    join(db, org, teammate).await;
    World {
        app,
        member,
        teammate,
        stranger,
    }
}

async fn publish(db: &DatabaseConnection, app: &apps::Model) {
    let build = Uuid::new_v4();
    app_builds::ActiveModel {
        id: Set(build),
        app_id: Set(app.id),
        build_id: Set(format!("b-{}", build.simple())),
        s3_prefix: Set(format!("builds/{build}/")),
        manifest_json: Set(Some(json!({
            "functions": { "lists": { "notify": { "kinds": { "list_overdue": {}, "temp_alarm": {} } } } },
            "notifications": { "kinds": {
                "list_overdue": { "label": "Overdue lists", "level": "time_sensitive" },
                "temp_alarm": { "label": "Temperature alarm", "level": "critical" }
            } }
        }))),
        created_at: Set(Utc::now().fixed_offset()),
        validation_status: Set("passed".into()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed build");
    record_move(
        db,
        app.id,
        &AppEnvironment::Production,
        Some(build),
        EnvAction::Publish,
        None,
    )
    .await
    .expect("move the pointer");
}

async fn signals_of_app(db: &DatabaseConnection, app: Uuid) -> Vec<notifications::Model> {
    notifications::Entity::find()
        .filter(notifications::Column::AppId.eq(app))
        .all(db)
        .await
        .expect("query signals")
}

async fn push_reason(db: &DatabaseConnection, send_id: Uuid) -> Option<String> {
    let signal = notifications::Entity::find()
        .filter(notifications::Column::MessageId.eq(send_id))
        .one(db)
        .await
        .expect("query signal")
        .expect("the send has a signal");
    notification_deliveries::Entity::find_by_id((signal.id, "push".to_string()))
        .one(db)
        .await
        .expect("query delivery")
        .expect("a push delivery was recorded")
        .reason
}

fn sent(outcome: TestOutcome) -> Uuid {
    match outcome {
        TestOutcome::Sent { id } => id,
        other => panic!("expected a send, got {other:?}"),
    }
}

/// The caller gets one signal as the app, with the kind and the test's words;
/// their teammate in the same org gets nothing.
#[tokio::test]
async fn console_test_notification_reaches_the_caller_alone_as_the_app() {
    with_fx_on(Schema::All, |fx| async move {
        fx.provisioner
            .provision(fx.org_id)
            .await
            .expect("provision");
        let w = seed_world(&fx.db, fx.org_id).await;

        let id = sent(
            send_test(
                &fx.db,
                live_kinds(),
                w.member,
                &w.app,
                "list_overdue",
                Utc::now(),
            )
            .await
            .expect("send"),
        );

        let rows = signals_of_app(&fx.db, w.app.id).await;
        assert_eq!(rows.len(), 1, "{rows:?}");
        let row = &rows[0];
        assert_eq!((row.user_id, row.message_id), (w.member, Some(id)));
        assert_eq!(row.kind, "list_overdue");
        assert!(rows.iter().all(|r| r.user_id != w.teammate));
    })
    .await;
}

/// A mute of the kind holds: the push is skipped as muted. A critical kind is
/// not muted (no transport in a test process: recorded as not configured).
#[tokio::test]
async fn console_test_notification_honours_the_callers_mute_but_not_for_a_critical_kind() {
    with_fx_on(Schema::All, |fx| async move {
        fx.provisioner
            .provision(fx.org_id)
            .await
            .expect("provision");
        let w = seed_world(&fx.db, fx.org_id).await;
        for kind in ["list_overdue", "temp_alarm"] {
            store::set_channel(
                &fx.db,
                w.member,
                Some(w.app.id),
                &Node::Kind(kind.into()),
                Channel::Push,
                Some(false),
            )
            .await
            .expect("mute");
        }

        let now = Utc::now();
        let muted = sent(
            send_test(&fx.db, live_kinds(), w.member, &w.app, "list_overdue", now)
                .await
                .expect("send"),
        );
        deliver_send(&fx.db, muted).await;
        assert_eq!(push_reason(&fx.db, muted).await.as_deref(), Some("muted"));

        let later = now + Duration::seconds(11);
        let critical = sent(
            send_test(&fx.db, live_kinds(), w.member, &w.app, "temp_alarm", later)
                .await
                .expect("send"),
        );
        deliver_send(&fx.db, critical).await;
        assert_eq!(
            push_reason(&fx.db, critical).await.as_deref(),
            Some("not_configured")
        );
    })
    .await;
}

/// One test per person per app every ten seconds, whatever the kind; two
/// clicks that land together are one send.
#[tokio::test]
async fn console_test_notification_sends_once_per_ten_seconds() {
    with_fx_on(Schema::All, |fx| async move {
        fx.provisioner
            .provision(fx.org_id)
            .await
            .expect("provision");
        let w = seed_world(&fx.db, fx.org_id).await;

        let (a, b) = tokio::join!(
            send_test(
                &fx.db,
                live_kinds(),
                w.member,
                &w.app,
                "list_overdue",
                Utc::now()
            ),
            send_test(
                &fx.db,
                live_kinds(),
                w.member,
                &w.app,
                "list_overdue",
                Utc::now()
            ),
        );
        let outcomes = [a.expect("first"), b.expect("second")];
        let sends = outcomes
            .iter()
            .filter(|o| matches!(o, TestOutcome::Sent { .. }))
            .count();
        assert_eq!(sends, 1, "{outcomes:?}");
        assert_eq!(signals_of_app(&fx.db, w.app.id).await.len(), 1);

        let again = send_test(
            &fx.db,
            live_kinds(),
            w.member,
            &w.app,
            "temp_alarm",
            Utc::now(),
        )
        .await
        .expect("again");
        assert!(
            matches!(
                again,
                TestOutcome::RateLimited {
                    retry_after_secs: 1..=10
                }
            ),
            "{again:?}"
        );

        // The teammate has their own allowance.
        sent(
            send_test(
                &fx.db,
                live_kinds(),
                w.teammate,
                &w.app,
                "list_overdue",
                Utc::now(),
            )
            .await
            .expect("teammate"),
        );

        let later = Utc::now() + Duration::seconds(11);
        sent(
            send_test(
                &fx.db,
                live_kinds(),
                w.member,
                &w.app,
                "list_overdue",
                later,
            )
            .await
            .expect("later"),
        );
        assert_eq!(signals_of_app(&fx.db, w.app.id).await.len(), 3);
    })
    .await;
}

/// A click just past a ten-second window's edge, under ten seconds after the
/// last test, still waits: the windows key the sends, but the wait is counted
/// from the send itself.
#[tokio::test]
async fn console_test_notification_waits_out_ten_seconds_across_a_window_edge() {
    with_fx_on(Schema::All, |fx| async move {
        fx.provisioner
            .provision(fx.org_id)
            .await
            .expect("provision");
        let w = seed_world(&fx.db, fx.org_id).await;
        let first = sent(
            send_test(
                &fx.db,
                live_kinds(),
                w.member,
                &w.app,
                "list_overdue",
                Utc::now(),
            )
            .await
            .expect("send"),
        );
        let at = notification_sends::Entity::find_by_id(first)
            .one(&fx.db)
            .await
            .expect("query send")
            .expect("the send is recorded")
            .created_at
            .with_timezone(&Utc);

        // The next window's start, or a second after the send if that is later:
        // in the next window either way, and under ten seconds after it.
        let next_window = Utc
            .timestamp_opt((window(at) + 1) * 10, 0)
            .single()
            .expect("valid timestamp");
        let edge = next_window.max(at + Duration::seconds(1));
        assert_eq!(window(edge), window(at) + 1);

        let outcome = send_test(&fx.db, live_kinds(), w.member, &w.app, "list_overdue", edge)
            .await
            .expect("edge");
        assert!(
            matches!(outcome, TestOutcome::RateLimited { .. }),
            "{outcome:?}"
        );
        assert_eq!(signals_of_app(&fx.db, w.app.id).await.len(), 1);
    })
    .await;
}

/// Someone outside the app's org, and a kind its live build does not declare,
/// are refused before anything is written.
#[tokio::test]
async fn console_test_notification_refuses_outsiders_and_undeclared_kinds() {
    with_fx_on(Schema::All, |fx| async move {
        fx.provisioner
            .provision(fx.org_id)
            .await
            .expect("provision");
        let w = seed_world(&fx.db, fx.org_id).await;

        let outsider = send_test(
            &fx.db,
            live_kinds(),
            w.stranger,
            &w.app,
            "list_overdue",
            Utc::now(),
        )
        .await
        .expect("outsider");
        assert_eq!(outsider, TestOutcome::NotInAudience);
        let undeclared = send_test(
            &fx.db,
            live_kinds(),
            w.member,
            &w.app,
            "made_up",
            Utc::now(),
        )
        .await
        .expect("undeclared");
        assert_eq!(undeclared, TestOutcome::UnknownKind);
        assert!(signals_of_app(&fx.db, w.app.id).await.is_empty());
    })
    .await;
}
