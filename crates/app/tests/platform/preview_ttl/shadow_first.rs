//! Shadow first: the host records a step's shadow-map changes before it sends
//! the step's SQL (`registry::record_step`), so a crash anywhere in a step
//! leaves nothing the preview made unrecorded — and the TTL drop takes the
//! whole schema rather than mistaking the preview's own table for someone
//! else's.

use airhouse::preview_sql::{Prelude, RewriteOptions, ShadowMap, rewrite};
use oxy_app::server::previews::registry;
use serde_json::json;

use super::done_metadata;
use super::fixture::{Fx, setup};

/// One host step as S8 runs it, up to a crash: rewrite, ensure the step's
/// schemas, record, and send the SQL only when `send`. No live table exists
/// in the stand-in, so no copy is planned.
async fn step(fx: &Fx, before: ShadowMap, sql: &str, send: bool) -> ShadowMap {
    let rewritten = rewrite(sql, &fx.ns, &before, &RewriteOptions::default()).expect(sql);
    for prelude in &rewritten.preludes {
        if let Prelude::EnsureSchema { live, .. } = prelude {
            fx.ensure(live).await;
        }
    }
    let after = registry::record_step(&fx.db, fx.ws, fx.key(), "run-1", &before, &rewritten, &[])
        .await
        .unwrap();
    if send {
        fx.duck_exec(&rewritten.sql);
    }
    after
}

#[tokio::test]
async fn a_step_that_crashed_around_its_ddl_is_still_recorded() {
    let fx = setup().await;
    let schema = format!("{}toast_pos", fx.ns.prefix());
    let map = step(
        &fx,
        ShadowMap::default(),
        "CREATE TABLE toast_pos.orders AS SELECT 1 AS id",
        true,
    )
    .await;
    // Crashed right after its DDL ran: nothing of the step came after the send.
    let map = step(
        &fx,
        map,
        "ALTER TABLE toast_pos.orders RENAME TO orders_v2",
        true,
    )
    .await;
    // Crashed before its DDL: `orders_v2` is recorded `dropped`, and still there.
    let map = step(&fx, map, "DROP TABLE toast_pos.orders_v2", false).await;
    // Crashed before its DDL: recorded, never made.
    step(
        &fx,
        map,
        "CREATE VIEW toast_pos.recent AS SELECT 1 AS id",
        false,
    )
    .await;
    assert_eq!(fx.duck_count(&format!("\"{schema}\".orders_v2")), 1);

    let claims = fx.sweep_after(73).await;
    let report = done_metadata(&fx.run_drop(fx.droppers(), &claims[0]).await);

    assert_eq!(report["dropped"], json!([[schema, 1]]), "{report}");
    assert_eq!(report["orphaned"], json!([]), "{report}");
    assert!(!fx.duck_schemas().contains(&schema));
    assert!(fx.row(&schema).await.dropped_at.is_some());
}
