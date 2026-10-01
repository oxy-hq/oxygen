//! Phase 2b S8, fix round 1: the fences around a preview's Airhouse writes
//! that a whole run exercises — an agent's direct write, an Airhouse whose
//! scope echo fails, and a copy a crash recorded but never made.

use agentic_pipeline::platform::PlatformContext;
use oxy_app::server::previews::airhouse_duckdb::{DuckDbAirhouse, UNCONFINED_ECHO};
use oxy_app::server::previews::namespace::preview_key;
use serde_json::json;
use std::sync::Arc;

use super::airhouse::{Lake, rows, schema_of, shadow, sql_step};
use super::world::{self, base_ctx, run_to_the_end_with};
use crate::preview_routes::fixture::{BRANCH, exec};

/// A write that reaches the preview Airhouse connector without a step's
/// review — an agent's `CREATE TABLE` into a preview schema — is refused: the
/// run never recorded that relation, so the TTL drop would never drop it and
/// its schema (and the live rows it copied) would stay for good. A write to a
/// relation the run did record is let through (the control).
#[tokio::test]
async fn an_agents_direct_write_into_a_preview_schema_is_refused() {
    let fx = world::world(json!([sql_step(
        "add",
        "INSERT INTO toast_pos.orders VALUES (4, 40)"
    )]))
    .await;
    let lake = Lake::seeded();
    let resolver = lake.resolver_on(&fx, DuckDbAirhouse::new);
    let detail = run_to_the_end_with(&fx, resolver.clone()).await;
    assert_eq!(detail["outcome"], "succeeded", "{detail}");
    let run_id = detail["run_id"].as_str().unwrap();

    let root = agentic_runtime::crud::get_run(&fx.db, run_id)
        .await
        .unwrap()
        .expect("run");
    let dir = tempfile::tempdir().unwrap();
    let base: Arc<dyn PlatformContext> = base_ctx(&fx, dir.path()).await;
    let platform = resolver.platform_for(&root, base).await.unwrap();
    let conn = platform.get_connector("airhouse").await.unwrap();
    let orders = schema_of(&fx, "toast_pos");

    let scratch = format!("CREATE TABLE \"{orders}\".scratch AS SELECT * FROM toast_pos.orders");
    let err = conn
        .execute_statement(&scratch)
        .await
        .expect_err("an unrecorded relation");
    assert!(err.to_string().contains("has not recorded"), "{err}");
    let made = lake.int(&format!(
        "SELECT count(*) FROM information_schema.tables \
         WHERE table_schema = '{orders}' AND table_name = 'scratch'"
    ));
    assert_eq!(made, 0, "nothing was created");

    conn.execute_statement(&format!("INSERT INTO \"{orders}\".orders VALUES (9, 90)"))
        .await
        .expect("control: a recorded relation is written");
    assert_eq!(
        lake.int(&format!("SELECT count(*) FROM \"{orders}\".orders")),
        5
    );
    assert_eq!(lake.live_orders(), (3, 60), "a live table changed");
}

/// The deployment says it can confine a Writer, but the step's own mint comes
/// back unconfined: the step is held, and nothing is created — no schema, no
/// registry row, no shadow row — before that answer.
#[tokio::test]
async fn an_unconfined_echo_holds_the_step_and_creates_no_schema() {
    let fx = world::world(json!([sql_step(
        "add",
        "INSERT INTO toast_pos.orders VALUES (4, 40)"
    )]))
    .await;
    let lake = Lake::seeded();
    let resolver = lake.resolver_on(&fx, DuckDbAirhouse::unconfined_echo);
    let detail = run_to_the_end_with(&fx, resolver).await;

    assert_eq!(detail["outcome"], "succeeded", "{detail}");
    assert_eq!(detail["held_count"], 1, "{detail}");
    let reason = detail["steps"][0]["held"]["reason"].as_str().unwrap();
    assert!(reason.contains(UNCONFINED_ECHO), "{reason}");
    assert!(!lake.schema_exists(&schema_of(&fx, "toast_pos")));
    let registered = "SELECT 1 FROM workspace_preview_schemas WHERE workspace_id = $1";
    assert!(rows(&fx, registered).await.is_empty());
    assert!(shadow(&fx).await.is_empty());
    assert_eq!(lake.live_orders(), (3, 60));
}

/// A step recorded its copy of `toast_pos.orders` and then never sent it (a
/// crash between the two): the map says the preview holds the table, the
/// preview schema has none. The next write in place copies it again rather
/// than meeting a missing table.
#[tokio::test]
async fn a_copy_recorded_but_never_made_is_made_again() {
    let fx = world::world(json!([sql_step(
        "add",
        "INSERT INTO toast_pos.orders VALUES (4, 40)"
    )]))
    .await;
    exec(
        &fx.db,
        "INSERT INTO workspace_preview_tables \
             (workspace_id, preview_key, live_schema, table_name, state, last_run_id) \
         VALUES ($1, $2, 'toast_pos', 'orders', 'shadow', 'a-run-that-crashed')",
        vec![fx.ws.into(), preview_key(fx.ws, BRANCH).into()],
    )
    .await;
    let lake = Lake::seeded();
    let detail = run_to_the_end_with(&fx, lake.resolver_on(&fx, DuckDbAirhouse::new)).await;

    assert_eq!(detail["outcome"], "succeeded", "{detail}");
    let orders = schema_of(&fx, "toast_pos");
    assert_eq!(
        lake.int(&format!("SELECT count(*) FROM \"{orders}\".orders")),
        4,
        "copied again from live, then written"
    );
    assert_eq!(
        detail["steps"][0]["redirected"]["copies"][0]["live"], "toast_pos.orders",
        "{detail}"
    );
    assert_eq!(lake.live_orders(), (3, 60));
}
