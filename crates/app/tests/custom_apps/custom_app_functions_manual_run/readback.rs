//! Reading back what ran: the fixture the read-back modules share, and the
//! invocation id `/fn` returns.
//!
//! Calls go through the real serve route on a `staging--` or production host
//! (as `staging_functions` does), so each invocation row is written by the
//! runtime; the listings and the held route are read through the admin stack
//! of the parent module. The reads themselves live beside this module:
//! `invocation_listings`, `held_readback` and `log_reads`.
//!
//! - `/fn` answers `x-oxy-invocation-id`, naming the row it wrote — on an
//!   idempotent replay too — and a result-cache hit names none; an environment
//!   that serves no build answers `EnvironmentHasNoBuild`, production still
//!   `AppNotPublished`.

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{
    FunctionSpec, Tenant, invocations, publish_build, seeded_tenant, serve_router,
};
use crate::staging_functions::{make_guest_staff, production_host, staging_host};

pub(super) const APP: &str = "fn-readback";
pub(super) const OTHER_APP: &str = "fn-readback-other";
pub(super) const PRODUCTION_BUILD: &str = "rb-prod-1";
pub(super) const STAGING_BUILD: &str = "rb-stg-1";

const WHOAMI_JS: &str =
    "export default async (req, ctx) => Response.json({ channel: ctx.channel });";
/// A third-party write: sent in production's policy, held outside it. The
/// host is a name that cannot resolve, so production's attempt needs no
/// network and the function swallows its failure.
const WRITES_JS: &str = r#"
export default async (req, ctx) => {
  let status = "failed";
  try {
    status = (await ctx.fetch("https://readback-probe.invalid/orders", { method: "POST", body: "{}" })).status;
  } catch (e) {}
  return Response.json({ channel: ctx.channel, write: status });
};
"#;
const CACHED_JS: &str = "export default async () => Response.json({ at: Date.now() });";

fn functions() -> Vec<FunctionSpec> {
    vec![
        FunctionSpec {
            name: "whoami",
            manifest: json!({ "route": true }),
            js: WHOAMI_JS,
        },
        FunctionSpec {
            name: "writes",
            manifest: json!({ "route": true, "timeoutSeconds": 30 }),
            js: WRITES_JS,
        },
        FunctionSpec {
            name: "cached",
            manifest: json!({ "route": true, "cache": { "ttlSeconds": 300 } }),
            js: CACHED_JS,
        },
    ]
}

/// App `slug` with a promoted build and a newer staging build, in `t`'s org.
pub(super) async fn two_builds(t: &Tenant, slug: &str, production: &str, staging: &str) -> Uuid {
    let ws = demo_workspace_id();
    let app_id = publish_build(t, slug, ws, production, true, &functions())
        .await
        .app_id;
    publish_build(t, slug, ws, staging, false, &functions()).await;
    app_id
}

/// `POST …/fn/<name>` on `host`: the status, the response headers and the body.
pub(super) async fn call(
    t: &Tenant,
    app: &str,
    name: &str,
    host: &str,
) -> (StatusCode, HeaderMap, String) {
    call_keyed(t, app, name, host, None).await
}

/// [`call`], under `Idempotency-Key: key` when one is given.
async fn call_keyed(
    t: &Tenant,
    app: &str,
    name: &str,
    host: &str,
    key: Option<&str>,
) -> (StatusCode, HeaderMap, String) {
    let mut request = Request::builder()
        .method("POST")
        .uri(format!("/customer-apps/{}/{app}/fn/{name}", t.org_slug))
        .header("content-type", "application/json")
        .header("host", host);
    if let Some(key) = key {
        request = request.header("idempotency-key", key);
    }
    let request = request.body(Body::from("{}")).expect("request");
    let response = serve_router().oneshot(request).await.expect("oneshot");
    let (status, headers) = (response.status(), response.headers().clone());
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("read body");
    (
        status,
        headers,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

/// The invocation id a `/fn` response names.
pub(super) fn invocation_id(headers: &HeaderMap) -> Option<Uuid> {
    headers
        .get("x-oxy-invocation-id")
        .map(|v| v.to_str().expect("a header value").parse().expect("a uuid"))
}

/// A call that ran, and the invocation its response names.
pub(super) async fn ran(t: &Tenant, app: &str, name: &str, host: &str) -> Uuid {
    let (status, headers, body) = call(t, app, name, host).await;
    assert_eq!(status, StatusCode::OK, "{name} on {host}: {body}");
    invocation_id(&headers).unwrap_or_else(|| panic!("{name} on {host} named no invocation"))
}

pub(super) fn ids(listed: &Value) -> Vec<String> {
    listed
        .as_array()
        .expect("a list of invocations")
        .iter()
        .map(|row| row["id"].as_str().expect("id").to_string())
        .collect()
}

pub(super) async fn build_pk(t: &Tenant, app_id: Uuid, label: &str) -> Uuid {
    use entity::app_builds;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    app_builds::Entity::find()
        .filter(app_builds::Column::AppId.eq(app_id))
        .filter(app_builds::Column::BuildId.eq(label))
        .one(&t.db)
        .await
        .expect("read builds")
        .unwrap_or_else(|| panic!("build {label}"))
        .id
}

#[tokio::test]
async fn fn_names_the_invocation_it_wrote_and_a_cache_hit_names_none() {
    let t = seeded_tenant().await;
    let app_id = two_builds(&t, APP, PRODUCTION_BUILD, STAGING_BUILD).await;
    make_guest_staff();

    let production = ran(&t, APP, "whoami", &production_host(&t, APP)).await;
    let staging = ran(&t, APP, "whoami", &staging_host(&t, APP)).await;
    let rows = invocations(&t.db, app_id, "whoami").await;
    let written: Vec<(Uuid, &str)> = rows
        .iter()
        .map(|r| (r.id, r.environment.as_str()))
        .collect();
    assert_eq!(
        written,
        vec![(production, "production"), (staging, "staging")],
        "each response names the row its call wrote"
    );

    // The first call runs and names its row; the second is served from the
    // result cache, ran nothing, and names none.
    let host = production_host(&t, APP);
    let first = ran(&t, APP, "cached", &host).await;
    let (status, headers, body) = call(&t, APP, "cached", &host).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(invocation_id(&headers), None, "a cache hit wrote no row");
    let rows = invocations(&t.db, app_id, "cached").await;
    assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![first]);
}

/// A keyed call that is replayed ran nothing the second time, but it answers
/// for a row: the one the first call wrote. It names that row, so a caller in
/// a sandbox finds the held writes of the call it is replaying — only a
/// result-cache hit, which answers for no row at all, names none.
#[tokio::test]
async fn an_idempotent_replay_names_the_invocation_it_replays() {
    let t = seeded_tenant().await;
    let app_id = two_builds(&t, APP, PRODUCTION_BUILD, STAGING_BUILD).await;
    make_guest_staff();

    for host in [production_host(&t, APP), staging_host(&t, APP)] {
        let key = format!("replay-{}", Uuid::new_v4());
        let (status, headers, body) = call_keyed(&t, APP, "whoami", &host, Some(&key)).await;
        assert_eq!(status, StatusCode::OK, "{host}: {body}");
        let wrote = invocation_id(&headers).expect("the first call names its row");

        let (status, headers, body) = call_keyed(&t, APP, "whoami", &host, Some(&key)).await;
        assert_eq!(status, StatusCode::OK, "{host}: {body}");
        assert_eq!(
            invocation_id(&headers),
            Some(wrote),
            "{host}: a replay names the row it replays"
        );
    }
    assert_eq!(
        invocations(&t.db, app_id, "whoami").await.len(),
        2,
        "one row per key: the replays ran nothing"
    );
}

/// An environment that serves no build says which one is empty; production
/// keeps the answer it has always given. Neither writes an invocation.
#[tokio::test]
async fn fn_in_an_environment_with_no_build_names_the_environment() {
    use sea_orm::{ConnectionTrait, DbBackend, Statement};
    let t = seeded_tenant().await;
    let app_id = two_builds(&t, APP, PRODUCTION_BUILD, STAGING_BUILD).await;
    make_guest_staff();
    let unset = |sql: &'static str| {
        let db = t.db.clone();
        async move {
            db.execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                sql,
                [app_id.into()],
            ))
            .await
            .expect("unset a build pointer");
        }
    };
    let error = |body: &str| -> Value { serde_json::from_str(body).expect("a JSON error") };

    unset("UPDATE app_environments SET build_id = NULL WHERE app_id = $1 AND name = 'staging'")
        .await;
    let (status, headers, body) = call(&t, APP, "whoami", &staging_host(&t, APP)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(error(&body)["error"], "EnvironmentHasNoBuild", "{body}");
    assert!(body.contains("staging"), "{body}");
    assert_eq!(invocation_id(&headers), None);
    // Production still serves its build.
    ran(&t, APP, "whoami", &production_host(&t, APP)).await;

    unset("UPDATE apps SET published_build_id = NULL, draft_build_id = NULL WHERE id = $1").await;
    let (status, headers, body) = call(&t, APP, "whoami", &production_host(&t, APP)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(error(&body)["error"], "AppNotPublished", "{body}");
    assert_eq!(invocation_id(&headers), None);
    assert_eq!(
        invocations(&t.db, app_id, "whoami").await.len(),
        1,
        "only the call that ran wrote a row"
    );
}
