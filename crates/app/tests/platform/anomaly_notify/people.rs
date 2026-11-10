//! `notify.people`: an `oxy.anomaly` inbox notice per event to the org members
//! a `.monitor.yml` names, claimed on its own ledger channel.

use chrono::Utc;
use entity::org_members::{self, OrgRole};
use entity::workspaces::WorkspaceStatus;
use entity::{notifications, organizations, users, workspaces};
use oxy_app::server::anomaly_notify::people::PeopleInbox;
use oxy_metric_monitoring::notify::{OrgRoleTarget, PersonTarget, announce, ledger};
use oxy_metric_monitoring::{NotifyConfig, Severity};
use sea_orm::ActiveValue::Set;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;

use super::{HEADING, Row, due, seed};

struct World {
    org: Uuid,
    workspace: Uuid,
    admin: Uuid,
    member: Uuid,
}

async fn person(db: &DatabaseConnection, org: Uuid, role: OrgRole) -> Uuid {
    let id = Uuid::new_v4();
    let now = Utc::now().fixed_offset();
    users::ActiveModel {
        id: Set(id),
        email: Set(Some(format!("p-{}@example.com", id.simple()))),
        name: Set("Person".into()),
        picture: Set(None),
        email_verified: Set(true),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed user");
    org_members::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org),
        user_id: Set(id),
        role: Set(role),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .expect("join org");
    id
}

async fn world(db: &DatabaseConnection) -> World {
    let now = Utc::now().fixed_offset();
    let org = Uuid::new_v4();
    organizations::ActiveModel {
        id: Set(org),
        name: Set("insights-org".into()),
        slug: Set(format!("insights-{}", org.simple())),
        created_at: Set(now),
        updated_at: Set(now),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed org");
    let workspace = Uuid::new_v4();
    workspaces::ActiveModel {
        id: Set(workspace),
        name: Set("insights-ws".into()),
        created_at: Set(now),
        updated_at: Set(now),
        org_id: Set(Some(org)),
        status: Set(WorkspaceStatus::Ready),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed workspace");
    World {
        org,
        workspace,
        admin: person(db, org, OrgRole::Admin).await,
        member: person(db, org, OrgRole::Member).await,
    }
}

fn people_block(people: Vec<PersonTarget>) -> NotifyConfig {
    NotifyConfig {
        slack_channel: None,
        people,
        min_severity: Severity::High,
    }
}

fn admins() -> Vec<PersonTarget> {
    vec![PersonTarget {
        role: Some(OrgRoleTarget::Admin),
        ..Default::default()
    }]
}

async fn notices(db: &DatabaseConnection, event: Uuid) -> Vec<notifications::Model> {
    notifications::Entity::find()
        .filter(notifications::Column::Kind.eq("oxy.anomaly"))
        .filter(notifications::Column::SubjectId.eq(event.to_string()))
        .all(db)
        .await
        .expect("query notices")
}

/// Each event is told once to the people the block names, on a channel of its
/// own: the Slack claim is untouched, and a quieter event is not told.
#[tokio::test]
async fn people_are_told_each_event_once_on_their_own_channel() {
    let (db, _url) = crate::common::fresh_db(crate::common::Schema::All).await;
    let w = world(&db).await;
    let event = Uuid::new_v4();
    seed(&db, Row::due(w.workspace, event)).await;
    let quiet = Uuid::new_v4();
    seed(
        &db,
        Row {
            severity: "low",
            ..Row::due(w.workspace, quiet)
        },
    )
    .await;

    let to = PeopleInbox::resolve(&db, w.org, w.workspace, &people_block(admins()))
        .await
        .expect("resolve");
    assert_eq!(to.reach(), 1, "admins and above: the one admin");
    assert_eq!(
        announce(&db, due(w.workspace), HEADING, &to)
            .await
            .expect("announce"),
        1
    );
    assert_eq!(
        announce(&db, due(w.workspace), HEADING, &to)
            .await
            .expect("again"),
        0
    );

    let told = notices(&db, event).await;
    assert_eq!(
        told.iter().map(|n| n.user_id).collect::<Vec<_>>(),
        [w.admin]
    );
    assert!(notices(&db, quiet).await.is_empty(), "below min_severity");
    assert!(
        ledger::anything_due(&db, due(w.workspace)).await.unwrap(),
        "the Slack channel's claim is its own"
    );

    // How the notice reads in the inbox is the notifications surface's
    // (`oxy-api-notifications`, `platform_notices::anomaly`).
    let _ = w.member;
}

/// A list that names nobody in the org refuses, so the claim is given back and
/// the run says what to fix.
#[tokio::test]
async fn a_people_list_that_names_nobody_refuses_and_keeps_the_event_due() {
    let (db, _url) = crate::common::fresh_db(crate::common::Schema::All).await;
    let w = world(&db).await;
    seed(&db, Row::due(w.workspace, Uuid::new_v4())).await;
    let nobody = vec![PersonTarget {
        email: Some("ghost@example.com".into()),
        ..Default::default()
    }];
    let to = PeopleInbox::resolve(&db, w.org, w.workspace, &people_block(nobody))
        .await
        .expect("resolve");
    let err = announce(&db, due(w.workspace), HEADING, &to)
        .await
        .expect_err("nobody to tell");
    assert!(err.to_string().contains("notify.people"), "{err}");
    assert!(
        ledger::anything_due_on(&db, due(w.workspace), ledger::CHANNEL_INBOX)
            .await
            .unwrap()
    );
}
