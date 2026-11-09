//! Check a candidate world without persisting anything.
//!
//! Neither *what happens* ([`super::worlds`]) nor *what someone does about it*
//! ([`super::runs`]) — this is a third thing: whether a spec nobody has
//! written to a `.simulation.yml` yet is even coherent. It exists so a UI form
//! can point at the one field that's wrong (an unreachable optimum, an
//! absorbing lever floor, too little history to clear the fitter's floor)
//! before a run ever gets queued, rather than after one fails minutes later.
//!
//! Pure computation over the request body — no filesystem, no git, no
//! per-instance state — which is what keeps this `FleetOk` alongside its
//! siblings: it is mounted by `oxy_api_experiment::workspace_routes` with no
//! entry in `workspace_route_roles`, so `role_manifest::classify` gives it the
//! FleetOk default.

use axum::Json;
use serde::Serialize;

use oxy_simulation::SimulationSpec;

use oxy_app::surface::WorkspaceManagerReadOnly;

/// `{ ok: true }`, or `{ ok: false, error: "<why>" }` — never a 4xx. The body
/// is a draft a person is still typing, not a resource the caller claims
/// exists, so "invalid" is a normal answer rather than a request error.
#[derive(Debug, Serialize)]
pub struct ValidateResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// `POST /simulations/validate` — same checks [`SimulationSpec::from_yaml`]
/// runs at run-queue time ([`super::runs::start_run`]), run here instead
/// against whatever a form has assembled so far.
///
/// Takes any JSON body rather than a typed request: a spec still missing
/// fields (or carrying the wrong type in one) must come back as `ok: false`
/// with a readable message, not a 422 that tells a form nothing about which
/// field to blame.
pub async fn validate_simulation(
    WorkspaceManagerReadOnly(_workspace_manager): WorkspaceManagerReadOnly,
    Json(body): Json<serde_json::Value>,
) -> Json<ValidateResponse> {
    Json(check_spec(body))
}

/// Split from the handler so it is reachable from a test without standing up
/// a `WorkspaceManager` — the transport layer stays extract-call-serialize.
fn check_spec(body: serde_json::Value) -> ValidateResponse {
    match SimulationSpec::from_value(body) {
        Ok(_) => ValidateResponse {
            ok: true,
            error: None,
        },
        Err(e) => ValidateResponse {
            ok: false,
            error: Some(e.to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal() -> serde_json::Value {
        serde_json::json!({
            "name": "minimal",
            "seed": 1,
            "periods": 4,
            "period_days": 7,
            "history_days": 90,
            "start_date": "2025-01-06",
            "entities": { "count": 2, "scale_sigma": 0.4 },
            "baseline": {
                "sales_per_entity_day": 1500.0,
                "margin": 0.36,
                "demand_shock_rho": 0.7,
                "demand_shock_sd": 0.12,
                "weekly_seasonality": 0.15
            },
            "mechanism": {
                "driver": "marketing_spend",
                "target": "net_sales",
                "lag_days": 7,
                "noise_ratio": 0.05,
                "calibrate": {
                    "anchor_spend_share": 0.02,
                    "local_slope_at_anchor": 4.0,
                    "optimum_at": 3.0
                }
            }
        })
    }

    #[test]
    fn a_coherent_world_validates_clean() {
        let response = check_spec(minimal());
        assert!(response.ok);
        assert!(response.error.is_none());
    }

    #[test]
    fn an_unreachable_optimum_reports_ok_false_not_a_panic() {
        // margin 0.36 × local_slope_at_anchor 4.0 = 1.44, so no saturating
        // curve puts the optimum below that — 1.2 asks for exactly that.
        let mut bad = minimal();
        bad["mechanism"]["calibrate"]["optimum_at"] = serde_json::json!(1.2);
        let response = check_spec(bad);
        assert!(!response.ok);
        assert!(
            response.error.unwrap().contains("unreachable"),
            "the response should name what's wrong, not just say no"
        );
    }

    #[test]
    fn a_body_that_is_not_a_world_at_all_reports_ok_false() {
        // A form mid-edit can post something with a missing required field —
        // this must come back as a readable `ok: false`, not a 500.
        let response = check_spec(serde_json::json!({ "name": "incomplete" }));
        assert!(!response.ok);
        assert!(response.error.is_some());
    }
}
