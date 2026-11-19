//! What a `ctx.*` call's trace record says about it, read from the call
//! before dispatch consumes it — the same fields its span reads
//! (`host_call_span::span_for`), and no others.
//!
//! - query / queryStream: the SQL verb and table (`db_query_summary`);
//! - fetch: the method and the host only (`url_shape::fetch_target`);
//! - semantic: the number of measures; email: recipients; notify: targets;
//! - airway: the pipeline; secrets.set: the key's name when it is an
//!   identifier; warehouse: the database and table the payload names;
//! - oltp, airhouse, tx, and warehouse statements: the namespace, verb and
//!   table the host resolves while running the op, from the call note
//!   (`custom_apps_functions::call_note`).
//!
//! Every string then passes the trace's own `identifier_like` gate
//! (`CallTrace::record`), the one gate on what is kept; this module only
//! decides which positions are read.

use super::super::call_note::CallNote;
use super::super::host_call_attrs::{db_query_summary, fetch_target};
use super::host_call::{Facets, HostCall};
use crate::custom_apps_call_trace::{CallFacts, CallStatus};

/// A call's identifiers, before it runs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct CallLabel {
    pub op: &'static str,
    pub family: &'static str,
    pub verb: Option<String>,
    pub target: Option<String>,
    pub count: Option<u32>,
}

fn count(n: usize) -> Option<u32> {
    u32::try_from(n).ok()
}

fn non_empty(s: String) -> Option<String> {
    (!s.is_empty()).then_some(s)
}

impl CallLabel {
    /// The label of `call`, paged under `op` (`host_call_span`'s name).
    pub(super) fn of(call: &HostCall, op: &'static str) -> Self {
        let mut label = CallLabel {
            op,
            family: call.family().wire(),
            ..CallLabel::default()
        };
        match call {
            HostCall::Query { sql, .. } | HostCall::QueryStream { sql, .. } => {
                let summary = db_query_summary(sql);
                label.verb = non_empty(summary.verb);
                label.target = non_empty(summary.table);
            }
            HostCall::Fetch { url, init, .. } => {
                let method = init
                    .get("method")
                    .and_then(|m| m.as_str())
                    .map(str::to_ascii_uppercase)
                    .unwrap_or_else(|| "GET".to_string());
                label.verb = Some(method);
                label.target = non_empty(fetch_target(url).host);
            }
            HostCall::SemanticQuery { spec, .. } => {
                label.count = spec
                    .get("measures")
                    .and_then(|m| m.as_array())
                    .and_then(|m| count(m.len()));
            }
            HostCall::AirwayRun { pipeline_ref, .. } => label.target = Some(pipeline_ref.clone()),
            HostCall::WarehouseWrite { payload, .. } => label.target = warehouse_target(payload),
            // Raw on purpose: the trace's own gate (`CallTrace::record`) is
            // the one that drops a name that is not an identifier.
            HostCall::SecretsSet { key, .. } => label.target = Some(key.clone()),
            HostCall::SendEmail { input, .. } => label.count = count(recipients(input, false)),
            HostCall::Notify { input, .. } => label.count = count(recipients(input, true)),
            HostCall::Storage { .. }
            | HostCall::OrgPeople { .. }
            | HostCall::OrgPlaces { .. }
            | HostCall::OrgAssignments { .. }
            | HostCall::PlatformFacts { .. }
            | HostCall::AppAction { .. }
            | HostCall::Tx { .. }
            | HostCall::Oltp { .. }
            | HostCall::Airhouse { .. }
            | HostCall::Events { .. }
            | HostCall::Apps { .. } => {}
        }
        label
    }

    /// The record's facts: this label, what the host noted while running the
    /// op (it knows the namespace and table a statement resolved to), and
    /// how the op ended.
    pub(super) fn into_facts(self, note: CallNote, facets: Facets) -> CallFacts {
        let target = note.target().or(self.target);
        // Held by the host's own mechanism: it answered, but did not perform.
        let (status, error_type) = match facets.status {
            CallStatus::Ok if note.held => (CallStatus::Held, Some("held")),
            status => (status, facets.error_type),
        };
        CallFacts {
            op: self.op,
            family: self.family,
            status,
            error_type,
            verb: note.verb.or(self.verb),
            target,
            count: self.count,
            rows: facets.rows,
            rows_truncated: facets.rows_truncated,
            http_status: facets.http_status,
        }
    }
}

/// `database.table` of a `warehouse.*` payload, or the database alone. Read
/// raw: a part that is not an identifier makes the whole target fail the
/// trace's gate, which drops it.
fn warehouse_target(payload: &serde_json::Value) -> Option<String> {
    let field = |name: &str| payload.get(name).and_then(|v| v.as_str());
    let database = field("database")?;
    Some(match field("table") {
        Some(table) => format!("{database}.{table}"),
        None => database.to_string(),
    })
}

/// How many recipients `input.to` names, as the span counts them: an array's
/// length, or one for a single value (a string for email, an object for
/// notify).
fn recipients(input: &serde_json::Value, notify: bool) -> usize {
    match input.get("to") {
        Some(serde_json::Value::Array(a)) => a.len(),
        Some(serde_json::Value::String(_)) if !notify => 1,
        Some(serde_json::Value::Object(_)) if notify => 1,
        _ => 0,
    }
}
