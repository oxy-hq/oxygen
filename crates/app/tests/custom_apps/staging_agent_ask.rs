//! An agent ask from a custom app's staging host runs with every write held
//! (`projects::agent_ask_staging`).
//!
//! A real LLM ask cannot run here, so each case drives the pieces `start_ask`
//! composes, at the boundaries it composes them:
//!
//! - **the decision** — `ask_scope` over the request's real headers, against
//!   a published app, oxy-authz's `may_open_non_production` and the database;
//! - **the platform** — `build_project_context` (the builder `start_ask`
//!   calls) inside the hold, over the seeded demo workspace, used from a
//!   spawned task as the run's drive uses it;
//! - **the held write** — DDL sent through the connector that platform hands
//!   the pipeline, then read back from `GET …/staging/held`;
//! - **the run** — the stamp `start_analytics` applies, on a real
//!   `agentic_runs` row, then the startup recovery entry point.
//!
//! The environment guard's allowance (only the ask routes, staging only) is
//! unit-tested beside it in `oxy-app-core`.

use std::sync::Arc;

use agentic_pipeline::platform::preview_stamp::{INTERRUPTED, RUN_STAMP, stamp};
use agentic_pipeline::platform::{PlatformContext, ProjectContext};
use axum::extract::{Path, Query};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use entity::users::UserStatus;
use oxy_app::agentic_wiring::OxyProjectContext;
use oxy_app::server::api::custom_apps_gates::build_project_context;
use oxy_app::server::api::custom_apps_staging_held::{HeldQuery, list_held};
use oxy_app::server::api::projects::agent_ask::caller::{
    CallerRunResolver, RUN_CALLER_KEY, RunCaller,
};
use oxy_app::server::api::projects::agent_ask_staging::ask_scope;
use oxy_app::server::previews::request_hold::{HoldScope, scope_if};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_auth::types::AuthenticatedUser;
use oxy_auth::user::LOCAL_GUEST_EMAIL;
use sea_orm::EntityTrait;
use serde_json::json;
use uuid::Uuid;

use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{Tenant, publish_build, seeded_tenant};
use crate::staging_functions::{held_rows, make_guest_staff, production_host, staging_host};

/// The demo workspace's small DuckDB directory.
const DATABASE: &str = "training";

fn guest(t: &Tenant) -> AuthenticatedUser {
    AuthenticatedUser {
        id: t.guest_id,
        email: Some(LOCAL_GUEST_EMAIL.to_string()),
        name: "Developer".to_string(),
        picture: None,
        status: UserStatus::Active,
    }
}

fn on_host(host: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert("host", HeaderValue::from_str(host).unwrap());
    h
}

/// An app published, promoted, from the demo workspace.
async fn app(t: &Tenant, slug: &str) -> Uuid {
    publish_build(t, slug, demo_workspace_id(), "ask-1", true, &[])
        .await
        .app_id
}

/// The platform `start_ask` builds, inside `hold` when there is one.
async fn platform(t: &Tenant, hold: Option<HoldScope>) -> Arc<OxyProjectContext> {
    let workspace = entity::workspaces::Entity::find_by_id(demo_workspace_id())
        .one(&t.db)
        .await
        .expect("query workspace")
        .expect("the demo workspace is seeded");
    let built = scope_if(
        hold,
        build_project_context(&workspace, t.guest_id, demo_workspace_id()),
    )
    .await;
    Arc::new(built.unwrap_or_else(|r| panic!("context build failed: {}", r.status())))
}

async fn body(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("body");
    serde_json::from_slice(&bytes).expect("json body")
}

#[tokio::test]
async fn a_staging_ask_holds_every_write_and_the_console_lists_it() {
    let t = seeded_tenant().await;
    let slug = "stg-ask-held";
    let app_id = app(&t, slug).await;
    make_guest_staff();

    let hold = ask_scope(
        &t.db,
        &on_host(&staging_host(&t, slug)),
        &guest(&t),
        demo_workspace_id(),
    )
    .await
    .unwrap_or_else(|r| panic!("staff may ask on staging: {}", r.status()))
    .expect("a staging ask runs held");
    assert_eq!(hold.app_id(), Some(app_id));

    let ctx = platform(&t, Some(hold)).await;
    // The run is driven on a spawned task, outside the request's scope.
    let on_drive = {
        let ctx = ctx.clone();
        tokio::spawn(async move {
            (
                ctx.holds_writes(),
                ctx.is_workspace_preview(),
                ctx.staging_app_id(),
            )
        })
        .await
        .unwrap()
    };
    assert_eq!(
        on_drive,
        (true, true, Some(app_id)),
        "no runner, no bridges"
    );

    let conn = ctx
        .resolve_pre_built_connector(DATABASE)
        .await
        .expect("a held platform pre-builds every database, held");
    conn.execute_query("SELECT 1", 1)
        .await
        .expect("a read is sent");
    let err = conn
        .execute_statement("CREATE TABLE ask_scratch AS SELECT 1 AS x")
        .await
        .expect_err("DDL is held")
        .to_string();
    assert!(
        err.contains("cannot be written in this app's staging environment"),
        "{err}"
    );

    let rows = list_held(
        Path(app_id),
        Query(HeldQuery::default()),
        AuthenticatedUserExtractor(guest(&t)),
    )
    .await
    .map(|axum::Json(rows)| rows)
    .expect("the asker may open staging");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].function, "agent");
    let w = &rows[0].writes[0];
    assert_eq!(
        (w.namespace.as_str(), w.table.as_str()),
        (DATABASE, "ask_scratch")
    );
    assert!(w.verb.starts_with("CREATE"), "{w:?}");

    let audit = held_rows(&t).await;
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].environment, "staging");
    assert_eq!(audit[0].actor_user_id, Some(t.guest_id));
    assert_eq!(audit[0].metadata["app_id"], json!(app_id));
    assert_eq!(audit[0].metadata["mode"], "agent");
}

#[tokio::test]
async fn a_staging_ask_run_is_stamped_and_recovery_retires_it() {
    let t = seeded_tenant().await;
    let slug = "stg-ask-stamp";
    let app_id = app(&t, slug).await;
    make_guest_staff();
    let hold = ask_scope(
        &t.db,
        &on_host(&staging_host(&t, slug)),
        &guest(&t),
        demo_workspace_id(),
    )
    .await
    .ok()
    .flatten()
    .expect("a staging hold");
    let held: Arc<dyn PlatformContext> = platform(&t, Some(hold)).await;

    // Its own workspace id, so recovery here touches this run alone. No
    // workspace has that id, so the caller recorded below can never be
    // rebuilt: the run must still be retired, not skipped on every tick.
    let run_ws = Uuid::new_v4();
    let mut metadata = json!({ "agent_id": "analytics" });
    stamp(held.as_ref(), &mut metadata);
    assert_eq!(metadata[RUN_STAMP]["app_id"], json!(app_id), "{metadata}");
    // What `start_ask` records next to the stamp since #3475.
    metadata[RUN_CALLER_KEY] = RunCaller {
        user_id: guest(&t).id,
        staging_pin: None,
    }
    .to_metadata();
    let run_id = format!("ask-{}", Uuid::new_v4().simple());
    agentic_runtime::crud::insert_run(
        &t.db,
        &run_id,
        "q",
        None,
        "analytics",
        Some(metadata),
        run_ws,
    )
    .await
    .expect("insert the run as start_analytics does");

    // A restart: production's platform, every root resumed.
    let production: Arc<dyn PlatformContext> = platform(&t, None).await;
    agentic_pipeline::recovery::recover_active_runs(
        t.db.clone(),
        Arc::new(agentic_runtime::state::RuntimeState::new()),
        production,
        // The resolver production recovery uses, not a stand-in.
        CallerRunResolver::shared(&t.db),
        None,
        None,
        None,
        None,
        Arc::new(agentic_runtime::router::NoopTaskRouter),
        Some(run_ws),
        None,
        agentic_pipeline::recovery::DrivePolicy::ALL,
    )
    .await;

    let row = agentic_runtime::crud::get_run(&t.db, &run_id)
        .await
        .unwrap()
        .expect("run");
    assert_eq!(row.task_status.as_deref(), Some("failed"), "{row:?}");
    assert!(
        row.error_message.unwrap_or_default().contains(INTERRUPTED),
        "retired, never driven on production"
    );
}

#[tokio::test]
async fn a_production_ask_is_unchanged_no_hold_no_stamp() {
    let t = seeded_tenant().await;
    let slug = "stg-ask-prod";
    app(&t, slug).await;
    make_guest_staff();

    let hold = ask_scope(
        &t.db,
        &on_host(&production_host(&t, slug)),
        &guest(&t),
        demo_workspace_id(),
    )
    .await
    .ok()
    .expect("production asks pass");
    assert!(hold.is_none(), "production holds nothing");
    let none_from_admin_host = ask_scope(
        &t.db,
        &on_host("app.oxygen-hq.com"),
        &guest(&t),
        demo_workspace_id(),
    )
    .await
    .ok()
    .expect("the admin host is production");
    assert!(none_from_admin_host.is_none());

    let ctx: Arc<dyn PlatformContext> = platform(&t, hold).await;
    assert!(!ctx.is_workspace_preview());
    assert_eq!(ctx.staging_app_id(), None);
    assert!(
        ctx.resolve_pre_built_connector(DATABASE).await.is_none(),
        "production pre-builds Airhouse only; nothing is wrapped"
    );
    let mut metadata = json!({ "agent_id": "analytics" });
    stamp(ctx.as_ref(), &mut metadata);
    assert!(metadata.get(RUN_STAMP).is_none(), "{metadata}");
    assert!(held_rows(&t).await.is_empty());
}

#[tokio::test]
async fn a_staging_ask_from_someone_who_may_not_open_staging_is_404() {
    let t = seeded_tenant().await;
    let slug = "stg-ask-404";
    app(&t, slug).await;
    // The guest owns the org but is not Oxy staff: no `AppNonProduction`.

    let refused = ask_scope(
        &t.db,
        &on_host(&staging_host(&t, slug)),
        &guest(&t),
        demo_workspace_id(),
    )
    .await
    .expect_err("not staff");
    assert_eq!(refused.status(), StatusCode::NOT_FOUND);
    let refused = body(refused).await;
    assert_eq!(refused["error"], "EnvironmentRefused", "{refused}");
    assert_eq!(refused["environment"], "staging");

    // Staff, but the app was not published from this workspace.
    make_guest_staff();
    let other_workspace = ask_scope(
        &t.db,
        &on_host(&staging_host(&t, slug)),
        &guest(&t),
        Uuid::new_v4(),
    )
    .await
    .expect_err("another workspace's app");
    assert_eq!(other_workspace.status(), StatusCode::NOT_FOUND);
    assert!(held_rows(&t).await.is_empty());
}

/// On the staging host the host names the app: an `x-oxy-app` header naming
/// another app of the same workspace does not relabel the ask.
#[tokio::test]
async fn the_staging_host_names_the_ask_app_over_the_header() {
    let t = seeded_tenant().await;
    let slug = "stg-ask-host";
    let host_app = app(&t, slug).await;
    let other_app = app(&t, "stg-ask-other").await;
    make_guest_staff();

    let mut headers = on_host(&staging_host(&t, slug));
    headers.insert(
        "x-oxy-app",
        HeaderValue::from_str(&other_app.to_string()).unwrap(),
    );
    let hold = ask_scope(&t.db, &headers, &guest(&t), demo_workspace_id())
        .await
        .ok()
        .flatten()
        .expect("a staging hold");
    assert_eq!(hold.app_id(), Some(host_app));
}
