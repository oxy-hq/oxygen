//! The preview Airhouse connector, against a fake Airhouse behind its port.

mod fake;
mod sessions;

use std::sync::{Arc, RwLock};

use agentic_connector::{ConnectorError, DatabaseConnector};
use airhouse::preview_sql::{PreviewNamespace, RewriteOptions, ShadowMap, ShadowState};
use futures::StreamExt;
use uuid::Uuid;

use super::{AirhouseBackend, Ddl, PreviewAirhouseConnector, Scope, Use, system_ddl};
use crate::server::previews::ddl::{PreviewDdlError, Relation, RelationKind};
use fake::FakeAirhouse;

const KEY: &str = "feat_je_v2_92a1b7";

fn ns() -> PreviewNamespace {
    PreviewNamespace::from_key(KEY).unwrap()
}

/// The preview schema standing in for `live`, and its quoted form.
fn p(live: &str) -> String {
    format!("preview_{KEY}__{live}")
}

fn q(live: &str) -> String {
    format!("\"{}\"", p(live))
}

fn opts() -> RewriteOptions {
    RewriteOptions {
        catalog: Some("lake".into()),
        read_live_only: false,
    }
}

fn over(
    fake: &Arc<FakeAirhouse>,
    shadow: ShadowMap,
    opts: RewriteOptions,
) -> PreviewAirhouseConnector {
    PreviewAirhouseConnector::new(
        ns(),
        Arc::new(RwLock::new(shadow)),
        opts,
        Arc::clone(fake) as Arc<dyn super::PreviewAirhouseBackend>,
    )
}

/// Over a shadow map recording the relations these tests write, as a step's
/// review would have before sending them.
fn connector(fake: &Arc<FakeAirhouse>) -> PreviewAirhouseConnector {
    over(fake, shadowing(RECORDED), opts())
}

const RECORDED: &[(&str, &str)] = &[
    ("toast_pos", "orders"),
    ("toast_pos", "t"),
    ("site_selection", "boom"),
    ("site_selection", "cells"),
];

fn shadowing(pairs: &[(&str, &str)]) -> ShadowMap {
    let mut map = ShadowMap::default();
    for (schema, table) in pairs {
        map.0
            .insert((schema.to_string(), table.to_string()), ShadowState::Shadow);
    }
    map
}

/// Every method that takes SQL, once each: `Ok` is "reached Airhouse".
async fn call_every_sql_method(
    conn: &PreviewAirhouseConnector,
    sql: &str,
) -> Vec<Result<(), String>> {
    let text = |e: ConnectorError| e.to_string();
    vec![
        conn.execute_query(sql, 10).await.map(|_| ()).map_err(text),
        conn.execute_query_full(sql).await.map(|_| ()).map_err(text),
        conn.execute_query_full_untyped(sql)
            .await
            .map(|_| ())
            .map_err(text),
        conn.execute_statement(sql).await.map_err(text),
        conn.execute_statement_tagged(sql, "oxy_run='r'")
            .await
            .map_err(text),
    ]
}

#[tokio::test]
async fn a_write_outside_the_namespace_never_reaches_the_inner_connector() {
    let other = PreviewNamespace::for_branch(Uuid::new_v4(), "someone-else");
    let own = format!("{}.\"orders\"", q("toast_pos"));
    let hostile = [
        "INSERT INTO toast_pos.orders VALUES (1)".to_string(),
        "UPDATE toast_pos.orders SET id = 2".into(),
        "DELETE FROM site_selection.site_hex_cells".into(),
        "CREATE TABLE bookkeeping.qb_je_account_map AS SELECT 1 AS id".into(),
        "CREATE OR REPLACE VIEW toast_pos.v AS SELECT 1".into(),
        "DROP TABLE toast_pos.orders".into(),
        "TRUNCATE toast_pos.orders".into(),
        "ALTER TABLE toast_pos.orders ADD COLUMN x INT".into(),
        format!("ALTER TABLE {own} RENAME TO toast_pos.orders"),
        "INSERT INTO orders VALUES (1)".into(),
        "CREATE SCHEMA toast_pos".into(),
        "DROP SCHEMA toast_pos".into(),
        format!("INSERT INTO other.{own} VALUES (1)"),
        format!(
            "INSERT INTO \"{}toast_pos\".\"t\" VALUES (1)",
            other.prefix()
        ),
        "SELECT * FROM postgres_execute('pg', 'DELETE FROM t')".into(),
        "SELECT 1 INTO toast_pos.t".into(),
        "ATTACH 'x.db' AS x".into(),
        "COPY toast_pos.orders TO 's3://bucket/x.parquet'".into(),
        // One bad statement refuses the batch: the good one is not sent either.
        format!(
            "BEGIN; INSERT INTO {own} VALUES (1); INSERT INTO toast_pos.orders VALUES (1); COMMIT"
        ),
        format!("INSERT INTO {own} VALUES (1); DELETE FROM toast_pos.orders"),
        // Even its own schemas: only the registry creates one, only the TTL
        // sweep drops one.
        format!("CREATE SCHEMA IF NOT EXISTS {}", q("scratch")),
        format!("DROP SCHEMA {}", q("toast_pos")),
        // EXPLAIN ANALYZE runs its body: it would carry either past the
        // schema-DDL refusal, or send a write as a read.
        format!("EXPLAIN ANALYZE DROP SCHEMA {} CASCADE", q("toast_pos")),
        format!("EXPLAIN ANALYZE INSERT INTO {own} VALUES (1)"),
        "SELEKT frm".into(),
        String::new(),
    ];
    for sql in &hostile {
        let fake = Arc::new(FakeAirhouse::default());
        let conn = connector(&fake);
        for (i, outcome) in call_every_sql_method(&conn, sql)
            .await
            .into_iter()
            .enumerate()
        {
            let err = outcome.expect_err(&format!("method #{i} sent `{sql}`"));
            assert!(err.contains("Nothing was sent"), "{sql}: {err}");
        }
        assert!(fake.checkouts().is_empty(), "`{sql}` opened a connection");
        assert!(
            fake.sent().is_empty(),
            "`{sql}` reached Airhouse: {:?}",
            fake.sent()
        );
    }

    // The control: the preview's own table is written, on a Writer scoped to
    // exactly its schema — the refusals above are about the SQL.
    let fake = Arc::new(FakeAirhouse::default());
    connector(&fake)
        .execute_statement(&format!("INSERT INTO {own} VALUES (1)"))
        .await
        .expect("a write to the preview's own table");
    assert_eq!(fake.checkouts(), vec![Scope::Writer(vec![p("toast_pos")])]);
    assert_eq!(
        fake.sent(),
        vec![format!("#1 statement INSERT INTO {own} VALUES (1)")]
    );
}

/// A write the run has not recorded — an agent's own `CREATE TABLE` into a
/// preview schema, say — is refused before anything is sent: the TTL drop
/// drops only recorded relations, so an unrecorded one would keep its schema,
/// and whatever it copied, for good. Recording it (what a step's review does
/// before the step is sent) lets the same statement through.
#[tokio::test]
async fn a_write_the_run_has_not_recorded_never_reaches_airhouse() {
    let scratch = format!("{}.\"scratch\"", q("toast_pos"));
    let renamed = format!("{}.\"orders\"", q("toast_pos"));
    let unrecorded = [
        format!("CREATE TABLE {scratch} AS SELECT * FROM toast_pos.orders"),
        format!("INSERT INTO {scratch} SELECT * FROM toast_pos.orders"),
        format!("CREATE OR REPLACE VIEW {} AS SELECT 1", q("gl") + ".\"v\""),
        format!("DROP TABLE {scratch}"),
        format!("ALTER TABLE {renamed} RENAME TO scratch"),
        // One unrecorded write refuses the batch, the recorded one included.
        format!("INSERT INTO {renamed} VALUES (1); INSERT INTO {scratch} VALUES (1)"),
    ];
    for sql in &unrecorded {
        let fake = Arc::new(FakeAirhouse::default());
        for (i, outcome) in call_every_sql_method(&connector(&fake), sql)
            .await
            .into_iter()
            .enumerate()
        {
            let err = outcome.expect_err(&format!("method #{i} sent `{sql}`"));
            assert!(err.contains("has not recorded"), "{sql}: {err}");
        }
        assert!(fake.checkouts().is_empty(), "`{sql}` opened a connection");
    }

    // Control: once the run records the relation, the same write is sent.
    let fake = Arc::new(FakeAirhouse::default());
    let mut recorded = RECORDED.to_vec();
    recorded.push(("toast_pos", "scratch"));
    over(&fake, shadowing(&recorded), opts())
        .execute_statement(&unrecorded[0])
        .await
        .expect("a recorded relation is written");
    assert_eq!(fake.sent().len(), 1);
}

#[tokio::test]
async fn reads_get_the_overlay() {
    let fake = Arc::new(FakeAirhouse::default());
    let shadow = Arc::new(RwLock::new(shadowing(&[("toast_pos", "orders")])));
    let conn = PreviewAirhouseConnector::new(
        ns(),
        Arc::clone(&shadow),
        opts(),
        Arc::clone(&fake) as Arc<dyn super::PreviewAirhouseBackend>,
    );
    let sql = "SELECT * FROM toast_pos.orders JOIN toast_pos.items USING (id)";
    conn.execute_query(sql, 10).await.unwrap();
    let sent = fake.sent().pop().unwrap();
    assert!(
        sent.contains(&format!("{}.\"orders\"", q("toast_pos"))),
        "{sent}"
    );
    assert!(sent.contains("toast_pos.items"), "{sent}");
    assert_eq!(fake.checkouts(), vec![Scope::Reader]);

    // The host records a step's write in the shared map; the next read sees it.
    shadow
        .write()
        .unwrap()
        .0
        .insert(("toast_pos".into(), "items".into()), ShadowState::Partial);
    let mut rows = conn.execute_query_full(sql).await.unwrap().rows;
    assert!(rows.next().await.is_none());
    let sent = fake.sent().pop().unwrap();
    assert!(sent.starts_with("#2 full "), "{sent}");
    assert!(!sent.contains("toast_pos.items"), "{sent}");
    assert!(
        sent.contains(&format!("{}.\"items\"", q("toast_pos"))),
        "{sent}"
    );

    // `read_live_only` turns the overlay off.
    let live = Arc::new(FakeAirhouse::default());
    let live_only = RewriteOptions {
        read_live_only: true,
        ..opts()
    };
    over(&live, shadowing(&[("toast_pos", "orders")]), live_only)
        .execute_query(sql, 10)
        .await
        .unwrap();
    let sent = live.sent().pop().unwrap();
    assert!(!sent.contains("preview_"), "{sent}");
}

#[tokio::test]
async fn the_escape_hatches_are_closed() {
    let fake = Arc::new(FakeAirhouse::default());
    let conn = connector(&fake);
    assert!(conn.begin_transaction().await.is_err());
    assert!(conn.as_arrow().is_none());
    assert!(fake.checkouts().is_empty());
}

/// One pool identity per scope, and transactions never share one with
/// anything else: a shared connection handed out earlier could otherwise send
/// into the transaction.
#[test]
fn writers_are_pooled_per_scope_and_transactions_apart() {
    let ws = Uuid::nil();
    let backend = AirhouseBackend::new(ws, ns());
    let writer = Scope::Writer(vec![p("a"), p("b")]);
    assert_eq!(
        backend.pool_key(&Scope::Reader, Use::Shared),
        format!("preview:{ws}:{KEY}")
    );
    assert_eq!(
        backend.pool_key(&writer, Use::Shared),
        format!("preview:{ws}:{KEY}:{},{}", p("a"), p("b"))
    );
    assert_eq!(
        backend.pool_key(&writer, Use::Transaction),
        format!("preview:tx:{ws}:{KEY}:{},{}", p("a"), p("b"))
    );
    assert_eq!(
        backend.pool_key(&Scope::Reader, Use::Transaction),
        format!("preview:tx:{ws}:{KEY}")
    );
}

#[tokio::test]
async fn system_ddl_only_builds_fixed_statements_from_owned_names() {
    let ws = Uuid::nil();
    let other = PreviewNamespace::for_branch(ws, "someone-else");
    let table = |name: &str| Relation {
        name: name.into(),
        kind: RelationKind::Table,
    };
    for schema in [
        "toast_pos".to_string(),
        "main".into(),
        format!("{}toast_pos", other.prefix()),
        format!("PREVIEW_{KEY}__toast_pos"),
        format!("{}\"; DROP SCHEMA toast_pos; --", p("x")),
        p("a__b"),
    ] {
        for ddl in [
            Ddl::CreateSchema(schema.clone()),
            Ddl::DropSchema(schema.clone()),
            Ddl::DropRelation(schema.clone(), table("orders")),
        ] {
            let err = system_ddl(ws, &ns(), ddl.clone()).await.unwrap_err();
            assert!(matches!(err, PreviewDdlError::Refused(_)), "{ddl:?}: {err}");
        }
    }
    // An owned name gets past the statement builder to the connection, which
    // fails here only because there is no Airhouse; the statement is fixed.
    let owned = Ddl::DropRelation(p("toast_pos"), table("we\"ird"));
    assert_eq!(
        owned.statement(&ns()).unwrap(),
        format!("DROP TABLE IF EXISTS {}.\"we\"\"ird\"", q("toast_pos"))
    );
    let err = system_ddl(ws, &ns(), owned).await.unwrap_err();
    assert!(matches!(err, PreviewDdlError::Backend(_)), "{err}");
}

/// A source scan: every method of `DatabaseConnector` is stated here, and
/// every one that takes `sql: &str` plans (verifies) it first.
#[test]
fn every_trait_method_is_stated() {
    let trait_src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../agentic/connector/src/connector.rs"
    ))
    .expect("agentic-connector source is readable from the workspace");
    let trait_fns = fns_in(&trait_src, "pub trait DatabaseConnector");
    let impl_fns = fns_in(
        include_str!("../preview_airhouse.rs"),
        "impl DatabaseConnector for PreviewAirhouseConnector",
    );
    assert!(trait_fns.len() >= 9, "too few trait methods: {trait_fns:?}");
    for (name, signature) in &trait_fns {
        let Some((_, body)) = impl_fns.iter().find(|(n, _)| n == name) else {
            panic!("PreviewAirhouseConnector does not state `{name}`");
        };
        if signature.contains("sql: &str") {
            assert!(
                body.contains("self.plan(sql)?"),
                "`{name}` does not verify first"
            );
        }
    }
}

/// `(name, text up to the next fn)` for each `fn` in the block after `marker`,
/// comment lines dropped so a brace in a doc cannot unbalance the scan.
fn fns_in(src: &str, marker: &str) -> Vec<(String, String)> {
    let start = src
        .find(marker)
        .unwrap_or_else(|| panic!("`{marker}` not found"));
    let code: String = src[start..]
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let open = code.find('{').expect("an opening brace");
    let mut depth = 0usize;
    let end = code[open..]
        .char_indices()
        .find_map(|(i, c)| {
            depth = match c {
                '{' => depth + 1,
                '}' => depth - 1,
                _ => depth,
            };
            (depth == 0).then_some(open + i)
        })
        .expect("balanced braces");
    let block = &code[open + 1..end];
    let starts: Vec<usize> = block.match_indices("fn ").map(|(i, _)| i).collect();
    starts
        .iter()
        .enumerate()
        .map(|(k, &at)| {
            let text = &block[at..starts.get(k + 1).copied().unwrap_or(block.len())];
            let name = text[3..]
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            (name, text.to_string())
        })
        .collect()
}
