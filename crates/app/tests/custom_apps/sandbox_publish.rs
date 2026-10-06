//! Publishing to a sandbox (`internal-docs/custom-app-sandboxes.md` §5.2):
//! `environment=dev-<handle>` moves that sandbox's pointer and nothing else —
//! never the app row, staging, production or a schedule — and the semantic pin
//! its build carries. What it refuses is `sandbox_publish_refusals`'; its
//! migrations and its builds' retention, `sandbox_publish_migrations`'; the
//! multipart route, `sandbox_publish_route`'s.

use entity::{app_builds, app_environments, apps};
use oxy_app::server::api::custom_apps_publish::{
    OrgRef, PublishError, PublishInput, PublishResult, PublishTarget, publish_to,
};
use oxy_app::server::api::custom_apps_sandboxes::ops;
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_auth::user::LOCAL_GUEST_EMAIL;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection, EntityTrait, QueryFilter,
    QueryOrder, Statement,
};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{
    BUILD_ID, FunctionSpec, Tenant, publish_app, seeded_tenant,
};
use crate::custom_apps_publish_function_artifacts::tar_gz;
use crate::staging_functions::make_guest_staff;

const INDEX_HTML: &[u8] =
    b"<!doctype html><html><head><title>sandbox</title></head><body></body></html>";
const NOOP_JS: &str = "export default async () => Response.json({});";

pub(crate) fn sandbox(handle: &str) -> AppEnvironment {
    AppEnvironment::Dev {
        handle: handle.into(),
    }
}

pub(crate) fn noop(name: &'static str, manifest: Value) -> FunctionSpec {
    FunctionSpec {
        name,
        manifest,
        js: NOOP_JS,
    }
}

/// A bundle of `functions`, with `extra` merged into its `oxy-app.json` and
/// `files` beside it.
pub(crate) fn bundle(
    slug: &str,
    functions: &[FunctionSpec],
    extra: Value,
    files: &[(&str, &[u8])],
) -> Vec<u8> {
    let declared: serde_json::Map<String, Value> = functions
        .iter()
        .map(|f| (f.name.to_string(), f.manifest.clone()))
        .collect();
    let mut manifest = json!({ "schemaVersion": 2, "slug": slug, "functions": declared });
    if let (Some(manifest), Some(extra)) = (manifest.as_object_mut(), extra.as_object()) {
        manifest.extend(extra.clone());
    }
    let manifest = manifest.to_string();
    let artifacts: Vec<(String, &[u8])> = functions
        .iter()
        .map(|f| (format!("functions/{}.js", f.name), f.js.as_bytes()))
        .collect();
    let mut all: Vec<(&str, &[u8])> = vec![
        ("index.html", INDEX_HTML),
        ("oxy-app.json", manifest.as_bytes()),
    ];
    all.extend(artifacts.iter().map(|(path, js)| (path.as_str(), *js)));
    all.extend_from_slice(files);
    tar_gz(&all)
}

/// A draft publish of `tarball` as `build_id`, by the guest.
pub(crate) fn input(t: &Tenant, slug: &str, build_id: &str, tarball: Vec<u8>) -> PublishInput {
    PublishInput {
        org_ref: Some(OrgRef::Id(t.org_id)),
        app_slug: slug.to_string(),
        project_id: demo_workspace_id(),
        branch: None,
        build_id: build_id.to_string(),
        name: None,
        promote: false,
        tarball,
        manifest: None,
        source_repo: None,
        commit_sha: None,
        published_by: Some(t.guest_id),
        published_by_email: Some(LOCAL_GUEST_EMAIL.to_string()),
        publisher: Some(oxy_app::server::authz::Caller::without_credential(
            t.guest_id,
            LOCAL_GUEST_EMAIL,
        )),
        machine_app_id: None,
        published_via: None,
        semantic_revision_id: None,
    }
}

/// Publish a no-op build `build_id` to the sandbox `handle`.
pub(crate) async fn publish_to_sandbox(
    t: &Tenant,
    slug: &str,
    build_id: &str,
    handle: &str,
) -> Result<PublishResult, PublishError> {
    let tarball = bundle(
        slug,
        &[noop("noop", json!({ "route": true }))],
        json!({}),
        &[],
    );
    publish_to(
        input(t, slug, build_id, tarball),
        PublishTarget::Sandbox(sandbox(handle)),
    )
    .await
}

pub(crate) async fn app_row(db: &DatabaseConnection, id: Uuid) -> apps::Model {
    apps::Entity::find_by_id(id)
        .one(db)
        .await
        .expect("read the app")
        .expect("the app")
}

pub(crate) async fn build_pk(db: &DatabaseConnection, app: Uuid, build_id: &str) -> Option<Uuid> {
    app_builds::Entity::find()
        .filter(app_builds::Column::AppId.eq(app))
        .filter(app_builds::Column::BuildId.eq(build_id))
        .one(db)
        .await
        .expect("read the build")
        .map(|build| build.id)
}

async fn environment_row(
    db: &DatabaseConnection,
    app: Uuid,
    name: &str,
) -> app_environments::Model {
    app_environments::Entity::find_by_id((app, name.to_string()))
        .one(db)
        .await
        .expect("read the environment")
        .unwrap_or_else(|| panic!("no {name} row"))
}

/// Everything a sandbox publish must leave byte-identical: the app row, every
/// environment row but the sandbox published to, and the workspace's
/// schedules.
async fn untouched(db: &DatabaseConnection, app: Uuid, published_to: &str) -> String {
    let rows = app_environments::Entity::find()
        .filter(app_environments::Column::AppId.eq(app))
        .filter(app_environments::Column::Name.ne(published_to))
        .order_by_asc(app_environments::Column::Name)
        .all(db)
        .await
        .expect("read the environments");
    let mut schedules = agentic_pipeline::scheduler::list_schedules(db, demo_workspace_id())
        .await
        .expect("read the schedules");
    schedules.sort_by(|a, b| a.id.cmp(&b.id));
    format!("{:#?}\n{rows:#?}\n{schedules:#?}", app_row(db, app).await)
}

pub(crate) async fn count(db: &DatabaseConnection, sql: &str, app: Uuid) -> i64 {
    db.query_one_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        [app.to_string().into()],
    ))
    .await
    .expect("count")
    .expect("a row")
    .try_get("", "n")
    .expect("n")
}

pub(crate) async fn queued(db: &DatabaseConnection, app: Uuid, kind: &str) -> i64 {
    count(
        db,
        &format!(
            "SELECT count(*) AS n FROM agentic_task_queue \
             WHERE spec->>'kind' = '{kind}' AND spec->'payload'->>'app_id' = $1"
        ),
        app,
    )
    .await
}

/// A published app whose production and staging serve [`BUILD_ID`] with an
/// hourly function, the guest made staff, and two sandboxes.
pub(crate) async fn app_with_two_sandboxes(t: &Tenant, slug: &str) -> apps::Model {
    let hourly = noop("tick", json!({ "schedule": "0 * * * *" }));
    let app = publish_app(t, slug, demo_workspace_id(), &[hourly])
        .await
        .app_id;
    make_guest_staff();
    // SAFETY: nextest runs each test in its own process.
    unsafe { std::env::set_var("OXY_API_URL", "https://app-dev.oxygen-hq.com") };
    let app = app_row(&t.db, app).await;
    for handle in ["a1", "b2"] {
        ops::create(&t.db, &app, &sandbox(handle), &t.guest())
            .await
            .expect("create the sandbox");
    }
    app
}

/// A publish with `environment` moves that sandbox's pointer and writes its
/// event — and the app row (`draft_build_id`, `name`, `branch`,
/// `last_synced_at`), the staging and production rows, the other sandbox and
/// the schedules are exactly what they were, though the bundle renames the
/// app, names another branch and re-times the scheduled function.
#[tokio::test]
async fn a_sandbox_publish_moves_that_sandbox_and_nothing_else() {
    let t = seeded_tenant().await;
    let app = app_with_two_sandboxes(&t, "sbx-pub").await;
    let before = untouched(&t.db, app.id, "dev-a1").await;
    let staging_tasks = queued(&t.db, app.id, "custom_app_staging_migrations").await;

    let retimed = noop("tick", json!({ "schedule": "*/5 * * * *" }));
    let added = noop("extra", json!({ "schedule": "0 0 * * *" }));
    let mut publish = input(
        &t,
        "sbx-pub",
        "sbx-1",
        bundle("sbx-pub", &[retimed, added], json!({}), &[]),
    );
    publish.name = Some("Renamed by a sandbox".into());
    publish.branch = Some("feature/sandbox".into());
    let result = publish_to(publish, PublishTarget::Sandbox(sandbox("a1")))
        .await
        .expect("publish to dev-a1");

    assert_eq!(result.channel, "sandbox");
    assert_eq!(result.environment, "dev-a1");
    assert_eq!(
        result.environment_url.as_deref(),
        Some("https://dev-a1--local--sbx-pub.customer-apps-dev.oxygen-hq.com/")
    );
    assert_eq!((result.app_id, result.is_new_app), (app.id, false));
    let build = build_pk(&t.db, app.id, "sbx-1")
        .await
        .expect("the build is recorded");
    let moved = environment_row(&t.db, app.id, "dev-a1").await;
    assert_eq!(moved.build_id, Some(build));
    assert_eq!(moved.updated_by, Some(t.guest_id));
    assert_eq!(moved.deleting_at, None);

    assert_eq!(
        untouched(&t.db, app.id, "dev-a1").await,
        before,
        "the app row, the other environments and the schedules are untouched"
    );
    let events = "SELECT count(*) AS n FROM app_environment_events \
                  WHERE app_id::text = $1 AND build_id = (SELECT id FROM app_builds \
                    WHERE app_id::text = $1 AND build_id = 'sbx-1')";
    assert_eq!(count(&t.db, events, app.id).await, 1, "one event: dev-a1's");
    assert_eq!(
        queued(&t.db, app.id, "custom_app_staging_migrations").await,
        staging_tasks,
        "staging's homes are not migrated by a sandbox publish"
    );
}

/// A semantic pin rides the sandbox's own build: the sandbox shows it, and
/// staging's build keeps none.
#[tokio::test]
async fn a_semantic_pin_rides_the_sandboxs_build() {
    let t = seeded_tenant().await;
    let app = app_with_two_sandboxes(&t, "sbx-pin").await;
    let revision =
        crate::sandbox_environments::seed_staging_revision(&t.db, demo_workspace_id(), "c0ffee")
            .await;
    let tarball = bundle(
        "sbx-pin",
        &[noop("noop", json!({ "route": true }))],
        json!({}),
        &[],
    );
    let mut publish = input(&t, "sbx-pin", "pinned-1", tarball);
    publish.semantic_revision_id = Some(revision);
    publish_to(publish, PublishTarget::Sandbox(sandbox("a1")))
        .await
        .expect("publish a pinned build to dev-a1");

    let shown = ops::get(&t.db, &app, &t.org_slug, &sandbox("a1"))
        .await
        .expect("show dev-a1");
    assert_eq!(shown.build_id.as_deref(), Some("pinned-1"));
    assert_eq!(shown.semantic_revision_id, Some(revision));
    let staging = ops::get(&t.db, &app, &t.org_slug, &AppEnvironment::Staging)
        .await
        .expect("show staging");
    assert_eq!(staging.build_id.as_deref(), Some(BUILD_ID));
    assert_eq!(staging.semantic_revision_id, None);
    let untouched = ops::get(&t.db, &app, &t.org_slug, &sandbox("b2"))
        .await
        .expect("show dev-b2");
    assert_eq!(untouched.build_id, None);
}
