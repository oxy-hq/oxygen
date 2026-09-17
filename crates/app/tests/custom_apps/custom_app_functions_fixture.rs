//! Shared fixture for the `custom_app_functions_*` modules — no tests of its own.
//!
//! It publishes an app that declares Oxy Functions and calls them the way a
//! browser does. Each piece is the production one:
//!
//! - **Tenant.** `oxy seed`'s rows (`seed_demo`) in a per-test database with
//!   every migrator applied (`Schema::All`: a manual run writes `agentic_runs`
//!   and `agentic_task_queue`). The Local org's guest is made its **Owner** —
//!   publish admits an org Admin or above, and a system run (manual, schedule)
//!   executes as the org owner and fails when the org has none. A test whose
//!   side effects key on the org id takes an org of its own from
//!   [`throwaway_org`] instead of the Local org's fixed id.
//! - **Bundle.** An in-memory tar.gz of `index.html`, `oxy-app.json` and one
//!   `functions/<name>.js` per declared function: the layout `oxyc publish`
//!   uploads (`sdk/cli/src/publish/functions.ts` bundles each function to that
//!   path as ESM), and the key the runtime reads before calling the module's
//!   `export default async (req, ctx)`.
//! - **Publish.** The real `publish()` with `promote: true`, into the filesystem
//!   build store (`test_db` unsets `OXY_CUSTOMER_APPS_S3_BUCKET`).
//! - **Route.** `/customer-apps/{*path}` → `serve_dispatch` with the data-plane
//!   query executor extension `serve.rs` layers onto that route. Its other layers
//!   (32 MiB body limit, compression, `record_error_body`, the preagg extension)
//!   are left off: these calls send small bodies, no `Accept-Encoding`, and
//!   `serve_dispatch` defaults the preagg context. A function answers
//!   `POST /customer-apps/<org>/<app>/fn/<name>` with an SSE stream. This is a
//!   copy of production, and `custom_app_functions_manual_run_guards` pins both
//!   sides: the layers `serve.rs` stacks on that route, and the one kept here.
//! - **Identity.** `BuiltInAuthenticator` falls back to the guest when no auth
//!   method is configured, and `user_can_access_app` decides from its membership
//!   — the path `example_app_serving` takes.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::any;
use chrono::Utc;
use entity::prelude::{AppFunctionInvocations, Organizations};
use entity::{app_function_invocations, org_members, org_members::OrgRole, organizations};
use oxy_app::cli::commands::seed;
use oxy_app::server::api::custom_apps_functions::seam::FunctionQueryExecutor;
use oxy_app::server::api::custom_apps_publish::{OrgRef, PublishInput, PublishResult, publish};
use oxy_app::server::api::custom_apps_serve;
use oxy_app::server::api::projects::query::DataPlaneQueryExecutor;
use oxy_auth::types::Identity;
use oxy_auth::user::{LOCAL_GUEST_EMAIL, UserService};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
    QueryOrder,
};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

use crate::common::{Schema, examples_path, test_db_with};
use crate::custom_apps_publish_function_artifacts::tar_gz;

/// The org `seed_demo` creates, which every app here is published into.
pub(crate) const ORG_SLUG: &str = "local";
/// Each app is published once, so one build id serves them all.
pub(crate) const BUILD_ID: &str = "fn-e2e-1";

const INDEX_HTML: &[u8] =
    b"<!doctype html><html><head><title>fn e2e</title></head><body></body></html>";

/// The seeded database, an org (the Local org, or one from [`throwaway_org`]),
/// and its guest Owner.
pub(crate) struct Tenant {
    pub(crate) db: DatabaseConnection,
    pub(crate) org_id: Uuid,
    /// The `<org>` in `/customer-apps/<org>/<app>/…`.
    pub(crate) org_slug: String,
    pub(crate) guest_id: Uuid,
}

/// One declared function: its `oxy-app.json` entry and its bundled module.
pub(crate) struct FunctionSpec {
    pub(crate) name: &'static str,
    pub(crate) manifest: Value,
    pub(crate) js: &'static str,
}

pub(crate) async fn seeded_tenant() -> Tenant {
    let db = test_db_with(Schema::All).await;
    seed::seed_demo(Some(examples_path()))
        .await
        .expect("seed_demo");
    let org_id = Organizations::find()
        .filter(organizations::Column::Slug.eq(ORG_SLUG))
        .one(&db)
        .await
        .expect("query org")
        .expect("seed_demo creates the local org")
        .id;
    let guest_id = add_guest_as_owner(&db, org_id).await;
    Tenant {
        db,
        org_id,
        org_slug: ORG_SLUG.to_string(),
        guest_id,
    }
}

/// A fresh org in `t`'s database with the guest as its Owner, for a test whose
/// side effects key on the org id. The OLTP zoo provisions and then drops the
/// org's tenant database on the cluster `OXY_DATABASE_URL` names; keyed on the
/// Local org's fixed id, that would take a dev box's real tenant with it.
pub(crate) async fn throwaway_org(t: &Tenant) -> Tenant {
    let id = Uuid::new_v4();
    let slug = format!("fn-e2e-{}", &id.simple().to_string()[..8]);
    organizations::ActiveModel {
        id: ActiveValue::Set(id),
        name: ActiveValue::Set(slug.clone()),
        slug: ActiveValue::Set(slug.clone()),
        ..Default::default()
    }
    .insert(&t.db)
    .await
    .expect("seed a throwaway org");
    let guest_id = add_guest_as_owner(&t.db, id).await;
    Tenant {
        db: t.db.clone(),
        org_id: id,
        org_slug: slug,
        guest_id,
    }
}

async fn add_guest_as_owner(db: &DatabaseConnection, org_id: Uuid) -> Uuid {
    let guest = UserService::get_or_create_user(&Identity {
        // A fixture that must MINT its guest, so no id.
        user_id: None,
        email: LOCAL_GUEST_EMAIL.to_string(),
        name: Some("Local User".to_string()),
        picture: None,
    })
    .await
    .expect("guest user");
    let now = Utc::now().fixed_offset();
    org_members::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        org_id: ActiveValue::Set(org_id),
        user_id: ActiveValue::Set(guest.id),
        role: ActiveValue::Set(OrgRole::Owner),
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
    }
    .insert(db)
    .await
    .expect("guest owner membership");
    guest.id
}

/// The tar.gz `oxyc publish` would upload for an app declaring `functions`.
pub(crate) fn bundle(slug: &str, functions: &[FunctionSpec]) -> Vec<u8> {
    let declared: serde_json::Map<String, Value> = functions
        .iter()
        .map(|f| (f.name.to_string(), f.manifest.clone()))
        .collect();
    let manifest =
        serde_json::json!({ "schemaVersion": 2, "slug": slug, "functions": declared }).to_string();
    let artifacts: Vec<(String, &[u8])> = functions
        .iter()
        .map(|f| (format!("functions/{}.js", f.name), f.js.as_bytes()))
        .collect();
    let mut files: Vec<(&str, &[u8])> = vec![
        ("index.html", INDEX_HTML),
        ("oxy-app.json", manifest.as_bytes()),
    ];
    files.extend(artifacts.iter().map(|(path, js)| (path.as_str(), *js)));
    tar_gz(&files)
}

/// Publishes `functions` as app `slug` in `workspace`, promoted, as the guest.
pub(crate) async fn publish_app(
    t: &Tenant,
    slug: &str,
    workspace: Uuid,
    functions: &[FunctionSpec],
) -> PublishResult {
    publish(PublishInput {
        org_ref: Some(OrgRef::Id(t.org_id)),
        app_slug: slug.to_string(),
        project_id: workspace,
        branch: None,
        build_id: BUILD_ID.to_string(),
        name: None,
        promote: true,
        tarball: bundle(slug, functions),
        manifest: None,
        source_repo: None,
        commit_sha: None,
        published_by: Some(t.guest_id),
        published_by_email: Some(LOCAL_GUEST_EMAIL.to_string()),
        machine_app_id: None,
    })
    .await
    .expect("a bundle carrying every declared function publishes and promotes")
}

/// The custom-app serve route with the extension `serve.rs` layers onto it.
/// Without the query executor the dispatcher refuses every function with a 500.
fn serve_router() -> Router {
    Router::new().route(
        "/customer-apps/{*path}",
        any(custom_apps_serve::serve_dispatch)
            .layer(axum::Extension(
                Arc::new(DataPlaneQueryExecutor) as Arc<dyn FunctionQueryExecutor>
            )),
    )
}

/// One function call's response: its status, and its SSE frames as
/// `(event, data)` alongside the raw stream for failure messages.
pub(crate) struct FnCall {
    pub(crate) status: StatusCode,
    pub(crate) frames: Vec<(String, Value)>,
    pub(crate) raw: String,
}

impl FnCall {
    /// The `data` of the first frame named `event`.
    pub(crate) fn frame(&self, event: &str) -> Option<&Value> {
        self.frames
            .iter()
            .find(|(name, _)| name == event)
            .map(|(_, data)| data)
    }
}

/// `POST /customer-apps/local/<app>/fn/<name>` with `body`, as the guest.
pub(crate) async fn call_function(app_slug: &str, name: &str, body: Value) -> FnCall {
    call_function_in(ORG_SLUG, app_slug, name, body).await
}

/// [`call_function`] for an app published in the org `org_slug`.
pub(crate) async fn call_function_in(
    org_slug: &str,
    app_slug: &str,
    name: &str,
    body: Value,
) -> FnCall {
    let request = Request::builder()
        .method("POST")
        .uri(format!("/customer-apps/{org_slug}/{app_slug}/fn/{name}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("request");
    let response = serve_router().oneshot(request).await.expect("oneshot");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .expect("read body");
    let raw = String::from_utf8_lossy(&bytes).into_owned();
    FnCall {
        status,
        frames: parse_sse(&raw),
        raw,
    }
}

fn parse_sse(raw: &str) -> Vec<(String, Value)> {
    raw.split("\n\n")
        .filter_map(|frame| {
            let event = frame.lines().find_map(|l| l.strip_prefix("event: "))?;
            let data = frame.lines().find_map(|l| l.strip_prefix("data: "))?;
            Some((event.to_string(), serde_json::from_str(data).ok()?))
        })
        .collect()
}

/// The invocation rows finalization wrote for one function, oldest first.
pub(crate) async fn invocations(
    db: &DatabaseConnection,
    app_id: Uuid,
    function_name: &str,
) -> Vec<app_function_invocations::Model> {
    AppFunctionInvocations::find()
        .filter(app_function_invocations::Column::AppId.eq(app_id))
        .filter(app_function_invocations::Column::FunctionName.eq(function_name))
        .order_by_asc(app_function_invocations::Column::CreatedAt)
        .all(db)
        .await
        .expect("query app_function_invocations")
}
