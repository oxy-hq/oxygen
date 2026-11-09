//! The oxy routes ferret reads, pinned from ferret's side.
//!
//! **ferret depends on this; changing it needs a matching change in
//! oxy-hq/infrastructure `services/ferret`.**
//!
//! ferret (`services/ferret` in oxy-hq/infrastructure) is the external monitor:
//! every published custom app is probed from outside the cluster, and the
//! verdicts are oxy's own, read over HTTP. Its client
//! (`src/uptime/staff-client.ts`) GETs four routes with one key in `x-api-key`,
//! and parses each answer with a zod schema (`checks.ts`, `discovery.ts`, the
//! function-failure tool). Every surprise maps to UNKNOWN, and UNKNOWN never
//! alerts: a renamed route, a renamed field, a narrower window or a key oxy
//! stops accepting does not break ferret, it blinds it. These tests are where
//! that shows up instead.
//!
//! | Module | Route (under `/api`) | What ferret reads |
//! | ------ | -------------------- | ----------------- |
//! | `app_list` | `GET /customer-apps?limit&offset` | `items[].{id, slug, org_slug, url_subdomain, published_at}`, `next_offset` |
//! | `fleet_health` | `GET /customer-apps/fleet-health?limit&offset` | `apps[].{app_id, app_slug, org_slug, health, reason, requests, failed}`, `has_more`, `evaluated_at`, `observability_configured`; the `health` vocabulary |
//! | `app_health` | `GET /customer-apps/{org}/{app}/health` | the status (200 / 503 / 404), `oxy_app_health`, `checks[].{name, result, detail}`, `error` |
//! | `function_failures` | `GET /customer-apps/function-failures?since&until&limit&org_id&app_id&cursor` | `failures[].*`, `next_cursor`; a 7-day window and `limit=500` accepted |
//! | `auth` | all four | `x-api-key` with ferret's service token or a staff key is 200; no key is 401; a key that is not staff is 403 |
//!
//! Fields are pinned by name and by the type ferret parses, and a field ferret
//! decides on (`published_at`, `health`, `oxy_app_health`) by value too: a
//! renamed nullable field still parses on ferret's side, as "absent", and
//! `published_at` absent drops the app from every check. Requests carry
//! ferret's own query parameters and encoding, so an endpoint that starts
//! refusing what ferret sends fails here, not in production.
//!
//! The routers are the served flat tree ([`super::stack::flat_api`]), so paths
//! are nest-relative: the server mounts this tree at `/api`, which
//! [`ferret_routes_are_in_the_served_catalog`] pins.

mod app_health;
mod app_list;
mod auth;
mod fleet_health;
mod function_failures;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use serde_json::Value;
use tower::ServiceExt;

use super::service_tokens::{FERRET_TOKEN, with_ferret};
use super::stack::{Reach, flat_api, make_staff, mint};
use super::{Fixture, fixture, legacy_key};

/// One answer as ferret sees it: the status, the content type it tests for
/// `json`, and the body.
pub(super) struct Answer {
    pub status: StatusCode,
    pub content_type: String,
    pub body: Value,
}

impl Answer {
    /// What ferret's `parseJson` accepts before it looks at a field: a 200
    /// whose content type says JSON. A 200 HTML answer is the SPA shell oxy
    /// serves for any path it does not route.
    pub fn assert_json_200(&self, what: &str) {
        assert_eq!(self.status, StatusCode::OK, "{what}: {}", self.body);
        assert!(
            self.content_type.to_ascii_lowercase().contains("json"),
            "{what}: ferret reads only a JSON content type, got {:?}",
            self.content_type
        );
    }
}

/// `GET path_and_query` on the served tree, the key in `x-api-key` as
/// ferret's `staff-client.ts` sends it (never a query parameter, never a bearer).
pub(super) async fn ferret_get(key: Option<&str>, path_and_query: &str) -> Answer {
    let mut req = Request::builder().method("GET").uri(path_and_query);
    if let Some(key) = key {
        req = req.header("x-api-key", key);
    }
    let resp = flat_api()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .expect("oneshot");
    let status = resp.status();
    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    Answer {
        status,
        content_type,
        body,
    }
}

/// A query string encoded the way ferret's `URL.searchParams.set` encodes it.
pub(super) fn query(pairs: &[(&str, &str)]) -> String {
    let mut out = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in pairs {
        out.append_pair(k, v);
    }
    out.finish()
}

/// The fixture's user as Oxy staff (a Global Admin over every org), which is
/// what ferret's key belongs to today.
pub(super) async fn staff_fixture() -> Fixture {
    let fx = fixture().await;
    make_staff(
        &fx.db,
        fx.user.email.as_deref().expect("seeded with an email"),
    )
    .await;
    fx
}

/// Every key ferret can hold: its service token (`oxysvc_`, configured as
/// code, what it uses), and the staff user's keys it falls back to — a legacy
/// `oxy_<hex>` key, and a personal token carrying platform standing.
pub(super) async fn staff_keys(fx: &Fixture) -> Vec<(&'static str, String)> {
    with_ferret(fx).await;
    let (_, legacy) = legacy_key(fx, None).await;
    let (_, token) = mint(&fx.db, fx.user.id, Reach::all_access().with_platform()).await;
    vec![
        ("service token", FERRET_TOKEN.to_string()),
        ("legacy key", legacy),
        ("personal token", token),
    ]
}

/// The type a ferret zod schema parses a field as.
#[derive(Clone, Copy, Debug)]
pub(super) enum Field {
    /// `z.string()`.
    Str,
    /// `z.string().nullish()`. Pinned as present (null allowed): a renamed
    /// field still parses on ferret's side, as absent.
    OptStr,
    /// `z.number().nullish()` that oxy always fills.
    Num,
    /// `z.number().nullish()`. Present, null or a number.
    OptNum,
    /// `z.number().int().nullish()`. Present, null or an integer.
    OptInt,
    /// `z.boolean()`.
    Bool,
    /// `z.array(..)`.
    Array,
}

impl Field {
    fn admits(self, v: Option<&Value>) -> bool {
        match (self, v) {
            (_, None) => false,
            (Self::Str, Some(v)) => v.is_string(),
            (Self::OptStr, Some(v)) => v.is_null() || v.is_string(),
            (Self::Num, Some(v)) => v.is_number(),
            (Self::OptNum, Some(v)) => v.is_null() || v.is_number(),
            (Self::OptInt, Some(v)) => v.is_null() || v.is_i64() || v.is_u64(),
            (Self::Bool, Some(v)) => v.is_boolean(),
            (Self::Array, Some(v)) => v.is_array(),
        }
    }
}

/// Every `(name, type)` in `fields` is on `v`, typed as ferret parses it.
pub(super) fn assert_fields(v: &Value, fields: &[(&str, Field)], what: &str) {
    for (name, ty) in fields {
        assert!(
            ty.admits(v.get(name)),
            "{what}.{name}: ferret parses it as {ty:?}, oxy sent {:?}. ferret depends on \
             this field; changing it needs a matching change in oxy-hq/infrastructure \
             services/ferret (src/uptime/staff-client.ts).",
            v.get(name)
        );
    }
}

/// The four API paths ferret's key may GET (`API_PATHS` in `staff-client.ts`),
/// as the served catalog spells them, `/api` prefix included.
const FERRET_API_PATHS: [&str; 4] = [
    "/api/customer-apps",
    "/api/customer-apps/fleet-health",
    "/api/customer-apps/{org_slug}/{app_slug}/health",
    "/api/customer-apps/function-failures",
];

#[test]
fn ferret_routes_are_in_the_served_catalog() {
    let catalog = oxy_route_catalog::catalog();
    for path in FERRET_API_PATHS {
        assert!(
            catalog
                .routes
                .iter()
                .any(|r| r.method == "GET" && r.path == path),
            "GET {path} is not served under /api any more. ferret depends on it; changing \
             it needs a matching change in oxy-hq/infrastructure services/ferret."
        );
    }
}
