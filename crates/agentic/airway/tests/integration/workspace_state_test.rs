//! The guarantee the per-workspace state table exists for: two workspaces
//! running a pipeline of the same name keep separate cursors.
//!
//! Before this, `airway_pipeline_state` was keyed by `pipeline_name` alone, and
//! prod had three workspaces (`poke-house`, `poke-house-staging`, `oxygen`)
//! sharing one `restaurant_analytics` row — so whichever ran last set where the
//! others resumed from. These cases pin the four behaviours that fix costs:
//! separation, one-time adoption of the legacy row, a reset that stays reset,
//! and a rolling deploy where an old pod is still writing the legacy row.
//!
//! Requires Docker (or `OXY_DATABASE_URL`).

use std::collections::HashMap;
use std::sync::Arc;

use agentic_airway::AirwayPgStateStore;
use agentic_airway::extension::{pipeline_state, workspace_pipeline_state};
use airway::Schema;
use airway::state::{PipelineState, ResourceState, StateStore};
use sea_orm::{ActiveValue, DatabaseConnection, EntityTrait};
use uuid::Uuid;

use crate::harness::test_db;

/// A cursor whose only job is to be distinguishable from another one.
fn state_at(high_water: &str) -> PipelineState {
    let mut state = PipelineState::default();
    state.resource_states.insert(
        "orders".to_string(),
        ResourceState {
            incremental: None,
            custom: HashMap::from([(
                "__connector_state".to_string(),
                serde_json::json!({ "high_water": high_water }),
            )]),
        },
    );
    state
}

/// The `high_water` a snapshot would resume from, or `None` for an empty cursor.
fn high_water_of(state: &PipelineState) -> Option<String> {
    state
        .resource_states
        .get("orders")?
        .custom
        .get("__connector_state")?
        .get("high_water")?
        .as_str()
        .map(str::to_string)
}

/// Write the legacy name-keyed row directly — the shape a pre-deploy pod left
/// behind, and the only thing adoption has to go on.
async fn seed_legacy_row(
    db: &DatabaseConnection,
    pipeline_name: &str,
    high_water: &str,
    version: i64,
) {
    let row = pipeline_state::ActiveModel {
        pipeline_name: ActiveValue::Set(pipeline_name.to_string()),
        state: ActiveValue::Set(serde_json::to_value(state_at(high_water)).unwrap()),
        schema_json: ActiveValue::Set(serde_json::to_value(Schema::new(pipeline_name)).unwrap()),
        version: ActiveValue::Set(version),
        updated_at: ActiveValue::Set(chrono::Utc::now()),
    };
    pipeline_state::Entity::insert(row)
        .exec(db)
        .await
        .expect("seed the legacy airway_pipeline_state row");
}

/// Two workspaces, one pipeline name: each keeps its own cursor. This is the
/// prod collision, inverted into an assertion.
#[tokio::test(flavor = "multi_thread")]
async fn two_workspaces_do_not_share_a_cursor() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let db = Arc::new(db);
    let name = format!("restaurant_analytics_{}", Uuid::new_v4().simple());
    let (prod, staging) = (Uuid::new_v4(), Uuid::new_v4());

    let prod_store = AirwayPgStateStore::new(Arc::clone(&db), prod, name.clone());
    let staging_store = AirwayPgStateStore::new(Arc::clone(&db), staging, name.clone());

    // Both start empty — neither has ever run.
    assert_eq!(prod_store.load().await.unwrap().version, 0);
    assert_eq!(staging_store.load().await.unwrap().version, 0);

    prod_store
        .save(&state_at("2026-09-16"), &Schema::new(&name), 0)
        .await
        .expect("prod saves its own cursor");
    // Staging saves from version 0 too: it has its own row, so prod's write is
    // not a concurrency conflict for it.
    staging_store
        .save(&state_at("2026-01-01"), &Schema::new(&name), 0)
        .await
        .expect("staging saves against its own row, not prod's");

    assert_eq!(
        high_water_of(&prod_store.load().await.unwrap().state).as_deref(),
        Some("2026-09-16"),
        "staging's run must not move prod's cursor"
    );
    assert_eq!(
        high_water_of(&staging_store.load().await.unwrap().state).as_deref(),
        Some("2026-01-01"),
        "prod's run must not move staging's cursor"
    );
}

/// The first load in a workspace adopts the legacy row, so the deploy loses no
/// ground: the run resumes exactly where the shared cursor stood, including its
/// version. A later change to the legacy row is ignored — adoption happens once.
#[tokio::test(flavor = "multi_thread")]
async fn the_legacy_row_is_adopted_once_then_ignored() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let db = Arc::new(db);
    let name = format!("adopt_once_{}", Uuid::new_v4().simple());
    seed_legacy_row(&db, &name, "2026-09-01", 342).await;

    let workspace_id = Uuid::new_v4();
    let store = AirwayPgStateStore::new(Arc::clone(&db), workspace_id, name.clone());

    let adopted = store.load().await.unwrap();
    assert_eq!(
        high_water_of(&adopted.state).as_deref(),
        Some("2026-09-01"),
        "the first load must continue from the legacy cursor, not re-read history"
    );
    assert_eq!(
        adopted.version, 342,
        "the adopted version carries over, so the first save's optimistic check \
         matches what the run actually read"
    );
    assert!(
        adopted.schema.is_some(),
        "the legacy schema carries over too"
    );

    // A save against the adopted version succeeds — proof the version the store
    // handed airway is the one the row actually holds.
    store
        .save(&state_at("2026-09-17"), &Schema::new(&name), 342)
        .await
        .expect("save against the adopted version");

    // A straggler pod on the old code advances the legacy row. This workspace
    // already has its own row, so nothing here re-reads that.
    let legacy = pipeline_state::Entity::find_by_id(name.clone())
        .one(db.as_ref())
        .await
        .unwrap()
        .expect("legacy row still present");
    let mut legacy: pipeline_state::ActiveModel = legacy.into();
    legacy.state = ActiveValue::Set(serde_json::to_value(state_at("1999-01-01")).unwrap());
    pipeline_state::Entity::update(legacy)
        .exec(db.as_ref())
        .await
        .unwrap();

    assert_eq!(
        high_water_of(&store.load().await.unwrap().state).as_deref(),
        Some("2026-09-17"),
        "once adopted, the workspace row is the only thing read — an old pod \
         still writing the legacy row cannot drag this workspace backwards"
    );
}

/// A reset must stay reset. Deleting the workspace row would send the next load
/// straight back to the legacy row and hand back the very cursor the reset
/// discarded, so the reset writes a tombstone instead.
#[tokio::test(flavor = "multi_thread")]
async fn a_reset_is_not_undone_by_re_adopting_the_legacy_row() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let db = Arc::new(db);
    let name = format!("reset_tombstone_{}", Uuid::new_v4().simple());
    seed_legacy_row(&db, &name, "2026-09-01", 7).await;

    let workspace_id = Uuid::new_v4();
    let store = AirwayPgStateStore::new(Arc::clone(&db), workspace_id, name.clone());
    let adopted = store.load().await.unwrap();
    assert_eq!(high_water_of(&adopted.state).as_deref(), Some("2026-09-01"));

    agentic_airway::reset::clear_pipeline_state(db.as_ref(), workspace_id, &name)
        .await
        .expect("reset the workspace's state");

    let after = store.load().await.unwrap();
    assert!(
        after.state.resource_states.is_empty(),
        "the cursor must be gone after a reset, got {:?}",
        after.state.resource_states
    );
    assert!(
        after.schema.is_none(),
        "no schema after a reset — the next run re-infers one"
    );
    assert!(
        after.version > adopted.version,
        "the reset must advance the version ({} → {}) so a run still holding \
         the pre-reset one cannot write the discarded cursor back",
        adopted.version,
        after.version,
    );

    // That stale writer is refused rather than silently restoring the cursor.
    let stale = store
        .save(
            &state_at("2026-09-01"),
            &Schema::new(&name),
            adopted.version,
        )
        .await;
    assert!(
        stale.is_err(),
        "a save at the pre-reset version must be refused, not applied"
    );
    assert!(
        store.load().await.unwrap().state.resource_states.is_empty(),
        "and the reset state must survive that refused write"
    );

    // The tombstone is a real row, so the legacy one is never consulted again.
    let row = workspace_pipeline_state::Entity::find_by_id((workspace_id, name.clone()))
        .one(db.as_ref())
        .await
        .unwrap()
        .expect("a reset leaves a row behind — that is the point");
    assert!(row.schema_json.is_none(), "the tombstone carries no schema");
}

/// A reset always lands strictly above whatever the row held when it ran, even
/// when the row moved after the caller last looked at it.
///
/// This is the postcondition that closes the reset race. Reset takes no
/// pipeline lease, so a run can save between the reset deciding on a version
/// and writing it: row at v5, run A loads v5, reset reads v5, run A saves (v6),
/// reset writes its tombstone at v6 — exactly what A's next save expects, so A
/// writes the discarded cursor straight back. The bump is therefore computed in
/// SQL from the stored value, never from an earlier read.
///
/// Note this asserts the postcondition, not the interleaving: a single-threaded
/// test cannot stage a write inside another statement, so it would also pass
/// against the read-then-write version. What it pins is that the tombstone's
/// version is derived from the row as it stands at write time.
#[tokio::test(flavor = "multi_thread")]
async fn a_reset_lands_above_the_row_it_finds() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let db = Arc::new(db);
    let name = format!("reset_race_{}", Uuid::new_v4().simple());
    let workspace_id = Uuid::new_v4();
    let store = AirwayPgStateStore::new(Arc::clone(&db), workspace_id, name.clone());

    store
        .save(&state_at("2026-09-16"), &Schema::new(&name), 0)
        .await
        .expect("seed the row");
    // The version a concurrent run would be holding.
    let a_expects = store.load().await.unwrap().version;
    // That run saves, moving the row out from under anyone who read it earlier.
    store
        .save(&state_at("2026-09-17"), &Schema::new(&name), a_expects)
        .await
        .expect("run A advances the row");
    let before_reset = store.load().await.unwrap().version;

    agentic_airway::reset::clear_pipeline_state(db.as_ref(), workspace_id, &name)
        .await
        .expect("reset after A's save");

    let after = store.load().await.unwrap();
    assert_eq!(
        after.version,
        before_reset + 1,
        "the tombstone must be derived from the row as it stands, not from an \
         earlier read"
    );
    // So every version handed out before the reset is now stale.
    let stale = store
        .save(&state_at("2026-09-17"), &Schema::new(&name), before_reset)
        .await;
    assert!(
        stale.is_err(),
        "a save carrying a pre-reset version must be refused"
    );
    assert!(
        store.load().await.unwrap().state.resource_states.is_empty(),
        "the reset must hold — the discarded cursor must not come back"
    );
}

/// Two writers that read the same version: the second is refused rather than
/// overwriting the first.
///
/// This is airway's optimistic-concurrency contract, and until now it was dark
/// on oxy's path — the guard was attached as an `ON CONFLICT … WHERE` index
/// predicate, which Postgres ignores against a non-partial primary key, and the
/// re-read that was supposed to catch it compared against the value the same
/// statement had just written. The single-flight lease was the only thing
/// keeping two runs off one cursor.
#[tokio::test(flavor = "multi_thread")]
async fn a_second_writer_at_the_same_version_is_refused() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let db = Arc::new(db);
    let name = format!("occ_{}", Uuid::new_v4().simple());
    let workspace_id = Uuid::new_v4();
    let store = AirwayPgStateStore::new(Arc::clone(&db), workspace_id, name.clone());

    store
        .save(&state_at("2026-09-16"), &Schema::new(&name), 0)
        .await
        .expect("first save creates the row at version 1");
    // A second writer that read version 0 before the first landed.
    let stale = store
        .save(&state_at("2020-01-01"), &Schema::new(&name), 0)
        .await;
    assert!(
        stale.is_err(),
        "a save at a superseded version must be refused"
    );
    assert_eq!(
        high_water_of(&store.load().await.unwrap().state).as_deref(),
        Some("2026-09-16"),
        "the refused write must not have moved the cursor"
    );
}

/// Mid-rolling-deploy: a new pod adopts and diverges while the legacy row still
/// exists for whatever old pods remain. Neither side's writes reach the other.
#[tokio::test(flavor = "multi_thread")]
async fn adoption_is_per_workspace_during_a_rolling_deploy() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let db = Arc::new(db);
    let name = format!("rolling_{}", Uuid::new_v4().simple());
    seed_legacy_row(&db, &name, "2026-08-01", 12).await;

    let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
    let store_a = AirwayPgStateStore::new(Arc::clone(&db), a, name.clone());
    let store_b = AirwayPgStateStore::new(Arc::clone(&db), b, name.clone());

    // Both adopt the same starting point — the shared cursor is where each of
    // them genuinely left off, since until now they were the same row.
    assert_eq!(store_a.load().await.unwrap().version, 12);
    assert_eq!(store_b.load().await.unwrap().version, 12);

    store_a
        .save(&state_at("2026-09-16"), &Schema::new(&name), 12)
        .await
        .expect("workspace A advances its own row");

    assert_eq!(
        high_water_of(&store_b.load().await.unwrap().state).as_deref(),
        Some("2026-08-01"),
        "A's run must not move B's cursor once both have adopted"
    );
    // B can still save from the version it read: its row is untouched by A.
    store_b
        .save(&state_at("2026-08-02"), &Schema::new(&name), 12)
        .await
        .expect("workspace B advances from the version it read");

    // The legacy row is left exactly as it was — nothing writes it any more.
    let legacy = pipeline_state::Entity::find_by_id(name.clone())
        .one(db.as_ref())
        .await
        .unwrap()
        .expect("legacy row still present");
    assert_eq!(
        legacy.version, 12,
        "adoption reads the legacy row; it must never write it"
    );
}
