//! `POST /api/webhooks/github` — the GitHub App's webhook receiver.
//!
//! Today it acts on one event, `push`: a pushed default branch queues a commit
//! compile for every workspace on that repository
//! ([`crate::server::compile_reconcile::push`]), so a merged pull request
//! reaches production without anyone clicking Compile. `ping` is answered so
//! GitHub's "Redeliver"/setup check shows green, and every other event the App
//! may be subscribed to is acknowledged and ignored.
//!
//! Anonymous by necessity, so the sender is authenticated instead: GitHub signs
//! the raw body with the App's webhook secret (`GITHUB_WEBHOOK_SECRET`) and
//! sends it as `X-Hub-Signature-256: sha256=<hex>`. Unconfigured is `404`
//! (the route does not exist on this deployment, as for Stripe), a missing or
//! wrong signature is `401`, and nothing is read from a body before it passes.
//!
//! `FleetOk`: it reads Postgres and queues a task, so any replica serves it.
//! Whether a verified push does anything is decided by the same gate as the
//! periodic check (`OXY_COMPILE_RECONCILE`); with it off the delivery is
//! acknowledged `202` and nothing is queued.

use axum::body::Bytes;
use axum::http::{HeaderMap, StatusCode};
use hmac::{Hmac, KeyInit, Mac};
use oxy::database::client::establish_connection;
use sha2::Sha256;

use crate::server::compile_reconcile::push::{self, PushEvent};

/// The App's webhook secret, as set in the GitHub App's settings.
const SECRET_ENV: &str = "GITHUB_WEBHOOK_SECRET";

pub async fn github_webhook(headers: HeaderMap, body: Bytes) -> (StatusCode, String) {
    let Some(secret) = std::env::var(SECRET_ENV).ok().filter(|s| !s.is_empty()) else {
        tracing::debug!("/webhooks/github called but {SECRET_ENV} is not set");
        return (StatusCode::NOT_FOUND, String::new());
    };
    let signature = headers
        .get("x-hub-signature-256")
        .and_then(|v| v.to_str().ok());
    if !signature_is_valid(secret.as_bytes(), &body, signature) {
        tracing::warn!("github webhook signature invalid");
        return (StatusCode::UNAUTHORIZED, "invalid signature".to_string());
    }
    let event = headers
        .get("x-github-event")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    match event {
        "ping" => (StatusCode::OK, "pong".to_string()),
        "push" => handle_push(&body).await,
        other => (StatusCode::ACCEPTED, format!("ignored: {other} events")),
    }
}

async fn handle_push(body: &[u8]) -> (StatusCode, String) {
    let push: PushEvent = match serde_json::from_slice(body) {
        Ok(push) => push,
        Err(e) => {
            tracing::warn!(error = %e, "github push payload malformed");
            return (
                StatusCode::BAD_REQUEST,
                format!("invalid push payload: {e}"),
            );
        }
    };
    let db = match establish_connection().await {
        Ok(db) => db,
        Err(e) => {
            tracing::error!(error = %e, "github push: no database connection");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "database unavailable".into(),
            );
        }
    };
    match push::handle(&db, &push).await {
        Ok(handled) => {
            tracing::info!(
                target: "compile_reconcile",
                repository = %push.repository.full_name,
                git_ref = %push.git_ref,
                outcome = %handled.describe(),
                "github push handled"
            );
            (StatusCode::ACCEPTED, handled.describe())
        }
        Err(e) => {
            tracing::error!(error = %e, "github push could not be handled");
            // GitHub does not redeliver on its own; a 500 shows the failure
            // under "Recent deliveries", and the periodic check covers it.
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not queue compiles".into(),
            )
        }
    }
}

/// Whether `header` is `sha256=<hex>` of the HMAC-SHA256 of `body` under
/// `secret`. Compared in constant time.
pub fn signature_is_valid(secret: &[u8], body: &[u8], header: Option<&str>) -> bool {
    let Some(provided) = header
        .and_then(|h| h.strip_prefix("sha256="))
        .and_then(|hex| hex::decode(hex).ok())
    else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret) else {
        return false;
    };
    mac.update(body);
    mac.verify_slice(&provided).is_ok()
}

#[cfg(test)]
#[path = "github_tests.rs"]
mod tests;
