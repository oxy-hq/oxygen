//! Preview schemas are dropped after the TTL, and nothing else is (phase 2
//! I5): the registry is written before a schema exists and a schema the
//! preview did not create is never adopted ([`registry_first`]); the sweep
//! claims only registered, expired schemas of keys with no live run, and lets
//! go of claims whose drop will not finish ([`claims`]); and the drop task
//! drops only what its own claim vouches for, and only the relations the
//! preview recorded — recorded before the step that made them ran
//! ([`shadow_first`]) — then cleans Postgres up (here).
//!
//! Database-backed (`Schema::All`: the drop's run and task live in the runtime
//! tables, the sample state and leases in Airway's), with in-process DuckDB
//! standing in for the workspace's Airhouse behind the `SchemaDropper` port.
//! The drop sends the same fixed statements to DuckDB that it sends Airhouse.

mod claims;
mod doubles;
mod fixture;
mod registry_first;
mod shadow_first;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use agentic_core::delegation::{TaskOutcome, TaskSpec};
use airhouse::preview_sql::ShadowState;
use oxy_app::server::previews::ddl::OpenSchemaDropper;
use oxy_app::server::previews::ddl_duckdb::DuckDbDroppers;
use oxy_app::server::previews::drop::PREVIEW_SCHEMA_DROP_KIND;
use oxy_app::server::previews::registry;
use oxy_app::server::previews::{analyze, service, store};
use serde_json::json;
use uuid::Uuid;

use doubles::Flaky;
use fixture::{BRANCH, Fx, TTL, setup};

fn done_metadata(outcome: &TaskOutcome) -> serde_json::Value {
    match outcome {
        TaskOutcome::Done { metadata, .. } => metadata.clone().unwrap_or_default(),
        other => panic!("expected the drop to finish: {other:?}"),
    }
}

/// Two preview schemas holding a table and a view, their shadow-map rows, and
/// Airway rows: the preview's sample state and finished lease, production's
/// own, and a lookalike that `LIKE 'preview:<key>:%'` would also match (`_` is
/// a wildcard). Returns the schemas, sorted, and the two Airway names.
async fn seed_expiring_preview(fx: &Fx) -> (Vec<String>, String, String) {
    let pos = fx.ensure("toast_pos").await;
    let analytics = fx.ensure("analytics").await;
    fx.duck_exec(&format!(
        "CREATE TABLE \"{pos}\".orders AS SELECT 1 AS id; \
         CREATE VIEW \"{pos}\".recent AS SELECT * FROM \"{pos}\".orders; \
         CREATE TABLE \"{analytics}\".daily (d INT);"
    ));
    let shadow = [
        (("toast_pos".into(), "orders".into()), ShadowState::Shadow),
        (("toast_pos".into(), "recent".into()), ShadowState::Shadow),
        (("analytics".into(), "daily".into()), ShadowState::Partial),
    ];
    registry::upsert_shadow(&fx.db, fx.ws, fx.key(), "run-1", &shadow)
        .await
        .unwrap();
    let sample = format!("preview:{}:toast", fx.key());
    let lookalike = format!("preview:{}:toast", fx.key().replace('_', "x"));
    fx.seed_airway(&sample, true).await;
    fx.seed_airway("toast", false).await;
    fx.seed_airway(&lookalike, true).await;
    let mut schemas = vec![pos, analytics];
    schemas.sort();
    (schemas, sample, lookalike)
}

#[tokio::test]
async fn expired_key_is_dropped_and_rows_cleaned() {
    let fx = setup().await;
    let (schemas, sample, lookalike) = seed_expiring_preview(&fx).await;

    let not_yet = fx.sweep_after(0).await;
    assert!(not_yet.is_empty(), "nothing is due before the TTL");
    let claims = fx.sweep_after(73).await;
    assert_eq!(claims.len(), 1, "{claims:?}");
    assert_eq!(claims[0].schemas, schemas);
    let again = fx.sweep_after(73).await;
    assert!(again.is_empty(), "a claimed key is not queued twice");
    let TaskSpec::Custom { kind, .. } = fx.queued_spec(&claims[0].run_id).await else {
        panic!("the drop is a custom task");
    };
    assert_eq!(kind, PREVIEW_SCHEMA_DROP_KIND);

    let report = done_metadata(&fx.run_drop(fx.droppers(), &claims[0]).await);

    assert_eq!(report["dropped"].as_array().unwrap().len(), 2, "{report}");
    let left = fx.duck_schemas();
    assert!(schemas.iter().all(|s| !left.contains(s)), "{left:?}");
    for schema in &schemas {
        assert!(fx.row(schema).await.dropped_at.is_some(), "{schema}");
    }
    let shadow = registry::load_shadow(&fx.db, fx.ws, fx.key())
        .await
        .unwrap();
    assert!(shadow.0.is_empty(), "{shadow:?}");
    for table in ["airway_workspace_pipeline_state", "airway_pipeline_leases"] {
        assert_eq!(fx.airway_rows(table, &[&sample]).await, 0, "{table}");
        assert_eq!(
            fx.airway_rows(table, &["toast", &lookalike]).await,
            2,
            "{table}: production's rows and the lookalike stay"
        );
    }
    let later = fx.sweep_after(200).await;
    assert!(later.is_empty(), "nothing is left to drop");
}

#[tokio::test]
async fn a_customer_schema_named_preview_is_never_dropped() {
    let fx = setup().await;
    let ours = fx.ensure("toast_pos").await;
    // A customer's own `preview_notes`, and one that even carries this
    // preview's prefix: neither has a registry row.
    let lookalike = format!("{}notes", fx.ns.prefix());
    fx.duck_exec(&format!(
        "CREATE SCHEMA preview_notes; CREATE TABLE preview_notes.t AS SELECT 1 AS id; \
         CREATE SCHEMA \"{lookalike}\"; CREATE TABLE \"{lookalike}\".t AS SELECT 1 AS id; \
         CREATE TABLE \"{ours}\".orders AS SELECT 1 AS id;"
    ));
    fx.record("toast_pos", &["orders"]).await;

    let claims = fx.sweep_after(73).await;
    assert_eq!(claims.len(), 1, "{claims:?}");
    assert_eq!(
        claims[0].schemas,
        vec![ours.clone()],
        "the sweep names only registered schemas"
    );
    // Even a payload that names them, under the run's own claim, drops only
    // what the registry vouches for.
    let forged = TaskSpec::Custom {
        kind: PREVIEW_SCHEMA_DROP_KIND.into(),
        payload: json!({
            "workspace_id": fx.ws,
            "preview_key": fx.key(),
            "schemas": [ours, "preview_notes", lookalike],
        }),
    };
    let outcome = fx.run_task(fx.droppers(), &claims[0].run_id, forged).await;

    let report = done_metadata(&outcome);
    assert_eq!(report["refused"].as_array().unwrap().len(), 2, "{report}");
    let schemas = fx.duck_schemas();
    assert!(!schemas.contains(&ours), "{schemas:?}");
    assert_eq!(fx.duck_count("preview_notes.t"), 1);
    assert_eq!(fx.duck_count(&format!("\"{lookalike}\".t")), 1);
}

/// Something other than the preview wrote into the preview's schema. The
/// drop drops only what the preview recorded, keeps the rest (and so the
/// schema), refuses the row so it is never claimed again, and still ends done.
#[tokio::test]
async fn a_schema_holding_relations_the_preview_did_not_create_is_left() {
    let fx = setup().await;
    let schema = fx.ensure("toast_pos").await;
    fx.duck_exec(&format!(
        "CREATE TABLE \"{schema}\".orders AS SELECT 1 AS id; \
         CREATE TABLE \"{schema}\".customer_notes AS SELECT 1 AS id;"
    ));
    fx.record("toast_pos", &["orders"]).await;

    let claims = fx.sweep_after(73).await;
    let report = done_metadata(&fx.run_drop(fx.droppers(), &claims[0]).await);

    assert_eq!(
        report["orphaned"],
        json!([[schema, 1, ["customer_notes"]]]),
        "{report}"
    );
    assert!(fx.duck_schemas().contains(&schema));
    assert_eq!(fx.duck_count(&format!("\"{schema}\".customer_notes")), 1);
    let row = fx.row(&schema).await;
    assert!(
        row.dropped_at.is_none() && row.refused_at.is_some(),
        "{row:?}"
    );
    assert!(
        row.refused_reason
            .as_deref()
            .unwrap()
            .contains("customer_notes"),
        "{row:?}"
    );
    assert!(fx.sweep_after(200).await.is_empty(), "never claimed again");
}

#[tokio::test]
async fn a_failed_drop_is_retried() {
    let fx = setup().await;
    let schema = fx.ensure("toast_pos").await;
    fx.duck_exec(&format!(
        "CREATE TABLE \"{schema}\".orders AS SELECT 1 AS id;"
    ));
    fx.record("toast_pos", &["orders"]).await;
    let fail = Arc::new(AtomicBool::new(true));
    let flaky: Arc<dyn OpenSchemaDropper> = Arc::new(Flaky {
        inner: DuckDbDroppers {
            conn: fx.duck.clone(),
        },
        fail: fail.clone(),
    });

    let first = fx.sweep_after(73).await;
    assert_eq!(first.len(), 1);
    assert!(matches!(
        fx.run_drop(flaky.clone(), &first[0]).await,
        TaskOutcome::Failed(_)
    ));
    assert!(fx.duck_schemas().contains(&schema));
    let row = fx.row(&schema).await;
    assert!(row.dropped_at.is_none());
    assert_eq!(row.drop_run_id.as_deref(), Some(first[0].run_id.as_str()));

    fail.store(false, Ordering::SeqCst);
    let retry = fx.sweep_after(73).await;
    assert_eq!(
        retry.len(),
        1,
        "the failed drop's claim is released and re-queued"
    );
    assert_ne!(retry[0].run_id, first[0].run_id);
    assert_eq!(retry[0].schemas, vec![schema.clone()]);
    done_metadata(&fx.run_drop(flaky, &retry[0]).await);
    assert!(!fx.duck_schemas().contains(&schema));
    let row = fx.row(&schema).await;
    assert!(row.dropped_at.is_some());
    assert_eq!(row.drop_attempts, 2);
}

/// A preview that writes again after its schemas were claimed keeps them: the
/// write re-arms the row and lets the claim go, and the drop, re-checking its
/// claim, skips the schema.
#[tokio::test]
async fn a_write_after_the_claim_keeps_the_schema() {
    let fx = setup().await;
    let schema = fx.ensure("toast_pos").await;
    let claims = fx.sweep_after(73).await;
    assert_eq!(claims.len(), 1);

    registry::touch_key(&fx.db, fx.ws, fx.key(), TTL)
        .await
        .unwrap();
    let report = done_metadata(&fx.run_drop(fx.droppers(), &claims[0]).await);

    assert!(report["dropped"].as_array().unwrap().is_empty(), "{report}");
    assert!(fx.duck_schemas().contains(&schema));
    let row = fx.row(&schema).await;
    assert!(
        row.dropped_at.is_none() && row.drop_run_id.is_none() && row.refused_at.is_none(),
        "{row:?}"
    );
    assert!(
        fx.sweep_after(1).await.is_empty(),
        "the touch pushed expiry out again"
    );
}

#[tokio::test]
async fn deleting_a_preview_expires_it_now() {
    let fx = setup().await;
    let schema = fx.ensure("toast_pos").await;
    store::upsert(&fx.db, fx.ws, BRANCH, "feedface", fx.user)
        .await
        .unwrap();
    // A few seconds' slack for the test host's clock against the database's.
    let soon = || chrono::Utc::now() + chrono::Duration::seconds(5);
    assert!(
        fx.sweep_at(soon()).await.is_empty(),
        "not due before the delete"
    );

    service::delete(&fx.db, fx.ws, BRANCH).await.unwrap();

    assert!(store::find(&fx.db, fx.ws, BRANCH).await.unwrap().is_none());
    let due = fx
        .count(
            "SELECT count(*)::bigint AS n FROM workspace_preview_schemas \
             WHERE workspace_id = $1 AND schema_name = $2 AND expires_at <= now()",
            vec![fx.ws.into(), schema.clone().into()],
        )
        .await;
    assert_eq!(due, 1, "the database's own clock says it expired");
    let claims = fx.sweep_at(soon()).await;
    assert_eq!(claims.len(), 1, "{claims:?}");
    assert_eq!(claims[0].schemas, vec![schema]);
}

/// P5: deleting a preview cancels its queued runs — the queue row stops being
/// claimable and the run ends cancelled — and leaves another branch's alone.
#[tokio::test]
async fn deleting_a_preview_cancels_its_queued_runs() {
    let fx = setup().await;
    store::upsert(&fx.db, fx.ws, BRANCH, "feedface", fx.user)
        .await
        .unwrap();
    let ours = analyze::ensure_enqueued(&fx.db, fx.ws, BRANCH, Uuid::new_v4())
        .await
        .unwrap()
        .expect("queued");
    let theirs = analyze::ensure_enqueued(&fx.db, fx.ws, "feat/other", Uuid::new_v4())
        .await
        .unwrap()
        .expect("queued");

    service::delete(&fx.db, fx.ws, BRANCH).await.unwrap();

    let state = |run: String| {
        let fx = &fx;
        async move {
            let q = "SELECT (SELECT state FROM workspace_preview_runs WHERE run_id = $1) || '/' || \
                     (SELECT task_status FROM agentic_runs WHERE id = $1) || '/' || \
                     (SELECT queue_status FROM agentic_task_queue WHERE task_id = $1) AS s";
            fx.text(q, vec![run.into()]).await
        }
    };
    assert_eq!(state(ours).await, "finished/cancelled/cancelled");
    assert_eq!(state(theirs).await, "queued/running/queued");
}
