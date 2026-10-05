//! The shape of a data-plane write, as an audit row describes it — the
//! part of `data_audit` that needs no function runtime.
//!
//! `data_audit` is compiled only with `custom-app-functions` (V8), but the
//! staging held-call log (`custom_apps_staging_held`), a held agent ask
//! (`projects::agent_ask_staging`) and a held automation start
//! (`projects::automation_run`) build the same rows without running a
//! function. These items are therefore ungated; `data_audit` re-exports them
//! so its own callers are unchanged.

use oxy_app_core::audit::{ActorType, AuditEntry};
use uuid::Uuid;

/// A token that can be a table or verb name, bounded — anything else (a
/// 4 KB expression, an unquoted fragment) is not recorded at all. This is the
/// guarantee "never the SQL" rests on: whatever reaches a span is one
/// whitespace-delimited, identifier-shaped token of at most 64 chars, taken
/// after `host_call_attrs::strip_string_literals` has removed standard single-quoted
/// literals. It is not a dialect-aware lexer — a backslash-escaped quote or a
/// double-quoted literal can still end a literal early — so the bound, not
/// the stripping, is what holds.
pub(crate) fn identifier_like(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 64
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '$' | '-'))
}

/// The plane a `ctx.tx()` destination belongs to, from its connector's
/// dialect: Airhouse speaks DuckDB over pgwire, a plain Postgres destination
/// is `postgres`, anything else is named by its dialect. `oltp` is reserved
/// for the app's own silo, which only `ctx.oltp` reaches.
pub(crate) fn plane_for_dialect(dialect: agentic_connector::SqlDialect) -> &'static str {
    use agentic_connector::SqlDialect as D;
    match dialect {
        D::DuckDb => "airhouse",
        D::Postgres => "postgres",
        D::Sqlite => "sqlite",
        D::BigQuery => "bigquery",
        D::Snowflake => "snowflake",
        _ => "other",
    }
}

/// One write, as the audit row describes it.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct WriteRecord {
    /// `oltp` (the app's silo), `app_airhouse` (its own Airhouse schema),
    /// `airhouse`, `postgres`, or another dialect name — see
    /// [`plane_for_dialect`].
    pub plane: &'static str,
    /// The schema (OLTP) or database (Airhouse) the statement ran against.
    pub namespace: String,
    pub verb: String,
    /// The table the summary found, or empty when the statement had none
    /// that survived the identifier bound.
    pub table: String,
    /// Rows the statements reported, summed; `None` when the plane does not
    /// report a count.
    pub rows: Option<u64>,
    /// How many statements this record stands for (coalesced).
    pub statements: u64,
    /// The host op a held call was (`storage.put`), on `app.staging.held`
    /// rows only. Absent from every write row, which serializes as before.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub op: Option<&'static str>,
    /// Why a held call could not run in its environment, when the fix is
    /// the operator's — `ctx.oltp` in an org with no OLTP staging branch
    /// (`env_policy::NO_BRANCH_NOTE`). On `app.staging.held` rows only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<&'static str>,
}

impl WriteRecord {
    /// `oltp:app_orders.orders`, the audit row's `target_id`.
    pub fn target(&self) -> String {
        if self.table.is_empty() || !identifier_like(&self.table) {
            format!("{}:{}", self.plane, self.namespace)
        } else {
            format!("{}:{}.{}", self.plane, self.namespace, self.table)
        }
    }
}

/// A row's actor: the verified user (`audit_events.actor_user_id`), or the
/// platform acting for the app (`system:app:<slug>`) when no human called.
pub(crate) fn actor_entry(
    action: &'static str,
    user: Option<(Uuid, Option<&str>)>,
    app_slug: &str,
) -> AuditEntry {
    match user {
        Some((uid, email)) => AuditEntry::new(email.unwrap_or("unknown").to_string(), action)
            .actor(uid, ActorType::User),
        None => {
            let mut e = AuditEntry::new(format!("system:app:{app_slug}"), action);
            e.actor_type = ActorType::System;
            e
        }
    }
}

/// The row's target: its first write's table.
pub(crate) fn with_first_target(e: AuditEntry, writes: &[WriteRecord]) -> AuditEntry {
    match writes.first() {
        Some(w) => e.target(
            format!("{}.table", w.plane),
            w.target(),
            format!("{} {}", w.verb, w.target()),
        ),
        None => e,
    }
}

/// What a non-production invocation would have written, and did not: every
/// call its environment policy held or refused, one row per invocation, in
/// the same shape as a write row — so "what would this staging run have done
/// to production?" is one audit query. Never written in production. Written
/// only by `custom_apps_staging_held::record_held`.
pub(crate) const ACTION_STAGING_HELD: &str = "app.staging.held";
