//! `LocalProvider` branch helpers that need no cluster: the derived name,
//! the guard on it, and the SQL the copy and the emptying run.

use super::*;

const TENANT: &str = "oxy_org_11111111_2222_3333_4444_555555555555";

#[test]
fn a_branch_database_is_a_valid_identifier_within_the_name_cap() {
    let name = branch_database_name(TENANT, "oxy-staging");
    assert!(name.len() <= MAX_NAME_LEN, "{name} is {} chars", name.len());
    assert!(validate_name(&name).is_ok(), "{name}");
    assert!(
        name.starts_with(TENANT),
        "readable as this tenant's: {name}"
    );
    assert_ne!(name, TENANT);
    assert_eq!(
        name,
        branch_database_name(TENANT, "oxy-staging"),
        "derived, so an orphan can be found again"
    );
}

#[test]
fn the_suffix_separates_branches_and_survives_truncation() {
    assert_ne!(
        branch_database_name(TENANT, "oxy-staging"),
        branch_database_name(TENANT, "oxy-dev")
    );
    // Two long tenants sharing everything the cap keeps still differ,
    // because the hash covers the whole name and is never the part trimmed.
    let a = format!("{}_aaaaaaaaaaaaaaaaaaaa", "x".repeat(40));
    let b = format!("{}_bbbbbbbbbbbbbbbbbbbb", "x".repeat(40));
    let (na, nb) = (
        branch_database_name(&a, "oxy-staging"),
        branch_database_name(&b, "oxy-staging"),
    );
    assert_ne!(na, nb);
    assert!(na.len() <= MAX_NAME_LEN && nb.len() <= MAX_NAME_LEN);
    assert!(validate_name(&na).is_ok() && validate_name(&nb).is_ok());
}

fn staging(database: &str) -> BranchRequest {
    BranchRequest::staging(
        database,
        "local",
        "oxy-staging",
        database,
        format!("{database}_owner"),
    )
}

#[test]
fn only_the_derived_branch_database_may_be_touched() {
    let req = staging(TENANT);
    let err = checked(&req, TENANT).unwrap_err();
    assert!(matches!(err, ProviderError::BranchIsProduction(_)), "{err}");

    // Another org's database — a valid identifier, not the tenant's, and
    // exactly what a corrupted row would name. Must not be dropped.
    let other = "oxy_org_99999999_2222_3333_4444_555555555555";
    assert!(checked(&req, other).is_err(), "another org's database");
    let other_branch = branch_database_name(other, "oxy-staging");
    assert!(
        checked(&req, &other_branch).is_err(),
        "another org's branch"
    );
    assert!(checked(&req, "bad\"; DROP DATABASE x; --").is_err());

    assert!(checked(&req, &branch_database_name(TENANT, "oxy-staging")).is_ok());
}

/// A shared box behind `OXY_OLTP_PROVIDER=local`: no branch copy, reset or
/// drop — refused before a connection is even attempted (the hosts below
/// do not resolve, so reaching one would fail differently).
#[tokio::test]
async fn branch_operations_refuse_a_cluster_that_is_not_loopback() {
    let req = staging(TENANT);
    let branch = branch_database_name(TENANT, "oxy-staging");
    for (admin, host) in [
        (
            "postgres://postgres:pw@db.internal.invalid:5432/postgres",
            "db.internal.invalid:5432",
        ),
        // Statements on loopback, clients sent elsewhere — still not a dev cluster.
        (
            "postgres://postgres:pw@localhost:5432/postgres",
            "10.0.0.5:5432",
        ),
        (
            "postgres://postgres:pw@10.0.0.5:5432/postgres",
            "localhost:5432",
        ),
    ] {
        let remote = LocalProvider::new(admin, host);
        let refusals = [
            remote.create_branch_impl(&req).await.map(|_| ()),
            remote.reset_branch_impl(&req, &branch).await.map(|_| ()),
            remote.delete_branch_impl(&req, &branch).await,
        ];
        for out in refusals {
            let err = out.expect_err("a non-loopback cluster must refuse");
            assert!(
                err.to_string().contains("loopback cluster"),
                "{admin}: {err}"
            );
        }
    }
    for (admin, host) in [
        (
            "postgres://postgres:pw@localhost:15432/postgres",
            "localhost:15432",
        ),
        (
            "postgres://postgres:p@ss@127.0.0.1:32768/postgres",
            "127.0.0.1:32768",
        ),
        ("postgres://postgres:pw@[::1]:5432/postgres", "[::1]:5432"),
    ] {
        assert!(
            LocalProvider::new(admin, host).only_on_loopback().is_ok(),
            "{admin}"
        );
    }
}

/// A unix-socket admin DSN names no TCP host at all, so `host_from_dsn`
/// answers `""`: correctly refused (not loopback), but the message must
/// say why the host is empty rather than reading like a bug.
#[test]
fn a_dsn_naming_no_tcp_host_gets_a_clarifying_refusal() {
    let admin = "postgres:///oxy?host=/var/run/postgresql";
    assert_eq!(super::super::host_from_dsn(admin), "");
    let err = LocalProvider::new(admin, "localhost:15432")
        .only_on_loopback()
        .expect_err("an empty host is not loopback");
    let msg = err.to_string();
    assert!(
        msg.contains("on \"\" (the DSN names no TCP host):"),
        "{msg}"
    );
    assert!(msg.contains("loopback cluster"), "{msg}");

    // A named, merely non-loopback host gets no such suffix.
    let named = LocalProvider::new(
        "postgres://postgres:pw@db.internal.invalid:5432/postgres",
        "localhost:15432",
    )
    .only_on_loopback()
    .expect_err("a non-loopback host is refused");
    assert!(!named.to_string().contains("names no TCP host"), "{named}");
}

/// Refused before any connection: the admin DSN below points at a closed
/// port, so reaching the cluster would fail with a transport error instead.
#[tokio::test]
async fn a_schema_only_branch_cannot_be_reset() {
    let mut req = staging(TENANT);
    req.init = BranchInit::SchemaOnly;
    let branch = branch_database_name(TENANT, "oxy-staging");
    let provider = LocalProvider::new("postgres://postgres:pw@127.0.0.1:1/postgres", "127.0.0.1:1");
    let err = provider
        .reset_branch_impl(&req, &branch)
        .await
        .map(|_| ())
        .expect_err("a schema-only branch has nothing to reset from");
    assert!(
        matches!(&err, ProviderError::Api { status: 400, message }
            if message == SCHEMA_ONLY_RESET_REFUSAL),
        "{err}"
    );
}

#[test]
fn only_object_in_use_counts_as_a_busy_template() {
    let busy = ProviderError::Api {
        status: 500,
        message: "[55006] source database \"x\" is being accessed by other users".into(),
    };
    let other = ProviderError::Api {
        status: 500,
        message: "[42P04] database \"y\" already exists".into(),
    };
    assert!(is_template_busy(&busy));
    assert!(!is_template_busy(&other));
}

#[test]
fn the_state_replay_names_both_databases_and_never_grants_public_blindly() {
    let sql = copy_database_state_sql("oxy_org_a", "oxy_org_a_1234abcd");
    assert!(sql.contains("WHERE datname = 'oxy_org_a'"));
    assert!(sql.contains("REVOKE ALL ON DATABASE %I FROM PUBLIC', 'oxy_org_a_1234abcd'"));
    // PUBLIC is re-granted only what the SOURCE granted it (grantee 0).
    assert!(sql.contains("IF a.grantee = 0 THEN"));
    assert!(sql.contains("SET %s FROM CURRENT"));
}

#[test]
fn emptying_keeps_the_platform_ledger_schema() {
    let sql = truncate_all_tables_sql();
    let kept = format!("'information_schema', '{}')", crate::platform::META_SCHEMA);
    assert!(sql.contains(&kept), "{sql}");
    assert!(sql.contains("RESTART IDENTITY CASCADE"));
}
