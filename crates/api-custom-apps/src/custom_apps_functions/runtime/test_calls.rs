//! A call of each registry op, shared by the broker's unit tests
//! (`host_call`, `call_label_tests`).

use tokio::sync::oneshot;

use super::super::env_policy::{Family, HostOp};
use super::host_call::HostCall;

/// A call of each op, with the arguments its span reads.
pub(super) fn sample(op: HostOp) -> HostCall {
    let (reply, _rx) = oneshot::channel();
    let sub = op.sub_name().unwrap_or_default().to_string();
    let payload = serde_json::json!({});
    match op.family() {
        Family::Query => HostCall::Query {
            sql: "select 1".into(),
            reply,
        },
        Family::QueryStream => HostCall::QueryStream {
            sql: "select 1".into(),
            reply,
        },
        Family::Fetch => HostCall::Fetch {
            url: "https://x.example".into(),
            init: payload,
            reply,
        },
        Family::Semantic => HostCall::SemanticQuery {
            spec: payload,
            reply,
        },
        Family::Airway => HostCall::AirwayRun {
            pipeline_ref: "p".into(),
            variables: payload,
            reply,
        },
        Family::Warehouse => HostCall::WarehouseWrite {
            op: sub,
            payload,
            reply,
        },
        Family::Tx => HostCall::Tx {
            op: sub,
            payload,
            reply,
        },
        Family::Oltp => HostCall::Oltp {
            op: sub,
            payload,
            reply,
        },
        Family::Airhouse => HostCall::Airhouse {
            op: sub,
            payload,
            reply,
        },
        Family::Events => HostCall::Events {
            op: sub,
            payload,
            reply,
        },
        Family::Apps => HostCall::Apps {
            op: sub,
            payload,
            reply,
        },
        Family::Storage => HostCall::Storage {
            op: sub,
            payload,
            reply,
        },
        Family::Secrets => HostCall::SecretsSet {
            key: "K".into(),
            value: "v".into(),
            reply,
        },
        Family::Email => HostCall::SendEmail {
            input: payload,
            reply,
        },
        Family::Notify => HostCall::Notify {
            input: payload,
            reply,
        },
        Family::Org => match op {
            HostOp::OrgPeople => HostCall::OrgPeople { reply },
            HostOp::OrgPlaces => HostCall::OrgPlaces { reply },
            _ => HostCall::OrgAssignments { reply },
        },
        Family::Platform => HostCall::PlatformFacts {
            opts: payload,
            reply,
        },
        Family::Oxygen => HostCall::AppAction {
            op: sub,
            payload,
            reply,
        },
    }
}

/// What [`loaded`] hides in every value position of a call.
pub(super) const SENTINEL: &str = "SENTINEL_x9";

/// A call of each op whose every *value* position — SQL literals, a URL's
/// path, query and userinfo, recipients, subject and body, a storage key,
/// notify text, semantic filters, a secret's value, statement parameters and
/// rows — holds [`SENTINEL`]. The identifier positions (tables, a host, a
/// database, a secret's name) hold ordinary names.
pub(super) fn loaded(op: HostOp) -> HostCall {
    let (reply, _rx) = oneshot::channel();
    let sub = op.sub_name().unwrap_or_default().to_string();
    let s = SENTINEL;
    let sql = format!("select * from orders where email = '{s}' and note = '{s}'");
    let payload = serde_json::json!({
        "database": "analytics",
        "table": "orders",
        "sql": sql,
        "params": [s, s],
        "rows": [{ "email": s }],
        "key": s,
        "prefix": s,
        "body": s,
        "value": s,
        "handle": s,
    });
    match op.family() {
        Family::Query => HostCall::Query { sql, reply },
        Family::QueryStream => HostCall::QueryStream { sql, reply },
        Family::Fetch => HostCall::Fetch {
            url: format!("https://u:{s}@api.x.com:8443/{s}/orders?key={s}#{s}"),
            init: serde_json::json!({
                "method": "post",
                "headers": { "authorization": s },
                "body": s,
            }),
            reply,
        },
        Family::Semantic => HostCall::SemanticQuery {
            spec: serde_json::json!({
                "measures": ["orders.count"],
                "filters": [{ "member": "orders.email", "values": [s] }],
            }),
            reply,
        },
        Family::Airway => HostCall::AirwayRun {
            pipeline_ref: "orders_sync".into(),
            variables: serde_json::json!({ "since": s }),
            reply,
        },
        Family::Warehouse => HostCall::WarehouseWrite {
            op: sub,
            payload,
            reply,
        },
        Family::Tx => HostCall::Tx {
            op: sub,
            payload,
            reply,
        },
        Family::Oltp => HostCall::Oltp {
            op: sub,
            payload,
            reply,
        },
        Family::Airhouse => HostCall::Airhouse {
            op: sub,
            payload,
            reply,
        },
        Family::Events => HostCall::Events {
            op: sub,
            payload,
            reply,
        },
        Family::Apps => HostCall::Apps {
            op: sub,
            payload,
            reply,
        },
        Family::Storage => HostCall::Storage {
            op: sub,
            payload,
            reply,
        },
        Family::Secrets => HostCall::SecretsSet {
            key: "API_KEY".into(),
            value: s.into(),
            reply,
        },
        Family::Email => HostCall::SendEmail {
            input: serde_json::json!({ "to": [s, s], "subject": s, "html": s }),
            reply,
        },
        Family::Notify => HostCall::Notify {
            input: serde_json::json!({ "to": [{ "email": s }], "title": s, "body": s }),
            reply,
        },
        Family::Org => match op {
            HostOp::OrgPeople => HostCall::OrgPeople { reply },
            HostOp::OrgPlaces => HostCall::OrgPlaces { reply },
            _ => HostCall::OrgAssignments { reply },
        },
        Family::Platform => HostCall::PlatformFacts {
            opts: serde_json::json!({ "weeks": s }),
            reply,
        },
        Family::Oxygen => HostCall::AppAction {
            op: sub,
            payload: serde_json::json!({
                "name": s,
                "returnTo": s,
                "file": s,
                "fileName": "report.csv",
                "pipelineRef": "pipelines/report.airway.yml",
                "workflowId": "w1",
            }),
            reply,
        },
    }
}
