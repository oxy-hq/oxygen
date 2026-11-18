//! Event subscriptions follow each environment's live build, on the database.
//!
//! A function's `on` becomes one `app_event_subscriptions` row per (environment,
//! function, event type) when its build goes live — production's at a promoting
//! publish, an environment's at each publish to it — and the next build
//! without it retires the row. A draft registers nothing in production, a
//! sandbox's teardown retires only its own rows, and `listeners` reads them by
//! org, environment and type.

use agentic_core::delegation::TaskOutcome;
use chrono::Utc;
use entity::{app_event_subscriptions as subs, organizations};
use oxy_api_custom_apps::app_events::subscriptions::listeners;
use oxy_api_custom_apps::custom_apps_sandboxes::{TeardownReason, ops};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{FunctionSpec, Tenant, publish_build, seeded_tenant};
use crate::env_schedules::publish_sandbox;
use crate::sandbox_publish::{app_row, sandbox};
use crate::staging_functions::make_guest_staff;

const JS: &str = "export default async () => Response.json({});";

fn listener(on: Value) -> FunctionSpec {
    FunctionSpec {
        name: "on-upload",
        manifest: json!({ "on": on }),
        js: JS,
    }
}

fn plain() -> FunctionSpec {
    FunctionSpec {
        name: "on-upload",
        manifest: json!({ "route": true }),
        js: JS,
    }
}

/// The app's rows as sorted `environment/function/event type`.
async fn rows(db: &DatabaseConnection, app: Uuid) -> Vec<String> {
    let mut rows: Vec<String> = subs::Entity::find()
        .filter(subs::Column::AppId.eq(app))
        .all(db)
        .await
        .expect("read the subscriptions")
        .into_iter()
        .map(|r| format!("{}/{}/{}", r.environment, r.function_name, r.event_type))
        .collect();
    rows.sort();
    rows
}

async fn app_of(t: &Tenant, slug: &str, build: &str, promote: bool, f: &[FunctionSpec]) -> Uuid {
    publish_build(t, slug, demo_workspace_id(), build, promote, f)
        .await
        .app_id
}

#[tokio::test]
async fn publishing_a_build_with_on_registers_it() {
    let t = seeded_tenant().await;
    let on = json!(["billing/statement.uploaded", "billing/report.ready"]);
    let app = app_of(&t, "subs-register", "b1", true, &[listener(on)]).await;

    let expected = |env: &str| {
        vec![
            format!("{env}/on-upload/billing/report.ready"),
            format!("{env}/on-upload/billing/statement.uploaded"),
        ]
    };
    let mut want = [expected("production"), expected("staging")].concat();
    want.sort();
    assert_eq!(rows(&t.db, app).await, want);

    // Reconciling again keeps the rows (same ids) and refreshes `updated_at`.
    let before = subs::Entity::find()
        .filter(subs::Column::AppId.eq(app))
        .all(&t.db)
        .await
        .unwrap();
    app_of(
        &t,
        "subs-register",
        "b2",
        true,
        &[listener(json!("billing/report.ready"))],
    )
    .await;
    let after = subs::Entity::find()
        .filter(subs::Column::AppId.eq(app))
        .all(&t.db)
        .await
        .unwrap();
    assert_eq!(after.len(), 2, "one event type per environment now");
    let kept = after
        .iter()
        .find(|r| r.environment == "production")
        .unwrap();
    let old = before
        .iter()
        .find(|r| r.environment == "production" && r.event_type == kept.event_type)
        .expect("the row was there before");
    assert_eq!(kept.id, old.id);
    assert!(kept.updated_at > old.updated_at);
    assert_ne!(kept.build_id, old.build_id, "it follows the new build");
}

#[tokio::test]
async fn the_next_build_without_on_retires_it() {
    let t = seeded_tenant().await;
    let on = json!("billing/statement.uploaded");
    let app = app_of(&t, "subs-retire", "b1", true, &[listener(on)]).await;
    assert_eq!(rows(&t.db, app).await.len(), 2);

    app_of(&t, "subs-retire", "b2", true, &[plain()]).await;
    assert_eq!(rows(&t.db, app).await, Vec::<String>::new());
}

#[tokio::test]
async fn a_draft_build_registers_nothing_in_production() {
    let t = seeded_tenant().await;
    let on = json!("billing/statement.uploaded");
    let app = app_of(&t, "subs-draft", "b1", false, &[listener(on)]).await;
    assert_eq!(
        rows(&t.db, app).await,
        vec!["staging/on-upload/billing/statement.uploaded".to_string()],
        "the draft is staging's build only"
    );
}

#[tokio::test]
async fn teardown_retires_the_environments_rows() {
    crate::sandbox_teardown_task::use_scratch_homes();
    let t = seeded_tenant().await;
    let on = json!("billing/statement.uploaded");
    let app_id = app_of(&t, "subs-down", "b1", true, &[listener(on.clone())]).await;
    make_guest_staff();
    let app = app_row(&t.db, app_id).await;
    ops::create(&t.db, &app, &sandbox("a1"), &t.guest())
        .await
        .expect("create the sandbox");
    publish_sandbox(&t, "subs-down", "sbx-1", "a1", &[listener(on)]).await;
    let all = rows(&t.db, app_id).await;
    assert_eq!(all.len(), 3, "{all:?}");
    assert!(all.iter().any(|r| r.starts_with("dev-a1/")));

    let run_id = ops::begin_delete(
        &t.db,
        &app,
        &sandbox("a1"),
        Some(&t.guest()),
        TeardownReason::Deleted,
    )
    .await
    .expect("delete dev-a1");
    let outcome = crate::sandbox_teardown_task::run_queued(&t.db, &run_id).await;
    let TaskOutcome::Done { answer, .. } = outcome else {
        panic!("the teardown failed: {outcome:?}");
    };
    assert!(answer.contains("1 event subscriptions retired"), "{answer}");
    let left = rows(&t.db, app_id).await;
    assert_eq!(left.len(), 2);
    assert!(left.iter().all(|r| !r.starts_with("dev-")), "{left:?}");
}

#[tokio::test]
async fn listeners_filters_by_org_environment_and_type() {
    let t = seeded_tenant().await;
    let on = json!(["billing/statement.uploaded", "billing/report.ready"]);
    let app = app_of(&t, "subs-listen", "b1", true, &[listener(on)]).await;

    // Another org's subscription to the same type, on the same app row.
    let other_org = Uuid::new_v4();
    organizations::ActiveModel {
        id: Set(other_org),
        name: Set("Other Org".into()),
        slug: Set(format!("other-{other_org}")),
        ..Default::default()
    }
    .insert(&t.db)
    .await
    .expect("seed org");
    let mine = subs::Entity::find()
        .filter(subs::Column::AppId.eq(app))
        .filter(subs::Column::Environment.eq("production"))
        .one(&t.db)
        .await
        .unwrap()
        .expect("a row");
    subs::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(other_org),
        function_name: Set("other-fn".into()),
        event_type: Set("billing/statement.uploaded".into()),
        created_at: Set(Utc::now().into()),
        updated_at: Set(Utc::now().into()),
        ..mine.clone().into()
    }
    .insert(&t.db)
    .await
    .expect("other org's row");

    let found = listeners(&t.db, t.org_id, "production", "billing/statement.uploaded")
        .await
        .expect("listeners");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].app_id, app);
    assert_eq!(found[0].app_slug, "subs-listen");
    assert_eq!(found[0].function_name, "on-upload");
    assert_eq!(found[0].environment, "production");
    assert_eq!(
        found[0].function_manifest["on"],
        json!(["billing/statement.uploaded", "billing/report.ready"])
    );

    let staging = listeners(&t.db, t.org_id, "staging", "billing/statement.uploaded")
        .await
        .unwrap();
    assert_eq!(staging.len(), 1);
    assert_eq!(staging[0].environment, "staging");
    let other_type = listeners(&t.db, t.org_id, "production", "billing/nothing")
        .await
        .unwrap();
    assert!(other_type.is_empty());
    let theirs = listeners(&t.db, other_org, "production", "billing/statement.uploaded")
        .await
        .unwrap();
    assert!(
        theirs.is_empty(),
        "its row names a function its build does not have, and is left out"
    );
    let none = listeners(
        &t.db,
        Uuid::new_v4(),
        "production",
        "billing/statement.uploaded",
    )
    .await
    .unwrap();
    assert!(none.is_empty());
}

/// Reconciles side by side, one of them handed an older build, end with the
/// rows of the build production serves: each resolves it under the lock.
#[tokio::test]
async fn concurrent_reconciles_end_with_the_live_builds_rows() {
    use entity::app_builds;
    use oxy_api_custom_apps::app_events::subscriptions::{
        reconcile_environment, reconcile_production,
    };
    use oxy_app_core::custom_app_environment::AppEnvironment;

    let t = seeded_tenant().await;
    let app_id = app_of(
        &t,
        "subs-race",
        "b1",
        true,
        &[listener(json!("billing/old.event"))],
    )
    .await;
    app_of(
        &t,
        "subs-race",
        "b2",
        true,
        &[listener(json!("billing/new.event"))],
    )
    .await;
    let app = app_row(&t.db, app_id).await;
    let old = app_builds::Entity::find()
        .filter(app_builds::Column::AppId.eq(app_id))
        .filter(app_builds::Column::BuildId.eq("b1"))
        .one(&t.db)
        .await
        .unwrap()
        .expect("the older build");

    for _ in 0..5 {
        let production = AppEnvironment::Production;
        let (a, b, c, d) = tokio::join!(
            reconcile_production(&t.db, &app, &old),
            reconcile_environment(&t.db, &app, &production),
            reconcile_production(&t.db, &app, &old),
            reconcile_environment(&t.db, &app, &production),
        );
        assert!(
            a.is_empty() || a.iter().all(|w| w.contains("not declared")),
            "{a:?}"
        );
        b.expect("reconcile");
        d.expect("reconcile");
        let _ = c;
        let live: Vec<String> = rows(&t.db, app_id)
            .await
            .into_iter()
            .filter(|r| r.starts_with("production/"))
            .collect();
        assert_eq!(
            live,
            vec!["production/on-upload/billing/new.event".to_string()]
        );
    }
}

/// The console's Make Live moves production through `move_production` directly;
/// it re-registers the build's subscriptions, as a promote does.
#[tokio::test]
async fn a_staff_rollback_reregisters_the_rolled_back_to_builds_subscriptions() {
    use crate::promote_rollback::{draft, person};
    use crate::sandbox_agent_token::fixture::agent;
    use crate::sandbox_publish::bundle;

    let agent = agent().await;
    let slug = agent.app.slug.clone();
    let tarball = bundle(
        &slug,
        &[listener(json!("billing/statement.uploaded"))],
        json!({}),
        &[],
    );
    let build = draft(&agent, "sub-draft", tarball).await;
    assert!(
        !rows(&agent.t.db, agent.app.id)
            .await
            .iter()
            .any(|r| r.starts_with("production/")),
        "a draft registers nothing in production"
    );

    let uri = format!("/customer-apps/{}/rollback", agent.app.id);
    let (status, body) = person(&agent, "POST", &uri, Some(json!({ "build_id": build }))).await;
    assert_eq!(status, axum::http::StatusCode::OK, "{body}");
    let live: Vec<String> = rows(&agent.t.db, agent.app.id)
        .await
        .into_iter()
        .filter(|r| r.starts_with("production/"))
        .collect();
    assert_eq!(
        live,
        vec!["production/on-upload/billing/statement.uploaded".to_string()]
    );
}
