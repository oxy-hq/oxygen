//! The custom-app automation run's **row**: how a `customer_app_automation_runs`
//! row is closed and begun ([`settle`]), the failure classifier
//! ([`automation_error_to_code`]), and whether a row is a preview-mode run's
//! ([`owns_run`]).
//!
//! The handlers (`POST …/procedures/{procedure_id}/runs`, poll, cancel), the
//! queued executor that drives a run and the TTL sweep are
//! `oxy-api-custom-apps`' `automation_run`. A preview-mode run's row is settled
//! from its finished preview run by `oxy-app`'s
//! `server::api::projects::automation_run::preview_settle`, which the
//! workspace-preview sweep (`previews::queue::sweep`) drives in the same tick it
//! marks the preview run finished; it closes the row through this state
//! machine, which is one thing whichever side writes it. Down from `oxy-app`'s
//! `server::api::projects::automation_run` (2026-11-22), re-exported there and
//! at the prelude's `automation_runs` until decompose R1 deleted both.
//!
//! **Sentry:** this path carries the `custom_apps` marker only because of the
//! crate it lives in. `oxy_telemetry::sentry_filter` exempts it from the marker
//! and lists it as data plane, so its lines are classified as they were under
//! `oxy_app::server::api::projects`: an issue at `error`, shaped in Sentry Logs.

use agentic_pipeline::automation_run::AutomationRunError;
use sea_orm::{DatabaseConnection, EntityTrait};
use tracing::warn;
use uuid::Uuid;

pub mod settle;

/// Best-effort categorization of automation-runner failures.
pub fn automation_error_to_code(e: &AutomationRunError) -> (&'static str, String) {
    let msg = e.to_string();
    let lower = msg.to_lowercase();
    let code = if lower.contains("agent")
        && (lower.contains("not configured") || lower.contains("inlineagentrunner"))
    {
        "automation_requires_agent_runner"
    } else if lower.contains("rate limit") {
        "automation_rate_limited"
    } else if lower.contains("timeout") || lower.contains("timed out") {
        "automation_run_timeout"
    } else if lower.contains("connect") || lower.contains("connection") {
        "automation_warehouse_unreachable"
    } else {
        "automation_run_failed"
    };
    (code, msg)
}

/// Whether `run_id` is a preview-mode run's poll row: its run is a workspace
/// preview run, which `oxy-app`'s preview settle (`settle_custom_app_runs`)
/// settles from the preview's outcome. The orphan reconcile (`settle::reconcile_abandoned`) leaves such a
/// row alone. A lookup that fails reads as owned, so a row is never closed on
/// a database error.
pub async fn owns_run(db: &DatabaseConnection, run_id: Uuid) -> bool {
    match entity::workspace_preview_runs::Entity::find_by_id(run_id.to_string())
        .one(db)
        .await
    {
        Ok(row) => row.is_some(),
        Err(e) => {
            warn!(%run_id, error = %e, "preview ownership lookup failed");
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automation_error_classifies_known_patterns() {
        let cases = [
            (
                "connection refused to warehouse",
                "automation_warehouse_unreachable",
            ),
            ("OpenAI returned rate limit", "automation_rate_limited"),
            ("query timed out", "automation_run_timeout"),
            (
                "InlineAgentRunner not configured",
                "automation_requires_agent_runner",
            ),
            ("something else entirely", "automation_run_failed"),
        ];
        for (msg, want) in cases {
            let e = AutomationRunError::Inline(msg.to_string());
            let (code, _) = automation_error_to_code(&e);
            assert_eq!(code, want, "for message {msg:?}");
        }
    }
}
