//! The broker side of a `ctx.*` call: the request an op sends over the
//! channel ([`HostCall`]), the span it runs under, and how it is dispatched to
//! the [`FunctionHost`] and recorded. Engine-neutral — nothing here knows
//! which JS engine sent the call.
//!
//! Every call resolves to a row of the host-op registry
//! (`env_policy::HostOp::spec`) before anything runs: its family is the
//! `HostCall` variant, its sub-op the string the isolate named. A call with
//! no row is refused as unknown and never reaches the host, so a host method
//! only ever sees an op the registry knows.

use super::super::env_policy::{Decision, Family, HeldTarget, HoldBy, HostOp, Verb};
use super::super::host_error::HostError;
use super::FunctionHost;
use super::ops::HostReply;
use crate::custom_apps_call_trace::CallStatus;

/// A request the isolate sends to the broker loop.
pub(super) enum HostCall {
    Query {
        sql: String,
        reply: HostReply,
    },
    QueryStream {
        sql: String,
        reply: HostReply,
    },
    Fetch {
        url: String,
        init: serde_json::Value,
        reply: HostReply,
    },
    SemanticQuery {
        spec: serde_json::Value,
        reply: HostReply,
    },
    AirwayRun {
        pipeline_ref: String,
        variables: serde_json::Value,
        reply: HostReply,
    },
    WarehouseWrite {
        op: String,
        payload: serde_json::Value,
        reply: HostReply,
    },
    SecretsSet {
        key: String,
        value: String,
        reply: HostReply,
    },
    SendEmail {
        input: serde_json::Value,
        reply: HostReply,
    },
    Notify {
        input: serde_json::Value,
        reply: HostReply,
    },
    Storage {
        op: String,
        payload: serde_json::Value,
        reply: HostReply,
    },
    /// `ctx.org.people()` — no arguments; the org is the host's, never the
    /// function's.
    OrgPeople {
        reply: HostReply,
    },
    OrgPlaces {
        reply: HostReply,
    },
    OrgAssignments {
        reply: HostReply,
    },
    /// `ctx.platform.facts(opts)` — `opts` names a span, never an org.
    PlatformFacts {
        opts: serde_json::Value,
        reply: HostReply,
    },
    Tx {
        op: String,
        payload: serde_json::Value,
        reply: HostReply,
    },
    Oltp {
        op: String,
        payload: serde_json::Value,
        reply: HostReply,
    },
    Airhouse {
        op: String,
        payload: serde_json::Value,
        reply: HostReply,
    },
    /// `ctx.events.publish(type, data, opts)` — `payload` is
    /// `{ type, data, key?, delaySeconds? }`; the app is the host's, never
    /// the function's. The same publish on an open `ctx.oltp.tx` handle
    /// arrives as [`HostCall::Tx`], op `publish`.
    Events {
        op: String,
        payload: serde_json::Value,
        reply: HostReply,
    },
    /// `ctx.apps.call(app, fn, body, opts)` and `ctx.apps.send(...)`:
    /// `payload` is `{ app, function, body, ... }`; the caller is the host's,
    /// never the function's.
    Apps {
        op: String,
        payload: serde_json::Value,
        reply: HostReply,
    },
}

impl HostCall {
    /// The registry family this call arrives on: one per variant.
    pub(super) fn family(&self) -> Family {
        match self {
            HostCall::Query { .. } => Family::Query,
            HostCall::QueryStream { .. } => Family::QueryStream,
            HostCall::Fetch { .. } => Family::Fetch,
            HostCall::SemanticQuery { .. } => Family::Semantic,
            HostCall::AirwayRun { .. } => Family::Airway,
            HostCall::WarehouseWrite { .. } => Family::Warehouse,
            HostCall::SecretsSet { .. } => Family::Secrets,
            HostCall::SendEmail { .. } => Family::Email,
            HostCall::Notify { .. } => Family::Notify,
            HostCall::Storage { .. } => Family::Storage,
            HostCall::OrgPeople { .. }
            | HostCall::OrgPlaces { .. }
            | HostCall::OrgAssignments { .. } => Family::Org,
            HostCall::PlatformFacts { .. } => Family::Platform,
            HostCall::Tx { .. } => Family::Tx,
            HostCall::Oltp { .. } => Family::Oltp,
            HostCall::Airhouse { .. } => Family::Airhouse,
            HostCall::Events { .. } => Family::Events,
            HostCall::Apps { .. } => Family::Apps,
        }
    }

    /// The member after the family in the op's name: the isolate's string
    /// for a family with sub-ops, the fixed member for a dotted single op
    /// (`semantic.query`, `org.people`), `None` for a bare name (`query`,
    /// `fetch`, `notify`).
    pub(super) fn sub_op(&self) -> Option<&str> {
        match self {
            HostCall::WarehouseWrite { op, .. }
            | HostCall::Storage { op, .. }
            | HostCall::Tx { op, .. }
            | HostCall::Oltp { op, .. }
            | HostCall::Airhouse { op, .. }
            | HostCall::Events { op, .. }
            | HostCall::Apps { op, .. } => Some(op),
            HostCall::SemanticQuery { .. } => Some("query"),
            HostCall::AirwayRun { .. } => Some("run"),
            HostCall::SecretsSet { .. } => Some("set"),
            HostCall::SendEmail { .. } => Some("send"),
            HostCall::OrgPeople { .. } => Some("people"),
            HostCall::OrgPlaces { .. } => Some("places"),
            HostCall::OrgAssignments { .. } => Some("assignments"),
            HostCall::PlatformFacts { .. } => Some("facts"),
            HostCall::Query { .. }
            | HostCall::QueryStream { .. }
            | HostCall::Fetch { .. }
            | HostCall::Notify { .. } => None,
        }
    }

    /// The registry op this call is, or `None` for a sub-op off the list.
    pub(super) fn op(&self) -> Option<HostOp> {
        HostOp::of(self.family(), self.sub_op())
    }

    /// What a broker-held call is logged under beside its op's plane and
    /// verb (`HoldBy::Broker`): the namespace and table the call names, as
    /// identifiers only — a pipeline, a database, a secret's name when it is
    /// one. Never a value.
    fn held_subject(&self) -> (&str, &str) {
        use super::super::host_call_attrs::identifier_like;
        match self {
            HostCall::AirwayRun { pipeline_ref, .. } => (pipeline_ref, ""),
            HostCall::WarehouseWrite { payload, .. } => (
                payload
                    .get("database")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default(),
                "",
            ),
            HostCall::SecretsSet { key, .. } if identifier_like(key) => ("", key),
            HostCall::PlatformFacts { .. } => ("facts", ""),
            _ => ("", ""),
        }
    }

    /// The call's reply channel, for answering without dispatching it.
    fn into_reply(self) -> HostReply {
        match self {
            HostCall::Query { reply, .. }
            | HostCall::QueryStream { reply, .. }
            | HostCall::Fetch { reply, .. }
            | HostCall::SemanticQuery { reply, .. }
            | HostCall::AirwayRun { reply, .. }
            | HostCall::WarehouseWrite { reply, .. }
            | HostCall::SecretsSet { reply, .. }
            | HostCall::SendEmail { reply, .. }
            | HostCall::Notify { reply, .. }
            | HostCall::Storage { reply, .. }
            | HostCall::OrgPeople { reply }
            | HostCall::OrgPlaces { reply }
            | HostCall::OrgAssignments { reply }
            | HostCall::PlatformFacts { reply, .. }
            | HostCall::Tx { reply, .. }
            | HostCall::Oltp { reply, .. }
            | HostCall::Airhouse { reply, .. }
            | HostCall::Events { reply, .. }
            | HostCall::Apps { reply, .. } => reply,
        }
    }
}

/// Which reply shape [`record_host_call_outcome`] should read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HostCallKind {
    Query,
    Fetch,
    Other,
}

/// Where a host call went, for the failure note's clear-on-success rule
/// (`FunctionHost::note_host_call_success`): a later success of the same op
/// clears a noted failure only when it went to the same place. Read from the
/// call before dispatch, on both the failing and the succeeding side, so the
/// two cannot disagree.
///
/// Only what a span of this call already records, and never a value the
/// caller could put anything in: a `fetch`'s host, by the one parser its span
/// uses (`url_shape::fetch_target`) — scheme, port, path and query stay out,
/// and a URL with no host has no target; a `warehouse.*` op's database and,
/// when the payload names one, table (`db.namespace`, `db.collection.name`),
/// gated as the span gates them (`data_audit::record_db_span`): a value that
/// is not `identifier_like` — over 64 chars, or anything outside
/// `[A-Za-z0-9_.$-]` — drops out of the target rather than into it, so the
/// note holds exactly what the span records. Every other op clears by name
/// alone: a storage key or an OLTP statement is payload, an app's own
/// Airhouse and OLTP schemas are one destination each, and a transaction op
/// names its transaction, not a place. The host normalizes what it keeps of
/// this (`HostCallFailure::noted`).
pub(super) fn host_call_target(call: &HostCall) -> Option<String> {
    use super::super::host_call_attrs::{fetch_target, identifier_like};
    match call {
        HostCall::Fetch { url, .. } => Some(fetch_target(url).host).filter(|host| !host.is_empty()),
        HostCall::WarehouseWrite { payload, .. } => {
            let field = |name: &str| {
                payload
                    .get(name)
                    .and_then(|v| v.as_str())
                    .filter(|v| identifier_like(v))
            };
            let database = field("database")?;
            Some(match field("table") {
                Some(table) => format!("{database} {table}"),
                None => database.to_string(),
            })
        }
        _ => None,
    }
}

/// The span one host op runs under (`host_call_span::span_for`), the reply
/// shape to read, and the fixed op name a failure of this call pages under:
/// the registry's name, or `<family>.other` for a sub-op off the list
/// (`Family::other_name`).
pub(super) fn host_call_span(call: &HostCall) -> (tracing::Span, HostCallKind, &'static str) {
    let family = call.family();
    let kind = match family {
        Family::Query | Family::QueryStream => HostCallKind::Query,
        Family::Fetch => HostCallKind::Fetch,
        _ => HostCallKind::Other,
    };
    let name = match call.op() {
        Some(op) => op.name(),
        None => family.other_name(),
    };
    (super::host_call_span::span_for(call), kind, name)
}

/// Run one host op and hand back the reply channel with its result, so the
/// caller can record the outcome on the current span before answering.
///
/// The registry's gates run here, before any implementation, in the order
/// production refuses: a call whose sub-op has no row is unknown; one whose
/// capability the manifest lacks is refused in the host's words; one the
/// environment holds or refuses by not running is answered held or refused
/// and noted in the held-write log. None of these reaches the host.
pub(super) async fn dispatch_host_call(
    call: HostCall,
    host: std::sync::Arc<dyn FunctionHost>,
) -> (HostReply, Result<serde_json::Value, HostError>) {
    let Some(op) = call.op() else {
        let message = call
            .family()
            .unknown_op_message(call.sub_op().unwrap_or_default());
        return (call.into_reply(), Err(HostError::backend(message)));
    };
    if let Err(refusal) = host.capabilities().refuse(op) {
        return (call.into_reply(), Err(refusal));
    }
    if let Err(refusal) = admit_by_environment(&call, op, host.as_ref()).await {
        return (call.into_reply(), Err(refusal));
    }
    match call {
        HostCall::Query { sql, reply } => (reply, host.query(sql).await),
        HostCall::QueryStream { sql, reply } => (reply, host.query_stream(sql).await),
        HostCall::Fetch { url, init, reply } => (reply, host.fetch(url, init).await),
        HostCall::SemanticQuery { spec, reply } => (reply, host.semantic_query(spec).await),
        HostCall::AirwayRun {
            pipeline_ref,
            variables,
            reply,
        } => (reply, host.airway_run(pipeline_ref, variables).await),
        HostCall::WarehouseWrite {
            op: _,
            payload,
            reply,
        } => {
            let result = if op == HostOp::WarehouseQuery {
                host.warehouse_query(payload).await
            } else {
                host.warehouse_write(sub_op_name(op), payload).await
            };
            (reply, result)
        }
        HostCall::Tx { payload, reply, .. } => (reply, host.tx(sub_op_name(op), payload).await),
        HostCall::Oltp { payload, reply, .. } => (reply, host.oltp(sub_op_name(op), payload).await),
        HostCall::Airhouse { payload, reply, .. } => {
            (reply, host.airhouse(sub_op_name(op), payload).await)
        }
        HostCall::Events { payload, reply, .. } => {
            (reply, host.events(sub_op_name(op), payload).await)
        }
        HostCall::Apps { payload, reply, .. } => (reply, host.apps(sub_op_name(op), payload).await),
        HostCall::SecretsSet { key, value, reply } => (reply, host.secrets_set(key, value).await),
        HostCall::SendEmail { input, reply } => (reply, host.send_email(input).await),
        HostCall::Notify { input, reply } => (reply, host.notify(input).await),
        HostCall::Storage { payload, reply, .. } => {
            (reply, host.storage(sub_op_name(op), payload).await)
        }
        HostCall::OrgPeople { reply } => (reply, host.org_people().await),
        HostCall::OrgPlaces { reply } => (reply, host.org_places().await),
        HostCall::OrgAssignments { reply } => (reply, host.org_assignments().await),
        HostCall::PlatformFacts { opts, reply } => (reply, host.platform_facts(opts).await),
    }
}

/// The sub-op a family's host method takes, as the registry spells it —
/// the isolate's string, now known to be on the list.
fn sub_op_name(op: HostOp) -> String {
    op.sub_name().unwrap_or_default().to_string()
}

/// The environment's decision for an op that holds by not running
/// (`HoldBy::Broker`): `Allow` and `Isolate` go on to the host, which routes
/// an isolated op to its home; `Hold` and `Refuse` are noted in the
/// held-write log under the row's plane and verb and answered with the
/// policy's own words — a preview's where a production run reads a branch.
/// An op that holds by a mechanism of its own (`HoldBy::Host`) is the
/// host's to decide, and passes.
async fn admit_by_environment(
    call: &HostCall,
    op: HostOp,
    host: &dyn FunctionHost,
) -> Result<(), HostError> {
    let HoldBy::Broker { plane, verb } = op.spec().holds else {
        return Ok(());
    };
    let policy = host.env_policy();
    let fix = match policy.decide(op) {
        Decision::Allow | Decision::Isolate(_) => return Ok(()),
        Decision::Hold => None,
        Decision::Refuse { fix } => Some(fix),
    };
    let (namespace, table) = call.held_subject();
    let verb = match verb {
        Verb::Fixed(verb) => verb,
        Verb::SubOp => call.sub_op().unwrap_or_default(),
    };
    let target: HeldTarget<'_> = (plane, namespace, verb, table);
    host.record_held(op, target).await;
    Err(match fix {
        Some(fix) => policy.refusal(op, fix),
        None => policy.held_error(op),
    })
}

/// How a host op ended, read once from its result: what the op's span
/// records and what its trace record keeps (`oxy_custom_apps::call_trace`),
/// so the two cannot disagree.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Facets {
    pub status: CallStatus,
    /// The fixed `error.type` facet, never the message.
    pub error_type: Option<&'static str>,
    pub rows: Option<u64>,
    pub rows_truncated: Option<bool>,
    pub http_status: Option<u16>,
}

/// The facets of `result`: an `error.type` on failure (`held` / `refused`
/// when the environment's policy decided it), rows / truncation for a query,
/// the upstream status for a fetch — a 5xx is an `upstream_5xx` error.
pub(super) fn outcome_facets(
    kind: HostCallKind,
    result: &Result<serde_json::Value, HostError>,
    environment_labels: bool,
) -> Facets {
    use super::super::host_call_attrs::{fetch_status, rows_and_truncated};
    use super::super::host_error_class::classify_host_error_typed;
    let value = match result {
        Err(error) => {
            let error_type = classify_host_error_typed(error, environment_labels);
            let status = match error_type {
                "held" => CallStatus::Held,
                "refused" => CallStatus::Refused,
                _ => CallStatus::Error,
            };
            return Facets {
                status,
                error_type: Some(error_type),
                ..Facets::default()
            };
        }
        Ok(value) => value,
    };
    match kind {
        HostCallKind::Query => {
            let (rows, rows_truncated) = rows_and_truncated(value);
            Facets {
                rows,
                rows_truncated,
                ..Facets::default()
            }
        }
        HostCallKind::Fetch => {
            let http_status = fetch_status(value).and_then(|s| u16::try_from(s).ok());
            let failed = http_status.is_some_and(|s| s >= 500);
            Facets {
                status: if failed {
                    CallStatus::Error
                } else {
                    CallStatus::Ok
                },
                error_type: failed.then_some("upstream_5xx"),
                http_status,
                ..Facets::default()
            }
        }
        HostCallKind::Other => Facets::default(),
    }
}

/// Record `facets` on the current span: `otel.status_code` and `error.type`
/// for a call that did not succeed, and the rows, truncation and upstream
/// status it returned.
pub(super) fn record_host_call_outcome(facets: &Facets) {
    let span = tracing::Span::current();
    if let Some(error_type) = facets.error_type {
        span.record("otel.status_code", "ERROR");
        span.record("error.type", error_type);
    }
    if let Some(rows) = facets.rows {
        span.record("db.response.returned_rows", rows);
    }
    if let Some(truncated) = facets.rows_truncated {
        span.record("oxy.truncated", truncated);
    }
    if let Some(status) = facets.http_status {
        span.record("http.response.status_code", u64::from(status));
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::oneshot;

    use super::super::test_calls::sample;
    use super::*;

    /// Every registry op arrives as one call, and the broker names it by the
    /// registry — so `HOST_OPS` is what `host_call_span` returns, and nothing
    /// is named by a literal here.
    #[test]
    fn every_registry_op_is_one_call_named_by_the_registry() {
        for op in HostOp::ALL {
            let call = sample(*op);
            assert_eq!(call.op(), Some(*op), "{op:?}");
            assert_eq!(host_call_span(&call).2, op.name());
        }
    }

    #[test]
    fn a_sub_op_off_the_list_pages_under_other_and_never_reaches_the_host() {
        let (reply, _rx) = oneshot::channel();
        let call = HostCall::Storage {
            op: "rename".into(),
            payload: serde_json::json!({}),
            reply,
        };
        assert_eq!(call.op(), None);
        assert_eq!(host_call_span(&call).2, "storage.other");
    }
}
