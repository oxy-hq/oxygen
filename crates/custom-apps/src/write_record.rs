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
pub fn identifier_like(token: &str) -> bool {
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
pub fn plane_for_dialect(dialect: agentic_connector::SqlDialect) -> &'static str {
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

/// Why a held-log entry is a call that was *sent* rather than held: a
/// mutating `ctx.fetch` to a registered provider sandbox ran for real, and is
/// noted in the log so the read-back lists it beside the held ones.
pub const SENT_NOTE: &str = "sent: a registered provider sandbox";

/// The most bytes of a sent call's path that are recorded.
const SENT_PATH_CAP: usize = 256;

/// A call that was sent to a registered provider sandbox. The path only —
/// never the query string, headers or body.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SentCall {
    pub path: String,
    /// The response status; `None` when the call got no response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
}

impl SentCall {
    /// A sent call, its path cut at the first `?` or `#` (a query string is
    /// never recorded) and capped at 256 bytes on a char boundary.
    pub fn new(path: &str, status: Option<u16>) -> Self {
        let path = path.split(['?', '#']).next().unwrap_or_default();
        let mut end = path.len().min(SENT_PATH_CAP);
        while !path.is_char_boundary(end) {
            end -= 1;
        }
        Self {
            path: path[..end].to_string(),
            status,
        }
    }
}

/// One write, as the audit row describes it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct WriteRecord {
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
    /// Set when the call was sent to a registered provider sandbox rather than
    /// held: its `path` and `status` serialize at the top level of the record.
    /// Absent (no keys) on every other record, which serializes as before.
    #[serde(flatten)]
    pub sent: Option<SentCall>,
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
pub fn actor_entry(
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
pub fn with_first_target(e: AuditEntry, writes: &[WriteRecord]) -> AuditEntry {
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
pub const ACTION_STAGING_HELD: &str = "app.staging.held";

#[cfg(test)]
mod tests {
    use super::*;

    fn record(sent: Option<SentCall>) -> WriteRecord {
        WriteRecord {
            plane: "oltp",
            namespace: "app_x".into(),
            verb: "INSERT".into(),
            table: "orders".into(),
            rows: Some(1),
            statements: 1,
            op: None,
            note: None,
            sent,
        }
    }

    #[test]
    fn a_record_without_a_sent_call_serializes_byte_identically() {
        assert_eq!(
            serde_json::to_string(&record(None)).unwrap(),
            r#"{"plane":"oltp","namespace":"app_x","verb":"INSERT","table":"orders","rows":1,"statements":1}"#
        );
    }

    #[test]
    fn a_sent_call_serializes_path_and_status_at_the_top_level() {
        let mut r = record(Some(SentCall::new("/v1/charges", Some(200))));
        r.op = Some("fetch");
        r.note = Some(SENT_NOTE);
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["path"], "/v1/charges");
        assert_eq!(v["status"], 200);
        assert_eq!(v["note"], SENT_NOTE);
        let no_response = serde_json::to_value(record(Some(SentCall::new("/v1", None)))).unwrap();
        assert_eq!(no_response["path"], "/v1");
        assert!(no_response.get("status").is_none());
    }

    #[test]
    fn a_long_path_is_capped_on_a_char_boundary() {
        let long = format!("/{}", "a".repeat(1024));
        assert_eq!(SentCall::new(&long, None).path.len(), 256);
        // 255 ASCII bytes, then a 2-byte char straddling the cap.
        let straddle = format!("{}é{}", "a".repeat(255), "b".repeat(10));
        let capped = SentCall::new(&straddle, None).path;
        assert_eq!(capped.len(), 255);
        assert!(capped.chars().all(|c| c == 'a'));
    }

    #[test]
    fn the_query_and_fragment_are_never_recorded() {
        for given in [
            "/v1/charges?api_key=sk_test_x",
            "/v1/charges#frag",
            "/v1/charges?a=b#frag",
        ] {
            assert_eq!(SentCall::new(given, Some(200)).path, "/v1/charges");
        }
        assert_eq!(SentCall::new("?only=query", None).path, "");
    }
}
