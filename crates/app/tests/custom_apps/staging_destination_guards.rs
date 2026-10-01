//! A staging bundle declares its own `nonProduction.destinations`, so the host
//! does not take the mapping on trust: on every staging write it isolates to a
//! mapped database it re-checks the whole mapping against the workspace's
//! config **as it is now** — the config can change after the publish that
//! checked it — and fails closed. Refused, before anything connects and with
//! nothing written anywhere:
//!
//! - a mapped database that resolves to production's same host and user;
//! - one that resolves to **another** production database the mapping names
//!   (B3), and a chain whose target is itself a mapped production database;
//! - one onto the workspace's own Airhouse (`airhouse_managed`);
//! - a DuckDB mapping from a DuckDB production database (B2: one process);
//! - one whose host could not be resolved (a missing secret).
//!
//! Each refusal is `EnvironmentRefused` — never a page — and listed in the
//! invocation's held row; a chain is held (no home), not refused.

use serde_json::json;

use crate::staging_homes_fixture::{
    Workspace, duck, exec, host, postgres_entry_as_own_role, staging, workspace,
};
use crate::warehouse_writes_on_engines::postgres_entry;

/// `pg` (production) and the mappings the host must refuse, plus
/// `pg_staging`, which it must accept — so a refusal is the mapping's doing,
/// not the harness's.
async fn guarded() -> Workspace {
    let ws = workspace(&format!(
        "{}{}{}{}{}  - name: lake\n    type: airhouse_managed\n  \
         - name: ch_prod\n    type: clickhouse\n    host: ch.example.com\n    user: app\n  \
         - name: ch_unset\n    type: clickhouse\n    host_var: OXY_TEST_P5B_UNSET_CH_HOST\n    \
         user: app\n",
        postgres_entry("pg").await,
        postgres_entry("pg_alias").await,
        postgres_entry_as_own_role("pg_staging").await,
        duck("duck"),
        duck("duck_staging"),
    ))
    .await;
    for database in WRITTEN {
        exec(
            &*ws.connector(database).await,
            "CREATE TABLE t (a INTEGER PRIMARY KEY, b TEXT)",
        )
        .await;
    }
    ws
}

/// The databases a refused write must not have reached.
const WRITTEN: &[&str] = &["pg", "pg_alias", "pg_staging", "duck", "duck_staging"];

const ALLOWED: &[&str] = &[
    "pg",
    "pg_alias",
    "pg_staging",
    "lake",
    "ch_prod",
    "ch_unset",
    "duck",
    "duck_staging",
];
const REASONS: &[&str] = &[
    "pg",
    "pg_alias",
    "pg_staging",
    "ch_prod",
    "ch_unset",
    "duck",
    "duck_staging",
];

/// Insert into `from` from staging, under `mapping`.
async fn insert(ws: &Workspace, mapping: &[(&str, &str)], from: &str) -> Result<(), String> {
    let stg = host(ws.ctx(), ALLOWED, REASONS, staging(mapping));
    stg.warehouse_write(
        "insert".into(),
        json!({ "database": from, "table": "t", "rows": [{ "a": 1, "b": "x" }] }),
    )
    .await
    .map(|_| ())
}

async fn refused(ws: &Workspace, mapping: &[(&str, &str)], from: &str, says: &str) {
    let err = insert(ws, mapping, from)
        .await
        .expect_err("the mapping is refused");
    assert!(err.starts_with("EnvironmentRefused:"), "{err}");
    assert!(err.contains(says), "{err}");
}

async fn nothing_written(ws: &Workspace) {
    for database in WRITTEN {
        assert!(ws.ids(database).await.is_empty(), "{database} was written");
    }
}

#[tokio::test]
async fn a_mapped_destination_on_productions_host_and_user_is_refused_at_invocation() {
    let ws = guarded().await;
    refused(&ws, &[("pg", "pg_alias")], "pg", "same host and user").await;
    nothing_written(&ws).await;

    insert(&ws, &[("pg", "pg_staging")], "pg")
        .await
        .expect("a separate database is accepted");
    assert_eq!(ws.ids("pg_staging").await, vec![1]);
    assert!(ws.ids("pg").await.is_empty());
}

/// B3: `pg_alias` is not production's `ch_prod`, but it is production's `pg`
/// under another name — the mapping names both, so both are compared.
#[tokio::test]
async fn a_mapped_destination_matching_any_mapped_production_database_is_refused() {
    let ws = guarded().await;
    refused(
        &ws,
        &[("ch_prod", "pg_alias"), ("pg", "pg_staging")],
        "ch_prod",
        "production database `pg`",
    )
    .await;
    nothing_written(&ws).await;
}

/// B3: a chain `{pg: pg_alias, pg_alias: pg_staging}` gives `pg` no home —
/// `pg_alias` is a production database — so the write is held.
#[tokio::test]
async fn a_chained_mapping_gives_no_home_and_holds() {
    let ws = guarded().await;
    let err = insert(&ws, &[("pg", "pg_alias"), ("pg_alias", "pg_staging")], "pg")
        .await
        .expect_err("held");
    assert!(err.starts_with("HeldInStaging:"), "{err}");
    nothing_written(&ws).await;
}

#[tokio::test]
async fn a_mapped_destination_on_the_workspaces_own_airhouse_is_refused() {
    let ws = guarded().await;
    refused(&ws, &[("pg", "lake")], "pg", "airhouse_managed").await;
    nothing_written(&ws).await;
}

/// B2: two DuckDB entries share the process, and a statement can ATTACH
/// production's file — refused whatever the files are.
#[tokio::test]
async fn a_duckdb_to_duckdb_mapping_is_refused() {
    let ws = guarded().await;
    refused(&ws, &[("duck", "duck_staging")], "duck", "both are DuckDB").await;
    nothing_written(&ws).await;
}

#[tokio::test]
async fn a_mapped_destination_whose_host_does_not_resolve_fails_closed() {
    let ws = guarded().await;
    refused(&ws, &[("pg", "ch_unset")], "pg", "did not resolve").await;
    nothing_written(&ws).await;
}
