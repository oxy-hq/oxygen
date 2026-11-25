//! The relay task's payload.

use agentic_core::delegation::TaskSpec;
use serde_json::json;
use uuid::Uuid;

use super::{RELAY_KIND, RelayPayload};

#[test]
fn a_payload_reads_back_from_its_spec() {
    let payload = RelayPayload {
        org_id: Uuid::from_u128(7),
        environment: "production".to_owned(),
    };
    let spec = payload.spec().expect("a spec");
    assert!(matches!(&spec, TaskSpec::Custom { kind, .. } if kind == RELAY_KIND));
    assert_eq!(RelayPayload::from_spec(&spec), Ok(payload));
}

#[test]
fn another_kind_is_not_a_relay() {
    let spec = TaskSpec::Custom {
        kind: "app_events_relay".to_owned(),
        payload: json!({ "org_id": Uuid::from_u128(7), "environment": "production" }),
    };
    assert!(RelayPayload::from_spec(&spec).is_err());
    let spec = TaskSpec::Custom {
        kind: RELAY_KIND.to_owned(),
        payload: json!({ "org": "x" }),
    };
    assert!(RelayPayload::from_spec(&spec).is_err());
}

#[test]
fn the_task_kind_is_the_relays() {
    assert_eq!(RELAY_KIND, "oltp_changes_relay");
}
