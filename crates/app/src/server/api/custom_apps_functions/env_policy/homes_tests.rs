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
