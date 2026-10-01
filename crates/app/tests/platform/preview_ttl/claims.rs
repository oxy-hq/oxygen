//! The sweep's claims: which keys a run holds, a claim re-checking the find,
//! and letting go of a claim whose drop will not finish — and giving up
//! after repeated failures.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use agentic_core::delegation::TaskOutcome;
use airhouse::preview_sql::PreviewNamespace;
use oxy_app::server::previews::ddl::OpenSchemaDropper;
use oxy_app::server::previews::ddl_duckdb::{DuckDbDroppers, DuckDbPreviewDdl};
use oxy_app::server::previews::maintenance::{self, MAX_DROP_ATTEMPTS};
use oxy_app::server::previews::registry;

use super::doubles::Flaky;
use super::fixture::{TTL, setup};

#[tokio::test]
async fn a_key_with_a_running_run_is_kept() {
    let fx = setup().await;
    let schema = fx.ensure("toast_pos").await;
    // Another preview in the same workspace expires on its own schedule.
    let other_ns = PreviewNamespace::for_branch(fx.ws, "feat/other");
    let other = registry::ensure_schema(
        &fx.db,
        &DuckDbPreviewDdl::new(fx.duck.clone(), other_ns.clone()),
        fx.ws,
        &other_ns,
        "toast_pos",
        "run-9",
        TTL,
    )
    .await
    .unwrap();

    fx.preview_run("run-2", "procedure", "running", Some("running"))
        .await;
    let claims = fx.sweep_after(73).await;
    let claimed: Vec<&String> = claims.iter().flat_map(|c| &c.schemas).collect();
    assert_eq!(claimed, vec![&other], "a run holds only its own key");
    assert!(fx.duck_schemas().contains(&schema));
    assert!(fx.row(&schema).await.drop_run_id.is_none());

    fx.preview_run("run-2", "procedure", "finished", Some("done"))
        .await;
    // A change check that is running never touches a preview schema, so it
    // does not hold the key.
    fx.preview_run("check-1", "analyze", "running", Some("running"))
        .await;
    let claims = fx.sweep_after(73).await;
    assert_eq!(claims.len(), 1, "{claims:?}");
    assert_eq!(claims[0].schemas, vec![schema]);
}

/// S9 records a run `queued` with no `agentic_runs` row until the queue
/// starts it (`runs::advance`). Such a run holds its key — until it has
/// waited longer than the run ceiling, when a queue that is not moving stops
/// keeping the preview's schemas.
#[tokio::test]
async fn a_queued_run_holds_its_key_until_it_outlives_the_ceiling() {
    let fx = setup().await;
    let schema = fx.ensure("toast_pos").await;
    fx.preview_run("run-3", "procedure", "queued", None).await;

    assert!(
        fx.sweep_after(73).await.is_empty(),
        "a queued run with no run row yet holds the key"
    );
    assert!(fx.row(&schema).await.drop_run_id.is_none());

    fx.exec(
        "UPDATE workspace_preview_runs SET created_at = now() - interval '2 hours' \
         WHERE run_id = 'run-3'",
        vec![],
    )
    .await;
    let claims = fx.sweep_after(73).await;
    assert_eq!(claims.len(), 1, "a run queued past the ceiling does not");
    assert_eq!(claims[0].schemas, vec![schema]);
}

/// A worker that died mid-run leaves its preview-run row `running` for ever;
/// once its `agentic_runs` row is terminal, it no longer holds the key.
#[tokio::test]
async fn a_crashed_run_does_not_hold_its_key() {
    let fx = setup().await;
    let schema = fx.ensure("toast_pos").await;
    fx.preview_run("run-2", "procedure", "running", Some("failed"))
        .await;

    let claims = fx.sweep_after(73).await;

    assert_eq!(claims.len(), 1, "{claims:?}");
    assert_eq!(claims[0].schemas, vec![schema]);
}

/// The claim re-checks what the find checked: a run that starts on the key
/// between the two keeps it.
#[tokio::test]
async fn a_run_started_between_the_find_and_the_claim_keeps_the_key() {
    let fx = setup().await;
    let schema = fx.ensure("toast_pos").await;
    let now = chrono::Utc::now() + chrono::Duration::hours(73);
    let found = maintenance::find_expired_keys(&fx.db, now).await.unwrap();
    assert_eq!(found, vec![(fx.ws, fx.key().to_string())]);

    fx.preview_run("run-2", "procedure", "running", Some("running"))
        .await;
    let claim = maintenance::claim_key(&fx.db, fx.ws, fx.key(), now)
        .await
        .unwrap();

    assert!(claim.is_none(), "{claim:?}");
    assert!(fx.row(&schema).await.drop_run_id.is_none());
}

/// A drop whose task the queue dead-lettered, or whose claim is too old to
/// be alive, is released and queued again under a new run — and, since no
/// worker ever ran either, neither costs an attempt.
#[tokio::test]
async fn a_dead_lettered_or_stale_claim_is_released() {
    let fx = setup().await;
    let schema = fx.ensure("toast_pos").await;
    let first = fx.sweep_after(73).await;
    assert_eq!(first.len(), 1);
    // The run row still reads `running`; only the queue says it is dead.
    fx.exec(
        "UPDATE agentic_task_queue SET queue_status = 'dead' WHERE task_id = $1",
        vec![first[0].run_id.clone().into()],
    )
    .await;
    let second = fx.sweep_after(73).await;
    assert_eq!(second.len(), 1, "a dead-lettered drop is released");
    assert_ne!(second[0].run_id, first[0].run_id);

    // Its run is alive and its task queued, but the claim is hours old.
    fx.exec(
        "UPDATE workspace_preview_schemas \
         SET drop_claimed_at = now() - interval '7 hours' WHERE schema_name = $1",
        vec![schema.clone().into()],
    )
    .await;
    let third = fx.sweep_after(73).await;
    assert_eq!(third.len(), 1, "a stale claim is released");
    assert_ne!(third[0].run_id, second[0].run_id);
    assert_eq!(fx.row(&schema).await.drop_attempts, 0);
}

/// Each drop a worker runs counts. After `MAX_DROP_ATTEMPTS` failed drops the
/// sweep stops queueing the schema, and the row says how many times it tried.
#[tokio::test]
async fn a_drop_that_keeps_failing_stops_after_five_attempts() {
    let fx = setup().await;
    let schema = fx.ensure("toast_pos").await;
    let flaky: Arc<dyn OpenSchemaDropper> = Arc::new(Flaky {
        inner: DuckDbDroppers {
            conn: fx.duck.clone(),
        },
        fail: Arc::new(AtomicBool::new(true)),
    });

    for attempt in 1..=MAX_DROP_ATTEMPTS {
        let claims = fx.sweep_after(73).await;
        assert_eq!(claims.len(), 1, "attempt {attempt}");
        assert!(matches!(
            fx.run_drop(flaky.clone(), &claims[0]).await,
            TaskOutcome::Failed(_)
        ));
    }

    assert!(fx.sweep_after(73).await.is_empty(), "the sweep gave up");
    let row = fx.row(&schema).await;
    assert_eq!(row.drop_attempts, MAX_DROP_ATTEMPTS);
    assert!(
        row.dropped_at.is_none() && row.drop_run_id.is_none(),
        "{row:?}"
    );
    assert!(fx.duck_schemas().contains(&schema));
}
