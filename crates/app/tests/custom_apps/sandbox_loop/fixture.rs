//! What the loop is run on: the four bundles, the app on two builds, and
//! the calls and reads each step makes.

use axum::http::StatusCode;
use entity::{app_environments, apps};
use oxy::service::secret_manager::SecretManagerService;
use oxy_app::server::api::custom_apps_publish::publish;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::custom_app_functions_fixture::{FnCall, FunctionSpec, Tenant, call_function_with};
use crate::custom_app_functions_manual_run::get_admin;
use crate::sandbox_publish::{app_row, bundle, input};
use crate::staging_functions::{data, make_guest_staff};

pub(super) const APP: &str = "sbx-loop";
pub(super) const A: &str = "dev-a1";
pub(super) const B: &str = "dev-b2";

/// One bundle per environment, told apart by `mark`. `probe` is the route
/// call: it reports the build, the channel and the secrets it runs with,
/// optionally stores an object, and lists its silo. `smoke` is the check: the
/// same report, after a third-party write the non-production policy holds.
pub(super) fn functions(mark: &str) -> Vec<FunctionSpec> {
    // `FunctionSpec::js` is `&'static str`; a test leaks two short strings.
    let marked = |js: &str| -> &'static str {
        Box::leak(format!("const MARK = {mark:?};\n{js}").into_boxed_str())
    };
    vec![
        FunctionSpec {
            name: "probe",
            manifest: json!({ "route": true, "storage": { "read": true, "write": true } }),
            js: marked(PROBE_JS),
        },
        FunctionSpec {
            name: "smoke",
            manifest: json!({ "check": true, "storage": { "read": true } }),
            js: marked(SMOKE_JS),
        },
    ]
}

const PROBE_JS: &str = r#"
export default async (req, ctx) => {
  const { put } = JSON.parse(req.body || "{}");
  const out = { build: MARK, channel: ctx.channel, token: ctx.env.TOKEN ?? null, only_a: ctx.env.ONLY_A ?? null };
  if (put) out.put = (await ctx.storage.put(put, MARK, { allowOverwrite: true })).key;
  out.list = (await ctx.storage.list()).objects.map((o) => o.key).sort();
  return Response.json(out);
};
"#;

const SMOKE_JS: &str = r#"
export default async (req, ctx) => {
  const held = await ctx.fetch("https://api.example.com/orders", { method: "POST", body: "{}" });
  const list = (await ctx.storage.list()).objects.map((o) => o.key).sort();
  return Response.json({ build: MARK, channel: ctx.channel, token: ctx.env.TOKEN ?? null, write: held.status, list });
};
"#;

pub(super) fn tarball(functions: &[FunctionSpec]) -> Vec<u8> {
    let env = json!({ "env": { "TOKEN": {}, "ONLY_A": {} } });
    bundle(APP, functions, env, &[])
}

/// The app with a promoted build and a newer staging build, the guest made
/// Oxy staff, and a `TOKEN` of its own in production and in staging.
pub(super) async fn app_on_two_builds(t: &Tenant) -> apps::Model {
    let mut production = input(t, APP, "loop-prod", tarball(&functions("production")));
    production.promote = true;
    let app_id = publish(production).await.expect("production").app_id;
    publish(input(t, APP, "loop-stg", tarball(&functions("staging"))))
        .await
        .expect("staging");
    make_guest_staff();
    // SAFETY: nextest runs each test in its own process.
    unsafe { std::env::set_var("OXY_API_URL", "https://app-dev.oxygen-hq.com") };
    let app = app_row(&t.db, app_id).await;
    set_token(t, &app, None, "prod-token").await;
    set_token(t, &app, Some("staging"), "stg-token").await;
    app
}

pub(super) async fn set_token(
    t: &Tenant,
    app: &apps::Model,
    environment: Option<&str>,
    value: &str,
) {
    set_secret(t, app, environment, "TOKEN", value).await;
}

pub(super) async fn set_secret(
    t: &Tenant,
    app: &apps::Model,
    environment: Option<&str>,
    key: &str,
    v: &str,
) {
    SecretManagerService::new(app.project_id)
        .set_app_secret_in(&t.db, app.id, environment, key, v, t.guest_id)
        .await
        .expect("set a secret");
}

/// What production and staging point at: the two columns on the app row and
/// the two fixed `app_environments` rows, with the time each last moved.
pub(super) async fn fixed_pointers(t: &Tenant, app_id: Uuid) -> Value {
    let app = app_row(&t.db, app_id).await;
    let rows = app_environments::Entity::find()
        .filter(app_environments::Column::AppId.eq(app_id))
        .filter(app_environments::Column::Name.is_in(["production", "staging"]))
        .order_by_asc(app_environments::Column::Name)
        .all(&t.db)
        .await
        .expect("read the fixed environments");
    let rows: Vec<Value> = rows
        .iter()
        .map(|r| json!([r.name, r.build_id, r.updated_at.to_rfc3339()]))
        .collect();
    json!({ "published": app.published_build_id, "draft": app.draft_build_id, "rows": rows })
}

/// `POST …/fn/<name>` in `environment`, named by `X-Oxy-App-Env` on a
/// bearer request; production is the same request without the header.
pub(super) async fn call_in(t: &Tenant, environment: &str, name: &str, body: Value) -> FnCall {
    let named = [
        ("authorization", "Bearer t"),
        ("x-oxy-app-env", environment),
    ];
    let headers: &[(&str, &str)] = if environment == "production" {
        &[]
    } else {
        &named
    };
    call_function_with(&t.org_slug, APP, name, body, headers).await
}

/// `probe`'s answer in `environment`, storing `put` first when given.
pub(super) async fn probe(t: &Tenant, environment: &str, put: Option<&str>) -> Value {
    let call = call_in(t, environment, "probe", json!({ "put": put })).await;
    assert_eq!(call.status, StatusCode::OK, "{environment}: {}", call.raw);
    data(&call).clone()
}

/// The staff listing of one environment's invocations: `(function, build)`
/// of each row, having checked every row is that environment's.
pub(super) async fn ran_in(app_id: Uuid, environment: &str) -> Vec<(String, String)> {
    let (status, body) = get_admin(&format!(
        "/apps/{app_id}/invocations?environment={environment}"
    ))
    .await;
    assert_eq!(status, StatusCode::OK, "{environment}: {body}");
    let mut rows: Vec<(String, String)> = body["invocations"]
        .as_array()
        .expect("invocations")
        .iter()
        .map(|r| {
            assert_eq!(r["environment"], environment, "{r}");
            let text = |key: &str| r[key].as_str().expect(key).to_string();
            (text("function_name"), text("build_id"))
        })
        .collect();
    rows.sort();
    rows
}

pub(super) fn silo_key(app_id: Uuid, environment: &str, pathname: &str) -> String {
    format!("customer-app-storage/{app_id}~{environment}/{pathname}")
}
