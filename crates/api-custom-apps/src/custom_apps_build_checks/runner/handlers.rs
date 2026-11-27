//! The handlers of the runner routes that take JSON (the file route is
//! `super::super::files`): admit, parse, admit for the environment, call,
//! answer.
//!
//! Each refuses a sandbox agent token before it runs ([`RefuseSandboxAgent`])
//! and admits its caller before it reads the body
//! (`super::super::admit::runner`), so a caller who may not drive the build's
//! checks learns nothing from the shape of a body: the bare not-found an
//! unknown build gets. The body is read as bytes and parsed after that, for
//! the same reason; it names the attempt's environment, and the caller is
//! then held to that environment (`admit::runner_in`).
//!
//! **Two kinds of caller pass, and they are told apart** ([`is_checker`]): the
//! platform checker, whose run names itself and whose verdicts are the
//! platform's own, and anyone else with non-production reach, running by
//! hand. A run by hand is allowed and leaves a trace: the platform names its
//! runner from who called (`claim::Claimant::runner`), and its report is
//! audited (`report::ReportedBy::Hand`).

use axum::Json;
use axum::body::Bytes;
use axum::extract::Path;
use axum::http::{HeaderMap, StatusCode};
use entity::{app_builds, apps};
use oxy::database::client::establish_connection;
use oxy_app_core::agent_refusal::RefuseSandboxAgent;
use oxy_app_core::audit::RequestActor;
use oxy_app_core::config::BootConfig;
use oxy_auth::types::AppPublishTokenAuth;
use sea_orm::DatabaseConnection;
use serde::Serialize;
use serde::de::DeserializeOwned;
use uuid::Uuid;

use super::super::admit;
use super::super::design_system::{self, Source, StylesheetsError};
use super::super::environment::CheckEnvironment;
use super::super::error::{ChecksError, internal};
use super::super::requester::Requester;
use super::super::store;
use super::claim::{self, ClaimAnswer, ClaimRequest, Claimant};
use super::report::{self, ReportAnswer, ReportRequest, ReportedBy};
use super::settle::{self, SettleAnswer, SettleRequest};
use crate::admin::apps::environment_scope::Caller;

type Marker = Option<axum::Extension<AppPublishTokenAuth>>;

/// The app and build of a runner route, for a caller who may drive them.
async fn admitted(
    actor: &RequestActor,
    marker: &Marker,
    (id, build): (Uuid, String),
) -> Result<(DatabaseConnection, apps::Model, app_builds::Model), ChecksError> {
    let db = establish_connection().await.map_err(internal)?;
    let caller = Caller::new(&actor.user, marker.as_ref());
    let (app, build) = admit::runner(&db, &caller, id, &build).await?;
    Ok((db, app, build))
}

/// The environment a parsed body names, for a caller who may drive an
/// attempt there (`admit::runner_in`).
async fn in_environment(
    db: &DatabaseConnection,
    (actor, marker): (&RequestActor, &Marker),
    app: &apps::Model,
    named: Option<&str>,
) -> Result<CheckEnvironment, ChecksError> {
    let environment = CheckEnvironment::named(named)?;
    let caller = Caller::new(&actor.user, marker.as_ref());
    admit::runner_in(db, &caller, app, environment).await?;
    Ok(environment)
}

/// A JSON body; an empty one reads as `{}`.
fn parsed<T: DeserializeOwned>(body: &Bytes) -> Result<T, ChecksError> {
    let raw: &[u8] = if body.is_empty() { b"{}" } else { body };
    serde_json::from_slice(raw).map_err(|e| {
        ChecksError::bad_request("invalid_body", format!("the body is not valid: {e}"))
    })
}

/// The origin a request names for itself: the forwarded scheme and host its
/// proxy saw, else its own `Host` over https.
pub fn request_origin(headers: &HeaderMap) -> String {
    let first = |name: &str| {
        let value = headers.get(name)?.to_str().ok()?;
        let first = value.split(',').next()?.trim();
        (!first.is_empty()).then(|| first.to_string())
    };
    let scheme = first("x-forwarded-proto")
        .map(|scheme| scheme.to_ascii_lowercase())
        .filter(|scheme| scheme == "http" || scheme == "https")
        .unwrap_or_else(|| "https".to_string());
    let host = first("x-forwarded-host")
        .or_else(|| first("host"))
        .unwrap_or_else(|| "localhost".to_string());
    format!("{scheme}://{host}")
}

/// This deployment's origin: the configured API URL's (`OXY_API_URL`), else
/// the request's own.
fn deployment_origin(headers: &HeaderMap) -> String {
    match BootConfig::current().hosts.api_url_parsed() {
        Some(api) => api.origin().ascii_serialization(),
        None => request_origin(headers),
    }
}

/// What the checker's service token is called in a check's evidence.
const CHECKER: &str = "app_checker";

/// Whether the request rode in on the platform checker's service token
/// (`oxy_auth::token::presents_app_checker`). It names the credential and
/// admits nothing: who may call was decided by `admit`.
fn is_checker(headers: &HeaderMap) -> bool {
    oxy_auth::token::presents_app_checker(headers)
}

/// How the platform names the credential a request rode in on: the checker's
/// service token by its role, anyone else as every log names them.
fn identity(actor: &RequestActor, headers: &HeaderMap) -> String {
    if is_checker(headers) {
        CHECKER.to_string()
    } else {
        actor.user.label().to_string()
    }
}

/// `POST /api/customer-apps/{id}/builds/{build}/checks/claim`.
pub async fn claim(
    _: RefuseSandboxAgent,
    Requester { actor, marker }: Requester,
    Path(path): Path<(Uuid, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<ClaimAnswer>, ChecksError> {
    let (db, app, build) = admitted(&actor, &marker, path).await?;
    let request: ClaimRequest = parsed(&body)?;
    let named = request.environment.as_deref();
    let environment = in_environment(&db, (&actor, &marker), &app, named).await?;
    let (identity, origin) = (identity(&actor, &headers), deployment_origin(&headers));
    let by = Claimant {
        identity: &identity,
        checker: is_checker(&headers),
        origin: &origin,
    };
    let answer = claim::claim(&db, (&app, &build), &request, environment, by).await?;
    tracing::info!(%identity, app_id = %app.id, attempt = answer.attempt,
        "build checks: claimed by");
    Ok(Json(answer))
}

/// `POST /api/customer-apps/{id}/builds/{build}/checks/functions/settle`.
pub async fn settle(
    _: RefuseSandboxAgent,
    actor: RequestActor,
    marker: Marker,
    Path(path): Path<(Uuid, String)>,
    body: Bytes,
) -> Result<Json<SettleAnswer>, ChecksError> {
    let (db, app, build) = admitted(&actor, &marker, path).await?;
    let request: SettleRequest = parsed(&body)?;
    let named = request.environment.as_deref();
    let environment = in_environment(&db, (&actor, &marker), &app, named).await?;
    let settled = settle::settle(&db, (&app, &build), &request, environment).await?;
    Ok(Json(settled))
}

#[derive(Debug, Serialize)]
pub struct Stylesheets {
    pub sources: Vec<Source>,
}

/// `GET /api/customer-apps/{id}/builds/{build}/checks/stylesheets`. Only a
/// build that reached staging has checks, and only such a build's CSS is
/// read here: a staging attempt's route, held to staging's door.
pub async fn stylesheets(
    _: RefuseSandboxAgent,
    actor: RequestActor,
    marker: Marker,
    Path(path): Path<(Uuid, String)>,
) -> Result<Json<Stylesheets>, ChecksError> {
    let (db, app, build) = admitted(&actor, &marker, path).await?;
    let staging = in_environment(&db, (&actor, &marker), &app, None).await?;
    if store::latest_attempt(&db, build.id, staging.name())
        .await?
        .is_none()
    {
        return Err(ChecksError::NotFound);
    }
    let sources = design_system::stylesheets(&build)
        .await
        .map_err(|e| match e {
            StylesheetsError::TooLarge { .. } => ChecksError::coded(
                StatusCode::PAYLOAD_TOO_LARGE,
                "stylesheets_too_large",
                e.to_string(),
            ),
            StylesheetsError::Store(e) => internal(e),
        })?;
    Ok(Json(Stylesheets { sources }))
}

/// `POST /api/customer-apps/{id}/builds/{build}/checks/report`.
pub async fn report(
    _: RefuseSandboxAgent,
    Requester { actor, marker }: Requester,
    Path(path): Path<(Uuid, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<ReportAnswer>, ChecksError> {
    let (db, app, build) = admitted(&actor, &marker, path).await?;
    let request: ReportRequest = parsed(&body)?;
    let named = request.environment.as_deref();
    let environment = in_environment(&db, (&actor, &marker), &app, named).await?;
    let by = if is_checker(&headers) {
        ReportedBy::Checker
    } else {
        ReportedBy::Hand(&actor)
    };
    let reported = report::report(&db, (&app, &build), &request, environment, by).await?;
    Ok(Json(reported))
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.insert(*name, HeaderValue::from_static(value));
        }
        headers
    }

    #[test]
    fn a_requests_origin_is_what_its_proxy_saw_else_its_host() {
        let forwarded = headers(&[
            ("host", "oxy-serve:3000"),
            ("x-forwarded-host", "app.example.com, 10.0.0.1"),
            ("x-forwarded-proto", "HTTP, https"),
        ]);
        assert_eq!(request_origin(&forwarded), "http://app.example.com");
        let direct = headers(&[("host", "app.example.com")]);
        assert_eq!(request_origin(&direct), "https://app.example.com");
        let odd = headers(&[("host", "h.example"), ("x-forwarded-proto", "gopher")]);
        assert_eq!(request_origin(&odd), "https://h.example");
        assert_eq!(request_origin(&HeaderMap::new()), "https://localhost");
    }

    #[test]
    fn an_empty_body_reads_as_an_empty_object_and_a_bad_one_is_refused() {
        let newest: SettleRequest = parsed(&Bytes::new()).unwrap();
        assert_eq!(newest.attempt, None);
        let named: SettleRequest = parsed(&Bytes::from_static(b"{\"attempt\": 2}")).unwrap();
        assert_eq!(named.attempt, Some(2));
        let no_runner = parsed::<ClaimRequest>(&Bytes::new()).unwrap_err();
        assert_eq!(no_runner.code(), Some("invalid_body"));
        let not_json = parsed::<ReportRequest>(&Bytes::from_static(b"nope")).unwrap_err();
        assert_eq!(not_json.code(), Some("invalid_body"));
    }
}
