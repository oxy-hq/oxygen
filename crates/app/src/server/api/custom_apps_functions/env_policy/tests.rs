//! The policy table, and the skeleton of the differential host-op test
//! (`internal-docs/2026-09-10-custom-app-environments-design.md` §8).
//!
//! [`ROWS`] is written out by hand on purpose: it is the oracle
//! [`EnvPolicy::decide`] is checked against, so a change to one without the
//! other fails, and a new [`HostOp`] without a row fails. Each row says whether
//! the op writes, and what staging does with it **in an org with no OLTP
//! staging branch**. A held or refused staging write touches **no** resource,
//! so "production and staging touch disjoint resources" (§8) holds for it by
//! construction. A row isolated to a home is resolved in production and in
//! staging by `differential.rs`, through the functions the host calls, and the
//! resources each touched — mapped database, Airhouse schema, storage keys
//! (`delete` and `copy` included), secret path, email recipient — must be
//! disjoint. With a branch (P4b), exactly the rows that open an OLTP connection
//! become `Isolate(Target::OltpBranch)`, which the tests at the bottom pin; the
//! disjoint-database half of the §8 differential for them needs a database and
//! lives in `tests/custom_apps/staging_functions_oltp_branch.rs`.

use std::collections::BTreeSet;

use oxy_app_core::custom_app_environment::AppEnvironment;

use super::*;

struct Row {
    op: HostOp,
    /// Changes something outside the invocation when production runs it.
    writes: bool,
    staging: Decision,
}

const HOLD: Decision = Decision::Hold;
const ALLOW: Decision = Decision::Allow;
const MAPPED: Decision = Decision::Isolate(Target::MappedDestination);
const SIBLING: Decision = Decision::Isolate(Target::SiblingSchema);
const SILO: Decision = Decision::Isolate(Target::StorageSilo);

const fn read(op: HostOp) -> Row {
    Row {
        op,
        writes: false,
        staging: ALLOW,
    }
}

const fn write(op: HostOp, staging: Decision) -> Row {
    Row {
        op,
        writes: true,
        staging,
    }
}

/// A read that looks at the environment's own copy first, then production's.
const fn read_isolated(op: HostOp) -> Row {
    Row {
        op,
        writes: false,
        staging: SILO,
    }
}

const ROWS: &[Row] = &[
    read(HostOp::Query),
    read(HostOp::QueryStream),
    // A read method is sent; the decision covers the mutating ones.
    write(HostOp::Fetch, HOLD),
    read(HostOp::SemanticQuery),
    write(HostOp::AirwayRun, Decision::Refuse { fix: AIRWAY_FIX }),
    // Isolated to the mapped database; unmapped, `decide_on_database` holds.
    write(HostOp::WarehouseInsert, MAPPED),
    write(HostOp::WarehouseExec, MAPPED),
    write(HostOp::WarehouseUpsert, MAPPED),
    read(HostOp::WarehouseQuery),
    write(HostOp::TxBegin, MAPPED),
    write(HostOp::TxBeginOltp, HOLD),
    write(HostOp::TxQuery, HOLD),
    write(HostOp::TxExec, HOLD),
    write(HostOp::TxCommit, HOLD),
    read(HostOp::TxRollback),
    write(HostOp::OltpQuery, HOLD),
    write(HostOp::OltpExec, HOLD),
    read(HostOp::AirhouseQuery),
    write(HostOp::AirhouseExec, SIBLING),
    write(HostOp::AirhouseAppend, SIBLING),
    write(HostOp::StorageGetUploadUrl, SILO),
    read_isolated(HostOp::StorageGetDownloadUrl),
    write(HostOp::StoragePut, SILO),
    read_isolated(HostOp::StorageGet),
    read_isolated(HostOp::StorageHead),
    read_isolated(HostOp::StorageList),
    write(HostOp::StorageDelete, SILO),
    write(HostOp::StorageCopy, SILO),
    write(HostOp::SecretsSet, Decision::Isolate(Target::EnvSecrets)),
    write(HostOp::EmailSend, Decision::Isolate(Target::InvokerEmail)),
    read(HostOp::OrgPeople),
    read(HostOp::OrgPlaces),
    read(HostOp::OrgAssignments),
];

fn staging() -> EnvPolicy {
    EnvPolicy::for_environment(AppEnvironment::Staging)
}

/// Every write the staging policy isolates, with the home it names — the rows
/// `differential.rs` resolves in both environments.
pub(super) fn isolated_writes() -> Vec<(HostOp, Target)> {
    ROWS.iter()
        .filter(|row| row.writes)
        .filter_map(|row| match row.staging {
            Decision::Isolate(target) => Some((row.op, target)),
            _ => None,
        })
        .collect()
}

#[test]
fn every_host_op_has_exactly_one_row() {
    let rows: Vec<HostOp> = ROWS.iter().map(|r| r.op).collect();
    let unique: BTreeSet<HostOp> = rows.iter().copied().collect();
    assert_eq!(unique.len(), rows.len(), "an op has two rows");
    let all: BTreeSet<HostOp> = HostOp::ALL.iter().copied().collect();
    let missing: Vec<_> = all.difference(&unique).collect();
    assert!(
        missing.is_empty(),
        "host ops with no row in the differential table — decide what each does outside \
         production and add it: {missing:?}"
    );
}

#[test]
fn production_allows_every_op() {
    let production = EnvPolicy::production();
    for op in HostOp::ALL {
        assert_eq!(production.decide(*op), Decision::Allow, "{op:?}");
    }
}

/// A staging write is isolated, held or refused — it reaches no production
/// store (`differential` checks the isolated ones) — and a staging read reads
/// production, directly or behind the environment's own copy.
#[test]
fn staging_isolates_holds_or_refuses_every_write_and_reads_production() {
    let staging = staging();
    for row in ROWS {
        let decided = staging.decide(row.op);
        assert_eq!(decided, row.staging, "{:?}", row.op);
        if row.writes {
            assert!(
                matches!(
                    decided,
                    Decision::Isolate(_) | Decision::Hold | Decision::Refuse { .. }
                ),
                "{:?} writes, so staging must isolate, hold or refuse it; got {decided:?}",
                row.op
            );
        } else {
            assert!(
                matches!(decided, Decision::Allow | SILO),
                "{:?} reads production, or its environment's copy first; got {decided:?}",
                row.op
            );
        }
    }
}

fn sandbox() -> EnvPolicy {
    EnvPolicy::for_environment(AppEnvironment::Dev {
        handle: "luong".into(),
    })
}

/// A sandbox has no table of its own: it decides **exactly as staging** for
/// every op (`internal-docs/custom-app-sandboxes.md` §3) — the same row of
/// [`ROWS`], with or without the org's OLTP staging branch, on a handle, and
/// when a branch read is found. What differs is where an isolated write
/// lands, which `differential.rs` checks.
#[test]
fn a_sandbox_decides_exactly_as_staging_for_every_op() {
    let branch = OltpHome::StagingBranch("br-staging".into());
    for row in ROWS {
        let op = row.op;
        assert_eq!(sandbox().decide(op), row.staging, "{op:?}");
        assert_eq!(sandbox().decide(op), staging().decide(op), "{op:?}");
        assert_eq!(
            sandbox().with_oltp_home(branch.clone()).decide(op),
            staging().with_oltp_home(branch.clone()).decide(op),
            "{op:?} with the org's OLTP staging branch"
        );
        assert_eq!(
            sandbox().decide_on_branch(op),
            staging().decide_on_branch(op),
            "{op:?} reading a branch"
        );
        for opened_into in [
            None,
            Some(Target::MappedDestination),
            Some(Target::OltpBranch),
        ] {
            assert_eq!(
                sandbox().decide_on_handle(op, opened_into),
                staging().decide_on_handle(op, opened_into),
                "{op:?} on a handle opened into {opened_into:?}"
            );
        }
    }
    assert_eq!(
        sandbox().branch_reason(),
        None,
        "a sandbox's own pin is not a branch read"
    );
    let pin = Some(uuid::Uuid::from_u128(7));
    assert_eq!(sandbox().with_semantic_pin(pin).semantic_pin(), pin);
}

/// A held or refused op names the sandbox it ran in, not staging.
#[test]
fn held_and_refused_messages_name_the_sandbox() {
    let environment = AppEnvironment::Dev {
        handle: "luong".into(),
    };
    let held = held_message(HostOp::OltpExec, &environment);
    assert!(held.contains("dev-luong environment"), "{held}");
    let Decision::Refuse { fix } = sandbox().decide(HostOp::AirwayRun) else {
        panic!("ctx.airway.run is refused in a sandbox");
    };
    let refused = refused_message(HostOp::AirwayRun, &environment, fix);
    assert!(
        refused.contains("refused in the dev-luong environment"),
        "{refused}"
    );
    assert!(!refused.contains("dev slots"), "{refused}");
}

#[test]
fn names_round_trip_and_sub_ops_resolve() {
    for op in HostOp::ALL {
        assert_eq!(HostOp::from_name(op.name()), Some(*op));
    }
    assert_eq!(
        HostOp::sub_op("warehouse", "insert"),
        Some(HostOp::WarehouseInsert)
    );
    assert_eq!(HostOp::sub_op("storage", "rename"), None);
    assert_eq!(HostOp::from_name("other"), None);
}

/// A promoted build never reads a pin; a pin already in scope where the host
/// is built is kept whatever the environment — host calls run on tasks of
/// their own, and it is how a production run is found reading a branch.
#[test]
fn production_takes_no_build_pin_but_keeps_one_in_scope() {
    let pin = Some(uuid::Uuid::from_u128(7));
    assert_eq!(
        EnvPolicy::production()
            .with_semantic_pin(pin)
            .semantic_pin(),
        None
    );
    assert_eq!(staging().with_semantic_pin(pin).semantic_pin(), pin);
    assert_eq!(
        EnvPolicy::production().within_scope_pin(pin).semantic_pin(),
        pin
    );
    let build_pin = Some(uuid::Uuid::from_u128(8));
    assert_eq!(
        staging()
            .with_semantic_pin(build_pin)
            .within_scope_pin(pin)
            .semantic_pin(),
        build_pin,
        "the build's pin is what staging reads"
    );
}

/// Previews S9, I9: a production run reading a branch is a preview. It decides
/// as staging does — writes held, reads allowed — and refuses
/// `ctx.airway.run` with the branch's own reason. A pin in scope makes it
/// so from the start; a production run with none allows everything.
///
/// It has no environment, so none of staging's isolated homes: an op staging
/// isolates is held — its "own" silo, secret path or sibling would be
/// production's — except a storage read, which reads production's silo.
#[test]
fn a_production_run_reading_a_branch_decides_as_staging_and_refuses_airway() {
    let pinned = EnvPolicy::production().within_scope_pin(Some(uuid::Uuid::from_u128(9)));
    for op in HostOp::ALL {
        let on_branch = EnvPolicy::production().decide_on_branch(*op);
        let row = ROWS.iter().find(|r| r.op == *op).expect("a row per op");
        if *op == HostOp::AirwayRun {
            assert_eq!(
                on_branch,
                Decision::Refuse {
                    fix: BRANCH_AIRWAY_FIX
                }
            );
        } else if let Decision::Isolate(_) = staging().decide(*op) {
            let expected = if row.writes {
                Decision::Hold
            } else {
                Decision::Allow
            };
            assert_eq!(on_branch, expected, "{op:?}: no environment home");
        } else {
            assert_eq!(on_branch, staging().decide(*op), "{op:?}");
        }
        assert_eq!(pinned.decide(*op), on_branch, "{op:?}: a pin in scope");
        assert_eq!(EnvPolicy::production().decide(*op), Decision::Allow);
        assert_eq!(staging().decide_on_branch(*op), staging().decide(*op));
    }
    let app = uuid::Uuid::from_u128(7);
    for op in [
        HostOp::StorageGetUploadUrl,
        HostOp::StoragePut,
        HostOp::StorageDelete,
        HostOp::StorageCopy,
    ] {
        assert!(
            pinned.silo_for(op, app).is_none(),
            "{op:?}: a pinned production run writes no silo"
        );
    }
    assert!(pinned.silo_for(HostOp::StorageGet, app).is_some());
    assert_eq!(pinned.secret_segment(), None);
    assert!(pinned.branch_reason().is_some());
    assert_eq!(EnvPolicy::production().branch_reason(), None);
    assert_eq!(
        staging()
            .with_semantic_pin(Some(uuid::Uuid::from_u128(9)))
            .branch_reason(),
        None,
        "staging's own pin is not a branch read"
    );
}

#[test]
fn held_and_refused_messages_name_the_op_and_environment() {
    let held = held_message(HostOp::StoragePut, &AppEnvironment::Staging);
    assert!(held.starts_with("HeldInStaging: ctx.storage.put"), "{held}");
    assert!(held.contains("staging environment"), "{held}");
    let refused = refused_message(HostOp::AirwayRun, &AppEnvironment::Staging, AIRWAY_FIX);
    assert!(
        refused.starts_with("EnvironmentRefused: ctx.airway.run"),
        "{refused}"
    );
}

#[test]
fn a_held_fetch_answers_409_and_never_echoes_the_url() {
    let r = held_fetch_response(
        "POST",
        Some("api.example.com"),
        &held_message(HostOp::Fetch, &AppEnvironment::Staging),
    );
    assert_eq!(r["status"], 409);
    assert_eq!(r["held"], true);
    let body: serde_json::Value = serde_json::from_str(r["body"].as_str().unwrap()).unwrap();
    assert_eq!(body["error"], "held_in_staging");
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("api.example.com")
    );
}

#[test]
fn reads_are_the_closed_method_set() {
    for m in ["GET", "get", "HEAD", "OPTIONS"] {
        assert!(is_read_method(m), "{m}");
    }
    for m in ["POST", "PUT", "PATCH", "DELETE", "PROPFIND"] {
        assert!(!is_read_method(m), "{m}");
    }
}

#[test]
fn a_held_email_keeps_what_production_would_have_sent() {
    let r = held_email_result(
        &serde_json::json!({ "to": ["cfo@example.com"], "subject": "JE posted" }),
        &held_message(HostOp::EmailSend, &AppEnvironment::Staging),
    );
    assert_eq!(r["held"], true);
    assert_eq!(r["to"], serde_json::json!(["cfo@example.com"]));
    assert_eq!(r["subject"], "JE posted");
}

#[test]
fn recognises_postgres_read_only_refusal() {
    assert!(is_read_only_violation(
        "error returned from database: cannot execute INSERT in a read-only transaction"
    ));
    assert!(!is_read_only_violation("relation \"x\" does not exist"));
}

fn staging_on_branch() -> EnvPolicy {
    staging().with_oltp_home(OltpHome::StagingBranch("br-staging".into()))
}

/// P4b: with the org's staging branch, exactly the ops that open an OLTP
/// connection isolate to it; a statement or commit follows its handle; every
/// other row is what it is without a branch.
#[test]
fn with_a_staging_branch_the_oltp_rows_isolate_to_it_and_nothing_else_moves() {
    let branch = staging_on_branch();
    let isolated: BTreeSet<HostOp> = HostOp::ALL
        .iter()
        .copied()
        .filter(|op| branch.decide(*op) == Decision::Isolate(Target::OltpBranch))
        .collect();
    assert_eq!(
        isolated,
        BTreeSet::from([HostOp::TxBeginOltp, HostOp::OltpQuery, HostOp::OltpExec])
    );
    for row in ROWS.iter().filter(|r| !isolated.contains(&r.op)) {
        assert_eq!(branch.decide(row.op), row.staging, "{:?}", row.op);
    }
    for op in [
        HostOp::TxQuery,
        HostOp::TxExec,
        HostOp::TxCommit,
        HostOp::TxRollback,
    ] {
        assert_eq!(
            branch.decide_on_handle(op, Some(Target::OltpBranch)),
            Decision::Isolate(Target::OltpBranch),
            "{op:?} on a handle opened on the branch runs there"
        );
        assert_eq!(branch.decide_on_handle(op, None), branch.decide(op));
    }
    assert_eq!(
        staging().decide(HostOp::OltpExec),
        Decision::Hold,
        "without a branch the OLTP rows hold"
    );
}

/// Production writes its own database whatever it is handed, and a preview —
/// a production run reading a branch — holds as staging with no OLTP branch.
#[test]
fn production_and_a_preview_never_take_an_oltp_branch() {
    let home = OltpHome::StagingBranch("br-staging".into());
    let production = EnvPolicy::production().with_oltp_home(home.clone());
    assert_eq!(production.oltp_home(), &OltpHome::Production);
    for op in HostOp::ALL {
        assert_eq!(production.decide(*op), Decision::Allow, "{op:?}");
        assert_eq!(
            production.decide_on_handle(*op, Some(Target::OltpBranch)),
            Decision::Allow,
            "{op:?}"
        );
    }
    let preview = production.within_scope_pin(Some(uuid::Uuid::from_u128(9)));
    for op in [HostOp::TxBeginOltp, HostOp::OltpQuery, HostOp::OltpExec] {
        assert_eq!(preview.decide(op), Decision::Hold, "{op:?}");
    }
    assert_eq!(staging().oltp_home(), &OltpHome::Production, "the default");
    assert_eq!(staging_on_branch().oltp_home(), &home);
}
