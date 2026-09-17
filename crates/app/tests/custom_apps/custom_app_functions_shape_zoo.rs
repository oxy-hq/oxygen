//! Every shape-zoo case, read through `ctx.warehouse.query` in a published function.
//!
//! Each engine's zoo table is loaded straight through the connector, never through the function
//! under test. A published function then reads it one column at a time, and each column's JSON is
//! compared with the case's `expect.warehouse`. One column per read, because one undecodable
//! column fails a whole statement, and a `{"$error": …}` case must not take its neighbours with it.
//!
//! The workspace config is compiled and promoted into Postgres, as in
//! `custom_app_functions_clickhouse`. Its row points at the directory the config came from,
//! because a DuckDB `path` resolves inside the working copy.
//!
//! **Needs** Postgres (per-test databases from `common`) and ClickHouse from
//! `warehouse_writes_on_engines::clickhouse`. Without ClickHouse that test skips, unless
//! `OXY_TEST_REQUIRE_CLICKHOUSE=1` makes it fail. The first DuckDB `JSON` column autoinstalls the
//! `json` extension: `duckdb` is built `bundled` only, with autoinstall on, so that needs network.

use std::path::Path;

use agentic_connector::DatabaseConnector;
use agentic_core::result::CellValue;
use entity::workspaces;
use oxy::adapters::workspace::builder::WorkspaceBuilder;
use oxy::config::OnMissing;
use oxy_app::agentic_wiring::OxyProjectContext;
use oxy_app::server::compile_config_gate::runtime_config_gate;
use oxy_compile::{CompileRequest, Promotion, RevisionKind, compile_workspace, compiler_version};
use sea_orm::{ActiveModelTrait, ActiveValue};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::custom_app_functions_fixture::{
    FunctionSpec, ORG_SLUG, Tenant, call_function_in, publish_app, seeded_tenant,
};
use crate::shape_zoo::{self, Engine, LoadedZoo, Plane};
use crate::warehouse_writes_on_engines::{clickhouse, clickhouse_entry, postgres_entry};

const APP: &str = "shape-zoo";
const READ_WAREHOUSE: &str = "read-warehouse";

/// Runs each `{ column, sql }` the test sends; answers `{ column: value }`, or
/// `{ column: { "$error": message } }` for a read that throws. The SQL comes from `shape_zoo`.
const READ_WAREHOUSE_JS: &str = r#"
export default async (req, ctx) => {
  const { database, reads } = JSON.parse(req.body);
  const out = {};
  for (const { column, sql } of reads) {
    try {
      const { rows } = await ctx.warehouse.query(database, sql);
      out[column] = rows.length === 1 ? rows[0][column] : { $error: `expected 1 row, got ${rows.length}` };
    } catch (err) {
      out[column] = { $error: String(err && err.message ? err.message : err) };
    }
  }
  return Response.json(out);
};
"#;

fn functions() -> Vec<FunctionSpec> {
    vec![FunctionSpec {
        name: READ_WAREHOUSE,
        // A read needs no `destinations`: `warehouse_query` is not behind the write allowlist.
        manifest: json!({ "route": true, "timeoutSeconds": 60 }),
        js: READ_WAREHOUSE_JS,
    }]
}

/// A working copy whose `config.yml` declares `databases` (already-indented list entries).
pub(crate) fn write_config(databases: &str) -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("workspace dir");
    std::fs::write(
        root.path().join("config.yml"),
        format!("databases:\n{databases}models: []\n"),
    )
    .expect("write config.yml");
    root
}

/// A workspace row in the tenant's org at `root`, compiled and promoted.
pub(crate) async fn compile(t: &Tenant, root: &Path) -> Uuid {
    let id = Uuid::new_v4();
    workspaces::ActiveModel {
        id: ActiveValue::Set(id),
        name: ActiveValue::Set("Shape zoo".into()),
        org_id: ActiveValue::Set(Some(t.org_id)),
        path: ActiveValue::Set(Some(root.to_string_lossy().into_owned())),
        ..Default::default()
    }
    .insert(&t.db)
    .await
    .expect("seed workspace");
    let outcome = compile_workspace(CompileRequest {
        db: &t.db,
        workspace_id: id,
        workspace_path: root,
        git_sha: None,
        branch: None,
        compiler_version: compiler_version(),
        promote: true,
        kind: RevisionKind::Main,
        owner_user_id: None,
        config_gate: Some(runtime_config_gate()),
    })
    .await
    .expect("compile the workspace");
    assert_eq!(
        outcome.promotion,
        Promotion::Promoted,
        "the runtime reads the promoted revision; failures: {:?}",
        outcome.failures
    );
    id
}

/// Creates the zoo table and fills its one row unless a row is already there. The reused
/// ClickHouse container keeps `oxy_shape_zoo_<hash>` between runs.
pub(crate) async fn fill_zoo(connector: &dyn DatabaseConnector, engine: Engine, zoo: &LoadedZoo) {
    let (table, cases) = (zoo.table(), zoo.zoo.cases(engine));
    let create = shape_zoo::create_table_sql(engine, &table, cases);
    connector
        .execute_statement(&create)
        .await
        .unwrap_or_else(|e| panic!("create the {} zoo table: {e}\n{create}", engine.name()));
    let counted = connector
        .execute_query(&shape_zoo::count_sql(&table), 1)
        .await
        .expect("count zoo rows");
    let rows = match counted.result.rows.first().map(|row| &row.0[0]) {
        Some(CellValue::Number(n)) => *n as u64,
        Some(CellValue::Text(s)) => s.parse().expect("a count"),
        other => panic!("unexpected count {other:?}"),
    };
    if rows == 0 {
        let insert = shape_zoo::insert_sql(&table, cases);
        connector
            .execute_statement(&insert)
            .await
            .unwrap_or_else(|e| panic!("fill the {} zoo row: {e}\n{insert}", engine.name()));
    }
}

/// Loads `database`'s zoo through a connector built from the working copy, then drops it.
async fn load_warehouse_zoo(root: &Path, database: &str, engine: Engine, zoo: &LoadedZoo) {
    let manager = WorkspaceBuilder::new(Uuid::new_v4())
        .with_working_copy(root, None, OnMissing::Fail)
        .await
        .expect("config.yml loads")
        .build()
        .await
        .expect("workspace manager");
    let connector = OxyProjectContext::new(manager)
        .build_connector_for(database)
        .await
        .expect("connector");
    fill_zoo(&*connector, engine, zoo).await;
}

/// The request body's `reads`: one `{ column, sql }` per case, in case order.
pub(crate) fn reads(zoo: &LoadedZoo, engine: Engine) -> Vec<Value> {
    let table = zoo.table();
    (0..zoo.zoo.cases(engine).len())
        .map(|i| json!({ "column": shape_zoo::column(i), "sql": shape_zoo::select_column_sql(&table, i) }))
        .collect()
}

pub(crate) async fn read_through(app: &str, function: &str, body: Value) -> Map<String, Value> {
    read_through_in(ORG_SLUG, app, function, body).await
}

/// [`read_through`] for an app published in the org `org_slug`.
pub(crate) async fn read_through_in(
    org_slug: &str,
    app: &str,
    function: &str,
    body: Value,
) -> Map<String, Value> {
    let call = call_function_in(org_slug, app, function, body).await;
    match call.frame("data") {
        Some(Value::Object(row)) => row.clone(),
        _ => panic!("{function} answered no row object; stream: {}", call.raw),
    }
}

pub(crate) fn assert_cases(
    engine: Engine,
    plane: Plane,
    zoo: &LoadedZoo,
    got: &Map<String, Value>,
) {
    let cases = zoo.zoo.cases(engine);
    let failures = shape_zoo::check_reads(cases, plane, got);
    assert!(
        failures.is_empty(),
        "{} of {} {} zoo cases differ on {}:\n{}",
        failures.len(),
        cases.len(),
        engine.name(),
        plane.name(),
        failures.join("\n")
    );
}

async fn read_warehouse_zoo(engine: Engine, database: &str, databases: &str) {
    let t = seeded_tenant().await;
    let zoo = shape_zoo::load();
    let root = write_config(databases);
    load_warehouse_zoo(root.path(), database, engine, &zoo).await;
    let workspace = compile(&t, root.path()).await;
    publish_app(&t, APP, workspace, &functions()).await;
    let body = json!({ "database": database, "reads": reads(&zoo, engine) });
    let got = read_through(APP, READ_WAREHOUSE, body).await;
    assert_cases(engine, Plane::Warehouse, &zoo, &got);
}

#[tokio::test]
async fn shape_zoo_clickhouse_cases_read_through_a_published_function() {
    let Some(ch) = clickhouse().await else {
        return;
    };
    read_warehouse_zoo(Engine::ClickHouse, "ch", &clickhouse_entry("ch", &ch)).await;
}

#[tokio::test]
async fn shape_zoo_postgres_cases_read_through_a_published_function() {
    let entry = postgres_entry("pg").await;
    read_warehouse_zoo(Engine::Postgres, "pg", &entry).await;
}

#[tokio::test]
async fn shape_zoo_duckdb_cases_read_through_a_published_function() {
    let entry = "  - name: duck\n    type: duckdb\n    path: zoo.duckdb\n";
    read_warehouse_zoo(Engine::DuckDb, "duck", entry).await;
}
