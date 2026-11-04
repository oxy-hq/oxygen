//! Host-call spans and the targets they record.

use super::*;

/// Every host-op span must carry [`HOST_CALL_TARGET`]: the product
/// collector keeps them out of the tenant Traces console with an
/// `oxy::host_call=off` directive, and a span that falls back to the
/// module target slips past it. A new `HostCall` variant fails the
/// exhaustive `match` below until it is added to the list.
#[test]
fn every_host_call_span_carries_the_platform_only_target() {
    use super::super::super::host_call_attrs::HOST_CALL_TARGET;
    use tokio::sync::oneshot;
    fn reply() -> oneshot::Sender<Result<serde_json::Value, HostError>> {
        oneshot::channel().0
    }
    let json = serde_json::Value::Null;
    let calls = vec![
        HostCall::Query {
            sql: "select 1".into(),
            reply: reply(),
        },
        HostCall::QueryStream {
            sql: "select 1".into(),
            reply: reply(),
        },
        HostCall::Fetch {
            url: "https://example.test/".into(),
            init: json.clone(),
            reply: reply(),
        },
        HostCall::SemanticQuery {
            spec: json.clone(),
            reply: reply(),
        },
        HostCall::AirwayRun {
            pipeline_ref: "p".into(),
            variables: json.clone(),
            reply: reply(),
        },
        HostCall::WarehouseWrite {
            op: "query".into(),
            payload: json.clone(),
            reply: reply(),
        },
        HostCall::SecretsSet {
            key: "k".into(),
            value: "v".into(),
            reply: reply(),
        },
        HostCall::SendEmail {
            input: json.clone(),
            reply: reply(),
        },
        HostCall::Notify {
            input: json.clone(),
            reply: reply(),
        },
        HostCall::Storage {
            op: "put".into(),
            payload: json.clone(),
            reply: reply(),
        },
        HostCall::OrgPeople { reply: reply() },
        HostCall::OrgPlaces { reply: reply() },
        HostCall::OrgAssignments { reply: reply() },
        HostCall::PlatformFacts {
            opts: json.clone(),
            reply: reply(),
        },
        HostCall::Tx {
            op: "begin".into(),
            payload: json.clone(),
            reply: reply(),
        },
        HostCall::Oltp {
            op: "query".into(),
            payload: json.clone(),
            reply: reply(),
        },
        HostCall::Airhouse {
            op: "append".into(),
            payload: json,
            reply: reply(),
        },
    ];
    for call in &calls {
        // Exhaustive on purpose: a new variant must be listed above.
        match call {
            HostCall::Query { .. }
            | HostCall::QueryStream { .. }
            | HostCall::Fetch { .. }
            | HostCall::SemanticQuery { .. }
            | HostCall::AirwayRun { .. }
            | HostCall::WarehouseWrite { .. }
            | HostCall::SecretsSet { .. }
            | HostCall::SendEmail { .. }
            | HostCall::Notify { .. }
            | HostCall::Storage { .. }
            | HostCall::OrgPeople { .. }
            | HostCall::OrgPlaces { .. }
            | HostCall::OrgAssignments { .. }
            | HostCall::PlatformFacts { .. }
            | HostCall::Tx { .. }
            | HostCall::Oltp { .. }
            | HostCall::Airhouse { .. } => {}
        }
    }
    // A bare registry enables every span, so `metadata()` is `Some`.
    tracing::subscriber::with_default(tracing_subscriber::registry(), || {
        for call in &calls {
            let (span, _, _) = host_call_span(call);
            let meta = span.metadata().expect("span is enabled under a registry");
            assert_eq!(
                meta.target(),
                HOST_CALL_TARGET,
                "span `{}` would leak into the product store",
                meta.name()
            );
        }
    });
}

/// The note holds exactly what the call's span records, gated the same
/// way (`data_audit::record_db_span`): a database or table that is not
/// `identifier_like` — over 64 chars, or carrying anything outside
/// `[A-Za-z0-9_.$-]` — drops out of the target rather than into it, and a
/// fetch keeps the host alone.
#[test]
fn host_call_target_keeps_only_what_the_span_records() {
    fn warehouse(payload: serde_json::Value) -> HostCall {
        let (reply, _rx) = tokio::sync::oneshot::channel();
        HostCall::WarehouseWrite {
            op: "insert".into(),
            payload,
            reply,
        }
    }
    fn fetch(url: &str) -> HostCall {
        let (reply, _rx) = tokio::sync::oneshot::channel();
        HostCall::Fetch {
            url: url.into(),
            init: serde_json::json!({}),
            reply,
        }
    }
    let target = |call: HostCall| host_call_target(&call);
    assert_eq!(
        target(warehouse(
            serde_json::json!({ "database": "dw", "table": "ledger" })
        )),
        Some("dw ledger".into())
    );
    assert_eq!(
        target(warehouse(serde_json::json!({ "database": "dw" }))),
        Some("dw".into())
    );
    // A table the span would not record leaves the database alone.
    for table in ["x".repeat(65), "ledger; drop table x".into(), "".into()] {
        assert_eq!(
            target(warehouse(
                serde_json::json!({ "database": "dw", "table": table })
            )),
            Some("dw".into()),
            "{table}"
        );
    }
    // A database the span would not record is no target at all.
    assert_eq!(
        target(warehouse(
            serde_json::json!({ "database": "x".repeat(65), "table": "ledger" })
        )),
        None
    );
    assert_eq!(
        target(warehouse(serde_json::json!({ "table": "ledger" }))),
        None
    );
    // A fetch keeps the host: no userinfo, port, path or query.
    assert_eq!(
        target(fetch(
            "https://user:pw@api.example.com:8443/v1/x?key=sk_live_1"
        )),
        Some("api.example.com".into())
    );
    // An op that names no place.
    let (reply, _rx) = tokio::sync::oneshot::channel();
    assert_eq!(
        target(HostCall::Query {
            sql: "select 1".into(),
            reply
        }),
        None
    );
}
