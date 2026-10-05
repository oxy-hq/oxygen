use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Persistent state for automation runs triggered from custom-app
/// bundles via `useAutomationRun` (legacy: `useProcedureRun`). See
/// `migration::m20260526_000001_create_customer_app_procedure_runs`
/// for the original schema rationale; the table was renamed from
/// `customer_app_procedure_runs` to `customer_app_automation_runs` by
/// `m20260623_000001_rename_procedures_to_automations` (which leaves a
/// back-compat view under the old name). The module
/// `entity::customer_app_procedure_runs` is kept as an alias (see
/// `lib.rs`).
#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "customer_app_automation_runs")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub procedure_id: String,
    /// `running` | `done` | `failed` | `cancelled`.
    pub status: String,
    /// Caller-supplied params object passed through to the automation's
    /// render context. Stored verbatim so a re-poll can return them
    /// for diagnostics.
    pub params: Option<Json>,
    pub progress_step: Option<String>,
    pub progress_percent: Option<i16>,
    pub result_summary: Option<String>,
    pub result_outputs: Option<Json>,
    pub error_message: Option<String>,
    pub error_code: Option<String>,
    /// Non-NULL when a cancel was requested; the driver polls the run's
    /// durable cancel flag and stops, and a claim that finds this set does
    /// not start.
    pub cancel_requested_at: Option<DateTimeWithTimeZone>,
    /// When the run was accepted and queued (`status = 'running'` from here).
    pub started_at: DateTimeWithTimeZone,
    /// Non-NULL once a driver began executing the first step. Stamped by one
    /// atomic `UPDATE … WHERE execution_started_at IS NULL` right before the
    /// runner starts, so a later attempt at the same run — after its driver
    /// died or was replaced — finds it set and does not repeat steps that
    /// already ran. Never served to the bundle; see
    /// `m20261003_000001_automation_runs_execution_started_at`.
    pub execution_started_at: Option<DateTimeWithTimeZone>,
    /// The executing attempt's proof of life: set with the stamp above and
    /// re-stamped on an interval until the run settles. A later attempt
    /// closes the run as interrupted only once this is stale, and steps
    /// aside while it is fresh. NULL beside a non-NULL stamp is a run begun
    /// by a binary from before the column, read as last alive at the stamp.
    /// Never served to the bundle.
    pub execution_heartbeat_at: Option<DateTimeWithTimeZone>,
    pub completed_at: Option<DateTimeWithTimeZone>,
}

impl ActiveModelBehavior for ActiveModel {}
