use super::*;

/// `PreviewNamespace::for_branch(Uuid::from_u128(1), "feat/je-v2")`.
const KEY: &str = "feat_je_v2_92a1b7";

fn ns() -> PreviewNamespace {
    PreviewNamespace::from_key(KEY).expect("a valid key")
}

/// The quoted preview schema for `live`.
fn p(live: &str) -> String {
    format!("\"preview_{KEY}__{live}\"")
}

fn opts() -> RewriteOptions {
    RewriteOptions {
        catalog: Some("lake".into()),
        read_live_only: false,
    }
}

fn shadowed(entries: &[(&str, &str, ShadowState)]) -> ShadowMap {
    let mut map = ShadowMap::default();
    for (schema, table, state) in entries {
        map.0
            .insert((schema.to_string(), table.to_string()), *state);
    }
    map
}

fn rw_with(sql: &str, shadow: &ShadowMap, opts: &RewriteOptions) -> Rewrite {
    rewrite(sql, &ns(), shadow, opts).unwrap_or_else(|e| panic!("expected {sql:?} to rewrite: {e}"))
}

fn rw(sql: &str) -> Rewrite {
    rw_with(sql, &ShadowMap::default(), &opts())
}

fn refused(sql: &str) -> String {
    match rewrite(sql, &ns(), &ShadowMap::default(), &opts()) {
        Ok(out) => panic!("expected {sql:?} to be refused, got {:?}", out.sql),
        Err(Refused(msg)) => msg,
    }
}

/// The workspace catalog `opts()` names, for `verify`.
const CATALOG: Option<&str> = Some("lake");

fn verified(sql: &str) {
    if let Err(e) = verify(sql, &ns(), CATALOG) {
        panic!("expected the verifier to pass {sql:?}: {e}");
    }
}

fn pair(schema: &str, table: &str) -> (String, String) {
    (schema.to_string(), table.to_string())
}

#[test]
fn every_write_verb_targets_the_preview_schema() {
    for sql in [
        "CREATE TABLE toast_pos.daily (d DATE, net DOUBLE)",
        "CREATE TABLE IF NOT EXISTS toast_pos.daily (d DATE, net DOUBLE)",
        "CREATE OR REPLACE TABLE toast_pos.daily AS SELECT d, sum(net) FROM toast_pos.orders GROUP BY d",
        "INSERT INTO toast_pos.daily SELECT d, sum(net) FROM toast_pos.orders GROUP BY d",
        "INSERT OR REPLACE INTO lake.toast_pos.daily VALUES (DATE '2026-01-01', 1)",
        "UPDATE toast_pos.daily SET net = 0 WHERE d < DATE '2026-01-01'",
        "DELETE FROM toast_pos.daily WHERE d >= DATE '2026-01-01'",
        "MERGE INTO toast_pos.daily t USING toast_pos.orders o ON t.d = o.d \
         WHEN MATCHED THEN UPDATE SET net = o.net WHEN NOT MATCHED THEN INSERT VALUES (o.d, o.net)",
        "ALTER TABLE toast_pos.daily ADD COLUMN note VARCHAR",
        "ALTER TABLE toast_pos.daily RENAME TO daily_v2",
        "TRUNCATE toast_pos.daily",
        "DROP TABLE IF EXISTS toast_pos.daily",
        "DROP TABLE toast_pos.daily",
        "CREATE VIEW toast_pos.daily AS SELECT * FROM toast_pos.orders",
    ] {
        let out = rw(sql);
        assert_eq!(
            out.writes.first(),
            Some(&pair("toast_pos", "daily")),
            "{sql}"
        );
        assert!(out.sql.contains(&p("toast_pos")), "{sql} -> {}", out.sql);
        verified(&out.sql);
        assert!(
            out.preludes.contains(&Prelude::EnsureSchema {
                live: "toast_pos".into(),
                preview: format!("preview_{KEY}__toast_pos"),
            }),
            "{sql}: {:?}",
            out.preludes
        );
        for prelude in &out.preludes {
            let statements = [
                prelude.statement(CATALOG),
                prelude.statement(None),
                prelude
                    .copy_plan(CATALOG, true, 10, 100)
                    .map(|c| c.statement),
                prelude.copy_plan(None, true, 10, 5).map(|c| c.statement),
            ];
            statements.iter().flatten().for_each(|sql| verified(sql));
        }
    }
}

#[test]
fn a_partial_write_copies_the_live_table_first_and_only_once() {
    let out = rw("DELETE FROM toast_pos.daily WHERE d >= DATE '2026-01-01';
         INSERT INTO toast_pos.daily SELECT * FROM toast_pos.daily_staging;
         SELECT count(*) FROM toast_pos.daily");
    let copy = Prelude::CopyOnWrite {
        live: pair("toast_pos", "daily"),
        preview: pair(&format!("preview_{KEY}__toast_pos"), "daily"),
    };
    assert_eq!(out.preludes.iter().filter(|p| **p == copy).count(), 1);
    // The copy's state is the host's to settle, from the copy it made.
    assert_eq!(out.shadow_updates, vec![]);
    let mut settled = ShadowMap::default();
    let plan = copy.copy_plan(CATALOG, true, 9, 5).unwrap();
    settled.apply_rewrite(&out, &[plan]);
    assert_eq!(
        settled.state(&pair("toast_pos", "daily")),
        Some(ShadowState::Partial)
    );
    // The second statement sees the first one's write, and so does the read.
    assert!(out.sql.ends_with(&format!(
        "SELECT count(*) FROM {}.\"daily\"",
        p("toast_pos")
    )));
    assert_eq!(out.redirected_reads, vec![pair("toast_pos", "daily")]);
    // A full replacement copies nothing.
    let out = rw("CREATE OR REPLACE TABLE toast_pos.daily AS SELECT 1 AS d");
    assert!(
        !out.preludes
            .iter()
            .any(|p| matches!(p, Prelude::CopyOnWrite { .. }))
    );
}

#[test]
fn unqualified_write_is_refused() {
    for sql in [
        "INSERT INTO orders VALUES (1)",
        "CREATE TABLE daily AS SELECT 1",
        "UPDATE orders SET x = 1",
        "DELETE FROM orders",
        "DROP TABLE orders",
        "TRUNCATE orders",
    ] {
        let msg = refused(sql);
        assert!(msg.contains("unqualified"), "{sql}: {msg}");
        assert!(
            verify(sql, &ns(), CATALOG).is_err(),
            "the verifier passed {sql}"
        );
    }
}

#[test]
fn three_part_name_with_foreign_catalog_is_refused() {
    let out = rw("INSERT INTO lake.toast_pos.daily SELECT * FROM lake.toast_pos.orders");
    assert!(
        out.sql
            .starts_with(&format!("INSERT INTO lake.{}.\"daily\"", p("toast_pos"))),
        "{}",
        out.sql
    );
    for sql in [
        "INSERT INTO other.toast_pos.daily VALUES (1)",
        "CREATE TABLE memory.toast_pos.daily AS SELECT 1",
        "SELECT * FROM other.toast_pos.orders",
        "INSERT INTO lake.toast_pos.daily SELECT * FROM pg.public.orders",
        "INSERT INTO a.b.c.d VALUES (1)",
    ] {
        let msg = refused(sql);
        assert!(
            msg.contains("catalog") || msg.contains("name"),
            "{sql}: {msg}"
        );
    }
    let no_catalog = RewriteOptions::default();
    assert!(
        rewrite(
            "INSERT INTO lake.toast_pos.daily VALUES (1)",
            &ns(),
            &ShadowMap::default(),
            &no_catalog
        )
        .is_err()
    );
}

#[test]
fn cte_names_are_not_rewritten() {
    let shadow = shadowed(&[("main", "recent", ShadowState::Shadow)]);
    let sql = "INSERT INTO toast_pos.daily \
               WITH recent AS (SELECT * FROM toast_pos.orders WHERE d > DATE '2026-01-01') \
               SELECT * FROM recent WHERE d IN (SELECT d FROM recent)";
    let out = rw_with(sql, &shadow, &opts());
    assert!(out.sql.contains("FROM recent WHERE"), "{}", out.sql);
    assert!(out.sql.contains("SELECT d FROM recent)"), "{}", out.sql);
    assert!(
        out.redirected_reads.is_empty(),
        "{:?}",
        out.redirected_reads
    );
    // Out of the CTE's scope the same name is a table, and it is shadowed.
    let out = rw_with("SELECT * FROM recent", &shadow, &opts());
    assert_eq!(out.sql, format!("SELECT * FROM {}.\"recent\"", p("main")));
}

#[test]
fn drop_schema_is_refused() {
    for sql in [
        "DROP SCHEMA toast_pos",
        "DROP SCHEMA lake.toast_pos",
        "DROP SCHEMA IF EXISTS toast_pos",
    ] {
        let msg = refused(sql);
        assert!(msg.contains("DROP SCHEMA"), "{sql}: {msg}");
    }
    // CREATE SCHEMA is not sent: the host ensures the preview's schema instead.
    let out = rw("CREATE SCHEMA IF NOT EXISTS site_selection");
    assert_eq!(out.sql, "");
    assert_eq!(
        out.preludes,
        vec![Prelude::EnsureSchema {
            live: "site_selection".into(),
            preview: format!("preview_{KEY}__site_selection"),
        }]
    );
}

#[test]
fn io_table_functions_are_refused() {
    for sql in [
        "SELECT * FROM postgres_execute('__ducklake_metadata_lake', 'DELETE FROM x')",
        "SELECT * FROM postgres_scan('host=x', 'public', 'orders')",
        "SELECT * FROM ducklake_add_data_files('lake', 'orders', 's3://bucket/x.parquet')",
        "SELECT * FROM query('DELETE FROM toast_pos.orders')",
        "SELECT * FROM query_table('toast_pos.orders')",
        "INSERT INTO toast_pos.daily SELECT * FROM postgres_query('pg', 'SELECT 1')",
        "SELECT getenv('HOME')",
        "SELECT current_setting('s3_secret_access_key')",
    ] {
        refused(sql);
        assert!(
            verify(sql, &ns(), CATALOG).is_err(),
            "the verifier passed {sql}"
        );
    }
    for sql in [
        "SELECT * FROM range(10)",
        "SELECT * FROM read_parquet('s3://bucket/census/*.parquet')",
        "SELECT * FROM read_csv_auto('s3://bucket/seed.csv')",
    ] {
        rw(sql);
        verified(sql);
    }
}

#[test]
fn drop_and_truncate_behave_as_they_would_in_prod() {
    // No IF EXISTS: the live table must exist, as prod would require.
    let out = rw("DROP TABLE toast_pos.daily");
    assert_eq!(
        out.sql,
        format!(
            "SELECT * FROM toast_pos.daily LIMIT 0;\nDROP TABLE IF EXISTS {}.\"daily\"",
            p("toast_pos")
        )
    );
    assert_eq!(
        out.shadow_updates,
        vec![(pair("toast_pos", "daily"), ShadowState::Dropped)]
    );
    // Dropped earlier in the preview: the preview's own DROP fails as prod's would.
    let dropped = shadowed(&[("toast_pos", "daily", ShadowState::Dropped)]);
    let out = rw_with("DROP TABLE toast_pos.daily", &dropped, &opts());
    assert_eq!(out.sql, format!("DROP TABLE {}.\"daily\"", p("toast_pos")));
    let out = rw("TRUNCATE toast_pos.daily");
    assert_eq!(
        out.sql,
        format!(
            "CREATE OR REPLACE TABLE {}.\"daily\" AS SELECT * FROM toast_pos.daily LIMIT 0",
            p("toast_pos")
        )
    );
    // A renamed copy keeps the copy's state, which the host settles.
    let out = rw("ALTER TABLE toast_pos.daily_staging RENAME TO daily");
    assert_eq!(
        out.shadow_updates,
        vec![
            (pair("toast_pos", "daily"), ShadowState::Shadow),
            (pair("toast_pos", "daily_staging"), ShadowState::Dropped),
        ]
    );
    let mut settled = ShadowMap::default();
    let plan = out.preludes[1].copy_plan(CATALOG, true, 9, 5).unwrap();
    settled.apply_rewrite(&out, &[plan]);
    assert_eq!(
        settled.state(&pair("toast_pos", "daily")),
        Some(ShadowState::Partial)
    );
    assert_eq!(
        settled.state(&pair("toast_pos", "daily_staging")),
        Some(ShadowState::Dropped)
    );
    refused("ALTER TABLE toast_pos.daily RENAME TO bookkeeping.daily");
    refused("DROP TABLE toast_pos.daily CASCADE");
}

#[test]
fn verify_refuses_a_live_target_even_when_rewrite_is_skipped() {
    let preview_t = format!("{}.\"t\"", p("toast_pos"));
    for sql in [
        "INSERT INTO toast_pos.orders VALUES (1)".to_string(),
        "INSERT INTO orders VALUES (1)".into(),
        "INSERT INTO lake.toast_pos.orders VALUES (1)".into(),
        "UPDATE toast_pos.orders SET x = 1".into(),
        "DELETE FROM toast_pos.orders".into(),
        "MERGE INTO toast_pos.orders t USING toast_pos.x s ON t.id = s.id WHEN MATCHED THEN DELETE"
            .into(),
        "CREATE TABLE toast_pos.t AS SELECT 1".into(),
        "CREATE OR REPLACE VIEW toast_pos.v AS SELECT 1".into(),
        "ALTER TABLE toast_pos.orders ADD COLUMN x INT".into(),
        format!("ALTER TABLE {preview_t} RENAME TO toast_pos.t"),
        "DROP TABLE toast_pos.orders".into(),
        "TRUNCATE toast_pos.orders".into(),
        "CREATE SCHEMA toast_pos".into(),
        "DROP SCHEMA toast_pos".into(),
        "INSERT INTO preview_feat_je_v3_000000__toast_pos.orders VALUES (1)".into(),
        "SELECT 1; DELETE FROM toast_pos.orders".into(),
        // The preview's schema name in another catalog is not the preview's.
        format!("CREATE SCHEMA other.{}", p("toast_pos")),
        format!(
            "CREATE TABLE __ducklake_metadata_lake.{}.t AS SELECT 1",
            p("toast_pos")
        ),
        format!("DROP SCHEMA other.{} CASCADE", p("toast_pos")),
        format!("INSERT INTO other.{preview_t} VALUES (1)"),
        format!("DROP TABLE memory.{preview_t}"),
    ] {
        match verify(&sql, &ns(), CATALOG) {
            Ok(_) => panic!("the verifier passed a live write: {sql}"),
            Err(Refused(msg)) => assert!(!msg.is_empty(), "{sql}"),
        }
    }
    // What the rewrite produces passes, in the workspace's catalog too;
    // reads of anything pass.
    for sql in [
        format!("INSERT INTO {preview_t} SELECT * FROM toast_pos.orders"),
        format!("INSERT INTO LAKE.{preview_t} SELECT * FROM toast_pos.orders"),
        format!("CREATE SCHEMA IF NOT EXISTS lake.{}", p("toast_pos")),
        format!("DROP SCHEMA {} CASCADE", p("toast_pos")),
        "SELECT * FROM toast_pos.orders".into(),
        "BEGIN; SELECT 1; COMMIT".into(),
    ] {
        verified(&sql);
    }
    // With no workspace catalog, no catalog may be named.
    let named = format!("INSERT INTO lake.{preview_t} VALUES (1)");
    assert!(verify(&named, &ns(), None).is_err());
    let statements = verify("select *   from toast_pos.orders", &ns(), CATALOG).unwrap();
    assert_eq!(
        statements,
        vec!["SELECT * FROM toast_pos.orders".to_string()]
    );
}
