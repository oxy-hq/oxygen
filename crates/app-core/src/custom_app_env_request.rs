//! Which app environment a request addresses, and the two refusals that follow
//! from it before any handler runs
//! (`internal-docs/2026-09-10-custom-app-environments-design.md` §3.2).
//!
//! ## Resolution, first match wins
//!
//! 1. **Host label.** `staging--<org>--<slug>.customer-apps…` is staging,
//!    `dev-<handle>--<org>--<slug>.…` that dev slot, `<org>--<slug>.…`
//!    production ([`crate::custom_apps_host_dispatch::parse_app_host`]).
//! 2. **`X-Oxy-App-Env`**, on a request carrying an explicit credential (a
//!    bearer token or an API key) and only there.
//! 3. **Otherwise production**: the path URL on the admin host, and `/a/<slug>/`
//!    on an org subdomain.
//!
//! A header that cannot be honoured is a **400, never a downgrade to
//! production**: on a cookie-authenticated request, naming an unknown
//! environment, or disagreeing with the environment the host already names.
//!
//! ## The two refusals ([`environment_guard_middleware`])
//!
//! - **`/api` writes outside production** answer 403. `/api/*` is not rewritten
//!   on an app host, so a staging bundle's direct calls — procedure runs, agent
//!   asks, threads, events — reach the ordinary API, which has no environment
//!   to isolate them into. Reads stay allowed, and so do the data plane's
//!   read-only `POST`s ([`is_read_only_post`]). On the **staging** host three
//!   named surfaces are let through as well ([`is_staging_write`]):
//!   starting and cancelling an agent ask, which `start_ask` runs with every
//!   data write held, and starting an automation run, which
//!   `start_automation_run` refuses with a `409` logged as held rather than
//!   running (`projects::automation_run::staging_hold`).
//! - **A cookie-authenticated write whose `Origin` belongs to another
//!   environment** answers 403. Every app host shares the `.oxygen-hq.com`
//!   `SameSite=Lax` session cookie, so without this a staging page could post
//!   to production with the viewer's session. A request carrying a bearer token
//!   or an API key skips it: that credential is not ambient, so a cross-site
//!   page cannot ride it.
//!
//! `/fn` makes both decisions itself, in `custom_apps_functions` (the refusal
//! outside production is the seam the hold layer will replace), using
//! [`request_environment`] and [`check_origin`] from here.

use axum::extract::Request;
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::custom_app_environment::AppEnvironment;
use crate::custom_apps_host_dispatch::parse_app_host;

/// The header a bearer-authenticated caller (`oxyc dev`) names an environment
/// with.
pub const ENV_HEADER: &str = "x-oxy-app-env";

/// The session cookie every app host shares.
const SESSION_COOKIE: &str = "oxy_session";

/// How a request is authenticated, as far as the headers say. Decides whether
/// the environment header can be honoured and whether the origin check applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Credential {
    /// An `Authorization` header or an `X-API-Key`: attached on purpose by the
    /// caller, so no page can make a browser send it cross-site.
    Explicit,
    /// Only the session cookie, which the browser attaches on its own.
    Ambient,
    /// Nothing: the request authenticates as nobody (or as the zero-config
    /// guest).
    None,
}

/// Which credential the request carries. Mirrors the authenticator's order: a
/// non-empty `Authorization` header is tried before the cookie, and an API key
/// is its fallback.
pub fn credential_of(headers: &HeaderMap) -> Credential {
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| !v.trim().is_empty());
    if authorization || headers.contains_key("x-api-key") {
        return Credential::Explicit;
    }
    let has_session = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .any(|pair| {
            pair.trim()
                .split_once('=')
                .is_some_and(|(name, value)| name == SESSION_COOKIE && !value.is_empty())
        });
    if has_session {
        Credential::Ambient
    } else {
        Credential::None
    }
}

/// Why an `X-Oxy-App-Env` header was not honoured. Every variant is a 400.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EnvRequestError {
    /// The header rode a request authenticated only by the session cookie.
    /// Honouring it would let any page pick another environment for the
    /// viewer; ignoring it would silently run production.
    AmbientCredential,
    /// The header names no environment (`Production` is not `production`).
    UnknownEnvironment(String),
    /// The header and the host name different environments.
    ConflictsWithHost { host: String, header: String },
}

impl EnvRequestError {
    pub fn message(&self) -> String {
        match self {
            Self::AmbientCredential => format!(
                "{ENV_HEADER} is honoured only on a request authenticated by a bearer token or an \
                 API key; strip the session cookie from a request you tag"
            ),
            Self::UnknownEnvironment(name) => {
                format!("{ENV_HEADER} names no app environment: {name:?}")
            }
            Self::ConflictsWithHost { host, header } => format!(
                "{ENV_HEADER} names {header} but the host addresses {host}; send one or the other"
            ),
        }
    }

    pub fn into_response(self) -> Response {
        json_refusal(
            StatusCode::BAD_REQUEST,
            "InvalidAppEnvironment",
            self.message(),
        )
    }
}

/// The environment the request's `Host` names, when it is an app host.
fn host_environment(headers: &HeaderMap) -> Option<AppEnvironment> {
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok())?;
    parse_app_host(host).map(|h| h.environment)
}

/// The environment this request addresses. See the module docs for the order.
pub fn request_environment(headers: &HeaderMap) -> Result<AppEnvironment, EnvRequestError> {
    let from_host = host_environment(headers);
    let Some(raw) = headers.get(ENV_HEADER) else {
        return Ok(from_host.unwrap_or(AppEnvironment::Production));
    };
    let name = raw.to_str().unwrap_or("").trim().to_string();
    if credential_of(headers) != Credential::Explicit {
        return Err(EnvRequestError::AmbientCredential);
    }
    let named =
        AppEnvironment::parse(&name).ok_or(EnvRequestError::UnknownEnvironment(name.clone()))?;
    match from_host {
        Some(host) if host != named => Err(EnvRequestError::ConflictsWithHost {
            host: host.name(),
            header: name,
        }),
        _ => Ok(named),
    }
}

/// The environment of the page that sent this request: the `Origin`, or the
/// `Referer`'s origin when there is none. `None` when neither is present (a
/// non-browser client). An origin that is not an app host is production — the
/// admin host, an org subdomain, and every other site.
pub fn origin_environment(headers: &HeaderMap) -> Option<AppEnvironment> {
    let origin = headers
        .get(header::ORIGIN)
        .or_else(|| headers.get(header::REFERER))
        .and_then(|v| v.to_str().ok())?;
    let host = origin
        .split_once("://")
        .map(|(_, rest)| rest.split('/').next().unwrap_or(""))
        .unwrap_or("");
    Some(
        parse_app_host(host)
            .map(|h| h.environment)
            .unwrap_or(AppEnvironment::Production),
    )
}

/// A cookie-authenticated request sent from a page in another environment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OriginRefused {
    pub origin: AppEnvironment,
    pub target: AppEnvironment,
}

impl OriginRefused {
    pub fn into_response(self) -> Response {
        json_refusal(
            StatusCode::FORBIDDEN,
            "CrossEnvironmentOrigin",
            format!(
                "a page in the {} environment cannot write to {} with the session cookie",
                self.origin, self.target
            ),
        )
    }
}

/// The origin check (§3.2): a request authenticated by the session cookie must
/// come from a page in the environment it targets. A request with an explicit
/// credential, or with no `Origin` / `Referer` at all, passes.
pub fn check_origin(headers: &HeaderMap, target: &AppEnvironment) -> Result<(), OriginRefused> {
    if credential_of(headers) != Credential::Ambient {
        return Ok(());
    }
    match origin_environment(headers) {
        Some(origin) if origin != *target => Err(OriginRefused {
            origin,
            target: target.clone(),
        }),
        _ => Ok(()),
    }
}

/// A method that changes something. `GET`, `HEAD` and `OPTIONS` do not.
pub fn is_write_method(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// The custom-app data plane's `POST`s that only read: a query, a semantic
/// query, and the semantic analyses. They are `POST` for their request bodies,
/// and every one sits behind `check_custom_app_gates` and runs read-only (the
/// `/query` path refuses anything but a `SELECT`/`WITH`). A staging page reads
/// production data through them (§4.2), so they are not writes here.
pub fn is_read_only_post(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("/api/projects/") else {
        return false;
    };
    let Some((_, endpoint)) = rest.split_once('/') else {
        return false;
    };
    matches!(endpoint, "query" | "semantic-query" | "semantic/cohort")
        || endpoint
            .strip_prefix("semantic/metric-tree/")
            .is_some_and(|op| !op.is_empty() && !op.contains('/'))
}

/// The writes a page on an app's **staging** host may make: start an ask
/// (`POST /api/projects/{p}/agents/{agent}/asks`), cancel one
/// (`POST /api/projects/{p}/agents/asks/{run}/cancel`), and start an
/// automation run (`POST /api/projects/{p}/procedures/{procedure}/runs`,
/// `router/public.rs`'s route for `start_automation_run`). Ask streams are a
/// `GET`; an automation run's poll and cancel are not named here — they stay
/// refused, same as every other write.
///
/// Not [`is_read_only_post`]: each writes a thread, message, run or run-row.
/// They are allowed because the handler itself decides what happens next —
/// `start_ask` runs its write held (`previews::request_hold`), refusing a
/// caller who may not open staging; `start_automation_run` refuses every
/// staging caller with a `409`, logged as held for one it may open
/// (`projects::automation_run::staging_hold`). Staging only — a dev slot's
/// write is refused like any other.
pub fn is_staging_write(method: &Method, path: &str) -> bool {
    if *method != Method::POST {
        return false;
    }
    let Some(rest) = path.strip_prefix("/api/projects/") else {
        return false;
    };
    let segments: Vec<&str> = rest.split('/').collect();
    let named = |s: &str| !s.is_empty();
    match segments.as_slice() {
        [project, "agents", agent, "asks"] => named(project) && named(agent),
        [project, "agents", "asks", run, "cancel"] => named(project) && named(run),
        [project, "procedures", procedure, "runs"] => named(project) && named(procedure),
        _ => false,
    }
}

/// Whether `guard` lets this `/api` write through in `environment`.
fn write_allowed_in(environment: &AppEnvironment, method: &Method, path: &str) -> bool {
    *environment == AppEnvironment::Production
        || (*environment == AppEnvironment::Staging && is_staging_write(method, path))
}

fn is_api_path(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/")
}

/// Why an `/api` write was refused outside production.
fn api_write_refused(environment: &AppEnvironment) -> Response {
    json_refusal(
        StatusCode::FORBIDDEN,
        "EnvironmentRefused",
        format!(
            "writes are refused in the {environment} environment until they are isolated from \
             production; this endpoint writes production data"
        ),
    )
}

fn json_refusal(status: StatusCode, error: &str, message: String) -> Response {
    (
        status,
        axum::Json(serde_json::json!({ "error": error, "message": message })),
    )
        .into_response()
}

/// The decision [`environment_guard_middleware`] makes, as a pure function of
/// the request line and headers. `Ok(env)` lets the request through carrying
/// `env` as an extension.
pub fn guard(
    method: &Method,
    path: &str,
    headers: &HeaderMap,
) -> Result<AppEnvironment, Box<Response>> {
    let environment = request_environment(headers).map_err(|e| Box::new(e.into_response()))?;
    if is_api_path(path) && is_write_method(method) && !is_read_only_post(path) {
        if !write_allowed_in(&environment, method, path) {
            return Err(Box::new(api_write_refused(&environment)));
        }
        check_origin(headers, &environment).map_err(|e| Box::new(e.into_response()))?;
    }
    Ok(environment)
}

/// Resolve the request's environment, refuse what §3.2 refuses, and hand the
/// environment on as a request extension.
///
/// Runs outermost, before the host rewrite, so it sees the `Host` and path the
/// client sent. Cheap: a few header reads and no allocation on the common
/// path (no header, production host, a read).
pub async fn environment_guard_middleware(mut request: Request, next: Next) -> Response {
    match guard(request.method(), request.uri().path(), request.headers()) {
        Ok(environment) => {
            request.extensions_mut().insert(environment);
            next.run(request).await
        }
        Err(refusal) => *refusal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    const STAGING_HOST: &str = "staging--acme--store.customer-apps.oxygen-hq.com";
    const PROD_HOST: &str = "acme--store.customer-apps.oxygen-hq.com";

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (name, value) in pairs {
            h.append(*name, HeaderValue::from_str(value).unwrap());
        }
        h
    }

    #[test]
    fn the_host_label_decides_first() {
        assert_eq!(
            request_environment(&headers(&[("host", STAGING_HOST)])),
            Ok(AppEnvironment::Staging)
        );
        assert_eq!(
            request_environment(&headers(&[("host", PROD_HOST)])),
            Ok(AppEnvironment::Production)
        );
        assert_eq!(
            request_environment(&headers(&[("host", "app.oxygen-hq.com")])),
            Ok(AppEnvironment::Production),
            "the path URL on the admin host is production"
        );
    }

    #[test]
    fn the_header_is_honoured_on_a_bearer_request() {
        assert_eq!(
            request_environment(&headers(&[
                ("host", "app.oxygen-hq.com"),
                ("authorization", "Bearer t"),
                (ENV_HEADER, "dev-luong"),
            ])),
            Ok(AppEnvironment::Dev {
                handle: "luong".into()
            })
        );
        assert_eq!(
            request_environment(&headers(&[("x-api-key", "k"), (ENV_HEADER, "staging")])),
            Ok(AppEnvironment::Staging)
        );
    }

    /// Never downgraded to production: a header that cannot be honoured is a 400.
    #[test]
    fn a_header_on_a_cookie_authenticated_request_is_refused() {
        assert_eq!(
            request_environment(&headers(&[
                ("cookie", "oxy_session=jwt"),
                (ENV_HEADER, "staging"),
            ])),
            Err(EnvRequestError::AmbientCredential)
        );
        // No credential at all is not a bearer request either.
        assert_eq!(
            request_environment(&headers(&[(ENV_HEADER, "staging")])),
            Err(EnvRequestError::AmbientCredential)
        );
    }

    #[test]
    fn an_unknown_or_conflicting_header_is_refused() {
        assert!(matches!(
            request_environment(&headers(&[
                ("authorization", "Bearer t"),
                (ENV_HEADER, "Production"),
            ])),
            Err(EnvRequestError::UnknownEnvironment(_))
        ));
        assert!(matches!(
            request_environment(&headers(&[
                ("host", PROD_HOST),
                ("authorization", "Bearer t"),
                (ENV_HEADER, "staging"),
            ])),
            Err(EnvRequestError::ConflictsWithHost { .. })
        ));
    }

    #[test]
    fn the_credential_is_read_the_way_the_authenticator_reads_it() {
        assert_eq!(
            credential_of(&headers(&[("cookie", "a=1; oxy_session=x")])),
            Credential::Ambient
        );
        assert_eq!(
            credential_of(&headers(&[
                ("cookie", "oxy_session=x"),
                ("authorization", "Bearer t"),
            ])),
            Credential::Explicit
        );
        assert_eq!(
            credential_of(&headers(&[("cookie", "oxy_session=")])),
            Credential::None
        );
        assert_eq!(credential_of(&headers(&[])), Credential::None);
    }

    #[test]
    fn a_cross_environment_origin_on_a_cookie_request_is_refused() {
        let from_staging = headers(&[
            ("host", PROD_HOST),
            ("cookie", "oxy_session=x"),
            ("origin", &format!("https://{STAGING_HOST}")),
        ]);
        assert_eq!(
            check_origin(&from_staging, &AppEnvironment::Production),
            Err(OriginRefused {
                origin: AppEnvironment::Staging,
                target: AppEnvironment::Production,
            })
        );
    }

    #[test]
    fn the_origin_check_passes_explicit_credentials_and_same_environment_pages() {
        let bearer_no_origin = headers(&[("host", PROD_HOST), ("authorization", "Bearer t")]);
        assert_eq!(
            check_origin(&bearer_no_origin, &AppEnvironment::Production),
            Ok(())
        );
        let bearer_foreign_origin = headers(&[
            ("authorization", "Bearer t"),
            ("origin", &format!("https://{STAGING_HOST}")),
        ]);
        assert_eq!(
            check_origin(&bearer_foreign_origin, &AppEnvironment::Production),
            Ok(()),
            "an explicit credential is not ambient; a cross-site page cannot ride it"
        );
        for origin in [
            format!("https://{PROD_HOST}"),
            "https://app.oxygen-hq.com".to_string(),
            "https://acme.oxygen-hq.com".to_string(),
        ] {
            let same = headers(&[("cookie", "oxy_session=x"), ("origin", &origin)]);
            assert_eq!(
                check_origin(&same, &AppEnvironment::Production),
                Ok(()),
                "{origin} is a production page"
            );
        }
        let referer_only = headers(&[
            ("cookie", "oxy_session=x"),
            (
                "referer",
                &format!("https://{STAGING_HOST}/customer-apps/acme/store/"),
            ),
        ]);
        assert!(
            check_origin(&referer_only, &AppEnvironment::Production).is_err(),
            "the Referer stands in for a missing Origin"
        );
    }

    #[test]
    fn read_only_posts_are_the_data_plane_reads() {
        for path in [
            "/api/projects/p1/query",
            "/api/projects/p1/semantic-query",
            "/api/projects/p1/semantic/cohort",
            "/api/projects/p1/semantic/metric-tree/explain",
        ] {
            assert!(is_read_only_post(path), "{path}");
        }
        for path in [
            "/api/customer-apps/a1/events",
            "/api/projects/p1/threads",
            "/api/projects/p1/procedures/x/runs",
            "/api/projects/p1/semantic/metric-tree/",
            "/api/projects/p1/semantic/metric-tree/a/b",
            "/api/projects/p1/anomalies/scan",
        ] {
            assert!(!is_read_only_post(path), "{path}");
        }
    }

    fn status_of(method: Method, path: &str, h: &HeaderMap) -> Option<StatusCode> {
        guard(&method, path, h).err().map(|r| r.status())
    }

    #[test]
    fn api_writes_are_refused_outside_production_and_reads_are_not() {
        let staging = headers(&[("host", STAGING_HOST), ("cookie", "oxy_session=x")]);
        assert_eq!(
            status_of(Method::POST, "/api/customer-apps/a1/events", &staging),
            Some(StatusCode::FORBIDDEN)
        );
        assert_eq!(
            status_of(Method::DELETE, "/api/threads/t1", &staging),
            Some(StatusCode::FORBIDDEN)
        );
        assert_eq!(
            status_of(Method::GET, "/api/projects/p1/threads", &staging),
            None
        );
        assert_eq!(
            status_of(Method::POST, "/api/projects/p1/query", &staging),
            None
        );
        // A non-API path is not this middleware's to refuse; `/fn` decides itself.
        assert_eq!(
            status_of(Method::POST, "/customer-apps/acme/store/fn/x", &staging),
            None
        );
    }

    #[test]
    fn production_writes_pass_unless_a_staging_page_sent_them_with_the_cookie() {
        let prod = headers(&[
            ("host", PROD_HOST),
            ("cookie", "oxy_session=x"),
            ("origin", &format!("https://{PROD_HOST}")),
        ]);
        assert_eq!(
            status_of(Method::POST, "/api/customer-apps/a1/events", &prod),
            None
        );
        let from_staging = headers(&[
            ("host", "app.oxygen-hq.com"),
            ("cookie", "oxy_session=x"),
            ("origin", &format!("https://{STAGING_HOST}")),
        ]);
        assert_eq!(
            status_of(Method::POST, "/api/customer-apps/a1/events", &from_staging),
            Some(StatusCode::FORBIDDEN)
        );
        let bad_header = headers(&[("cookie", "oxy_session=x"), (ENV_HEADER, "staging")]);
        assert_eq!(
            status_of(Method::GET, "/api/projects/p1/threads", &bad_header),
            Some(StatusCode::BAD_REQUEST)
        );
    }

    #[test]
    fn a_staging_host_lets_the_ask_routes_through_and_nothing_else() {
        let staging = headers(&[
            ("host", STAGING_HOST),
            ("cookie", "oxy_session=x"),
            ("origin", &format!("https://{STAGING_HOST}")),
        ]);
        for path in [
            "/api/projects/p1/agents/analyst/asks",
            "/api/projects/p1/agents/asks/run-1/cancel",
        ] {
            assert_eq!(status_of(Method::POST, path, &staging), None, "{path}");
        }
        for (method, path) in [
            (Method::POST, "/api/projects/p1/threads"),
            (Method::POST, "/api/customer-apps/a1/events"),
            // Starting an automation run is named too (its own test below) —
            // not this surface's.
            (Method::PUT, "/api/projects/p1/agents/analyst/asks"),
            (Method::DELETE, "/api/projects/p1/agents/asks/run-1/cancel"),
            (Method::POST, "/api/projects/p1/agents/analyst/asks/extra"),
            (Method::POST, "/api/projects/p1/agents//asks"),
            (Method::POST, "/api/projects/p1/agents/asks/run-1"),
            (Method::POST, "/api/projects/p1/agents/a/b/asks"),
        ] {
            assert_eq!(
                status_of(method.clone(), path, &staging),
                Some(StatusCode::FORBIDDEN),
                "{method} {path}"
            );
        }
    }

    #[test]
    fn a_staging_host_lets_the_automation_run_route_through_and_nothing_else() {
        let staging = headers(&[
            ("host", STAGING_HOST),
            ("cookie", "oxy_session=x"),
            ("origin", &format!("https://{STAGING_HOST}")),
        ]);
        assert_eq!(
            status_of(
                Method::POST,
                "/api/projects/p1/procedures/weekly/runs",
                &staging
            ),
            None
        );
        for (method, path) in [
            // Wrong method on the right path — a GET is never a write, so it
            // is never refused either; that is `is_write_method`'s job, not
            // this matcher's, so PUT exercises this one.
            (Method::PUT, "/api/projects/p1/procedures/weekly/runs"),
            // Trailing slash.
            (Method::POST, "/api/projects/p1/procedures/weekly/runs/"),
            // Extra segment.
            (
                Method::POST,
                "/api/projects/p1/procedures/weekly/runs/extra",
            ),
            // Empty procedure id.
            (Method::POST, "/api/projects/p1/procedures//runs"),
            // The poll and cancel routes are not named here.
            (Method::POST, "/api/projects/p1/procedures/runs/run-1"),
            (
                Method::POST,
                "/api/projects/p1/procedures/runs/run-1/cancel",
            ),
        ] {
            assert_eq!(
                status_of(method.clone(), path, &staging),
                Some(StatusCode::FORBIDDEN),
                "{method} {path}"
            );
        }
    }

    #[test]
    fn a_dev_slot_automation_run_is_still_refused() {
        let dev = headers(&[
            ("host", "app.oxygen-hq.com"),
            ("authorization", "Bearer t"),
            (ENV_HEADER, "dev-luong"),
        ]);
        assert_eq!(
            status_of(
                Method::POST,
                "/api/projects/p1/procedures/weekly/runs",
                &dev
            ),
            Some(StatusCode::FORBIDDEN)
        );
    }

    #[test]
    fn a_dev_slot_ask_is_still_refused() {
        let dev = headers(&[
            ("host", "app.oxygen-hq.com"),
            ("authorization", "Bearer t"),
            (ENV_HEADER, "dev-luong"),
        ]);
        assert_eq!(
            status_of(Method::POST, "/api/projects/p1/agents/analyst/asks", &dev),
            Some(StatusCode::FORBIDDEN)
        );
    }

    /// The allowance is staging's own: it does not excuse the origin check, so
    /// a staging page cannot start a production ask with the cookie.
    #[test]
    fn a_staging_page_cannot_start_a_production_ask_with_the_cookie() {
        let from_staging = headers(&[
            ("host", PROD_HOST),
            ("cookie", "oxy_session=x"),
            ("origin", &format!("https://{STAGING_HOST}")),
        ]);
        assert_eq!(
            status_of(
                Method::POST,
                "/api/projects/p1/agents/analyst/asks",
                &from_staging
            ),
            Some(StatusCode::FORBIDDEN)
        );
        let prod = headers(&[("host", PROD_HOST), ("cookie", "oxy_session=x")]);
        assert_eq!(
            status_of(Method::POST, "/api/projects/p1/agents/analyst/asks", &prod),
            None,
            "production asks are unchanged"
        );
    }
}
