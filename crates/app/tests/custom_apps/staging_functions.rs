//! Phase 3 of the previews plan: a custom app's **staging** `/fn` runs for Oxy
//! staff, runs the staging build, and holds every write
//! (`custom_apps_functions::{environment_gate, env_policy}`; environments
//! design §4; Tay's hold layer, `internal-docs/customer-apps-staging.md` D5).
//!
//! Every call here goes through the real serve route on a `staging--` or
//! production host, so the environment is resolved exactly as a browser's
//! request resolves it. Staff standing is `OXY_OWNER`; nextest runs each test
//! in its own process, so it reaches no other test.
//!
//! - the staging host runs the staging build with `ctx.channel == "staging"`,
//!   and production runs the production build as before;
//! - the held row is written when the function throws and when it times out;
//! - a `ctx.fetch` GET carrying a body is held like a write, and a bodiless
//!   GET is sent.
//!
//! Elsewhere: every write op, from a table (`staging_write_probe`); the OLTP
//! statement holds and `READ ONLY` (`staging_functions_oltp`); the semantic
//! pin and rollups (`staging_functions_semantic`); the idempotency record and
//! result cache under a real staging call (`environment_scoped_keys`); the
//! pager (`staging_function_pager`). This module holds the helpers they share.

use entity::{apps, audit_events};
use oxy_auth::user::LOCAL_GUEST_EMAIL;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{
    FnCall, FunctionSpec, Tenant, call_function_with, invocations, publish_build, seeded_tenant,
};

/// Oxy staff: the Global Owner. Before any staging call of the test — the
/// non-production decision is cached per (user, app) for a minute.
pub(crate) fn make_guest_staff() {
    unsafe { std::env::set_var("OXY_OWNER", LOCAL_GUEST_EMAIL) };
}

pub(crate) fn staging_host(t: &Tenant, app: &str) -> String {
    format!("staging--{}--{app}.customer-apps.oxygen-hq.com", t.org_slug)
}

pub(crate) fn production_host(t: &Tenant, app: &str) -> String {
    format!("{}--{app}.customer-apps.oxygen-hq.com", t.org_slug)
}

/// `POST …/fn/<name>` addressed to `host`, with `extra` headers.
pub(crate) async fn call_on(
    t: &Tenant,
    app: &str,
    name: &str,
    host: &str,
    extra: &[(&str, &str)],
) -> FnCall {
    let mut headers = vec![("host", host)];
    headers.extend_from_slice(extra);
    call_function_with(&t.org_slug, app, name, json!({}), &headers).await
}

pub(crate) fn data(call: &FnCall) -> &Value {
    call.frame("data")
        .unwrap_or_else(|| panic!("no data frame ({}): {}", call.status, call.raw))
}

/// Every `app.staging.held` row of the tenant's org.
pub(crate) async fn held_rows(t: &Tenant) -> Vec<audit_events::Model> {
    audit_events::Entity::find()
        .filter(audit_events::Column::Action.eq("app.staging.held"))
        .filter(audit_events::Column::OrgId.eq(t.org_id))
        .all(&t.db)
        .await
        .expect("query audit_events")
}

/// The host ops a held row lists (`storage.put`, `tx.commit`, …).
pub(crate) fn held_ops(row: &audit_events::Model) -> Vec<String> {
    row.metadata["writes"]
        .as_array()
        .expect("writes")
        .iter()
        .filter_map(|w| w["op"].as_str().map(str::to_string))
        .collect()
}

pub(crate) fn one(name: &'static str, manifest: Value, js: &'static str) -> Vec<FunctionSpec> {
    vec![FunctionSpec { name, manifest, js }]
}

const WHOAMI_PRODUCTION_JS: &str = r#"
export default async (req, ctx) =>
  Response.json({ build: "production-code", channel: ctx.channel, environment: ctx.environment });
"#;
const WHOAMI_STAGING_JS: &str = r#"
export default async (req, ctx) =>
  Response.json({ build: "staging-code", channel: ctx.channel, environment: ctx.environment });
"#;

#[tokio::test]
async fn a_staging_host_runs_the_staging_build_as_staging() {
    let t = seeded_tenant().await;
    let app = "stg-whoami";
    let route = || json!({ "route": true });
    let live = publish_build(
        &t,
        app,
        demo_workspace_id(),
        "prod-1",
        true,
        &one("whoami", route(), WHOAMI_PRODUCTION_JS),
    )
    .await;
    publish_build(
        &t,
        app,
        demo_workspace_id(),
        "stg-1",
        false,
        &one("whoami", route(), WHOAMI_STAGING_JS),
    )
    .await;
    let row = apps::Entity::find_by_id(live.app_id)
        .one(&t.db)
        .await
        .expect("query app")
        .expect("the app");
    let (production_pk, staging_pk) = (
        row.published_build_id.expect("production build"),
        row.draft_build_id.expect("staging build"),
    );
    make_guest_staff();

    let staging = call_on(&t, app, "whoami", &staging_host(&t, app), &[]).await;
    assert_eq!(
        data(&staging),
        &json!({ "build": "staging-code", "channel": "staging", "environment": "staging" }),
        "the staging host runs the staging build's function, as staging"
    );
    let production = call_on(&t, app, "whoami", &production_host(&t, app), &[]).await;
    assert_eq!(
        data(&production),
        &json!({ "build": "production-code", "channel": "production", "environment": "production" }),
        "production, for the same staff viewer, is unchanged"
    );

    let rows = invocations(&t.db, live.app_id, "whoami").await;
    let seen: Vec<(Uuid, &str)> = rows
        .iter()
        .map(|r| (r.build_id, r.environment.as_str()))
        .collect();
    assert_eq!(
        seen,
        vec![(staging_pk, "staging"), (production_pk, "production")],
        "each invocation is recorded against the build and environment that ran it"
    );
    assert!(held_rows(&t).await.is_empty(), "nothing was held");
}

const HELD_THEN_THROW_JS: &str = r#"
export default async (req, ctx) => {
  await ctx.fetch("https://api.example.com/a", { method: "POST", body: "{}" });
  throw new Error("fails after a held write");
};
"#;
const HELD_THEN_HANG_JS: &str = r#"
export default async (req, ctx) => {
  await ctx.fetch("https://api.example.com/b", { method: "POST", body: "{}" });
  while (true) {}
};
"#;

/// Tay's condition: the held-write log is written on every exit path, not only
/// on success.
#[tokio::test]
async fn the_held_row_is_written_when_the_function_throws_or_times_out() {
    let t = seeded_tenant().await;
    let app = "stg-exits";
    let storage = |timeout: u64| json!({ "route": true, "timeoutSeconds": timeout, "storage": { "read": true, "write": true } });
    let functions = vec![
        FunctionSpec {
            name: "throws",
            manifest: storage(30),
            js: HELD_THEN_THROW_JS,
        },
        FunctionSpec {
            name: "hangs",
            manifest: storage(1),
            js: HELD_THEN_HANG_JS,
        },
    ];
    let published = publish_build(&t, app, demo_workspace_id(), "exits-1", true, &functions).await;
    make_guest_staff();

    call_on(&t, app, "throws", &staging_host(&t, app), &[]).await;
    call_on(&t, app, "hangs", &staging_host(&t, app), &[]).await;

    let statuses: Vec<(String, String)> = [
        invocations(&t.db, published.app_id, "throws").await,
        invocations(&t.db, published.app_id, "hangs").await,
    ]
    .into_iter()
    .flatten()
    .map(|r| (r.function_name, r.status))
    .collect();
    assert_eq!(
        statuses,
        vec![
            ("throws".to_string(), "error".to_string()),
            ("hangs".to_string(), "timeout".to_string())
        ]
    );
    let held = held_rows(&t).await;
    let functions: Vec<&str> = held
        .iter()
        .filter_map(|r| r.metadata["function"].as_str())
        .collect();
    assert_eq!(held.len(), 2, "one held row per invocation: {functions:?}");
    assert!(functions.contains(&"throws") && functions.contains(&"hangs"));
}

/// A GET is a read only when it carries no body: some APIs take a body on a
/// GET and act on it. A GET with a body is held; a bodiless one is sent (to a
/// name that cannot resolve, so the test needs no network).
const FETCH_JS: &str = r#"
export default async (req, ctx) => {
  const out = {};
  const attempt = async (key, f) => {
    try { out[key] = await f(); } catch (e) { out[key] = { error: String(e && e.message ? e.message : e) }; }
  };
  await attempt("getWithBody", async () => (await ctx.fetch("https://staging-probe.invalid/search", { method: "GET", body: "{}" })).status);
  await attempt("headWithBody", async () => (await ctx.fetch("https://staging-probe.invalid/", { method: "HEAD", body: "x" })).status);
  await attempt("get", async () => (await ctx.fetch("https://staging-probe.invalid/items")).status);
  return Response.json(out);
};
"#;

#[tokio::test]
async fn a_staging_fetch_holds_a_get_that_carries_a_body_and_sends_a_bodiless_get() {
    let t = seeded_tenant().await;
    let app = "stg-fetch";
    let fetch = one(
        "fetch",
        json!({ "route": true, "timeoutSeconds": 30 }),
        FETCH_JS,
    );
    publish_build(&t, app, demo_workspace_id(), "fetch-1", true, &fetch).await;
    make_guest_staff();

    let staged = call_on(&t, app, "fetch", &staging_host(&t, app), &[]).await;
    let got = data(&staged);
    assert_eq!(got["getWithBody"], 409, "{got}");
    assert_eq!(got["headWithBody"], 409, "{got}");
    let sent = got["get"]["error"].as_str().unwrap_or_default();
    assert!(
        sent.contains("fetch failed"),
        "a bodiless GET is sent (and fails to resolve), not held: {got}"
    );
    let held = held_rows(&t).await;
    assert_eq!(held.len(), 1);
    assert_eq!(held_ops(&held[0]), vec!["fetch", "fetch"]);
}
