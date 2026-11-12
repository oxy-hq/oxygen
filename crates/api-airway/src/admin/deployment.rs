//! `/api/admin/airway/deployment-config` — staff read/write of airway's
//! **operational tier** (`airway_deployment_config`), under the same
//! `cap(Action::PlatformOperate)` as the rest of `/admin/airway` (see
//! [`super`]). Not owner-only: a Global Admin holds that capability too.
//!
//! The sibling routes in [`super::handlers`] edit the *policy* tier: per
//! source kind, per workspace, resolved on every run. This one edits the eight
//! settings airway installs into a process-wide `OnceLock` — `timeout`,
//! `max_retries`, `user_agent`, `retry_initial_delay`, `retry_max_delay`,
//! `retry_backoff_factor`, `cursor_lag_floor`, `tls`.
//!
//! # A save here is not live, and the response says so
//!
//! `airway::config::global::install` is one-shot: the second call is refused
//! and its values are dropped. So editing this row changes what the *next*
//! process to boot installs, and nothing about the ones already running. That
//! is why the read route reports the installed state beside the configured one
//! instead of only echoing the row back — without it an operator saves a
//! value, sees it stored, and believes it is in force.
//!
//! # `installed` is one process's state, and is labelled as such
//!
//! [`agentic_airway::installed_values`] reads *this* process's `OnceLock`.
//! Every oxy-app entry point that can build an airway connector installs at
//! boot (`crate::boot`), so on a healthy fleet the replica answering
//! this request has installed — but it installed *its own* boot-time read of
//! the row, which on a rolling deploy is not necessarily the row as it stands
//! now, and on a replica that failed to resolve it is nothing at all. Neither
//! a replica's values nor its `None` is evidence about the deployment; they
//! are evidence about one process.
//!
//! Rather than presenting that as the deployment's state, the response carries
//! [`InstalledScope`]: which process answered (role, pid, hostname), whether a
//! node in that role drains the durable queue in-process at all, and whether
//! it installed. When it did not, [`DriftReport::status`] is `unknown` with a
//! reason — never `in_sync`, which is the reading that would actually mislead.
//! The frontend renders that as "this process installed no tier", not as a
//! green tick, and — since the install is at boot — points the operator at that
//! process's log rather than telling them a run has simply not happened yet.
//!
//! # `FleetOk`
//!
//! Postgres plus one process-local `OnceLock` read: no workspace FS, no
//! `.git`, no state dir. Classified in `role_manifest.rs` alongside the rest of
//! the airway-config surface.

use agentic_airway::deployment_config::{self, DeploymentValues, SINGLETON_ID};
use axum::Json;
use axum::http::StatusCode;
use axum::response::Response;
use chrono::{DateTime, FixedOffset};
use entity::airway_deployment_config;
use entity::airway_deployment_config::Column as DeployColumn;
use sea_orm::sea_query::OnConflict;
use sea_orm::{ActiveValue::Set, DatabaseConnection, DbErr, EntityTrait};
use serde::Serialize;

use oxy_app::surface::admin_db::{connect, db_err, error_body};

/// Where the `installed` half of the response came from.
///
/// Every field exists to stop one sentence being true by accident: "airway is
/// running these settings". It is running them *in this process*, from this
/// process's own boot-time read of the row.
#[derive(Serialize, Debug, PartialEq)]
pub struct InstalledScope {
    /// Always `"answering_process"`. A constant rather than an implicit
    /// convention, so a client cannot read the payload as deployment-wide.
    pub scope: &'static str,
    /// `OXY_ROLE` as this process resolved it at startup.
    pub process_role: &'static str,
    pub pid: u32,
    /// Best-effort — `HOSTNAME` is set in every containerised deploy and is
    /// the label an operator uses to find the replica. `None` locally.
    pub hostname: Option<String>,
    /// Whether a node in this role drains the durable task queue in-process,
    /// i.e. whether it is the kind of process that executes airway *runs*.
    /// `false` on a stateless `serve` replica — which still installs the tier
    /// at boot (it serves source discovery, which builds a connector), so this
    /// is about which workload the values are governing here, not about
    /// whether they were read.
    pub process_runs_airway: bool,
    /// Whether *this* process installed the tier.
    pub installed_in_this_process: bool,
}

impl InstalledScope {
    fn observe(installed: bool) -> Self {
        let role = oxy_app::surface::roles::current_process_role();
        Self {
            scope: "answering_process",
            process_role: role.as_str(),
            pid: std::process::id(),
            hostname: std::env::var("HOSTNAME").ok().filter(|h| !h.is_empty()),
            process_runs_airway: oxy_app::surface::roles::role_runs_inprocess_workers(role),
            installed_in_this_process: installed,
        }
    }
}

/// Configured-vs-installed, as three states rather than a boolean.
///
/// `unknown` is a first-class answer and must never be collapsed into
/// `in_sync`: "this replica did not install anything" and "this replica
/// installed exactly what is configured" are different facts, and only one of
/// them is reassuring.
#[derive(Serialize, Debug, PartialEq)]
pub struct DriftReport {
    /// `in_sync` | `drifted` | `unknown`.
    pub status: &'static str,
    /// airway's key names for the settings that differ. Empty unless
    /// `status == "drifted"`.
    pub fields: Vec<&'static str>,
    /// Set only when `status == "unknown"`.
    pub reason: Option<&'static str>,
}

impl DriftReport {
    /// Compare the row against the running process.
    ///
    /// Both sides go through [`DeploymentValues::effective`] first — the form
    /// airway would actually install. Comparing the raw row instead reports
    /// phantom drift for spellings airway normalises away (the standing
    /// example is `tls_danger_accept_invalid_certs = false` alone, which does
    /// not configure a trust store).
    ///
    /// A configured row that airway refuses is `unknown`, not `drifted`: the
    /// operator's next restart will fail on it, and calling that a difference
    /// of values would send them looking for the wrong thing.
    pub fn compare(configured: &DeploymentValues, installed: Option<&DeploymentValues>) -> Self {
        let Some(installed) = installed else {
            return Self {
                status: "unknown",
                fields: Vec::new(),
                reason: Some("not_installed_in_this_process"),
            };
        };
        let Ok(effective) = configured.effective() else {
            return Self {
                status: "unknown",
                fields: Vec::new(),
                reason: Some("configured_values_invalid"),
            };
        };
        let fields = deployment_config::drift(&effective, installed);
        if fields.is_empty() {
            Self {
                status: "in_sync",
                fields,
                reason: None,
            }
        } else {
            Self {
                status: "drifted",
                fields,
                reason: None,
            }
        }
    }
}

#[derive(Serialize)]
pub struct DeploymentConfigResponse {
    /// The stored row, or all-`null` when no row exists. `null` on a field
    /// means "airway's built-in default" — never zero.
    pub configured: DeploymentValues,
    /// Distinguishes "a row exists and every setting is cleared" from "no row
    /// has ever been written". Identical in effect; different to an operator
    /// deciding whether someone has been here.
    pub configured_row_exists: bool,
    pub updated_at: Option<DateTime<FixedOffset>>,
    /// What the answering process installed, or `null` if it installed
    /// nothing. Read [`InstalledScope`] before showing this to anyone.
    pub installed: Option<DeploymentValues>,
    pub installed_scope: InstalledScope,
    pub drift: DriftReport,
}

/// Widen the stored row into the airway-side shape.
///
/// Postgres has no unsigned integer, so the columns are signed and the schema
/// carries `>= 0` checks. A negative that got past them (a hand-rolled
/// `UPDATE` on an older row) is reported, not clamped and not defaulted —
/// defaulting it here is precisely the "absence is zero" collapse in reverse.
pub(crate) fn values_from_row(
    row: &airway_deployment_config::Model,
) -> Result<DeploymentValues, String> {
    // Named for what it checks, not for a unit: it is a generic `i64 → u64`
    // widen and one of its three callers passes milliseconds. Calling it `secs`
    // would put a wrong unit on the one thing this feature spells out
    // everywhere else.
    let nonneg = |v: Option<i64>, key: &str| -> Result<Option<u64>, String> {
        v.map(|v| u64::try_from(v).map_err(|_| format!("`{key}` is negative")))
            .transpose()
    };
    Ok(DeploymentValues {
        timeout_secs: nonneg(row.timeout_secs, "timeout_secs")?,
        max_retries: row
            .max_retries
            .map(|v| u32::try_from(v).map_err(|_| "`max_retries` is negative".to_string()))
            .transpose()?,
        user_agent: row.user_agent.clone(),
        retry_initial_delay_ms: nonneg(row.retry_initial_delay_ms, "retry_initial_delay_ms")?,
        retry_max_delay_secs: nonneg(row.retry_max_delay_secs, "retry_max_delay_secs")?,
        retry_backoff_factor: row.retry_backoff_factor,
        cursor_lag_floor_secs: nonneg(row.cursor_lag_floor_secs, "cursor_lag_floor_secs")?,
        tls_ca_cert: row.tls_ca_cert.clone(),
        tls_client_cert: row.tls_client_cert.clone(),
        tls_client_key_file: row.tls_client_key_file.clone(),
        tls_danger_accept_invalid_certs: row.tls_danger_accept_invalid_certs,
    })
}

/// Load the singleton row. `Ok(None)` = never written.
pub(crate) async fn load_row(
    db: &DatabaseConnection,
) -> Result<Option<airway_deployment_config::Model>, DbErr> {
    airway_deployment_config::Entity::find_by_id(SINGLETON_ID)
        .one(db)
        .await
}

/// Assemble the read response from the row and this process's install state.
/// Split out from the handler so the drift half is testable without HTTP.
pub(crate) fn build_response(
    row: Option<&airway_deployment_config::Model>,
    installed: Option<DeploymentValues>,
) -> Result<DeploymentConfigResponse, String> {
    let configured = match row {
        Some(row) => values_from_row(row)?,
        None => DeploymentValues::default(),
    };
    let drift = DriftReport::compare(&configured, installed.as_ref());
    Ok(DeploymentConfigResponse {
        configured,
        configured_row_exists: row.is_some(),
        updated_at: row.map(|r| r.updated_at),
        installed_scope: InstalledScope::observe(installed.is_some()),
        installed,
        drift,
    })
}

/// `GET /api/admin/airway/deployment-config`.
pub async fn get_deployment_config() -> Result<Json<DeploymentConfigResponse>, Response> {
    let db = connect().await?;
    let row = load_row(&db).await.map_err(db_err)?;
    let resp = build_response(row.as_ref(), agentic_airway::installed_values()).map_err(|e| {
        error_body(
            StatusCode::INTERNAL_SERVER_ERROR,
            "invalid_stored_value",
            Some(format!("airway_deployment_config: {e}")),
        )
    })?;
    Ok(Json(resp))
}

/// Everything that can go wrong writing the deployment row.
#[derive(Debug, thiserror::Error)]
pub(crate) enum DeploymentWriteError {
    /// A value airway refuses. Its `Display` already names the key (and never
    /// the value — airway's diagnostics are redacted by construction).
    #[error(transparent)]
    Invalid(#[from] agentic_airway::AirwayError),
    /// A value airway accepts that the column cannot hold — see
    /// [`narrow_i64`]. Names the field and the ceiling, not the value.
    #[error("`{field}` is too large to store: the column holds at most {max}")]
    OutOfRange { field: &'static str, max: i64 },
    #[error(transparent)]
    Db(#[from] DbErr),
}

fn deployment_write_err(err: DeploymentWriteError) -> Response {
    match err {
        DeploymentWriteError::Db(e) => db_err(e),
        DeploymentWriteError::Invalid(_) | DeploymentWriteError::OutOfRange { .. } => error_body(
            StatusCode::BAD_REQUEST,
            "invalid_deployment_config",
            Some(err.to_string()),
        ),
    }
}

/// Narrow a `u64` setting into its `BIGINT` column, refusing what will not fit.
///
/// Postgres has no unsigned integer, so `timeout_secs` and the two `retry_*`
/// durations are `BIGINT` against a `u64`, and `max_retries` (see
/// [`narrow_i32`]) is `INTEGER` against a `u32`. A bare `as` **wraps**:
/// `timeout_secs: 9223372036854775808` is a valid `u64` and valid JSON, and
/// would land in the row as a *negative* — a wrong value wearing a valid shape,
/// which then trips the schema's `>= 0` `CHECK` and comes back as a 500
/// `db_err` for what is plainly a caller's out-of-range input.
///
/// **The storage width is the only rule the oxy side owns here**, and it owns
/// it because it is a property of the column rather than of the setting.
/// Everything semantic — a zero duration, an empty `user_agent`, a backoff
/// factor below 1, half an mTLS identity — belongs to
/// [`DeploymentValues::to_global`], which [`upsert_deployment`] has already run
/// by this point; a second copy here is a copy that goes stale. Symmetric with
/// the read side ([`values_from_row`] and `deployment_config::load`), which
/// likewise report a value outside the type's domain rather than clamping it.
fn narrow_i64(
    value: Option<u64>,
    field: &'static str,
) -> Result<Option<i64>, DeploymentWriteError> {
    value
        .map(|v| {
            i64::try_from(v).map_err(|_| DeploymentWriteError::OutOfRange {
                field,
                max: i64::MAX,
            })
        })
        .transpose()
}

/// [`narrow_i64`] for `max_retries`, whose column is `INTEGER`.
fn narrow_i32(
    value: Option<u32>,
    field: &'static str,
) -> Result<Option<i32>, DeploymentWriteError> {
    value
        .map(|v| {
            i32::try_from(v).map_err(|_| DeploymentWriteError::OutOfRange {
                field,
                max: i32::MAX.into(),
            })
        })
        .transpose()
}

/// Validate against airway and upsert the singleton row.
///
/// **Validation is airway's**, through `DeploymentValues::to_global` — the
/// same call the worker makes at install. So a value the API accepts is a
/// value the next restart installs, and there is no second rule set on the
/// oxy side to drift from it.
///
/// The one refusal that is *not* airway's is storage width, and it is not a
/// rule about the setting: Postgres has no unsigned integer, so a `u64` above
/// `i64::MAX` has no representation in its column. [`narrow_i64`] refuses it by
/// name as a `400` rather than letting an `as` wrap it into a negative that the
/// schema's `CHECK` then rejects as a `500`.
///
/// A replace, not a patch, matching the sibling policy routes: a `null` field
/// clears that setting back to airway's default. Callers send all eleven fields
/// every time.
///
/// The `cursor_lag_floor_secs = 0` case is worth naming because it is the one
/// value that *looks* like a clear and is not: `null` clears the floor, `0` is
/// a floor of zero, and airway refuses the second precisely so the two cannot
/// be spelled the same way. It comes back from `to_global()` as a `400` naming
/// the key — no Oxy-side rule is written for it here.
///
/// `ON CONFLICT (id)` rather than a read-then-write: the `CHECK (id = 1)`
/// makes `id` the only conflict target there is, so this is an upsert with no
/// race to lose.
pub(crate) async fn upsert_deployment(
    db: &DatabaseConnection,
    values: &DeploymentValues,
) -> Result<(), DeploymentWriteError> {
    values.to_global()?;

    let model = airway_deployment_config::ActiveModel {
        id: Set(SINGLETON_ID),
        // Field names spelled as literals, matching `values_from_row` on the
        // read side of this same file — airway's key roster is not reachable
        // from here without taking a dependency on the engine crate.
        timeout_secs: Set(narrow_i64(values.timeout_secs, "timeout_secs")?),
        max_retries: Set(narrow_i32(values.max_retries, "max_retries")?),
        user_agent: Set(values.user_agent.clone()),
        retry_initial_delay_ms: Set(narrow_i64(
            values.retry_initial_delay_ms,
            "retry_initial_delay_ms",
        )?),
        retry_max_delay_secs: Set(narrow_i64(
            values.retry_max_delay_secs,
            "retry_max_delay_secs",
        )?),
        retry_backoff_factor: Set(values.retry_backoff_factor),
        cursor_lag_floor_secs: Set(values.cursor_lag_floor_secs.map(|v| v as i64)),
        tls_ca_cert: Set(values.tls_ca_cert.clone()),
        tls_client_cert: Set(values.tls_client_cert.clone()),
        tls_client_key_file: Set(values.tls_client_key_file.clone()),
        tls_danger_accept_invalid_certs: Set(values.tls_danger_accept_invalid_certs),
        updated_at: Set(chrono::Utc::now().fixed_offset()),
    };

    airway_deployment_config::Entity::insert(model)
        .on_conflict(
            OnConflict::column(DeployColumn::Id)
                .update_columns([
                    DeployColumn::TimeoutSecs,
                    DeployColumn::MaxRetries,
                    DeployColumn::UserAgent,
                    DeployColumn::RetryInitialDelayMs,
                    DeployColumn::RetryMaxDelaySecs,
                    DeployColumn::RetryBackoffFactor,
                    DeployColumn::CursorLagFloorSecs,
                    DeployColumn::TlsCaCert,
                    DeployColumn::TlsClientCert,
                    DeployColumn::TlsClientKeyFile,
                    DeployColumn::TlsDangerAcceptInvalidCerts,
                    DeployColumn::UpdatedAt,
                ])
                .to_owned(),
        )
        .exec(db)
        .await?;
    Ok(())
}

/// Delete the singleton row, if any. Idempotent — every setting goes back to
/// airway's built-in default at the next restart. Distinct from saving all
/// eleven fields as `null`, which leaves a row (and an `updated_at`) behind.
pub(crate) async fn delete_deployment(db: &DatabaseConnection) -> Result<(), DbErr> {
    airway_deployment_config::Entity::delete_by_id(SINGLETON_ID)
        .exec(db)
        .await?;
    Ok(())
}

/// `PUT /api/admin/airway/deployment-config`.
pub async fn put_deployment_config(
    Json(values): Json<DeploymentValues>,
) -> Result<StatusCode, Response> {
    let db = connect().await?;
    upsert_deployment(&db, &values)
        .await
        .map_err(deployment_write_err)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/admin/airway/deployment-config`.
pub async fn delete_deployment_config() -> Result<StatusCode, Response> {
    let db = connect().await?;
    delete_deployment(&db).await.map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
#[path = "deployment_tests.rs"]
mod tests;
