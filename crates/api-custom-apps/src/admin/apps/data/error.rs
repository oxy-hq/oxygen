//! The data browser's refusals, in the console's coded body
//! (`{"error":"<code>","message":"<text>"}`, [`ScopeError`]). Each code is
//! what the console keys on; each message is written for the operator who
//! reads it in the panel.

use axum::http::StatusCode;

use super::super::environment_scope::ScopeError;
use super::catalog::Column;

pub fn publish_token_refused() -> ScopeError {
    ScopeError::new(
        StatusCode::FORBIDDEN,
        "publish_token_refused",
        "a publish token cannot read an app's data; use a staff login",
    )
}

pub fn sandbox_token_refused() -> ScopeError {
    ScopeError::new(
        StatusCode::FORBIDDEN,
        "sandbox_token_refused",
        "a sandbox agent token cannot read an app's data from the staff console",
    )
}

/// Which lockdown refused the read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Locked {
    /// The workspace the app was published from.
    Workspace,
    /// Another workspace of the app's org: the OLTP database is the org's,
    /// shared by every workspace in it.
    Org,
}

pub fn locked_out(locked: Locked) -> ScopeError {
    let message = match locked {
        Locked::Workspace => {
            "this app's org has locked Oxy staff out of the workspace the app was published \
             from, so its data is not readable from the console"
        }
        Locked::Org => {
            "this app's org has locked Oxy staff out of one of its workspaces; the app's data \
             lives in the org's OLTP database, which every workspace shares, so it is not \
             readable from the console"
        }
    };
    ScopeError::new(StatusCode::FORBIDDEN, "oxy_locked_out", message)
}

pub fn app_admin_required() -> ScopeError {
    ScopeError::new(
        StatusCode::FORBIDDEN,
        "app_admin_required",
        "an app's data is open to the app's admins",
    )
}

pub fn non_production_refused(environment: &str) -> ScopeError {
    ScopeError::new(
        StatusCode::FORBIDDEN,
        "non_production_refused",
        format!(
            "the {environment} environment is open to Oxy staff who may open this app's \
             non-production environments"
        ),
    )
}

pub fn environment_not_found(environment: &str) -> ScopeError {
    ScopeError::new(
        StatusCode::NOT_FOUND,
        "environment_not_found",
        format!("this app has no environment {environment}"),
    )
}

pub fn invalid_page(message: impl Into<String>) -> ScopeError {
    ScopeError::new(StatusCode::BAD_REQUEST, "invalid_page", message)
}

/// A table the app's schema does not have. The same answer for a name that
/// exists in another schema and one that exists nowhere.
pub fn table_not_found(table: &str) -> ScopeError {
    ScopeError::new(
        StatusCode::NOT_FOUND,
        "table_not_found",
        format!("this app's schema has no table {table:?}"),
    )
}

pub fn unknown_column(table: &str, column: &str) -> ScopeError {
    ScopeError::new(
        StatusCode::BAD_REQUEST,
        "unknown_column",
        format!("table {table:?} has no column {column:?} to order by"),
    )
}

/// A column whose type Postgres has no ordering for (`json`, `xml`, `point`, …).
pub fn unsortable_column(table: &str, column: &Column) -> ScopeError {
    ScopeError::new(
        StatusCode::BAD_REQUEST,
        "unsortable_column",
        format!(
            "column {:?} of table {table:?} is {}, which Postgres cannot order; order by \
             another column",
            column.name, column.data_type
        ),
    )
}

/// The backstop for a sort the catalog called possible and Postgres refused
/// (`42883`, no ordering operator): the request's to change, not an outage.
pub fn unorderable() -> ScopeError {
    invalid_page("Postgres cannot order by the column asked for; order by another column")
}

/// The rows route asked of an environment with nothing to read.
pub fn no_store(reason: &str) -> ScopeError {
    ScopeError::new(StatusCode::CONFLICT, "no_oltp_store", reason)
}

pub fn store_unreachable(detail: impl std::fmt::Display) -> ScopeError {
    tracing::warn!(error = %detail, "app data browser: the OLTP database did not answer");
    ScopeError::new(
        StatusCode::BAD_GATEWAY,
        "oltp_unreachable",
        format!("the app's OLTP database did not answer: {detail}"),
    )
}

pub fn read_timeout() -> ScopeError {
    ScopeError::new(
        StatusCode::GATEWAY_TIMEOUT,
        "read_timeout",
        "the read took longer than its time limit and was cancelled; order by an indexed \
         column or go back to an earlier page",
    )
}

/// The read happened but its audit row could not be written, so its result
/// is withheld: no read of customer data leaves without its record.
pub fn audit_unavailable() -> ScopeError {
    ScopeError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "audit_unavailable",
        "the read could not be recorded in the audit log, so its result is withheld; retry \
         shortly",
    )
}
