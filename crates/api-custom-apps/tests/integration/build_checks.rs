//! Promote checks, phase 1 (`custom_apps_build_checks`), against a real
//! database, the filesystem build store and a fake GitHub on a loopback port:
//!
//! - a publish that moves staging queues ONE checks task for the build, with
//!   its four `queued` rows, before it answers; queueing that attempt again
//!   adds nothing; staging rolled back to a build queues its checks too;
//! - the executor production registers only starts the runner: it sends one
//!   workflow dispatch and writes no verdict; a deployment that starts none
//!   leaves the checks queued; a dispatch GitHub refuses closes the attempt;
//! - a person with non-production reach drives the four runner routes, and
//!   each answers the contract's shape; their runner is named by the platform
//!   from who called, never by the label they sent; a caller without reach
//!   and a publish token get the not-found an unknown build gets;
//! - a claim starts the build's check functions once, and a settle writes
//!   their verdict from the runs; a claim after staging moved skips what
//!   needs staging; a verdict already written is never overwritten;
//! - an attempt nobody answers reads `failed` and can be run again at once;
//!   a runner that claims again is measured from its own claim;
//! - an attempt of a deployment that starts no runner says so: its checks
//!   read `skipped` at once, it never stalls, a claim by hand takes it, and a
//!   rerun is not refused.

use agentic_core::delegation::{TaskAssignment, TaskOutcome, TaskSpec};
use agentic_runtime::worker::TaskExecutor;
use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Query};
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use entity::app_build_checks;
use entity::users::UserStatus;
use oxy_api_custom_apps::custom_apps_build_checks::BUILD_CHECKS_KIND;
use oxy_api_custom_apps::custom_apps_build_checks::error::ChecksError;
use oxy_api_custom_apps::custom_apps_build_checks::executor::{BuildChecksExecutor, run_guarded};
use oxy_api_custom_apps::custom_apps_build_checks::handlers::{self, ShowQuery};
use oxy_api_custom_apps::custom_apps_build_checks::queue::{self, RerunRefusal};
use oxy_api_custom_apps::custom_apps_build_checks::requester::Requester;
use oxy_api_custom_apps::custom_apps_build_checks::runner::handlers as runner;
use oxy_api_custom_apps::custom_apps_build_checks::stalled::NotStarted;
use oxy_api_custom_apps::custom_apps_build_checks::store;
use oxy_api_custom_apps::custom_apps_build_checks::task::BuildChecksTask;
use oxy_api_custom_apps::custom_apps_publish::{OrgRef, PublishInput, PublishResult, publish};
use oxy_app_core::agent_refusal::RefuseSandboxAgent;
use oxy_app_core::audit::RequestActor;
use oxy_app_core::config::AppChecksConfig;
use oxy_auth::types::{AppPublishTokenAuth, AuthenticatedUser};
use oxy_auth::user::LOCAL_GUEST_EMAIL;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection, EntityTrait, QueryFilter,
    Statement,
};
use serde_json::{Value, json};
use uuid::Uuid;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{
    FunctionSpec, ORG_SLUG, Tenant, publish_build, seeded_tenant,
};
use crate::custom_apps_publish_function_artifacts::tar_gz;

/// A bundle whose `index.html` restates `--card` in a `<style>` block, beside
/// a stylesheet under `assets/`.
const INDEX: &[u8] = b"<!doctype html>\n<html><head><title>c</title>\n<style>\n:root { --card: #fff }\n</style></head><body></body></html>";
const CSS: &[u8] = b".panel{padding:4px}";

async fn publish_staging(t: &Tenant, slug: &str, build_id: &str) -> PublishResult {
    let manifest = json!({ "schemaVersion": 2, "slug": slug }).to_string();
    let files: Vec<(&str, &[u8])> = vec![
        ("index.html", INDEX),
        ("assets/app.css", CSS),
        ("oxy-app.json", manifest.as_bytes()),
    ];
    publish(PublishInput {
        org_ref: Some(OrgRef::Id(t.org_id)),
        app_slug: slug.to_string(),
        project_id: demo_workspace_id(),
        branch: None,
        build_id: build_id.to_string(),
        name: None,
        promote: false,
        tarball: tar_gz(&files),
        manifest: None,
        source_repo: None,
        commit_sha: None,
        published_by: Some(t.guest_id),
        published_by_email: Some(LOCAL_GUEST_EMAIL.to_string()),
        publisher: Some(oxy_server_authz::Caller::without_credential(
            t.guest_id,
            LOCAL_GUEST_EMAIL,
        )),
        machine_app_id: None,
        published_via: None,
        semantic_revision_id: None,
        confirm_schedule_removals: Vec::new(),
        sdk_version: Some("2.30.0".to_string()),
    })
    .await
    .expect("the bundle publishes")
}

/// The app's queued build-checks tasks, oldest first.
async fn queued(db: &DatabaseConnection, app_id: Uuid) -> Vec<(String, TaskSpec)> {
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT task_id, spec FROM agentic_task_queue \
             WHERE queue_status = 'queued' AND spec->>'kind' = $1 \
               AND spec->'payload'->>'app_id' = $2 \
             ORDER BY created_at, task_id",
            [BUILD_CHECKS_KIND.into(), app_id.to_string().into()],
        ))
        .await
        .expect("read the queue");
    rows.iter()
        .map(|row| {
            let task_id: String = row.try_get("", "task_id").expect("task_id");
            let spec: serde_json::Value = row.try_get("", "spec").expect("spec");
            (task_id, serde_json::from_value(spec).expect("a TaskSpec"))
        })
        .collect()
}

/// Run one queued task through the registered executor, as a worker would.
async fn run_one(db: &DatabaseConnection, task_id: &str, spec: TaskSpec) -> TaskOutcome {
    let mut running = BuildChecksExecutor { db: db.clone() }
        .execute(TaskAssignment {
            task_id: task_id.to_string(),
            parent_task_id: None,
            run_id: task_id.to_string(),
            spec,
            policy: None,
            claim_epoch: agentic_core::NO_CLAIM,
        })
        .await
        .expect("the executor takes a build checks task");
    running.outcomes.recv().await.expect("an outcome")
}

async fn rows(db: &DatabaseConnection, build_pk: Uuid) -> Vec<app_build_checks::Model> {
    let mut rows = app_build_checks::Entity::find()
        .filter(app_build_checks::Column::BuildId.eq(build_pk))
        .all(db)
        .await
        .expect("read the rows");
    rows.sort_by_key(|r| (r.attempt, r.kind.clone()));
    rows
}

/// `(kind, status)` of one attempt, by kind.
async fn statuses(db: &DatabaseConnection, build_pk: Uuid, attempt: i32) -> Vec<(String, String)> {
    rows(db, build_pk)
        .await
        .into_iter()
        .filter(|r| r.attempt == attempt)
        .map(|r| (r.kind, r.status))
        .collect()
}

async fn row_of(db: &DatabaseConnection, build_pk: Uuid, kind: &str) -> app_build_checks::Model {
    rows(db, build_pk)
        .await
        .into_iter()
        .rfind(|r| r.kind == kind)
        .expect(kind)
}

async fn models(
    t: &Tenant,
    published: &PublishResult,
) -> (entity::apps::Model, entity::app_builds::Model) {
    let app = entity::apps::Entity::find_by_id(published.app_id)
        .one(&t.db)
        .await
        .unwrap()
        .expect("the app");
    let build = entity::app_builds::Entity::find()
        .filter(entity::app_builds::Column::AppId.eq(published.app_id))
        .filter(entity::app_builds::Column::BuildId.eq(published.build_id.clone()))
        .one(&t.db)
        .await
        .unwrap()
        .expect("the build");
    (app, build)
}

async fn sql(db: &DatabaseConnection, statement: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        statement,
        values,
    ))
    .await
    .unwrap_or_else(|e| panic!("{statement}: {e}"));
}

fn actor(id: Uuid, email: &str) -> RequestActor {
    RequestActor::session(AuthenticatedUser {
        id,
        email: Some(email.to_string()),
        name: "Developer".to_string(),
        picture: None,
        credential: None,
        status: UserStatus::Active,
    })
}

/// The guest made Oxy staff (the `OXY_OWNER` list, set after the seed, which
/// writes its own): the one caller here with non-production reach.
fn staff(t: &Tenant) -> RequestActor {
    // SAFETY: nextest runs each test in its own process.
    unsafe { std::env::set_var("OXY_OWNER", LOCAL_GUEST_EMAIL) };
    actor(t.guest_id, LOCAL_GUEST_EMAIL)
}

type Marker = Option<axum::Extension<AppPublishTokenAuth>>;

/// One caller of the runner routes on one build.
struct Runner {
    actor: RequestActor,
    marker: Marker,
    app: Uuid,
    build: String,
}

/// A refusal as its status and body.
async fn refusal(e: ChecksError) -> (u16, Value) {
    let response = e.into_response();
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("read the body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn answer<T: serde::Serialize>(
    result: Result<Json<T>, ChecksError>,
) -> Result<Value, (u16, Value)> {
    match result {
        Ok(Json(body)) => Ok(serde_json::to_value(body).expect("an answer")),
        Err(e) => Err(refusal(e).await),
    }
}

impl Runner {
    fn of(actor: RequestActor, app: Uuid, build: &str) -> Self {
        Self {
            actor,
            marker: None,
            app,
            build: build.to_string(),
        }
    }

    fn path(&self) -> Path<(Uuid, String)> {
        Path((self.app, self.build.clone()))
    }

    fn body(body: Value) -> Bytes {
        Bytes::from(body.to_string())
    }

    async fn claim(&self, body: Value, headers: HeaderMap) -> Result<Value, (u16, Value)> {
        let (actor, marker) = (self.actor.clone(), self.marker.clone());
        let body = Self::body(body);
        answer(
            runner::claim(
                RefuseSandboxAgent,
                Requester { actor, marker },
                self.path(),
                headers,
                body,
            )
            .await,
        )
        .await
    }

    async fn settle(&self, body: Value) -> Result<Value, (u16, Value)> {
        let (actor, marker) = (self.actor.clone(), self.marker.clone());
        let body = Self::body(body);
        answer(runner::settle(RefuseSandboxAgent, actor, marker, self.path(), body).await).await
    }

    async fn stylesheets(&self) -> Result<Value, (u16, Value)> {
        let (actor, marker) = (self.actor.clone(), self.marker.clone());
        answer(runner::stylesheets(RefuseSandboxAgent, actor, marker, self.path()).await).await
    }

    async fn report(&self, body: Value) -> Result<Value, (u16, Value)> {
        let (actor, marker) = (self.actor.clone(), self.marker.clone());
        let body = Self::body(body);
        let by = Requester { actor, marker };
        let headers = HeaderMap::new();
        answer(runner::report(RefuseSandboxAgent, by, self.path(), headers, body).await).await
    }
}

fn by_hand() -> Value {
    json!({ "attempt": null, "runner": { "label": "by hand", "url": null } })
}

/// How the platform names the runner of a claim by [`staff`]: from who
/// called, whatever label the claim sent.
fn hand_label() -> String {
    format!("{LOCAL_GUEST_EMAIL}, by hand")
}

/// Every check of a fresh attempt, by kind.
const ALL_QUEUED: [(&str, &str); 4] = [
    ("design_system", "queued"),
    ("functions", "queued"),
    ("loads", "queued"),
    ("validation", "queued"),
];

fn pairs(expected: &[(&str, &str)]) -> Vec<(String, String)> {
    expected
        .iter()
        .map(|(kind, status)| (kind.to_string(), status.to_string()))
        .collect()
}

/// Move one attempt's rows `minutes` into the past.
async fn age_queue(db: &DatabaseConnection, build_pk: Uuid, attempt: i32, minutes: i32) {
    sql(
        db,
        "UPDATE app_build_checks SET created_at = created_at - make_interval(mins => $3) \
         WHERE build_id = $1 AND attempt = $2",
        vec![build_pk.into(), attempt.into(), minutes.into()],
    )
    .await;
}

/// Move one attempt's claim `minutes` into the past.
async fn age_claim(db: &DatabaseConnection, build_pk: Uuid, attempt: i32, minutes: i64) {
    let at = (chrono::Utc::now() - chrono::Duration::minutes(minutes)).to_rfc3339();
    sql(
        db,
        "UPDATE app_build_checks SET runner = jsonb_set(runner, '{started_at}', to_jsonb($3::text)) \
         WHERE build_id = $1 AND attempt = $2",
        vec![build_pk.into(), attempt.into(), at.into()],
    )
    .await;
}

async fn shown(t: &Tenant, app: Uuid, build: &str) -> Value {
    let Json(answer) = handlers::show(
        RefuseSandboxAgent,
        staff(t),
        None,
        Path((app, build.to_string())),
        Query(ShowQuery::default()),
    )
    .await
    .expect("staff read the checks");
    serde_json::to_value(answer).expect("the answer")
}

mod functions;
mod queueing;
mod runner_routes;
mod stalled;
mod starting;
