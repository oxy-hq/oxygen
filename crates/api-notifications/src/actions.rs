//! `POST /api/notifications/{id}/actions/{action_id}` — tap one of a
//! notification's buttons, as me.
//!
//! An "open" action (no function) answers the deep link to follow. A function
//! action calls the app's function as the caller ([`super::action_call`])
//! with `{ notificationId, action, link }` and answers its JSON. Either marks
//! the notification read — a function action only once it succeeded, so a
//! failed one leaves the notification waiting. A function route that refuses
//! the caller's credential or permission is `403 FunctionRefused` here, never
//! a `401`: the web app signs a person out on any `401`.
//!
//! The action must be in the catalogue the notification is read against
//! today: Oxygen's, or the app's LIVE build. One the live build no longer
//! declares is `409`, never run from a stale client's memory. A write, so it
//! runs only in production: a staging or sandbox app host is refused like
//! every other write there (`EnvironmentRefused`).

use axum::Extension;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use entity::{apps, notifications, organizations};
use oxy::database::client::establish_connection;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::{DatabaseConnection, EntityTrait};
use serde_json::{Value, json};
use tracing::instrument;
use uuid::Uuid;

use super::action_call::{FunctionCall, Outcome, invoke};
use super::catalog::model::valid_segment;
use super::catalog::platform::oxygen;
use super::inbox_view::Links;
use oxy_app_core::AppState;
use oxy_app_core::function_seam::FunctionRuntime;
use oxy_app_core::preagg_cache_ctx::PreaggCacheCtx;

/// What tapping an action does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// Follow the link: absolute on the admin host when the deployment knows
    /// it, else the relative `href`; `None` when the notification has none.
    Open {
        url: Option<String>,
    },
    Function(FunctionCall),
}

/// Why an action cannot run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// Not my notification, or not an action its kind could have.
    NotFound,
    /// The app's live build does not declare it (any more).
    NotDeclared(String),
    Internal,
}

impl IntoResponse for Refused {
    fn into_response(self) -> Response {
        let (status, error, message) = match self {
            Refused::NotFound => (
                StatusCode::NOT_FOUND,
                "NotFound",
                "no such notification or action".into(),
            ),
            Refused::NotDeclared(m) => (StatusCode::CONFLICT, "ActionNotDeclared", m),
            Refused::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Internal",
                "could not read the notification; retry shortly".into(),
            ),
        };
        (
            status,
            Json(json!({ "ok": false, "error": error, "message": message })),
        )
            .into_response()
    }
}

/// `POST /api/notifications/{id}/actions/{action_id}`
#[instrument(skip_all, fields(id = %id, action = %action_id))]
pub async fn run(
    State(state): State<AppState>,
    runtime: Option<Extension<std::sync::Arc<dyn FunctionRuntime>>>,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path((id, action_id)): Path<(Uuid, String)>,
    headers: HeaderMap,
) -> Response {
    // Production only, decided from the headers alone before anything is read.
    let path = format!("/api/notifications/{id}/actions/{action_id}");
    if let Err(refused) =
        oxy_app_core::custom_app_env_request::guard(&Method::POST, &path, &headers)
    {
        return *refused;
    }
    let Ok(db) = establish_connection().await else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let links = Links::for_request(&headers);
    let (row, plan) = match plan(&db, &user, id, &action_id, &links).await {
        Ok(planned) => planned,
        Err(refused) => return refused.into_response(),
    };
    let answer = match plan {
        Plan::Open { url } => json!({ "ok": true, "result": null, "url": url }),
        Plan::Function(call) => {
            let preagg = PreaggCacheCtx {
                cache: state.preagg_cache.clone(),
                renewal_threshold_secs: state.preagg_renewal_threshold_secs,
            };
            // No runtime on this server (a composition that mounted none): the
            // function did not run, which is what a failed action says.
            let Some(Extension(runtime)) = runtime else {
                return failed(Outcome::Failed(
                    "the function runtime is not available on this server".into(),
                ));
            };
            match invoke(&call, &headers, preagg, runtime.as_ref()).await {
                Outcome::Done(result) => json!({ "ok": true, "result": result, "url": null }),
                other => return failed(other),
            }
        }
    };
    if super::handlers::set_read(&db, row).await.is_err() {
        // The action ran; failing it now would invite a second tap.
        tracing::warn!(%id, "action ran but the notification could not be marked read");
    }
    Json(answer).into_response()
}

/// What a function action answers when the function route refused the
/// caller's own credential (`401`) or their right to run it (`403`).
///
/// Never `401`: this route has already authenticated the caller, and the web
/// app reads any `401` as an expired session — it clears the session and
/// signs the person out. A function route that will not take the credential
/// it was handed is a refusal of the action, not of the session.
const FUNCTION_REFUSED: &str = "FunctionRefused";

/// A function action that did not succeed: the function route's own refusal
/// with its status — a `401` or `403` as `403 FunctionRefused` — or `502` for
/// a function that ran and failed.
fn failed(outcome: Outcome) -> Response {
    let (status, error, message) = match outcome {
        Outcome::Refused(status, body)
            if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) =>
        {
            let message = match (status, body["message"].as_str()) {
                (StatusCode::FORBIDDEN, Some(said)) => said.to_string(),
                (StatusCode::FORBIDDEN, None) => "you cannot run this app's function".into(),
                _ => "the app's function did not accept your sign-in".into(),
            };
            (StatusCode::FORBIDDEN, FUNCTION_REFUSED.into(), message)
        }
        Outcome::Refused(status, body) => (
            status,
            body["error"]
                .as_str()
                .unwrap_or(FUNCTION_REFUSED)
                .to_string(),
            body["message"]
                .as_str()
                .unwrap_or("the app's function refused the call")
                .to_string(),
        ),
        Outcome::Failed(message) => (StatusCode::BAD_GATEWAY, "FunctionFailed".into(), message),
        Outcome::Done(_) => unreachable!("a success is answered by the caller"),
    };
    let body = json!({ "ok": false, "error": error, "message": message, "result": Value::Null });
    (status, Json(body)).into_response()
}

/// Resolve what tapping `action_id` on notification `id` does, for `user`.
/// The notification row comes back with it, to be marked read.
pub async fn plan(
    db: &DatabaseConnection,
    user: &AuthenticatedUser,
    id: Uuid,
    action_id: &str,
    links: &Links,
) -> Result<(notifications::Model, Plan), Refused> {
    let row = super::inbox::mine(db, user, id)
        .await
        .map_err(|s| match s {
            StatusCode::NOT_FOUND => Refused::NotFound,
            _ => Refused::Internal,
        })?;
    if !valid_segment(action_id) {
        return Err(Refused::NotFound);
    }
    let mut items = super::inbox_view::items(db, vec![row.clone()])
        .await
        .map_err(|_| Refused::Internal)?;
    links.apply(&mut items);
    let item = items.pop().ok_or(Refused::Internal)?;
    let Some(app_id) = row.app_id else {
        // Oxygen's catalogue is code: an action it lacks never existed.
        oxygen()
            .find_action(&row.kind, action_id)
            .ok_or(Refused::NotFound)?;
        let url = item.url.or(item.href);
        return Ok((row, Plan::Open { url }));
    };
    let not_declared = || {
        Refused::NotDeclared(format!(
            "the app's live build does not declare the action {action_id:?} for kind {:?}",
            row.kind
        ))
    };
    let app = apps::Entity::find_by_id(app_id)
        .one(db)
        .await
        .map_err(|_| Refused::Internal)?
        .ok_or_else(not_declared)?;
    let catalogue = super::catalog::live::load(db, &app)
        .await
        .map_err(|_| Refused::Internal)?;
    let action = catalogue
        .find_action(&row.kind, action_id)
        .ok_or_else(not_declared)?;
    let Some(function) = action.function else {
        let url = item.url.or(item.href);
        return Ok((row, Plan::Open { url }));
    };
    let org_slug = organizations::Entity::find_by_id(app.org_id)
        .one(db)
        .await
        .map_err(|_| Refused::Internal)?
        .ok_or(Refused::Internal)?
        .slug;
    let call = FunctionCall {
        org_slug,
        app_slug: app.slug,
        function,
        notification_id: row.id,
        action: action.id,
        link: item.link,
    };
    Ok((row, Plan::Function(call)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn guard(host: &str, env_header: Option<&str>) -> Result<(), StatusCode> {
        let mut headers = HeaderMap::new();
        headers.insert("host", HeaderValue::from_str(host).unwrap());
        headers.insert("authorization", HeaderValue::from_static("Bearer t"));
        if let Some(env) = env_header {
            headers.insert("x-oxy-app-env", HeaderValue::from_str(env).unwrap());
        }
        oxy_app_core::custom_app_env_request::guard(
            &Method::POST,
            "/api/notifications/00000000-0000-0000-0000-000000000000/actions/ack",
            &headers,
        )
        .map(|_| ())
        .map_err(|r| r.status())
    }

    #[test]
    fn an_action_is_a_write_refused_outside_production() {
        assert_eq!(guard("app.oxygen-hq.com", None), Ok(()));
        assert_eq!(
            guard("acme--store.customer-apps.oxygen-hq.com", None),
            Ok(())
        );
        assert_eq!(
            guard("staging--acme--store.customer-apps.oxygen-hq.com", None),
            Err(StatusCode::FORBIDDEN)
        );
        assert_eq!(
            guard("app.oxygen-hq.com", Some("staging")),
            Err(StatusCode::FORBIDDEN),
            "a bearer naming staging is refused on the admin host too"
        );
    }

    #[test]
    fn refusals_answer_404_409_and_never_ok() {
        let r = Refused::NotDeclared("gone".into()).into_response();
        assert_eq!(r.status(), StatusCode::CONFLICT);
        assert_eq!(
            Refused::NotFound.into_response().status(),
            StatusCode::NOT_FOUND
        );
        let r = failed(Outcome::Failed("boom".into()));
        assert_eq!(r.status(), StatusCode::BAD_GATEWAY);
        let r = failed(Outcome::Refused(
            StatusCode::TOO_MANY_REQUESTS,
            json!({ "error": "RateLimitExceeded", "message": "too many requests" }),
        ));
        assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    async fn answer(r: Response) -> (StatusCode, Value) {
        let status = r.status();
        let bytes = axum::body::to_bytes(r.into_body(), usize::MAX)
            .await
            .expect("body");
        (status, serde_json::from_slice(&bytes).expect("json"))
    }

    /// The web app signs a person out on any `401`, so a function route that
    /// refuses the forwarded credential must never come back as one.
    #[tokio::test]
    async fn a_refused_credential_or_permission_is_403_function_refused_never_401() {
        let (status, body) = answer(failed(Outcome::Refused(
            StatusCode::UNAUTHORIZED,
            Value::Null,
        )))
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["error"], FUNCTION_REFUSED);
        assert_eq!(body["ok"], false);
        assert_eq!(
            body["message"],
            "the app's function did not accept your sign-in"
        );

        let (status, body) =
            answer(failed(Outcome::Refused(StatusCode::FORBIDDEN, Value::Null))).await;
        assert_eq!(
            (status, body["error"].as_str()),
            (StatusCode::FORBIDDEN, Some(FUNCTION_REFUSED))
        );
        assert_eq!(body["message"], "you cannot run this app's function");

        // The route's own words for a 403 are kept; its code is not.
        let said = json!({ "error": "CrossEnvironmentOrigin", "message": "not from staging" });
        let (status, body) = answer(failed(Outcome::Refused(StatusCode::FORBIDDEN, said))).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(
            (body["error"].as_str(), body["message"].as_str()),
            (Some(FUNCTION_REFUSED), Some("not from staging"))
        );
    }
}
