//! The span one host op runs under, built per [`Family`] from what the call
//! carries. Named and attributed on the OpenTelemetry semantic conventions
//! where one exists (`db.query`, `http.client.request`), `oxy.*` otherwise —
//! see `host_call_attrs` for what is deliberately *not* recorded. The op's
//! fixed name comes from the registry, not from here (`host_call_span`).

use tracing::Span;
use tracing::field::Empty;

use super::super::env_policy::Family;
use super::super::host_call_attrs::{HOST_CALL_TARGET, db_query_summary, fetch_target};
use super::host_call::HostCall;

/// An op whose arguments carry nothing a span records. A macro, because the
/// span's name must be a literal.
macro_rules! plain_span {
    ($name:literal) => {
        tracing::info_span!(target: HOST_CALL_TARGET,
            $name,
            otel.status_code = Empty,
            error.type = Empty,
        )
    };
}

/// The span for `call`, by its family.
pub(super) fn span_for(call: &HostCall) -> Span {
    match call {
        HostCall::Query { sql, .. } | HostCall::QueryStream { sql, .. } => {
            query_span(sql, call.family())
        }
        HostCall::Fetch { url, init, .. } => fetch_span(url, init),
        HostCall::SemanticQuery { spec, .. } => {
            let measures = spec
                .get("measures")
                .and_then(|m| m.as_array())
                .map(|m| m.len());
            tracing::info_span!(target: HOST_CALL_TARGET,
                "oxy.semantic.query",
                oxy.semantic.measures = measures,
                otel.status_code = Empty,
                error.type = Empty,
            )
        }
        HostCall::AirwayRun { pipeline_ref, .. } => tracing::info_span!(target: HOST_CALL_TARGET,
            "oxy.airway.run",
            oxy.airway.pipeline = %pipeline_ref,
            otel.status_code = Empty,
            error.type = Empty,
        ),
        HostCall::WarehouseWrite { op, .. } | HostCall::Airhouse { op, .. } => {
            db_query_span("airhouse", op)
        }
        HostCall::Oltp { op, .. } => db_query_span("postgres", op),
        HostCall::Tx { op, .. } => tracing::info_span!(target: HOST_CALL_TARGET,
            "db.transaction",
            db.operation.name = %op,
            db.namespace = Empty,
            db.query.summary = Empty,
            db.collection.name = Empty,
            otel.status_code = Empty,
            error.type = Empty,
        ),
        HostCall::Events { .. } => plain_span!("oxy.events.publish"),
        HostCall::Apps { .. } => plain_span!("oxy.apps.call"),
        HostCall::SecretsSet { key, .. } => tracing::info_span!(target: HOST_CALL_TARGET,
            "oxy.secrets.set",
            oxy.secret.name = %key,
            otel.status_code = Empty,
            error.type = Empty,
        ),
        HostCall::SendEmail { input, .. } => {
            let recipients = match input.get("to") {
                Some(serde_json::Value::Array(a)) => a.len(),
                Some(serde_json::Value::String(_)) => 1,
                _ => 0,
            };
            tracing::info_span!(target: HOST_CALL_TARGET,
                "oxy.email.send",
                oxy.email.recipients = recipients,
                otel.status_code = Empty,
                error.type = Empty,
            )
        }
        HostCall::Notify { input, .. } => {
            let targets = match input.get("to") {
                Some(serde_json::Value::Array(a)) => a.len(),
                Some(serde_json::Value::Object(_)) => 1,
                _ => 0,
            };
            tracing::info_span!(target: HOST_CALL_TARGET,
                "oxy.notify",
                oxy.notify.targets = targets,
                otel.status_code = Empty,
                error.type = Empty,
            )
        }
        HostCall::Storage { op, .. } => tracing::info_span!(target: HOST_CALL_TARGET,
            "oxy.storage",
            oxy.storage.op = %op,
            otel.status_code = Empty,
            error.type = Empty,
        ),
        HostCall::OrgPeople { .. } => plain_span!("oxy.org.people"),
        HostCall::OrgPlaces { .. } => plain_span!("oxy.org.places"),
        HostCall::OrgAssignments { .. } => plain_span!("oxy.org.assignments"),
        HostCall::PlatformFacts { .. } => plain_span!("oxy.platform.facts"),
    }
}

/// `ctx.query` / `ctx.queryStream`: the statement's verb and table, never
/// its text.
fn query_span(sql: &str, family: Family) -> Span {
    let s = db_query_summary(sql);
    tracing::info_span!(target: HOST_CALL_TARGET,
        "db.query",
        db.operation.name = %s.verb,
        db.collection.name = %s.table,
        oxy.op = family.wire(),
        db.response.returned_rows = Empty,
        oxy.truncated = Empty,
        otel.status_code = Empty,
        error.type = Empty,
    )
}

/// `ctx.fetch`: scheme, host and port, never the path or query.
fn fetch_span(url: &str, init: &serde_json::Value) -> Span {
    let t = fetch_target(url);
    let method = init
        .get("method")
        .and_then(|m| m.as_str())
        .map(str::to_ascii_uppercase)
        .unwrap_or_else(|| "GET".to_string());
    tracing::info_span!(target: HOST_CALL_TARGET,
        "http.client.request",
        otel.kind = "client",
        http.request.method = %method,
        url.scheme = %t.scheme,
        server.address = %t.host,
        server.port = t.port,
        http.response.status_code = Empty,
        otel.status_code = Empty,
        error.type = Empty,
    )
}

/// A statement on a named store. `db.namespace` / `db.query.summary` /
/// `db.collection.name` are recorded by the host once it has resolved the
/// schema and summarised the SQL (`host::record_db_span`), so the statement
/// is not summarised twice.
fn db_query_span(system: &'static str, op: &str) -> Span {
    tracing::info_span!(target: HOST_CALL_TARGET,
        "db.query",
        db.system = system,
        db.operation.name = %op,
        db.namespace = Empty,
        db.query.summary = Empty,
        db.collection.name = Empty,
        otel.status_code = Empty,
        error.type = Empty,
    )
}
