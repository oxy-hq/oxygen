//! The audit row of every read: `admin.app.data_read`, one per request that
//! reached an app's OLTP store — who, which app, which environment, which
//! table and page.
//!
//! **Written before the result leaves, and the result is withheld without
//! it** ([`withhold_unless_recorded`]). Every other console read audits best
//! effort or not at all; this one is the console reading a customer's
//! business records, and `/admin/audit` is the only place an org can learn
//! that it happened. A read that failed (a table that is not there, a
//! timeout) is recorded as a failure, best effort: nothing left with it.
//!
//! An environment with no store reads nothing and writes no row.

use entity::apps;
use oxy_app_core::audit::{self, AuditEntry, RequestActor};
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::{DatabaseConnection, DbErr};
use serde_json::json;
use uuid::Uuid;

use super::super::environment_scope::ScopeError;
use super::error;
use super::page::OrderTerm;

/// The action every row is written under.
pub const DATA_READ: &str = "admin.app.data_read";

/// What was read.
#[derive(Debug, Clone, Copy)]
pub enum Read<'a> {
    /// The schema's table list.
    Tables,
    /// One page of one table.
    Rows {
        table: &'a str,
        limit: u32,
        offset: u32,
        order: &'a [OrderTerm],
    },
}

/// The row for `read` of `app`'s `schema` in `environment`, by `actor`.
pub fn entry(
    actor: &RequestActor,
    app: &apps::Model,
    environment: &AppEnvironment,
    schema: &str,
    read: Read<'_>,
) -> AuditEntry {
    let environment = environment.name();
    let detail = match read {
        Read::Tables => json!({ "read": "tables", "environment": environment, "schema": schema }),
        Read::Rows {
            table,
            limit,
            offset,
            order,
        } => json!({
            "read": "rows",
            "environment": environment,
            "schema": schema,
            "table": table,
            "limit": limit,
            "offset": offset,
            "order": order,
        }),
    };
    AuditEntry::for_request(actor, DATA_READ)
        .org(app.org_id)
        .workspace(app.project_id)
        .target(
            "custom_app_environment",
            format!("{}/{environment}", app.id),
            format!("{}/{environment}", app.slug),
        )
        .environment(environment)
        .metadata(detail)
}

/// A read's result, only once its row is written.
pub fn withhold_unless_recorded<T>(
    read: T,
    recorded: Result<Uuid, DbErr>,
) -> Result<T, ScopeError> {
    match recorded {
        Ok(_) => Ok(read),
        Err(e) => {
            tracing::error!(action = DATA_READ, error = %e, "audit: a data read was not recorded; withholding it");
            Err(error::audit_unavailable())
        }
    }
}

/// Record `entry` for `outcome`: a success must be recorded for its result
/// to be returned; a failure is recorded best effort and returned as it was.
pub async fn settle<T>(
    db: &DatabaseConnection,
    entry: AuditEntry,
    outcome: Result<T, ScopeError>,
) -> Result<T, ScopeError> {
    match outcome {
        Ok(read) => withhold_unless_recorded(read, audit::record(db, entry).await),
        Err(refused) => {
            audit::record_best_effort(db, entry.failure(refused.code)).await;
            Err(refused)
        }
    }
}

#[cfg(test)]
#[path = "audit_tests.rs"]
mod tests;
