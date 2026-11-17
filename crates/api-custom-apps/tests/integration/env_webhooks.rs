//! Webhooks outside production
//! (`internal-docs/2026-09-10-custom-app-environments-design.md` §4.5,
//! ruling 2): `POST /api/webhooks/apps-env/{environment}/{org}/{app}/{fn}`,
//! driven through its handler as the router mounts it, beside production's.
//!
//! - a delivery verifies against the environment's own secret and nothing
//!   else — staging's key does not open a sandbox, production's does not
//!   open staging — and a bad signature is refused before anything is queued;
//! - every unknown endpoint is the same `404`: production, a name nobody
//!   created, a deleting or expired sandbox, a function with no `webhook`
//!   block in that environment's build;
//! - an accepted delivery is queued naming its environment, and the runner
//!   runs it there as a `webhook` invocation;
//! - production's route is unchanged: its own key, a task naming no
//!   environment.

use axum::body::Bytes;
use axum::extract::Path;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use entity::apps;
use hmac::{Hmac, KeyInit, Mac};
use oxy::service::secret_manager::SecretManagerService;
use oxy_api_custom_apps::custom_apps_sandboxes::{TeardownReason, ops};
use oxy_api_custom_apps::webhooks::app_function::app_function_webhook;
use oxy_api_custom_apps::webhooks::app_function_env::app_function_env_webhook;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use serde_json::{Value, json};
use sha2::Sha256;
use uuid::Uuid;

use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{
    BUILD_ID, FunctionSpec, Tenant, invocations, promote_staging, publish_build, seeded_tenant,
};
use crate::env_schedules::{publish_sandbox, run_spec};
use crate::sandbox_publish::{app_row, sandbox};
use crate::staging_functions::make_guest_staff;

const HEADER: &str = "x-sig";
const HOOK_JS: &str =
    "export default async (req, ctx) => Response.json({ channel: ctx.channel, body: req.body });";
const BODY: &[u8] = br#"{"event":"order.created"}"#;

fn hook() -> FunctionSpec {
    FunctionSpec {
        name: "hook",
        manifest: json!({ "webhook": { "secretVar": "SIGNING", "signatureHeader": HEADER } }),
        js: HOOK_JS,
    }
}

fn route_only() -> FunctionSpec {
    FunctionSpec {
        name: "hook",
        manifest: json!({ "route": true }),
        js: HOOK_JS,
    }
}

fn use_a_fixed_encryption_key() {
    // SAFETY: nextest runs each test in its own process.
    unsafe {
        std::env::set_var(
            "OXY_ENCRYPTION_KEY",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
        );
    }
}

async fn set_secret(t: &Tenant, app: Uuid, environment: Option<&str>, value: &str) {
    let secrets = SecretManagerService::new(demo_workspace_id());
    secrets
        .set_app_secret_in(&t.db, app, environment, "SIGNING", value, t.guest_id)
        .await
        .expect("set the signing secret");
    secrets.clear_cache().await;
}

/// An app whose production, staging and `dev-a1` builds each declare the
/// `hook` webhook, production holding `prod-key` and staging `stg-key`; the
/// sandbox holds none of its own.
async fn hooked_app(t: &Tenant, slug: &str) -> apps::Model {
    use_a_fixed_encryption_key();
    // The hook's signing key is required by construction, and production
    // refuses a build declaring a required key it holds no value for — a
    // promoting publish included. So: publish, set production's key, promote.
    let app_id = publish_build(t, slug, demo_workspace_id(), BUILD_ID, false, &[hook()])
        .await
        .app_id;
    set_secret(t, app_id, None, "prod-key").await;
    promote_staging(t, app_id).await;
    make_guest_staff();
    let app = app_row(&t.db, app_id).await;
    ops::create(&t.db, &app, &sandbox("a1"), &t.guest())
        .await
        .expect("create the sandbox");
    publish_sandbox(t, slug, "sbx-1", "a1", &[hook()]).await;
    set_secret(t, app.id, Some("staging"), "stg-key").await;
    app
}

fn signed(key: &str) -> HeaderMap {
    let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes()).expect("a key");
    mac.update(BODY);
    let mut headers = HeaderMap::new();
    headers.insert(
        HeaderName::from_static(HEADER),
        HeaderValue::from_str(&hex::encode(mac.finalize().into_bytes())).expect("hex"),
    );
    headers
}

/// A delivery to the environment route: its status, and the refusal's text.
async fn deliver(t: &Tenant, environment: &str, slug: &str, key: &str) -> (StatusCode, String) {
    let path = Path((
        environment.to_string(),
        t.org_slug.clone(),
        slug.to_string(),
        "hook".to_string(),
    ));
    match app_function_env_webhook(path, signed(key), Bytes::from_static(BODY)).await {
        Ok(status) => (status, String::new()),
        Err(refused) => refused,
    }
}

async fn deliver_to_production(t: &Tenant, slug: &str, key: &str) -> (StatusCode, String) {
    let path = Path((t.org_slug.clone(), slug.to_string(), "hook".to_string()));
    match app_function_webhook(path, signed(key), Bytes::from_static(BODY)).await {
        Ok(status) => (status, String::new()),
        Err(refused) => refused,
    }
}

/// The webhook tasks queued for `app`, as `(environment, spec)`; production's
/// task names none.
async fn queued_webhooks(db: &DatabaseConnection, app: Uuid) -> Vec<(Option<String>, Value)> {
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT spec FROM agentic_task_queue \
              WHERE spec->>'kind' = 'app_function' AND spec->'payload'->>'app_id' = $1 \
                AND spec->'payload'->>'trigger' = 'webhook' AND queue_status = 'queued'",
            [app.to_string().into()],
        ))
        .await
        .expect("read the queue");
    rows.into_iter()
        .map(|row| {
            let spec: Value = row.try_get("", "spec").expect("spec");
            let environment = spec["payload"]["environment"].as_str().map(str::to_string);
            (environment, spec)
        })
        .collect()
}

/// Staging verifies with staging's own key, queues a task naming staging, and
/// the runner runs it there as a `webhook` invocation. Production's key is a
/// mismatch on staging's URL, refused before anything is queued.
#[tokio::test]
async fn a_staging_delivery_verifies_with_stagings_own_secret_and_runs_in_staging() {
    let t = seeded_tenant().await;
    let app = hooked_app(&t, "env-hook-stg").await;

    let (status, said) = deliver(&t, "staging", "env-hook-stg", "prod-key").await;
    assert_eq!(
        (status, said.as_str()),
        (StatusCode::UNAUTHORIZED, "signature mismatch")
    );
    assert!(
        queued_webhooks(&t.db, app.id).await.is_empty(),
        "refused before queueing"
    );

    let (status, said) = deliver(&t, "staging", "env-hook-stg", "stg-key").await;
    assert_eq!(status, StatusCode::ACCEPTED, "{said}");
    let mut queued = queued_webhooks(&t.db, app.id).await;
    assert_eq!(queued.len(), 1);
    let (environment, spec) = queued.remove(0);
    assert_eq!(environment.as_deref(), Some("staging"));

    let spec = serde_json::from_value(spec).expect("a TaskSpec");
    let agentic_core::delegation::TaskOutcome::Done { answer, .. } = run_spec(&t.db, spec).await
    else {
        panic!("the staging delivery ran");
    };
    let answer: Value = serde_json::from_str(&answer).expect("JSON");
    assert_eq!(answer["channel"], "staging");
    let rows = invocations(&t.db, app.id, "hook").await;
    let seen: Vec<(&str, &str)> = rows
        .iter()
        .map(|r| (r.environment.as_str(), r.mode.as_str()))
        .collect();
    assert_eq!(seen, vec![("staging", "webhook")]);
}

/// A sandbox reads no secret but its own: staging's key, which `ctx.env`
/// would fall back to, does not verify there. Unset is `401`; set, it opens.
#[tokio::test]
async fn a_sandbox_delivery_has_no_secret_fallback() {
    let t = seeded_tenant().await;
    let app = hooked_app(&t, "env-hook-sbx").await;
    for key in ["stg-key", "prod-key"] {
        let (status, said) = deliver(&t, "dev-a1", "env-hook-sbx", key).await;
        assert_eq!(
            (status, said.as_str()),
            (
                StatusCode::UNAUTHORIZED,
                "webhook signing secret is not configured"
            ),
            "{key}"
        );
    }
    set_secret(&t, app.id, Some("dev-a1"), "a1-key").await;
    let (status, said) = deliver(&t, "dev-a1", "env-hook-sbx", "stg-key").await;
    assert_eq!(
        (status, said.as_str()),
        (StatusCode::UNAUTHORIZED, "signature mismatch")
    );
    let (status, said) = deliver(&t, "dev-a1", "env-hook-sbx", "a1-key").await;
    assert_eq!(status, StatusCode::ACCEPTED, "{said}");
    let environments: Vec<Option<String>> = queued_webhooks(&t.db, app.id)
        .await
        .into_iter()
        .map(|(environment, _)| environment)
        .collect();
    assert_eq!(environments, vec![Some("dev-a1".to_string())]);
}

/// Everything that is not an open endpoint is the same `404`, signature or
/// not — and nothing is queued.
#[tokio::test]
async fn every_unknown_endpoint_is_the_same_404() {
    let t = seeded_tenant().await;
    let app = hooked_app(&t, "env-hook-404").await;
    set_secret(&t, app.id, Some("dev-a1"), "a1-key").await;
    ops::create(&t.db, &app, &sandbox("b2"), &t.guest())
        .await
        .expect("create a sandbox with no build");
    let no_such = (StatusCode::NOT_FOUND, "no such webhook".to_string());
    for environment in ["production", "Production", "nope", "dev-zz", "dev-b2"] {
        assert_eq!(
            deliver(&t, environment, "env-hook-404", "prod-key").await,
            no_such,
            "{environment}"
        );
    }

    // A function whose webhook block production's build has and staging's
    // does not: staging's build decides.
    crate::custom_app_functions_fixture::publish_build(
        &t,
        "env-hook-404",
        demo_workspace_id(),
        "stg-2",
        false,
        &[route_only()],
    )
    .await;
    assert_eq!(
        deliver(&t, "staging", "env-hook-404", "stg-key").await,
        no_such
    );

    // A sandbox past its idle expiry, then one being deleted.
    expire(&t.db, app.id, "dev-a1").await;
    assert_eq!(
        deliver(&t, "dev-a1", "env-hook-404", "a1-key").await,
        no_such
    );
    ops::begin_delete(
        &t.db,
        &app,
        &sandbox("a1"),
        Some(&t.guest()),
        TeardownReason::Deleted,
    )
    .await
    .expect("delete dev-a1");
    assert_eq!(
        deliver(&t, "dev-a1", "env-hook-404", "a1-key").await,
        no_such
    );
    assert!(queued_webhooks(&t.db, app.id).await.is_empty());
}

/// Production's route is what it was: its own key, a task naming no
/// environment; staging's key is a mismatch there.
#[tokio::test]
async fn production_webhooks_are_unchanged() {
    let t = seeded_tenant().await;
    let app = hooked_app(&t, "env-hook-prod").await;
    let (status, said) = deliver_to_production(&t, "env-hook-prod", "stg-key").await;
    assert_eq!(
        (status, said.as_str()),
        (StatusCode::UNAUTHORIZED, "signature mismatch")
    );
    let (status, said) = deliver_to_production(&t, "env-hook-prod", "prod-key").await;
    assert_eq!(status, StatusCode::ACCEPTED, "{said}");
    let environments: Vec<Option<String>> = queued_webhooks(&t.db, app.id)
        .await
        .into_iter()
        .map(|(environment, _)| environment)
        .collect();
    assert_eq!(
        environments,
        vec![None],
        "production's task names no environment"
    );
}

async fn expire(db: &DatabaseConnection, app: Uuid, name: &str) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE app_environments SET updated_at = now() - interval '30 days' \
          WHERE app_id = $1 AND name = $2",
        [app.into(), name.into()],
    ))
    .await
    .expect("age the sandbox");
}
