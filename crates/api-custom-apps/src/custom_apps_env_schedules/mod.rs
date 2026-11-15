//! A custom app's function schedules, per environment
//! (`internal-docs/2026-09-10-custom-app-environments-design.md` §4.5,
//! ruling 1).
//!
//! Production's schedules always run, registered by a promoting publish as
//! they always were. A non-production environment — staging or a
//! `dev-<handle>` sandbox — fires its functions' `schedule`s only once staff
//! turned its switch on (`app_environments.schedules_enabled`, off by
//! default): a schedule there reads production data on a timer and calls
//! third parties with staging's credentials.
//!
//! * [`reconcile`] — the `agentic_schedules` rows: production's two-segment
//!   refs at a promote, an environment's three-segment refs on every publish
//!   to it and every flip; [`lock`] keeps two reconciles of one environment
//!   apart;
//! * [`handlers`] — `GET`/`PUT …/environments/{name}/schedules`.
//!
//! A row fires through the global singleton's tick like production's
//! (`agentic_pipeline::function_schedule`), and the runner decides again
//! whether it may run (`custom_apps_functions::queued_admission`). A
//! sandbox's teardown retires its rows; the tenant's schedules API never sees
//! them (`oxy_api_automations::schedules`, whose own integration group
//! holds that).

pub mod handlers;
mod lock;
pub(crate) mod reconcile;

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use super::custom_apps_sandboxes::SandboxError;

/// Every way the switch routes refuse: a sandbox route's refusal, or
/// production, which has no switch. Answers `{"error","message"}`.
#[derive(Debug, thiserror::Error)]
pub enum SwitchError {
    #[error(transparent)]
    Refused(#[from] SandboxError),
    #[error("production has no schedules switch: its functions' schedules always run")]
    Production,
}

impl IntoResponse for SwitchError {
    fn into_response(self) -> Response {
        match self {
            Self::Refused(refused) => refused.into_response(),
            Self::Production => {
                let body = serde_json::json!({
                    "error": "production_has_no_switch",
                    "message": self.to_string(),
                });
                (StatusCode::BAD_REQUEST, Json(body)).into_response()
            }
        }
    }
}
