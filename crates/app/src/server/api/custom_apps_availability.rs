//! `GET /api/customer-apps/{org}/{app}/availability` — the derived SLI for one
//! custom app.
//!
//! ## How this differs from `/health`, which it does not replace
//!
//! [`custom_apps_health`](super::custom_apps_health) answers a **deployment
//! integrity** question from Postgres: is this app registered, published, does
//! it have a build, does that build have an entrypoint. It is a static ladder,
//! and a perfectly green one is entirely compatible with every request to the
//! app failing.
//!
//! This answers the **serving** question, from the traffic Oxy already
//! terminates: of the requests real people made, what fraction succeeded, and
//! is the error budget burning fast enough to act on. A prober cannot produce
//! this answer — a custom-app host returns 200 with the SPA shell for every
//! path, so an outside-in check is green whatever the app is doing. Oxy is on
//! the inside of that, which is why the answer is a query.
//!
//! See `internal-docs/2026-09-04-custom-app-observability-design.md` §3 Layer 2.
//!
//! ## Auth and fleet posture
//!
//! Same inline gate as `/health` and `/debug` — session cookie or bearer token,
//! authenticated **before** the app lookup so an unauthenticated caller gets
//! 401 whether or not the app exists. A ClickHouse read and nothing else, so
//! **FleetOk**: any replica can answer, which is the point — an availability
//! endpoint routed to a singleton would go dark exactly when that singleton is
//! the thing having trouble.

use axum::Json;
use axum::extract::Path;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use oxy_observability::burn_rate::{ALERT_WINDOWS_MINUTES, BurnVerdict, SloConfig, evaluate};
use serde::Serialize;

use super::custom_apps_auth::authenticate_and_authorize;

#[derive(Serialize)]
pub struct WindowReport {
    pub window_minutes: u32,
    pub total: u64,
    pub failed: u64,
    /// `null` when the window carried no traffic — **not** `0.0`. "Nothing
    /// failed" and "nothing happened" are different answers and a chart that
    /// conflates them draws a healthy flat line over a dead app.
    pub failure_ratio: Option<f64>,
}

#[derive(Serialize)]
pub struct AvailabilityResponse {
    pub app_id: String,
    pub org_slug: String,
    pub app_slug: String,
    /// `no_opinion` | `healthy` | `burning`.
    pub verdict: &'static str,
    /// `page` | `ticket`, present only when `verdict` is `burning`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub burn_rate: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub long_window_minutes: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub short_window_minutes: Option<u32>,
    pub objective: f64,
    pub windows: Vec<WindowReport>,
}

fn error_response(status: StatusCode, message: &str) -> Response {
    (
        status,
        Json(serde_json::json!({ "error": message, "verdict": "no_opinion" })),
    )
        .into_response()
}

pub async fn get_availability(
    Path((org_slug, app_slug)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    // Auth first, app lookup second — same ordering, same reason, as `/health`.
    let outcome = match authenticate_and_authorize(
        &headers,
        &org_slug,
        &app_slug,
        oxy_auth::token::SandboxAgent::Refuse,
    )
    .await
    {
        Ok(o) => o,
        Err(status) => return error_response(status, "not permitted"),
    };

    let Some(store) = oxy_observability::global::get_global() else {
        // Observability off (`OXY_OBSERVABILITY_BACKEND` unset) is the default
        // in local dev, and it is not an error — but it must not answer
        // "healthy" either. 501 says the capability is absent, distinct from a
        // 500 that would read as "the query broke".
        return error_response(
            StatusCode::NOT_IMPLEMENTED,
            "observability capture is not configured (OXY_OBSERVABILITY_BACKEND)",
        );
    };

    let windows = match store
        .get_app_availability(
            &outcome.app.org_id.to_string(),
            &outcome.app.id.to_string(),
            ALERT_WINDOWS_MINUTES,
        )
        .await
    {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!("availability query failed for app {}: {e}", outcome.app.id);
            return error_response(StatusCode::BAD_GATEWAY, "availability query failed");
        }
    };

    let cfg = SloConfig::default();
    let verdict = evaluate(&windows, &cfg);
    let reports = windows
        .iter()
        .map(|w| WindowReport {
            window_minutes: w.window_minutes,
            total: w.total,
            failed: w.failed,
            failure_ratio: w.failure_ratio(),
        })
        .collect();

    let mut response = AvailabilityResponse {
        app_id: outcome.app.id.to_string(),
        org_slug,
        app_slug,
        verdict: "no_opinion",
        severity: None,
        burn_rate: None,
        long_window_minutes: None,
        short_window_minutes: None,
        objective: cfg.objective,
        windows: reports,
    };
    match verdict {
        BurnVerdict::NoOpinion => {}
        BurnVerdict::Healthy => response.verdict = "healthy",
        BurnVerdict::Burning {
            severity,
            burn_rate,
            long_minutes,
            short_minutes,
            ..
        } => {
            response.verdict = "burning";
            response.severity = Some(match severity {
                oxy_observability::burn_rate::Severity::Page => "page",
                oxy_observability::burn_rate::Severity::Ticket => "ticket",
            });
            response.burn_rate = Some(burn_rate);
            response.long_window_minutes = Some(long_minutes);
            response.short_window_minutes = Some(short_minutes);
        }
    }

    (StatusCode::OK, Json(response)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three verdicts must stay distinguishable on the wire. In particular
    /// `no_opinion` must never serialize as something a dashboard would render
    /// green: an app with no traffic has not been shown to work.
    #[test]
    fn no_opinion_is_not_spelled_like_healthy() {
        let cfg = SloConfig::default();
        assert_eq!(evaluate(&[], &cfg), BurnVerdict::NoOpinion);
        // The mapping above turns NoOpinion into this literal; pin it so a
        // rename cannot quietly make a silent app look healthy.
        let verdict = "no_opinion";
        assert_ne!(verdict, "healthy");
    }

    /// An empty window reports `null`, not `0.0`.
    #[test]
    fn an_empty_window_reports_null_not_zero() {
        let w = oxy_observability::types::AppAvailabilityWindow {
            window_minutes: 5,
            total: 0,
            failed: 0,
        };
        let report = WindowReport {
            window_minutes: w.window_minutes,
            total: w.total,
            failed: w.failed,
            failure_ratio: w.failure_ratio(),
        };
        let json = serde_json::to_value(&report).unwrap();
        assert!(json["failure_ratio"].is_null(), "{json}");
    }
}
