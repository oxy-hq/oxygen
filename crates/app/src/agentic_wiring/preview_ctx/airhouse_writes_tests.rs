//! The pure halves of a preview Airhouse step: what counts as a write, which
//! schemas it needs, how the batch and its note are put together. Whole runs
//! are driven in `tests/platform/preview_runs/airhouse.rs`.

use super::*;

fn ns() -> PreviewNamespace {
    PreviewNamespace::from_key("feat_x_abc123").unwrap()
}

fn rewritten(sql: &str) -> Rewrite {
    rewrite(
        sql,
        &ns(),
        &ShadowMap::default(),
        &RewriteOptions::default(),
    )
    .unwrap()
}

#[test]
fn a_read_is_sent_as_written_and_a_write_names_its_schemas() {
    let read = rewritten("SELECT * FROM toast_pos.orders");
    assert!(!writes_anything(&read));
    assert_eq!(read_review(&read, &ns()), SqlReview::Proceed);

    let write = rewritten(
        "CREATE SCHEMA IF NOT EXISTS gl; INSERT INTO toast_pos.orders SELECT 1; \
         CREATE OR REPLACE TABLE gl.daily AS SELECT 1 AS n",
    );
    assert!(writes_anything(&write));
    assert_eq!(
        preview_schemas(&write, &ns()),
        vec![
            "preview_feat_x_abc123__gl".to_string(),
            "preview_feat_x_abc123__toast_pos".to_string()
        ]
    );
}

#[test]
fn copies_go_first_and_a_lone_schema_sends_a_read_of_nothing() {
    let write = rewritten("INSERT INTO toast_pos.orders SELECT 1");
    let copy = CopyPlan {
        live: ("toast_pos".into(), "orders".into()),
        statement: "CREATE OR REPLACE TABLE p.orders AS SELECT * FROM toast_pos.orders".into(),
        state: ShadowState::Partial,
    };
    let sql = batch_sql(std::slice::from_ref(&copy), &write);
    assert!(sql.starts_with("CREATE OR REPLACE TABLE p.orders"), "{sql}");
    assert!(sql.ends_with(&write.sql), "{sql}");
    let note = notes(&write, &[copy], &[], &ns());
    assert_eq!(note["copies"][0]["state"], "partial");
    assert_eq!(
        note["writes"][0]["preview"],
        "preview_feat_x_abc123__toast_pos.orders"
    );
    assert_eq!(note["rewritten"], true);
    assert!(note.get("held").is_none(), "a rewritten step is not held");

    let lone = rewritten("CREATE SCHEMA IF NOT EXISTS gl");
    assert!(writes_anything(&lone));
    assert_eq!(batch_sql(&[], &lone), NOTHING_TO_SEND);
}

#[test]
fn read_live_only_comes_from_the_run_options() {
    assert!(read_live_only(&json!({ "read_live_only": true })));
    assert!(!read_live_only(&json!({ "variables": {} })));
}

/// A step whose batch failed before its copy ran is reviewed again with the
/// copy planned again — strictly — once that batch is done; and a strict copy
/// never replaces the table the preview has been writing, whatever a lagging
/// listing said.
#[tokio::test]
async fn a_batch_that_failed_before_its_copy_is_copied_again_strictly_on_retry() {
    use crate::server::test_support::{SKIP_MSG, test_db};
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let (ws, lake) = (
        seed_workspace(&db).await,
        Arc::new(std::sync::Mutex::new(lake())),
    );
    let ports = crate::server::previews::airhouse_duckdb::DuckDbAirhouse::new(lake.clone())
        .unwrap()
        .with_failing_writes(1)
        .shared();
    let airhouse = PreviewAirhouse::open(&db, &run_row(ws), ports)
        .await
        .unwrap();
    let insert = "INSERT INTO toast_pos.orders VALUES (4, 40)";

    let first = batch_of(airhouse.review(insert, true).await.unwrap());
    assert!(first.starts_with("CREATE OR REPLACE TABLE "), "{first}");
    let conn = airhouse.connector();
    conn.execute_query(&first, 1)
        .await
        .expect_err("the Writer dropped");

    let retry = batch_of(airhouse.review(insert, true).await.unwrap());
    assert!(
        retry.starts_with("CREATE TABLE "),
        "copied again, strictly: {retry}"
    );
    conn.execute_query(&retry, 1)
        .await
        .expect("the retry lands");
    let preview = "\"preview_feat_x_abc123__toast_pos\".orders";
    let count = |sql: &str| -> i64 {
        lake.lock()
            .unwrap()
            .query_row(sql, [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(count(&format!("SELECT count(*) FROM {preview}")), 4);

    // A listing that lagged would plan the copy again: strict, so it fails
    // and the preview keeps what it wrote.
    let step = rewritten(insert);
    let again = HashSet::from([("toast_pos".to_string(), "orders".to_string())]);
    let copies = copy_plans(conn.as_ref(), &step, None, &again)
        .await
        .unwrap();
    assert!(copies[0].statement.starts_with("CREATE TABLE "));
    conn.execute_query(&copies[0].statement, 1)
        .await
        .expect_err("a strict copy never replaces");
    assert_eq!(count(&format!("SELECT count(*) FROM {preview}")), 4);
}

fn batch_of(step: Step) -> String {
    match step {
        Step::Review(SqlReview::Rewrite { sql, .. }) => sql,
        other => panic!("not a rewrite: {other:?}"),
    }
}

/// Three live orders.
fn lake() -> duckdb::Connection {
    let conn = duckdb::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE SCHEMA toast_pos; CREATE TABLE toast_pos.orders (id INTEGER, amount INTEGER); \
         INSERT INTO toast_pos.orders VALUES (1, 10), (2, 20), (3, 30);",
    )
    .unwrap();
    conn
}

async fn seed_workspace(db: &DatabaseConnection) -> Uuid {
    use sea_orm::{ActiveModelTrait, Set};
    let id = Uuid::new_v4();
    entity::workspaces::ActiveModel {
        id: Set(id),
        name: Set(format!("preview-airhouse-{id}")),
        status: Set(entity::workspaces::WorkspaceStatus::Ready),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed workspace");
    id
}

fn run_row(ws: Uuid) -> entity::workspace_preview_runs::Model {
    let now = chrono::Utc::now().fixed_offset();
    entity::workspace_preview_runs::Model {
        run_id: Uuid::new_v4().to_string(),
        workspace_id: ws,
        branch: "feat/x".into(),
        preview_key: "feat_x_abc123".into(),
        revision_id: Uuid::new_v4(),
        kind: "procedure".into(),
        target_ref: None,
        parent_run_id: None,
        options: json!({}),
        state: "running".into(),
        requested_by: None,
        created_at: now,
        started_at: Some(now),
        finished_at: None,
    }
}
