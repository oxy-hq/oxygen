//! `ctx.notify`'s write and delivery path against a real Oxy database and a
//! real org OLTP store.
//!
//! What only a live schema can show: the content lands in the org's store and
//! not in Oxy's, the idempotency index turns a retry into the first send, a
//! delivery run claims each (recipient, channel) once, a mute is honoured and a
//! critical kind is not muted, and an org with no store is refused before
//! anyone is signalled.
//!
//! Run with:
//! `cargo nextest run -p oxy-api-notifications --test integration -E 'test(notifications_send)'`

use entity::{
    apps, notification_deliveries, notifications, org_members, org_team_members, org_teams, users,
    workspaces,
};
use oxy_api_notifications::delivery::{Channel, deliver_send};
use oxy_api_notifications::inbox_view;
use oxy_api_notifications::input::Target;
use oxy_api_notifications::preferences::may_be_told_by;
use oxy_api_notifications::recipients::resolve;
use oxy_api_notifications::send::{AppRef, Outgoing, SendError, send};
use oxy_api_notifications::settings::model::Node;
use oxy_api_notifications::settings::store;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait,
    QueryFilter,
};
use serde_json::json;
use uuid::Uuid;

use crate::common::Schema;
use crate::oltp_fx::{seed_org, with_fx_on};

struct World {
    workspace: Uuid,
    app: AppRef,
    member: Uuid,
    teammate: Uuid,
    stranger: Uuid,
    team: Uuid,
}

async fn seed_user(db: &DatabaseConnection, email: Option<String>) -> Uuid {
    let id = Uuid::new_v4();
    users::ActiveModel {
        id: ActiveValue::Set(id),
        email: ActiveValue::Set(email),
        name: ActiveValue::Set(format!("Person {}", &id.simple().to_string()[..6])),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(true),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed user");
    id
}

async fn join(db: &DatabaseConnection, org: Uuid, user: Uuid) {
    let now = chrono::Utc::now().fixed_offset();
    org_members::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        org_id: ActiveValue::Set(org),
        user_id: ActiveValue::Set(user),
        role: ActiveValue::Set(org_members::OrgRole::Member),
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
    }
    .insert(db)
    .await
    .expect("join org");
}

async fn seed_world(db: &DatabaseConnection, org: Uuid) -> World {
    let workspace = Uuid::new_v4();
    workspaces::ActiveModel {
        id: ActiveValue::Set(workspace),
        org_id: ActiveValue::Set(Some(org)),
        name: ActiveValue::Set("ops".into()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed workspace");
    let app_id = Uuid::new_v4();
    apps::ActiveModel {
        id: ActiveValue::Set(app_id),
        org_id: ActiveValue::Set(org),
        project_id: ActiveValue::Set(workspace),
        slug: ActiveValue::Set("store-ops".into()),
        name: ActiveValue::Set("Store Ops".into()),
        branch: ActiveValue::Set("main".into()),
        source_repo: ActiveValue::Set("git@example.com:acme/store-ops.git".into()),
        status: ActiveValue::Set("active".into()),
        source_type: ActiveValue::Set("git".into()),
        source_config: ActiveValue::Set(json!({})),
        visibility: ActiveValue::Set("org".into()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed app");

    let member = seed_user(db, Some(format!("m-{}@example.com", Uuid::new_v4()))).await;
    let teammate = seed_user(db, None).await;
    let stranger = seed_user(db, Some(format!("s-{}@example.com", Uuid::new_v4()))).await;
    join(db, org, member).await;
    join(db, org, teammate).await;

    let now = chrono::Utc::now().fixed_offset();
    let team = Uuid::new_v4();
    org_teams::ActiveModel {
        id: ActiveValue::Set(team),
        org_id: ActiveValue::Set(org),
        name: ActiveValue::Set("Openers".into()),
        description: ActiveValue::Set(None),
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
        created_by: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("seed team");
    for user in [teammate, stranger] {
        org_team_members::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            team_id: ActiveValue::Set(team),
            user_id: ActiveValue::Set(user),
            created_at: ActiveValue::Set(now),
            created_by: ActiveValue::Set(None),
        }
        .insert(db)
        .await
        .expect("seed team member");
    }
    World {
        workspace,
        app: AppRef {
            id: app_id,
            slug: "store-ops".into(),
            name: "Store Ops".into(),
        },
        member,
        teammate,
        stranger,
        team,
    }
}

fn outgoing(org: Uuid, w: &World, recipients: Vec<Uuid>) -> Outgoing {
    Outgoing {
        org_id: org,
        app: Some(w.app.clone()),
        environment: "production".into(),
        workspace_id: Some(w.workspace),
        kind: "list_overdue".into(),
        critical: false,
        title: "Opening list is late".into(),
        body: Some("Clovis has 3 items open.".into()),
        link: Some("/lists/42".into()),
        email: None,
        slack: false,
        idempotency_key: None,
        thread_key: None,
        recipients,
    }
}

async fn signals(db: &DatabaseConnection, send_id: Uuid) -> Vec<notifications::Model> {
    notifications::Entity::find()
        .filter(notifications::Column::MessageId.eq(send_id))
        .all(db)
        .await
        .expect("query signals")
}

async fn delivery(
    db: &DatabaseConnection,
    signal: Uuid,
    channel: &str,
) -> notification_deliveries::Model {
    notification_deliveries::Entity::find_by_id((signal, channel.to_string()))
        .one(db)
        .await
        .expect("query delivery")
        .unwrap_or_else(|| panic!("no {channel} delivery for {signal}"))
}

/// Targets resolve to the app's audience only: the team's outsider and the
/// stranger named by id are skipped, never told.
#[tokio::test]
async fn notifications_send_reaches_only_the_apps_audience() {
    with_fx_on(Schema::All, |fx| async move {
        let w = seed_world(&fx.db, fx.org_id).await;
        let targets = vec![
            (
                Target::User(w.member),
                json!({ "userId": w.member.to_string() }),
            ),
            (Target::Team(w.team), json!({ "team": w.team.to_string() })),
            (
                Target::User(w.stranger),
                json!({ "userId": w.stranger.to_string() }),
            ),
            (Target::Team(Uuid::new_v4()), json!({ "team": "ghost" })),
        ];
        let (users, skipped) = resolve(&fx.db, fx.org_id, w.app.id, &targets)
            .await
            .expect("resolve");
        assert_eq!(
            users,
            vec![w.member, w.teammate],
            "the stranger is on the team but not in the org"
        );
        let reasons: Vec<_> = skipped.iter().map(|s| s.reason).collect();
        assert_eq!(reasons, vec!["not_in_audience", "empty_or_unknown_team"]);
    })
    .await;
}

/// The content is the org's: it is in the org's store, and Oxy's signal rows
/// carry only who, which send and whether it was read. The inbox page puts the
/// two back together, with the link resolved to the app's path URL.
#[tokio::test]
async fn notifications_send_keeps_content_in_the_orgs_store() {
    with_fx_on(Schema::All, |fx| async move {
        fx.provisioner
            .provision(fx.org_id)
            .await
            .expect("provision");
        let w = seed_world(&fx.db, fx.org_id).await;
        let sent = send(&fx.db, outgoing(fx.org_id, &w, vec![w.member, w.teammate]))
            .await
            .expect("send");
        assert_eq!(sent.recipients, 2);
        assert!(!sent.deduplicated);
        // Queued in the send's own transaction, for whichever pod drains it.
        let send_row = entity::notification_sends::Entity::find_by_id(sent.id)
            .one(&fx.db)
            .await
            .expect("query send")
            .expect("the send is recorded");
        let run_id = send_row.run_id.expect("a delivery run was queued");
        let queued = fx
            .db
            .query_one_raw(sea_orm::Statement::from_sql_and_values(
                sea_orm::DatabaseBackend::Postgres,
                "SELECT spec FROM agentic_task_queue WHERE task_id = $1",
                [run_id.into()],
            ))
            .await
            .expect("query the queue")
            .expect("the delivery task is queued");
        let spec: serde_json::Value = queued.try_get("", "spec").expect("spec");
        assert_eq!(spec["payload"]["send_id"], json!(sent.id), "{spec}");

        let rows = signals(&fx.db, sent.id).await;
        assert_eq!(rows.len(), 2);
        for row in &rows {
            assert_eq!(row.title, "", "no content in Oxy's database");
            assert_eq!(row.body, None);
            assert_eq!(row.link, None);
            assert_eq!(row.app_id, Some(w.app.id));
        }
        let stored = oxy_oltp::inbox::read(&fx.db, fx.org_id, &[sent.id])
            .await
            .expect("read the org's store");
        assert_eq!(stored.len(), 1, "one content row however many were told");
        assert_eq!(stored[0].title, "Opening list is late");

        let mine: Vec<_> = rows.into_iter().filter(|r| r.user_id == w.member).collect();
        let items = inbox_view::items(&fx.db, mine).await.expect("inbox page");
        assert_eq!(items[0].title.as_deref(), Some("Opening list is late"));
        let org_slug = format!("oltp-test-{}", fx.org_id.simple());
        assert_eq!(
            items[0].href.as_deref(),
            Some(format!("/customer-apps/{org_slug}/store-ops/lists/42").as_str())
        );
        assert_eq!(
            items[0].link.as_deref(),
            Some("/lists/42"),
            "the app's own link, as it gave it, for the bell inside the app"
        );
        assert_eq!(
            items[0].app.as_ref().map(|a| a.name.as_str()),
            Some("Store Ops")
        );
    })
    .await;
}

/// A retried call with the same key tells nobody twice and answers with the
/// first send.
#[tokio::test]
async fn notifications_send_dedupes_a_repeated_key() {
    with_fx_on(Schema::All, |fx| async move {
        fx.provisioner
            .provision(fx.org_id)
            .await
            .expect("provision");
        let w = seed_world(&fx.db, fx.org_id).await;
        let mut out = outgoing(fx.org_id, &w, vec![w.member]);
        out.idempotency_key = Some("overdue:42".into());
        let first = send(&fx.db, out.clone()).await.expect("first");
        let again = send(&fx.db, out.clone()).await.expect("again");
        assert!(again.deduplicated);
        assert_eq!(again.id, first.id);
        assert_eq!(signals(&fx.db, first.id).await.len(), 1);

        // The scope is the environment too: staging's key is not production's.
        out.environment = "staging".into();
        let staging = send(&fx.db, out).await.expect("staging");
        assert!(!staging.deduplicated);
        assert_ne!(staging.id, first.id);
    })
    .await;
}

/// Each (recipient, channel) is claimed once: a second delivery run sends
/// nothing. A muted channel is recorded as muted — unless the kind is
/// critical, which mutes do not reach.
#[tokio::test]
async fn notifications_send_delivers_each_channel_once_and_honours_mutes() {
    with_fx_on(Schema::All, |fx| async move {
        fx.provisioner
            .provision(fx.org_id)
            .await
            .expect("provision");
        let w = seed_world(&fx.db, fx.org_id).await;
        let kind = Node::Kind("list_overdue".into());
        store::set_channel(
            &fx.db,
            w.member,
            Some(w.app.id),
            &kind,
            Channel::Push,
            Some(false),
        )
        .await
        .expect("mute");

        let sent = send(&fx.db, outgoing(fx.org_id, &w, vec![w.member, w.teammate]))
            .await
            .expect("send");
        let first = deliver_send(&fx.db, sent.id).await;
        assert_eq!(first.failed, 0, "{first:?}");
        let rows = signals(&fx.db, sent.id).await;
        let of = |user: Uuid| rows.iter().find(|r| r.user_id == user).unwrap().id;
        let muted = delivery(&fx.db, of(w.member), "push").await;
        assert_eq!(
            (muted.status.as_str(), muted.reason.as_deref()),
            ("skipped", Some("muted"))
        );
        // No transport is registered in a test process: an unmuted push is
        // recorded as not configured, never as sent.
        let unmuted = delivery(&fx.db, of(w.teammate), "push").await;
        assert_eq!(
            (unmuted.status.as_str(), unmuted.reason.as_deref()),
            ("skipped", Some("not_configured"))
        );

        let again = deliver_send(&fx.db, sent.id).await;
        assert_eq!((again.sent, again.failed), (0, 0), "{again:?}");
        assert_eq!(
            delivery(&fx.db, of(w.member), "push")
                .await
                .reason
                .as_deref(),
            Some("muted"),
            "a re-run must not overwrite what the first run recorded"
        );

        let mut critical = outgoing(fx.org_id, &w, vec![w.member]);
        critical.critical = true;
        let sent = send(&fx.db, critical).await.expect("critical send");
        deliver_send(&fx.db, sent.id).await;
        let row = signals(&fx.db, sent.id).await.remove(0);
        assert_eq!(
            delivery(&fx.db, row.id, "push").await.reason.as_deref(),
            Some("not_configured"),
            "a critical kind ignores the mute"
        );
    })
    .await;
}

/// An org with no OLTP store has nowhere to keep content: refused, with the
/// operator step named, and nobody signalled.
#[tokio::test]
async fn notifications_send_refuses_an_org_with_no_store() {
    with_fx_on(Schema::All, |fx| async move {
        let org = seed_org(&fx.db).await;
        let w = seed_world(&fx.db, org).await;
        let err = send(&fx.db, outgoing(org, &w, vec![w.member]))
            .await
            .expect_err("no store");
        assert!(matches!(err, SendError::NoStore(_)), "{err:?}");
        assert!(err.to_string().contains("oxyc oltp provision"), "{err}");
        let any = notifications::Entity::find()
            .filter(notifications::Column::OrgId.eq(org))
            .all(&fx.db)
            .await
            .expect("query");
        assert!(
            any.is_empty(),
            "nobody may be signalled about content that was never kept"
        );
    })
    .await;
}

/// A mute is a write: only someone the app can tell may make one. Org
/// membership by itself is not the test (`reaches_org` is true for every
/// session); the app's audience, or a notification already received, is.
#[tokio::test]
async fn notifications_send_mutes_are_bounded_by_who_the_app_can_tell() {
    with_fx_on(Schema::All, |fx| async move {
        let w = seed_world(&fx.db, fx.org_id).await;
        let app = apps::Entity::find_by_id(w.app.id)
            .one(&fx.db)
            .await
            .expect("query app")
            .expect("seeded app");
        assert!(
            may_be_told_by(&fx.db, w.member, &app)
                .await
                .expect("member")
        );
        assert!(
            !may_be_told_by(&fx.db, w.stranger, &app)
                .await
                .expect("stranger"),
            "outside the org and never told: no mute rows for this app"
        );
    })
    .await;
}
