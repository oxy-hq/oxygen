//! The P5b half of the policy (`homes`): homes a call's names pick, and the
//! decisions that depend on them.

use oxy_app_core::custom_app_environment::AppEnvironment;

use super::*;

const HOLD: Decision = Decision::Hold;
const ALLOW: Decision = Decision::Allow;
const MAPPED: Decision = Decision::Isolate(Target::MappedDestination);
const SIBLING: Decision = Decision::Isolate(Target::SiblingSchema);

fn staging() -> EnvPolicy {
    EnvPolicy::for_environment(AppEnvironment::Staging)
}

fn mapped(pairs: &[(&str, &str)]) -> std::collections::BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(from, to)| (from.to_string(), to.to_string()))
        .collect()
}

/// A write to a database the build maps is isolated there; any other stays
/// held — never allowed onto production's.
#[test]
fn only_a_mapped_database_is_isolated_and_an_unmapped_one_holds() {
    let staging = staging().with_destinations(mapped(&[
        ("clickhouse", "clickhouse_staging"),
        ("self", "self"),
        ("blank", ""),
    ]));
    for op in [
        HostOp::WarehouseInsert,
        HostOp::WarehouseExec,
        HostOp::WarehouseUpsert,
        HostOp::TxBegin,
    ] {
        assert_eq!(
            staging.decide_on_database(op, "clickhouse"),
            MAPPED,
            "{op:?}"
        );
        for unmapped in ["pokehouse", "self", "blank", "clickhouse_staging"] {
            assert_eq!(
                staging.decide_on_database(op, unmapped),
                HOLD,
                "{op:?} to {unmapped}"
            );
        }
    }
    assert_eq!(
        staging.mapped_destination("clickhouse"),
        Some("clickhouse_staging")
    );
    assert_eq!(
        staging.mapped_destination("self"),
        None,
        "a mapping onto itself is none"
    );
    let chained = staging_policy_chained();
    assert_eq!(
        chained.mapped_destination("a"),
        None,
        "`b` is a production database the block maps too"
    );
    assert_eq!(
        chained.decide_on_database(HostOp::WarehouseInsert, "a"),
        HOLD
    );
    assert_eq!(chained.mapped_destination("b"), Some("c"));
    assert_eq!(
        staging.decide_on_database(HostOp::WarehouseQuery, "clickhouse"),
        ALLOW,
        "reads stay on production"
    );
}

/// Production ignores the mapping: it writes where it is told.
#[test]
fn production_ignores_the_non_production_mapping() {
    let production =
        EnvPolicy::production().with_destinations(mapped(&[("clickhouse", "clickhouse_staging")]));
    assert_eq!(production.mapped_destination("clickhouse"), None);
    for op in HostOp::ALL {
        assert_eq!(
            production.decide_on_database(*op, "clickhouse"),
            ALLOW,
            "{op:?}"
        );
        assert_eq!(production.decide_in_schema(*op, "app_x"), ALLOW, "{op:?}");
        assert_eq!(
            production.decide_on_handle(*op, Some(Target::MappedDestination)),
            ALLOW,
            "{op:?}"
        );
    }
    assert_eq!(production.sibling_schema("app_x"), None);
}

/// Staging's Airhouse writes land in the sibling; a schema that names none
/// holds them.
#[test]
fn airhouse_writes_are_isolated_to_the_sibling_or_held() {
    let staging = staging();
    assert_eq!(
        staging.sibling_schema("app_store_ops").as_deref(),
        Some("app_store_ops__staging")
    );
    for op in [HostOp::AirhouseExec, HostOp::AirhouseAppend] {
        assert_eq!(
            staging.decide_in_schema(op, "app_store_ops"),
            SIBLING,
            "{op:?}"
        );
        assert_eq!(staging.decide_in_schema(op, "app_a__b"), HOLD, "{op:?}");
    }
    assert_eq!(
        staging.decide_in_schema(HostOp::AirhouseQuery, "app_store_ops"),
        ALLOW,
        "ctx.airhouse.query reads production"
    );
}

fn sandbox(handle: &str) -> EnvPolicy {
    EnvPolicy::for_environment(AppEnvironment::Dev {
        handle: handle.into(),
    })
}

/// A sandbox has a sibling of its own, named from its schema label — the
/// handle's hyphens as underscores — and distinct from staging's and from
/// another sandbox's. A writer too long for the label names none.
#[test]
fn a_sandbox_writes_its_own_sibling() {
    assert_eq!(
        sandbox("a1-b2").sibling_schema("app_store_ops").as_deref(),
        Some("app_store_ops__dev_a1_b2")
    );
    assert_eq!(
        sandbox("a1").sibling_schema("app_store_ops").as_deref(),
        Some("app_store_ops__dev_a1")
    );
    assert_ne!(
        sandbox("a1").sibling_schema("app_store_ops"),
        staging().sibling_schema("app_store_ops")
    );
    let long_writer = format!("app_{}", "a".repeat(42));
    assert_eq!(
        sandbox("abcdefghijkl").sibling_schema(&long_writer),
        None,
        "64 bytes: no sibling, so the write holds"
    );
}

/// `airhouse::app_schema` recognises a sandbox's sibling by a handle rule of
/// its own (that crate does not depend on `oxy-app-core`). The two rules must
/// agree: a sibling this policy names for a valid handle is hidden from
/// schema listings, and a name built on an invalid handle is not one.
#[test]
fn the_airhouse_handle_rule_matches_the_environment_grammar() {
    for handle in [
        "a",
        "a1",
        "a1-b2",
        "abcdefghijkl",
        "a-b-c-d-e-f",
        "0",
        "9-z",
    ] {
        let environment = AppEnvironment::parse(&format!("dev-{handle}")).expect(handle);
        let sibling = EnvPolicy::for_environment(environment)
            .sibling_schema("app_x")
            .expect(handle);
        assert!(
            airhouse::app_schema::is_environment_schema(&sibling),
            "{sibling}"
        );
    }
    for handle in ["", "-a", "a-", "a--b", "abcdefghijklm", "a_b"] {
        assert_eq!(AppEnvironment::parse(&format!("dev-{handle}")), None);
        let lookalike = format!("app_x__dev_{}", handle.replace('-', "_"));
        // `a_b` reads back as the valid handle `a-b`: the label is the
        // sandbox `dev-a-b`'s, so that one IS a sibling.
        let expected = handle == "a_b";
        assert_eq!(
            airhouse::app_schema::is_environment_schema(&lookalike),
            expected,
            "{lookalike}"
        );
    }
}

/// A statement on a handle opened into an isolated home runs there; one on a
/// handle opened as asked is decided by its op — an OLTP statement holds.
#[test]
fn a_statement_follows_the_home_its_handle_was_opened_into() {
    let staging = staging();
    for op in [
        HostOp::TxQuery,
        HostOp::TxExec,
        HostOp::TxCommit,
        HostOp::TxRollback,
    ] {
        assert_eq!(
            staging.decide_on_handle(op, Some(Target::MappedDestination)),
            MAPPED,
            "{op:?}"
        );
        assert_eq!(
            staging.decide_on_handle(op, None),
            staging.decide(op),
            "{op:?}"
        );
    }
    assert_eq!(staging.decide_on_handle(HostOp::TxExec, None), HOLD);
    assert_eq!(
        staging.decide_on_handle(HostOp::OltpExec, Some(Target::MappedDestination)),
        HOLD,
        "only a handle's own statements follow it"
    );
}

/// `{a: b, b: c}`: `b` is production's `b`, so `a` has no staging home.
fn staging_policy_chained() -> EnvPolicy {
    staging().with_destinations(mapped(&[("a", "b"), ("b", "c")]))
}
