//! The differential host-op test (environments design §8) for the P5b homes,
//! through the real host on a real engine: the same write, made from staging
//! and from production, lands on **disjoint** resources.
//!
//! - `ctx.warehouse.{insert,exec,upsert}` and `ctx.tx` naming a database the
//!   build's `nonProduction.destinations` maps land in the mapped database;
//!   production's gets only production's writes. An unmapped database is held.
//! - A statement on the mapped connection that names production's database,
//!   or writes through another database, is held unsent
//!   (`env_policy::destination_sql`).
//!
//! The Airhouse half — the app schema's sibling — is `staging_airhouse_sibling`.
//!
//! No V8: each test drives `ProjectFunctionHost` directly
//! (`staging_homes_fixture`). Per-test Postgres databases stand in for the
//! warehouses; the staging one signs in as a role of its own, as the host
//! requires of a mapped destination.

use serde_json::json;

use crate::common::{Schema, fresh_db};
use crate::staging_homes_fixture::{
    Workspace, exec, host, host_on, postgres_entry_as_own_role, production, staging, workspace,
};
use crate::warehouse_writes_on_engines::postgres_entry;
use oxy_app::server::api::custom_apps_functions::runtime::FunctionHost;

/// Insert, exec and upsert, each keyed from `base`.
async fn warehouse_writes(host: &dyn FunctionHost, database: &str, base: i64) {
    let writes = [
        (
            "insert",
            json!({ "database": database, "table": "t", "rows": [{ "a": base, "b": "insert" }] }),
        ),
        (
            "exec",
            json!({ "database": database, "sql": format!("INSERT INTO t VALUES ({}, 'exec')", base + 1) }),
        ),
        (
            "upsert",
            json!({ "database": database, "table": "t", "rows": [{ "a": base + 2, "b": "upsert" }],
                    "conflictColumns": ["a"] }),
        ),
    ];
    for (op, payload) in writes {
        host.warehouse_write(op.to_string(), payload)
            .await
            .unwrap_or_else(|e| panic!("warehouse.{op} to {database}: {e}"));
    }
}

/// `pg` (production), `pg_staging` (its mapping, on a role of its own) and
/// `pg_other` (unmapped), each with an empty `t`; and the database names the
/// `pg` and `pg_other` configs name.
async fn pg_workspace() -> (Workspace, String, String) {
    let production = postgres_entry("pg").await;
    let other = postgres_entry("pg_other").await;
    let database_of = |entry: &str| {
        entry
            .lines()
            .find_map(|l| l.trim().strip_prefix("database: "))
            .expect("the entry names its database")
            .to_string()
    };
    let (production_database, other_database) = (database_of(&production), database_of(&other));
    let ws = workspace(&format!(
        "{production}{}{other}",
        postgres_entry_as_own_role("pg_staging").await,
    ))
    .await;
    for database in ALL {
        exec(
            &*ws.connector(database).await,
            "CREATE TABLE t (a INTEGER PRIMARY KEY, b TEXT)",
        )
        .await;
    }
    (ws, production_database, other_database)
}

const ALL: &[&str] = &["pg", "pg_staging", "pg_other"];
const MAPPING: &[(&str, &str)] = &[("pg", "pg_staging")];

#[tokio::test]
async fn a_staging_warehouse_write_lands_in_the_mapped_destination_and_never_in_production() {
    let (ws, _, _) = pg_workspace().await;
    let stg = host(ws.ctx(), ALL, ALL, staging(MAPPING));
    let prod = host(ws.ctx(), ALL, ALL, production(MAPPING));

    warehouse_writes(&*stg, "pg", 1).await;
    warehouse_writes(&*prod, "pg", 100).await;

    assert_eq!(
        ws.ids("pg_staging").await,
        vec![1, 2, 3],
        "staging's writes land in the mapped database"
    );
    assert_eq!(
        ws.ids("pg").await,
        vec![100, 101, 102],
        "production's database holds production's writes alone"
    );
    assert!(ws.ids("pg_other").await.is_empty());
}

/// An unmapped database is held — never written, in staging's copy or in
/// production's — and the error says how to map it.
#[tokio::test]
async fn an_unmapped_staging_warehouse_write_is_held_and_reaches_no_database() {
    let (ws, _, _) = pg_workspace().await;
    let stg = host(ws.ctx(), ALL, ALL, staging(MAPPING));
    for (op, payload) in [
        (
            "insert",
            json!({ "database": "pg_other", "table": "t", "rows": [{ "a": 7, "b": "x" }] }),
        ),
        (
            "exec",
            json!({ "database": "pg_other", "sql": "INSERT INTO t VALUES (8, 'x')" }),
        ),
    ] {
        let err = stg
            .warehouse_write(op.to_string(), payload)
            .await
            .expect_err("an unmapped staging write is held");
        assert!(err.starts_with("HeldInStaging:"), "{err}");
        assert!(err.contains("\"nonProduction\""), "names the fix: {err}");
    }
    for database in ALL {
        assert!(ws.ids(database).await.is_empty(), "{database} was written");
    }
}

/// The mapped name must pass production's gate on its own: declared in the
/// function's `destinations`, and — a customer warehouse — named with a
/// reason. Refused otherwise, before anything connects.
#[tokio::test]
async fn the_mapped_database_must_pass_the_same_destination_gate() {
    let (ws, _, _) = pg_workspace().await;
    let not_declared = host(ws.ctx(), &["pg"], &["pg"], staging(MAPPING));
    let no_reason = host(ws.ctx(), ALL, &["pg"], staging(MAPPING));
    for (stg, expected) in [
        (not_declared, "`destinations` allowlist"),
        (no_reason, "customerWarehouseWrites"),
    ] {
        let err = stg
            .warehouse_write(
                "insert".into(),
                json!({ "database": "pg", "table": "t", "rows": [{ "a": 1, "b": "x" }] }),
            )
            .await
            .expect_err("the mapped database is refused");
        assert!(err.contains("nonProduction.destinations"), "{err}");
        assert!(err.contains("pg_staging"), "{err}");
        assert!(err.contains(expected), "{err}");
    }
    for database in ALL {
        assert!(ws.ids(database).await.is_empty(), "{database} was written");
    }
}

/// B1: the mapping moves the connection, not what a statement names. A
/// write into production's database, or through any database but the mapped
/// one's, or any statement off the allowlist, is held unsent on `exec` and on
/// a `ctx.tx` handle; an unqualified write proceeds on the mapped connection.
#[tokio::test]
async fn a_statement_naming_past_the_mapping_is_held_on_the_mapped_connection() {
    let (ws, production_database, _) = pg_workspace().await;
    let stg = host(ws.ctx(), ALL, ALL, staging(MAPPING));
    for sql in [
        format!("INSERT INTO {production_database}.public.t VALUES (1, 'x')"),
        "INSERT INTO some_other_db.public.t VALUES (2, 'x')".to_string(),
        "ALTER TABLE t RENAME TO t_old".to_string(),
        "SELECT a INTO t_copy FROM t".to_string(),
        "SET search_path = other".to_string(),
    ] {
        let err = stg
            .warehouse_write("exec".into(), json!({ "database": "pg", "sql": sql }))
            .await
            .expect_err("held");
        assert!(err.starts_with("HeldInStaging:"), "{sql}: {err}");
        assert!(err.contains("mapped destination"), "{err}");
    }
    stg.warehouse_write(
        "exec".into(),
        json!({ "database": "pg", "sql": "INSERT INTO public.t VALUES (3, 'x')" }),
    )
    .await
    .expect("an unqualified statement runs on the mapped connection");

    let opened = stg
        .tx("begin".into(), json!({ "database": "pg" }))
        .await
        .expect("begin");
    let id = opened["id"].clone();
    let qualified = format!("INSERT INTO {production_database}.public.t VALUES (4)");
    let err = stg
        .tx("exec".into(), json!({ "id": id, "sql": qualified }))
        .await
        .expect_err("held on the handle too");
    assert!(err.starts_with("HeldInStaging:"), "{err}");
    stg.tx(
        "exec".into(),
        json!({ "id": id, "sql": "INSERT INTO t (a) VALUES (5)" }),
    )
    .await
    .expect("the handle goes on");
    stg.tx("rollback".into(), json!({ "id": id }))
        .await
        .expect("rollback");

    assert_eq!(ws.ids("pg_staging").await, vec![3]);
    assert!(ws.ids("pg").await.is_empty(), "production was written");
}

/// `ctx.tx` is Postgres-only. A staging transaction opens on the mapped
/// database, its statements and its commit run there, and production's
/// database never sees them.
#[tokio::test]
async fn a_staging_transaction_opens_commits_and_writes_on_the_mapped_database() {
    let (ws, _, _) = pg_workspace().await;
    let both = ALL;
    let mapping = MAPPING;
    let (control, _) = fresh_db(Schema::Central).await;
    let stg = host_on(control.clone(), ws.ctx(), both, both, staging(mapping));
    let prod = host_on(control, ws.ctx(), both, both, production(mapping));

    for (h, a) in [(&stg, 1), (&prod, 100)] {
        let opened = h
            .tx("begin".into(), json!({ "database": "pg" }))
            .await
            .expect("begin");
        let id = opened["id"].clone();
        h.tx(
            "exec".into(),
            json!({ "id": id, "sql": format!("INSERT INTO t (a) VALUES ({a})") }),
        )
        .await
        .expect("exec");
        h.tx("commit".into(), json!({ "id": id }))
            .await
            .expect("commit");
    }

    assert_eq!(
        ws.ids("pg_staging").await,
        vec![1],
        "staging's commit landed"
    );
    assert_eq!(ws.ids("pg").await, vec![100], "production's alone");
}

/// Fix round 2, ruling 4: the fence knows every production database the
/// mapping names, not only the one this write named — a write into the other
/// key's database is held as production's.
#[tokio::test]
async fn a_write_into_another_mapped_production_database_is_held_as_production() {
    let (ws, _, other_database) = pg_workspace().await;
    let mapping = &[("pg", "pg_staging"), ("pg_other", "pg_staging")];
    let stg = host(ws.ctx(), ALL, ALL, staging(mapping));
    let sql = format!("INSERT INTO {other_database}.public.t VALUES (1, 'x')");
    let err = stg
        .warehouse_write("exec".into(), json!({ "database": "pg", "sql": sql }))
        .await
        .expect_err("held");
    assert!(err.starts_with("HeldInStaging:"), "{err}");
    assert!(
        err.contains(&format!("production's database `{other_database}`")),
        "held as production's, not merely as another database: {err}"
    );
    for database in ALL {
        assert!(ws.ids(database).await.is_empty(), "{database} was written");
    }
}
