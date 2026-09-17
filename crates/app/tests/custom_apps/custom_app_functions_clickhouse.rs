//! `ctx.warehouse.insert` from a published function, run by the V8 isolate,
//! onto a real ClickHouse — the call that failed for every app in
//! 0.5.140–0.5.144 with `Code: 27`.
//!
//! `warehouse_writes_on_engines` drives `ProjectFunctionHost` from Rust, so it
//! proves the host and the connector but not what a bundled module sends
//! through the op bridge. This publishes a function, calls its route, and reads
//! the rows back from ClickHouse directly.
//!
//! **Workspace config goes through the compile boundary.** The app's workspace
//! is compiled and promoted (`oxy_compile::compile_workspace`) into its
//! `*_definitions` rows — the revision `resolve_request_revision` hands the
//! serve fleet — while the workspace row's `path` is an empty directory. The
//! `ch` destination can therefore only have come from Postgres, as on a
//! stateless replica with no working copy.
//!
//! **Needs** Postgres and ClickHouse. ClickHouse is `OXY_TEST_CLICKHOUSE_URL`
//! (with `OXY_TEST_CLICKHOUSE_USER` / `OXY_TEST_CLICKHOUSE_PASSWORD`) or a
//! labelled, reused testcontainer — acquired by `warehouse_writes_on_engines`'
//! own helper. Absent, the test skips; with `OXY_TEST_REQUIRE_CLICKHOUSE=1`
//! it fails instead.

use std::path::Path;

use agentic_core::result::CellValue;
use entity::workspaces;
use oxy::adapters::workspace::builder::WorkspaceBuilder;
use oxy::config::OnMissing;
use oxy_app::agentic_wiring::OxyProjectContext;
use oxy_app::server::compile_config_gate::runtime_config_gate;
use oxy_compile::{CompileRequest, Promotion, RevisionKind, compile_workspace, compiler_version};
use sea_orm::{ActiveModelTrait, ActiveValue};
use serde_json::json;
use uuid::Uuid;

use crate::custom_app_functions_fixture::{
    FunctionSpec, Tenant, call_function, invocations, publish_app, seeded_tenant,
};
use crate::warehouse_writes_on_engines::{ClickHouseServer, clickhouse, clickhouse_entry};

const APP: &str = "fn-e2e-receiving";

/// Creates the table, then inserts rows whose values only the JS computes.
const RECORD_RECEIPTS_JS: &str = r#"
export default async (req, ctx) => {
  const { table, lines } = JSON.parse(req.body);
  await ctx.warehouse.exec(
    "ch",
    `CREATE TABLE ${table} (sku String, qty Int32) ENGINE = MergeTree ORDER BY sku`,
  );
  const rows = lines.map((line) => ({ sku: line.sku.toUpperCase(), qty: line.cases * line.perCase }));
  await ctx.warehouse.insert("ch", table, rows);
  return Response.json({ inserted: rows.length });
};
"#;

fn functions() -> Vec<FunctionSpec> {
    vec![FunctionSpec {
        name: "record-receipts",
        // ClickHouse is a customer warehouse, so a destination alone is refused.
        manifest: json!({
            "route": true,
            "destinations": ["ch"],
            "customerWarehouseWrites": { "ch": "the functions end-to-end test writes this warehouse" },
        }),
        js: RECORD_RECEIPTS_JS,
    }]
}

/// A workspace in the tenant's org whose config lives only in Postgres.
struct ReceivingWorkspace {
    id: Uuid,
    /// Where the config was compiled from — also what the read-back connects with.
    source: tempfile::TempDir,
    /// What the workspace row points at: empty, like a serve replica's disk.
    _served: tempfile::TempDir,
}

async fn receiving_workspace(t: &Tenant, ch: &ClickHouseServer) -> ReceivingWorkspace {
    let source = tempfile::tempdir().expect("source dir");
    // By `password_var`: the compile below redacts an inline password to `""`,
    // and CI's ClickHouse requires its password.
    std::fs::write(
        source.path().join("config.yml"),
        format!("databases:\n{}models: []\n", clickhouse_entry("ch", ch)),
    )
    .expect("write config.yml");
    let served = tempfile::tempdir().expect("served dir");

    let id = Uuid::new_v4();
    workspaces::ActiveModel {
        id: ActiveValue::Set(id),
        name: ActiveValue::Set("Receiving".into()),
        org_id: ActiveValue::Set(Some(t.org_id)),
        path: ActiveValue::Set(Some(served.path().to_string_lossy().into_owned())),
        ..Default::default()
    }
    .insert(&t.db)
    .await
    .expect("seed workspace");

    let outcome = compile_workspace(CompileRequest {
        db: &t.db,
        workspace_id: id,
        workspace_path: source.path(),
        git_sha: None,
        branch: None,
        compiler_version: compiler_version(),
        promote: true,
        kind: RevisionKind::Main,
        owner_user_id: None,
        // The gate the production compile paths (worker and CLI) install.
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
    ReceivingWorkspace {
        id,
        source,
        _served: served,
    }
}

/// `(sku, qty)` rows of `table`, read straight from ClickHouse.
async fn rows_in(source: &Path, table: &str) -> Vec<(String, i64)> {
    let manager = WorkspaceBuilder::new(Uuid::new_v4())
        .with_working_copy(source, None, OnMissing::Fail)
        .await
        .expect("config.yml loads")
        .build()
        .await
        .expect("workspace manager");
    let connector = OxyProjectContext::new(manager)
        .build_connector_for("ch")
        .await
        .expect("connector");
    let result = connector
        .execute_query(&format!("SELECT sku, qty FROM {table} ORDER BY sku"), 100)
        .await
        .expect("read back");
    result
        .result
        .rows
        .iter()
        .map(|row| match (&row.0[0], &row.0[1]) {
            (CellValue::Text(sku), CellValue::Number(qty)) => (sku.clone(), *qty as i64),
            other => panic!("unexpected row shape {other:?}"),
        })
        .collect()
}

#[tokio::test]
async fn a_published_function_inserts_rows_into_clickhouse_through_the_isolate() {
    let Some(ch) = clickhouse().await else {
        return;
    };
    let t = seeded_tenant().await;
    let ws = receiving_workspace(&t, &ch).await;
    let published = publish_app(&t, APP, ws.id, &functions()).await;
    let table = format!("receiving_{}", Uuid::new_v4().simple());

    let call = call_function(
        APP,
        "record-receipts",
        json!({ "table": table, "lines": [
            { "sku": "pallet-b", "cases": 3, "perCase": 8 },
            { "sku": "pallet-a", "cases": 2, "perCase": 12 },
            { "sku": "crate-c", "cases": 5, "perCase": 1 },
        ] }),
    )
    .await;

    assert_eq!(
        call.frame("data"),
        Some(&json!({ "inserted": 3 })),
        "the function must finish its CREATE and INSERT on ClickHouse; stream: {}",
        call.raw
    );
    assert_eq!(
        rows_in(ws.source.path(), &table).await,
        vec![
            ("CRATE-C".to_string(), 5),
            ("PALLET-A".to_string(), 24),
            ("PALLET-B".to_string(), 24),
        ]
    );
    let rows = invocations(&t.db, published.app_id, "record-receipts").await;
    let seen: Vec<_> = rows
        .iter()
        .map(|r| (r.mode.as_str(), r.status.as_str()))
        .collect();
    assert_eq!(seen, vec![("route", "success")]);
}
