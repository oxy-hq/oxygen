//! `SubscriptionGuard` — single per-request paywall.
//!
//! There is no per-feature gating in v1. The only check is whether the
//! caller's org has an active (or in-grace) subscription. Routes mounted
//! under [`subscription_guard_middleware`] (org-scoped) or
//! [`workspace_subscription_guard_middleware`] (workspace-scoped) return
//! `402 PAYMENT_REQUIRED` with `{ code, status, contact_required: true }`
//! when the org's effective billing status doesn't grant access.
//!
//! Bypasses (mounted on routers without this middleware): `/orgs/{id}/billing`,
//! `/admin`, `/webhooks/stripe`, auth routes, and `/user/me`.
//!
//! When Stripe isn't configured (local mode, or cloud mode missing the env
//! vars) the middleware short-circuits to `Ok` so non-cloud deployments
//! continue to work.
//!
//! Refusal plumbing, not the billing decision: whether billing is switched off
//! and whether a status grants access are `oxy-billing`'s, asked through the
//! [`SubscriptionGate`] the router layers onto the API tree as an `Extension`
//! (`SurfaceSeams::subscription_gate`). This module only turns that answer into
//! the 402 every org- and workspace-scoped surface returns. A guarded route
//! reached without a gate answers 500: a missing wire never skips the paywall.

use axum::Json;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use entity::org_billing::{self, BillingStatus};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::Serialize;
use uuid::Uuid;

use super::org_context::OrgContext;
use crate::seams::SubscriptionGate;

/// What the guard does with a request once it knows the org.
enum Gate {
    /// Billing is switched off: let the request through.
    Off,
    /// Billing is on: check the org's row against this gate.
    On(SubscriptionGate),
}

/// Billing's gate from the request, or the 500 a router built without one gets.
fn gate_of(request: &axum::http::Request<axum::body::Body>) -> Result<Gate, Response> {
    let Some(gate) = request.extensions().get::<SubscriptionGate>().copied() else {
        tracing::error!(
            "subscription_guard: no SubscriptionGate extension on the request; the router \
             was built without `SurfaceSeams::subscription_gate`"
        );
        return Err(StatusCode::INTERNAL_SERVER_ERROR.into_response());
    };
    Ok(if (gate.disabled)() {
        Gate::Off
    } else {
        Gate::On(gate)
    })
}

/// 402 body shape. The web client reads `code = subscription_required` and
/// mounts the `PaywallScreen` overlay; `status` lets it render the right copy.
/// `contact_required: true` instructs the FE to show contact-sales messaging
/// without a self-serve Subscribe CTA — the central "no public pricing"
/// guarantee. Future self-serve work would flip this to `false` per-org.
#[derive(Serialize)]
struct SubscriptionRequiredBody {
    code: &'static str,
    status: &'static str,
    contact_required: bool,
}

fn paywall_response(status: BillingStatus) -> Response {
    let body = SubscriptionRequiredBody {
        code: "subscription_required",
        status: status.as_str(),
        contact_required: true,
    };
    (StatusCode::PAYMENT_REQUIRED, Json(body)).into_response()
}

/// Org-scoped guard. Reads `OrgContext` (inserted by `org_middleware`) for
/// the `org_id` and checks the matching `org_billing` row.
pub async fn subscription_guard_middleware(
    request: axum::http::Request<axum::body::Body>,
    next: Next,
) -> Result<Response, Response> {
    let Some(org_id) = request.extensions().get::<OrgContext>().map(|c| c.org.id) else {
        return Ok(next.run(request).await);
    };
    let Gate::On(gate) = gate_of(&request)? else {
        return Ok(next.run(request).await);
    };
    enforce(gate, org_id, request, next).await
}

/// Workspace-scoped guard. Reads `org_members::Model` (inserted by
/// `workspace_middleware`) for the `org_id`.
pub async fn workspace_subscription_guard_middleware(
    request: axum::http::Request<axum::body::Body>,
    next: Next,
) -> Result<Response, Response> {
    let Some(org_id) = request
        .extensions()
        .get::<entity::org_members::Model>()
        .map(|m| m.org_id)
    else {
        return Ok(next.run(request).await);
    };
    let Gate::On(gate) = gate_of(&request)? else {
        return Ok(next.run(request).await);
    };
    enforce(gate, org_id, request, next).await
}

async fn enforce(
    gate: SubscriptionGate,
    org_id: Uuid,
    request: axum::http::Request<axum::body::Body>,
    next: Next,
) -> Result<Response, Response> {
    let billing = match load_billing(org_id).await {
        Ok(Some(row)) => row,
        Ok(None) => {
            // Eager insert at org creation guarantees a row in steady state;
            // a missing row indicates data drift.
            tracing::error!(?org_id, "subscription_guard: org_billing row missing");
            return Err(paywall_response(BillingStatus::Incomplete));
        }
        Err(e) => {
            tracing::error!(?e, ?org_id, "subscription_guard: load billing failed");
            return Err(StatusCode::INTERNAL_SERVER_ERROR.into_response());
        }
    };

    let effective = (gate.effective_status)(
        billing.status,
        billing.grace_period_ends_at.map(|t| t.with_timezone(&Utc)),
        Utc::now(),
    );
    if !(gate.grants_access)(
        billing.status,
        billing.grace_period_ends_at.map(|t| t.with_timezone(&Utc)),
        Utc::now(),
    ) {
        return Err(paywall_response(effective));
    }
    Ok(next.run(request).await)
}

async fn load_billing(org_id: Uuid) -> Result<Option<org_billing::Model>, sea_orm::DbErr> {
    let db = oxy::database::client::establish_connection()
        .await
        .map_err(|e| sea_orm::DbErr::Custom(e.to_string()))?;
    org_billing::Entity::find()
        .filter(org_billing::Column::OrgId.eq(org_id))
        .one(&db)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    #[test]
    fn paywall_body_codes() {
        for s in [
            BillingStatus::Incomplete,
            BillingStatus::Unpaid,
            BillingStatus::Canceled,
        ] {
            let resp = paywall_response(s);
            assert_eq!(resp.status(), StatusCode::PAYMENT_REQUIRED);
        }
    }

    fn bare_request() -> axum::http::Request<axum::body::Body> {
        axum::http::Request::new(axum::body::Body::empty())
    }

    /// A router built without billing's gate fails closed: 500, never a pass.
    #[test]
    fn a_request_without_a_gate_is_refused_not_waved_through() {
        let Err(resp) = gate_of(&bare_request()) else {
            panic!("a request without a SubscriptionGate must not reach the handler");
        };
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn a_billing_off_gate_lets_the_request_through() {
        let mut request = bare_request();
        request
            .extensions_mut()
            .insert(SubscriptionGate::billing_off());
        assert!(matches!(gate_of(&request), Ok(Gate::Off)));
    }

    #[tokio::test]
    async fn paywall_body_carries_contact_required() {
        let resp = paywall_response(BillingStatus::Incomplete);
        let body = resp.into_body();
        let bytes = to_bytes(body, 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["code"], "subscription_required");
        assert_eq!(json["status"], "incomplete");
        assert_eq!(json["contact_required"], true);
    }
}
