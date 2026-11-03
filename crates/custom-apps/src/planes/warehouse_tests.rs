use std::collections::BTreeMap;

use oxy_app_core::custom_app_environment::AppEnvironment;
use serde_json::json;

use super::super::{PlaneContext, PlaneSpec, WarehouseMode};
use super::WarehousePlane;

fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(from, to)| (from.to_string(), to.to_string()))
        .collect()
}

fn spec(warehouse: serde_json::Value) -> PlaneSpec {
    PlaneSpec::parse(&json!({ "warehouse": warehouse })).expect("a valid spec")
}

fn staging() -> PlaneContext {
    PlaneContext {
        environment: AppEnvironment::Staging,
        fork_at: None,
        requested_at: None,
        now: chrono::Utc::now(),
        org_has_oltp: true,
        other_oltp_branches: 0,
        max_oltp_branches: 8,
        warehouse_refusal: None,
        existing_semantic_branch: None,
    }
}

/// The bare string is what every stored row has today: no overrides.
#[test]
fn the_bare_string_means_no_overrides() {
    let parsed = spec(json!("mapped"));
    assert_eq!(parsed.warehouse, Some(WarehousePlane::default()));
    assert!(parsed.warehouse.expect("set").destinations.is_empty());
}

#[test]
fn the_object_form_carries_its_destinations() {
    let parsed = spec(json!({
        "mode": "mapped",
        "destinations": { "ch": "ch_dev", "pg": "pg_dev" }
    }));
    assert_eq!(
        parsed.warehouse,
        Some(WarehousePlane {
            mode: WarehouseMode::Mapped,
            destinations: map(&[("ch", "ch_dev"), ("pg", "pg_dev")]),
        })
    );
    let no_map = spec(json!({ "mode": "mapped" }));
    assert_eq!(no_map.warehouse, Some(WarehousePlane::default()));
}

/// Both forms are kept as sent in the stored object, and resolve to the same
/// plane when they say the same thing.
#[test]
fn both_forms_are_stored_as_sent_and_resolve_alike() {
    let object = json!({ "warehouse": { "mode": "mapped", "destinations": {} } });
    let requested = PlaneSpec::from_request(None, Some(&object)).expect("the object form");
    assert_eq!(requested.stored, object);
    let bare = PlaneSpec::from_request(None, Some(&json!({ "warehouse": "mapped" })))
        .expect("the bare form");
    assert_eq!(bare.stored, json!({ "warehouse": "mapped" }));
    let resolved = |s: &PlaneSpec| {
        s.resolve(&AppEnvironment::Staging)
            .expect("planes")
            .warehouse
    };
    assert_eq!(resolved(&requested.spec), resolved(&bare.spec));
}

#[test]
fn a_malformed_warehouse_plane_is_refused_by_name() {
    for bad in [
        json!("teleport"),
        json!(3),
        json!({ "destinations": { "a": "b" } }),
        json!({ "mode": "teleport" }),
        json!({ "mode": "mapped", "extra": 1 }),
        json!({ "mode": "mapped", "destinations": ["a"] }),
        json!({ "mode": "mapped", "destinations": { "a": 1 } }),
    ] {
        PlaneSpec::parse(&json!({ "warehouse": bad })).expect_err("not a warehouse plane");
    }
}

#[test]
fn the_plane_still_lists_one_mode_and_reports_it() {
    use super::super::Plane;
    assert_eq!(Plane::Warehouse.modes(), vec!["mapped"]);
    let planes = spec(json!({ "mode": "mapped", "destinations": { "ch": "ch_dev" } }))
        .resolve(&AppEnvironment::Staging)
        .expect("planes");
    assert!(planes.entries().contains(&(Plane::Warehouse, "mapped")));
}

#[test]
fn validate_accepts_overrides_that_name_a_separate_database() {
    spec(json!({ "mode": "mapped", "destinations": { "ch": "ch_dev" } }))
        .validate(&staging())
        .expect("a mapping onto another database");
    spec(json!("mapped"))
        .validate(&staging())
        .expect("no overrides");
}

/// The production database is the key: a mapping onto itself writes it.
#[test]
fn validate_refuses_the_production_database() {
    let error = spec(json!({ "mode": "mapped", "destinations": { "ch": "ch" } }))
        .validate(&staging())
        .expect_err("onto itself");
    assert!(
        error.0.contains("`ch`") && error.0.contains("itself"),
        "{error}"
    );
    let chain = spec(json!({
        "mode": "mapped",
        "destinations": { "a": "b", "b": "c" }
    }))
    .validate(&staging())
    .expect_err("b is a production database");
    assert!(chain.0.contains("`b`"), "{chain}");
}

#[test]
fn validate_refuses_a_name_that_names_no_database() {
    for destinations in [
        json!({ "ch": "" }),
        json!({ " ": "ch_dev" }),
        json!({ "ch": "  " }),
    ] {
        let error = spec(json!({ "mode": "mapped", "destinations": destinations }))
            .validate(&staging())
            .expect_err("both must name a database");
        assert!(error.0.contains("must name a database"), "{error}");
    }
}

/// The facts the caller loaded (the workspace's databases, resolved) refuse
/// what a name check cannot: a credential that is production's.
#[test]
fn validate_carries_the_refusal_the_workspace_check_found() {
    let refused = PlaneContext {
        warehouse_refusal: Some("`ch_dev` resolves to the same host and user as `ch`".into()),
        ..staging()
    };
    let overrides = spec(json!({ "mode": "mapped", "destinations": { "ch": "ch_dev" } }));
    let error = overrides.validate(&refused).expect_err("the refusal");
    assert!(error.0.contains("same host and user"), "{error}");
    spec(json!("mapped"))
        .validate(&refused)
        .expect("no overrides, nothing to refuse");
}
