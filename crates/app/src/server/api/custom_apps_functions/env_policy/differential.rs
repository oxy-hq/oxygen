//! The differential host-op test
//! (`internal-docs/2026-09-10-custom-app-environments-design.md` §8): every
//! write the policy **isolates** is resolved in production and in staging, and
//! the two must write **disjoint** resources.
//!
//! Each resource is computed by the function the host itself routes that op
//! through (`homes`): the mapped database for a warehouse write, the moved
//! statement and its schema for an Airhouse write, the silo's key
//! normalization and re-rooting for `ctx.storage`, the secret naming the core
//! writer uses for `ctx.secrets.set`, the redirect `ctx.email.send` applies. A
//! new `Isolate(..)` row with no resource here fails to compile or fails the
//! count, as does any row whose staging resource could reach production's.

use std::collections::{BTreeMap, BTreeSet};

use airhouse::sql_rules::{self, Access};
use oxy::service::secret_manager::SecretManagerService;
use oxy_app_core::custom_app_environment::AppEnvironment;

use super::homes::{Routed, airhouse_write, warehouse_database};
use super::tests::isolated_writes;
use super::*;
use crate::emails::app_emailer::EmailSendInput;
use crate::server::api::custom_apps_storage::{normalize_in, validate_in};

const APP_SCHEMA: &str = "app_store_ops";
const DATABASE: &str = "clickhouse";
const MAPPED_TO: &str = "clickhouse_staging";
const APP: Uuid = Uuid::from_u128(7);
const INVOKER: &str = "staff@oxy.tech";

fn staging() -> EnvPolicy {
    EnvPolicy::for_environment(AppEnvironment::Staging).with_destinations(BTreeMap::from([(
        DATABASE.to_string(),
        MAPPED_TO.to_string(),
    )]))
}

/// A statement of the shape each Airhouse write op sends: `exec` as an author
/// writes it (with a read of its own schema inside), `append` as the host
/// builds it.
fn airhouse_sql(op: HostOp) -> &'static str {
    match op {
        HostOp::AirhouseExec => {
            "INSERT INTO app_store_ops.daily SELECT store, count(*) FROM app_store_ops.visits \
             GROUP BY store"
        }
        HostOp::AirhouseAppend => {
            r#"INSERT INTO "app_store_ops"."visits" ("visit_id") VALUES ('v1')"#
        }
        other => panic!("{other:?} is not an Airhouse write"),
    }
}

/// The database a warehouse write lands in.
fn databases(policy: &EnvPolicy, op: HostOp) -> BTreeSet<String> {
    match warehouse_database(policy, op, DATABASE) {
        Routed::AsAsked(db) | Routed::Isolated(db) => [db].into(),
        Routed::NotRun(_) => BTreeSet::new(),
    }
}

/// The schema an Airhouse write lands in — and proof, from the rules the host
/// checks with, that the statement sent writes nowhere else.
fn schemas(policy: &EnvPolicy, op: HostOp) -> BTreeSet<String> {
    match airhouse_write(policy, op, airhouse_sql(op), APP_SCHEMA).expect("a valid statement") {
        Routed::AsAsked(write) | Routed::Isolated(write) => {
            sql_rules::check(&write.statement, &write.schema, Access::Write)
                .unwrap_or_else(|e| panic!("{op:?} writes outside {}: {e}", write.schema));
            [write.schema].into()
        }
        Routed::NotRun(_) => BTreeSet::new(),
    }
}

/// What the call names, in every form a function can hold it: a pathname, a
/// production key (from production data) and a staging key.
fn named_keys() -> Vec<String> {
    vec![
        "uploads/a.pdf".to_string(),
        format!("customer-app-storage/{APP}/uploads/a.pdf"),
        format!("customer-app-storage/{APP}~staging/uploads/a.pdf"),
    ]
}

/// The objects a storage write creates, overwrites or deletes, in the silo
/// the host resolves (`EnvPolicy::silo_for`, which `env_homes` calls).
fn storage_writes(policy: &EnvPolicy, op: HostOp) -> BTreeSet<String> {
    let Some(silo) = policy.silo_for(op, APP) else {
        return BTreeSet::new();
    };
    let names = named_keys();
    match op {
        // A write lands at the normalized pathname; `copy`'s destination too.
        HostOp::StorageGetUploadUrl | HostOp::StoragePut | HostOp::StorageCopy => names
            .iter()
            .filter_map(|n| normalize_in(&silo, n, false).ok())
            .collect(),
        // A delete removes the key it validates to.
        HostOp::StorageDelete => names
            .iter()
            .filter_map(|n| validate_in(&silo, n).ok())
            .collect(),
        _ => BTreeSet::new(),
    }
}

/// The secret path `ctx.secrets.set("KEY")` writes.
fn secret_writes(policy: &EnvPolicy) -> BTreeSet<String> {
    let segment = match policy.decide(HostOp::SecretsSet) {
        Decision::Allow => None,
        Decision::Isolate(Target::EnvSecrets) => policy.secret_segment(),
        _ => return BTreeSet::new(),
    };
    [SecretManagerService::app_secret_name(
        APP,
        segment.as_deref(),
        "KEY",
    )]
    .into()
}

/// Who a `ctx.email.send` reaches.
fn email_recipients(policy: &EnvPolicy) -> BTreeSet<String> {
    let input: EmailSendInput = serde_json::from_value(serde_json::json!({
        "to": ["cfo@customer.com"], "cc": "controller@customer.com",
        "bcc": ["audit@customer.com"], "subject": "JE posted", "text": "x",
    }))
    .expect("parses");
    match policy.decide(HostOp::EmailSend) {
        Decision::Allow => input.recipients().into_iter().collect(),
        Decision::Isolate(Target::InvokerEmail) => {
            let environment = policy.environment().name();
            let (message, _) = input
                .redirected_to(INVOKER, &environment)
                .expect("redirects");
            message.recipients().into_iter().collect()
        }
        _ => BTreeSet::new(),
    }
}

fn written(policy: &EnvPolicy, op: HostOp, target: Target) -> BTreeSet<String> {
    match target {
        Target::MappedDestination => databases(policy, op),
        Target::SiblingSchema => schemas(policy, op),
        Target::StorageSilo => storage_writes(policy, op),
        Target::EnvSecrets => secret_writes(policy),
        Target::InvokerEmail => email_recipients(policy),
        Target::OltpBranch => oltp_databases(policy, op),
    }
}

/// The OLTP database a `ctx.oltp` op reaches: production's, or the org's
/// staging branch by its provider id. That the two are different databases on
/// the wire is `tests/custom_apps/staging_functions_oltp_branch.rs`.
fn oltp_databases(policy: &EnvPolicy, op: HostOp) -> BTreeSet<String> {
    match (policy.decide(op), policy.oltp_home()) {
        (Decision::Allow, _) => ["production".to_string()].into(),
        (Decision::Isolate(Target::OltpBranch), OltpHome::StagingBranch(id)) => {
            [format!("branch:{id}")].into()
        }
        _ => BTreeSet::new(),
    }
}

#[test]
fn every_isolated_write_touches_disjoint_resources_in_production_and_staging() {
    let production = EnvPolicy::production();
    let staging = staging();
    let mut checked = 0;
    for (op, target) in isolated_writes() {
        let prod = written(&production, op, target);
        let stg = written(&staging, op, target);
        assert!(!prod.is_empty(), "{op:?}: production writes something");
        assert!(!stg.is_empty(), "{op:?}: staging's isolated home is named");
        let shared: Vec<_> = prod.intersection(&stg).collect();
        assert!(
            shared.is_empty(),
            "{op:?}: staging writes production's {shared:?}"
        );
        checked += 1;
    }
    assert_eq!(
        checked, 12,
        "warehouse insert/exec/upsert, tx.begin, airhouse exec/append, storage ×4, \
         secrets.set, email.send"
    );
}

/// An unmapped database is held — it never becomes production's write.
#[test]
fn an_unmapped_warehouse_write_is_held_not_sent_to_production() {
    let staging = EnvPolicy::for_environment(AppEnvironment::Staging);
    for (op, target) in isolated_writes() {
        if target != Target::MappedDestination {
            continue;
        }
        assert_eq!(
            warehouse_database(&staging, op, DATABASE),
            Routed::NotRun(Decision::Hold),
            "{op:?}"
        );
    }
}

/// The staging statement no longer names production's schema anywhere, and
/// production's own rules refuse it — the second check is the fence.
#[test]
fn a_staging_airhouse_statement_cannot_write_productions_schema() {
    let staging = staging();
    for op in [HostOp::AirhouseExec, HostOp::AirhouseAppend] {
        let Routed::Isolated(write) =
            airhouse_write(&staging, op, airhouse_sql(op), APP_SCHEMA).expect("valid")
        else {
            panic!("{op:?} is isolated in staging");
        };
        assert_eq!(write.schema, "app_store_ops__staging");
        assert!(
            !write
                .statement
                .replace("app_store_ops__staging", "")
                .contains("app_store_ops"),
            "{op:?}: {}",
            write.statement
        );
        assert!(
            sql_rules::check(&write.statement, APP_SCHEMA, Access::Write).is_err(),
            "{op:?}: production's schema rules must refuse the moved statement"
        );
    }
}

/// Staging refuses what production refuses, before anything is decided.
#[test]
fn a_statement_production_refuses_is_refused_in_staging_too() {
    for policy in [EnvPolicy::production(), staging()] {
        for sql in [
            "INSERT INTO other_app.t VALUES (1)",
            "INSERT INTO app_store_ops__staging.t VALUES (1)",
            "CREATE TABLE app_store_ops.t (id VARCHAR)",
        ] {
            assert!(
                airhouse_write(&policy, HostOp::AirhouseExec, sql, APP_SCHEMA).is_err(),
                "{sql} in {:?}",
                policy.environment()
            );
        }
    }
}

#[test]
fn staging_storage_writes_stay_in_its_silo_whatever_key_the_call_names() {
    let staging = EnvPolicy::for_environment(AppEnvironment::Staging);
    let prefix = staging.storage_silo(APP).prefix();
    for op in [
        HostOp::StorageGetUploadUrl,
        HostOp::StoragePut,
        HostOp::StorageDelete,
        HostOp::StorageCopy,
    ] {
        let keys = storage_writes(&staging, op);
        assert!(
            keys.iter().all(|k| k.starts_with(&prefix)),
            "{op:?} wrote outside {prefix}: {keys:?}"
        );
        assert_eq!(
            keys.len(),
            1,
            "{op:?}: every named form re-roots to one key"
        );
    }
}

#[test]
fn staging_email_reaches_the_invoker_alone() {
    let staging = EnvPolicy::for_environment(AppEnvironment::Staging);
    assert_eq!(
        email_recipients(&staging),
        BTreeSet::from([INVOKER.to_string()])
    );
}

/// P4b: with the org's staging branch, every `ctx.oltp` op that isolates
/// reaches the branch, never production's database.
#[test]
fn staging_oltp_on_a_branch_touches_a_different_database_than_production() {
    let production = EnvPolicy::production();
    let branch = staging().with_oltp_home(OltpHome::StagingBranch("br-staging".into()));
    for op in [HostOp::TxBeginOltp, HostOp::OltpQuery, HostOp::OltpExec] {
        let prod = written(&production, op, Target::OltpBranch);
        let stg = written(&branch, op, Target::OltpBranch);
        assert_eq!(prod, BTreeSet::from(["production".to_string()]), "{op:?}");
        assert_eq!(
            stg,
            BTreeSet::from(["branch:br-staging".to_string()]),
            "{op:?}"
        );
    }
    let no_branch = staging();
    assert!(
        written(&no_branch, HostOp::OltpExec, Target::OltpBranch).is_empty(),
        "without a branch the op is held and touches nothing"
    );
}
