//! Deleting a custom app records who did it and takes the app's usage facts out
//! of its workspace's Airhouse, whatever state Airhouse is in at that moment.
//!
//! Drives the real handlers against a per-test database (`common::test_db`);
//! the workspace's Airhouse is `usage_fixtures::FakeAirhouse`. Global Owner
//! standing comes from `OXY_OWNER`; nextest gives each test its own process.
//!
//! | Test | Proves |
//! | ---- | ------ |
//! | `deleting_an_app_writes_an_audit_row_and_queues_the_purge_of_its_facts` | the single delete writes `app.deleted` and queues one purge and writes its ledger row; running it removes this app's facts and no other app's |
//! | `a_batch_delete_audits_and_queues_each_deleted_app` | one row and one purge per app deleted |
//! | `ledger::*` | the owed-purge ledger: backoff past the old retry window, one give-up, no requeue of a finished purge (`custom_app_deletion/ledger.rs`) |
//! | `the_purge_kind_is_handed_to_the_fleet_and_is_a_platform_daemon` | `oxy-server` registers the executor and every daemon list names the kind |

use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use chrono::Utc;
use entity::{
    apps, audit_events, custom_app_usage_purges as purges, organizations, users, workspaces,
};
use oxy_api_custom_apps::admin::apps::handlers::{batch_delete_apps, delete_app};
use oxy_api_custom_apps::custom_apps_usage_purge::{
    USAGE_PURGE_KIND, UsagePurgeTask, attempt, run,
};
use oxy_app_core::agent_refusal::RefuseSandboxAgent;
use oxy_app_core::audit::RequestActor;
use oxy_auth::types::AuthenticatedUser;
use oxy_custom_apps::usage_airhouse::{KIND_VIEW, UsageFact, UsageSink};
use oxy_custom_apps::usage_fixtures::FakeAirhouse;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseBackend,
    DatabaseConnection, EntityTrait, QueryFilter, Statement,
};
use serde_json::json;
use uuid::Uuid;

use crate::common::{Schema, test_db_with};

mod ledger;

struct World {
    db: DatabaseConnection,
    actor: RequestActor,
    org_id: Uuid,
    workspace_id: Uuid,
    /// Two apps of the one workspace, so one's delete can be shown to leave the
    /// other's facts alone.
    apps: [Uuid; 2],
    fake: std::sync::Arc<FakeAirhouse>,
}

async fn world() -> World {
    let db = test_db_with(Schema::All).await;
    let org_id = Uuid::new_v4();
    organizations::ActiveModel {
        id: Set(org_id),
        name: Set("Deletion Org".into()),
        slug: Set(format!("deletion-{}", &org_id.simple().to_string()[..12])),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed org");
    let workspace_id = Uuid::new_v4();
    workspaces::ActiveModel {
        id: Set(workspace_id),
        name: Set("Deletion Workspace".into()),
        org_id: Set(Some(org_id)),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed workspace");
    let user_id = Uuid::new_v4();
    let email = format!("staff-{user_id}@example.com");
    users::ActiveModel {
        id: Set(user_id),
        email: Set(Some(email.clone())),
        name: Set("Staff".into()),
        picture: Set(None),
        email_verified: Set(true),
        status: Set(users::UserStatus::Active),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed staff");
    // SAFETY: process-per-test (asserted by `test_db`), before any request runs.
    unsafe { std::env::set_var("OXY_OWNER", &email) };
    let actor = RequestActor::session(AuthenticatedUser {
        id: user_id,
        email: Some(email),
        name: "Staff".into(),
        picture: None,
        status: users::UserStatus::Active,
        credential: None,
    });
    let apps = [seed_app(&db, org_id, workspace_id).await, {
        seed_app(&db, org_id, workspace_id).await
    }];
    let fake = FakeAirhouse::new();
    fake.with_airhouse(workspace_id).install();
    let world = World {
        db,
        actor,
        org_id,
        workspace_id,
        apps,
        fake,
    };
    for app_id in world.apps {
        world.append_fact(app_id).await;
    }
    world
}

async fn seed_app(db: &DatabaseConnection, org_id: Uuid, workspace_id: Uuid) -> Uuid {
    let id = Uuid::new_v4();
    apps::ActiveModel {
        id: Set(id),
        slug: Set(format!("gone-{}", &id.simple().to_string()[..12])),
        name: Set("Doomed App".into()),
        org_id: Set(org_id),
        project_id: Set(workspace_id),
        branch: Set("main".into()),
        source_repo: Set("oxy-hq/customer-apps".into()),
        status: Set("active".into()),
        source_type: Set("s3".into()),
        source_config: Set(json!({})),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed app");
    id
}

impl World {
    /// One page view of `app_id` in the workspace's Airhouse, with a viewer's
    /// email in it: what a deletion owes the customer.
    async fn append_fact(&self, app_id: Uuid) {
        let fact = UsageFact {
            event_id: Uuid::new_v4(),
            kind: KIND_VIEW,
            occurred_at: Utc::now(),
            recorded_at: Utc::now(),
            org_id: self.org_id,
            app_id,
            environment: "production".into(),
            user_id: Uuid::new_v4(),
            user_email: "viewer@example.com".into(),
            session_id: Uuid::new_v4(),
            event_name: None,
            payload: None,
            referrer: None,
            user_agent_class: None,
            source: None,
            app_role: None,
            org_role: None,
        };
        self.fake
            .prepare(&self.db, self.workspace_id)
            .await
            .expect("table");
        self.fake
            .append(&self.db, self.workspace_id, &[fact])
            .await
            .expect("append");
    }

    fn facts_of(&self, i: usize) -> i64 {
        self.fake.rows_of_app(self.workspace_id, self.apps[i])
    }

    async fn audit(&self, action: &str) -> Vec<audit_events::Model> {
        audit_events::Entity::find()
            .filter(audit_events::Column::Action.eq(action))
            .filter(audit_events::Column::OrgId.eq(self.org_id))
            .all(&self.db)
            .await
            .expect("read the audit rows")
    }

    /// Queue rows of the purge kind for `app_id`, as `(task_id, status)`.
    async fn purges(&self, app_id: Uuid) -> Vec<(String, String)> {
        self.db
            .query_all_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT task_id, queue_status FROM agentic_task_queue \
                  WHERE spec->>'kind' = $1 AND spec->'payload'->>'app_id' = $2 \
                  ORDER BY task_id",
                [USAGE_PURGE_KIND.into(), app_id.to_string().into()],
            ))
            .await
            .expect("read the queue")
            .into_iter()
            .map(|r| {
                (
                    r.try_get("", "task_id").unwrap(),
                    r.try_get("", "queue_status").unwrap(),
                )
            })
            .collect()
    }

    async fn set_status(&self, app_id: Uuid, status: &str) {
        self.db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "UPDATE agentic_task_queue SET queue_status = $3 \
                  WHERE spec->>'kind' = $1 AND spec->'payload'->>'app_id' = $2",
                [
                    USAGE_PURGE_KIND.into(),
                    app_id.to_string().into(),
                    status.into(),
                ],
            ))
            .await
            .expect("set status");
    }

    fn task(&self, i: usize) -> UsagePurgeTask {
        UsagePurgeTask {
            app_id: self.apps[i],
            workspace_id: self.workspace_id,
        }
    }

    async fn app_exists(&self, i: usize) -> bool {
        apps::Entity::find_by_id(self.apps[i])
            .one(&self.db)
            .await
            .expect("load app")
            .is_some()
    }
}

#[tokio::test]
async fn deleting_an_app_writes_an_audit_row_and_queues_the_purge_of_its_facts() {
    let w = world().await;
    assert_eq!((w.facts_of(0), w.facts_of(1)), (1, 1));

    let status = delete_app(RefuseSandboxAgent, w.actor.clone(), Path(w.apps[0]))
        .await
        .expect("delete");
    assert_eq!(status, StatusCode::NO_CONTENT);

    assert!(!w.app_exists(0).await);
    let rows = w.audit("app.deleted").await;
    assert_eq!(rows.len(), 1, "one row for one delete");
    assert_eq!(rows[0].target_id.as_deref(), Some(&*w.apps[0].to_string()));
    assert_eq!(rows[0].target_label.as_deref(), Some("Doomed App"));
    assert_eq!(rows[0].actor_user_id, Some(w.actor.id));

    let queued = w.purges(w.apps[0]).await;
    assert_eq!(queued.len(), 1, "the purge is queued with the delete");
    assert_eq!(queued[0].1, "queued");
    assert_eq!(w.facts_of(0), 1, "nothing is purged until the task runs");

    assert!(w.row(0).await.completed_at.is_none(), "owed until it runs");

    attempt(&w.db, &w.task(0), Utc::now())
        .await
        .expect("the purge runs");
    assert!(
        w.row(0).await.completed_at.is_some(),
        "the ledger is closed"
    );
    assert_eq!(w.facts_of(0), 0, "the deleted app's facts are gone");
    assert_eq!(w.facts_of(1), 1, "another app's facts are not");
    assert!(w.purges(w.apps[1]).await.is_empty() && w.app_exists(1).await);
    assert!(
        purges::Entity::find_by_id(w.apps[1])
            .one(&w.db)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn a_batch_delete_audits_and_queues_each_deleted_app() {
    let w = world().await;
    let req = serde_json::from_value(json!({ "ids": w.apps })).expect("a batch body");

    let Json(resp) = batch_delete_apps(RefuseSandboxAgent, w.actor.clone(), Json(req))
        .await
        .expect("batch delete");
    assert!(resp.results.iter().all(|r| r.ok), "{:?}", resp.results);

    assert_eq!(w.audit("app.deleted").await.len(), 2);
    for i in 0..2 {
        assert_eq!(w.purges(w.apps[i]).await.len(), 1);
        assert_eq!(w.row(i).await.attempts, 0, "owed, not yet run");
    }
}

#[tokio::test]
async fn a_workspace_without_an_airhouse_has_nothing_to_purge() {
    let w = world().await;
    let task = UsagePurgeTask {
        app_id: w.apps[0],
        workspace_id: Uuid::new_v4(),
    };
    let summary = run(&w.db, &task).await.expect("nothing to delete is done");
    assert!(summary.contains("no Airhouse"), "{summary}");
    assert_eq!(w.facts_of(0), 1);
}

/// The worker fleet runs the purge only if `oxy-server` hands in its kind, and
/// a failed purge must not mark a customer's workspace unhealthy or fill the
/// run feed. Source scans: the registry and two of the lists are private to
/// their crates.
#[test]
fn the_purge_kind_is_handed_to_the_fleet_and_is_a_platform_daemon() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let read = |file: &str| std::fs::read_to_string(root.join(file)).expect(file);

    let seams = read("../server/src/custom_apps_seams.rs");
    let from = seams.find("fn tasks(").expect("`tasks` is gone");
    let tasks: String = seams[from..][..seams[from..].find("fn background(").unwrap()]
        .split_whitespace()
        .collect();
    assert!(
        tasks.contains(
            "TaskSeam{kind:USAGE_PURGE_KIND,\
             build:|deps|{Arc::new(custom_apps_usage_purge::UsagePurgeExecutor{db:deps.db.clone(),})},}"
        ),
        "`custom_apps_seams::tasks` no longer hands in the usage purge's executor"
    );

    for (file, list) in [
        (
            "../agentic/runtime/src/lifecycle/crud/queries.rs",
            "pub const SYSTEM_SOURCE_TYPES: &[&str] = &[",
        ),
        (
            "../usage-report/src/customer_runs.rs",
            "const NON_WORKSPACE_RUN_SOURCES: &[&str] = &[",
        ),
        (
            "../../web-app/src/pages/ide/coordinator/components/constants.ts",
            "const SYSTEM_SOURCE_TYPES: readonly string[] = [",
        ),
    ] {
        let source = read(file);
        let start = source
            .find(list)
            .unwrap_or_else(|| panic!("{file} no longer declares `{list}`"));
        let body = &source[start..];
        let body = &body[..body.find("];").expect("the list ends")];
        assert!(
            body.contains(&format!("\"{USAGE_PURGE_KIND}\"")),
            "{file}: `{USAGE_PURGE_KIND}` is missing from the daemon list"
        );
    }
}
