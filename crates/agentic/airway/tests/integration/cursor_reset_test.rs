//! Rewinding a cursor must not cost the rows.
//!
//! The shape that motivated this, measured on BMG's `amazon_vc` pipeline
//! (2026-09-21): four resources in one pipeline, one of which — `vendor_forecasting`,
//! 3,702,192 rows over eight daily pulls — is **append-only on purpose**. Amazon
//! serves only today's forecast, so each pull is a fact that cannot be re-fetched,
//! and comparing an old prediction against the PO Amazon later placed is the
//! resource's whole point. Backfilling `vendor_sales` by 180 days would have
//! meant Reset schema, which drops every table this pipeline owns. The backfill
//! was not run.
//!
//! So these cases pin the four things that make a cursor reset a different
//! button from a schema reset, not a politer name for it: the schema survives,
//! a sibling's cursor survives, the legacy shared row cannot smuggle the
//! discarded cursor back, and the version moves so an in-flight run cannot
//! write it back either.
//!
//! Requires Docker (or `OXY_DATABASE_URL`).

use std::collections::HashMap;
use std::sync::Arc;

use agentic_airway::AirwayPgStateStore;
use agentic_airway::extension::{pipeline_state, workspace_pipeline_state};
use agentic_airway::reset::{CursorScope, clear_pipeline_cursors, stored_resource_cursors};
use airway::Schema;
use airway::schema::Table;
use airway::state::{PipelineState, ResourceState, StateStore};
use airway::types::WriteDisposition;
use sea_orm::{ActiveValue, DatabaseConnection, EntityTrait};
use uuid::Uuid;

use crate::harness::test_db;

const CONNECTOR_STATE: &str = "__connector_state";

/// A cursor per named resource, each distinguishable from the others.
fn state_with(cursors: &[(&str, &str)]) -> PipelineState {
    let mut state = PipelineState::default();
    for (resource, high_water) in cursors {
        state.resource_states.insert(
            (*resource).to_string(),
            ResourceState {
                incremental: None,
                custom: HashMap::from([(
                    CONNECTOR_STATE.to_string(),
                    serde_json::json!({ "high_water": high_water }),
                )]),
            },
        );
    }
    state
}

fn high_water_of(state: &PipelineState, resource: &str) -> Option<String> {
    state
        .resource_states
        .get(resource)?
        .custom
        .get(CONNECTOR_STATE)?
        .get("high_water")?
        .as_str()
        .map(str::to_string)
}

/// `amazon_vc`'s shape: a merge-keyed resource beside an append-only one.
fn amazon_vc_schema(name: &str) -> Schema {
    let mut schema = Schema::new(name);
    let mut sales = Table::new("vendor_sales");
    sales.write_disposition = WriteDisposition::Merge;
    let mut asin = airway::schema::Column::new("asin", airway::types::DataType::Text);
    asin.primary_key = true;
    sales.columns.insert("asin".to_string(), asin);
    schema.tables.insert("vendor_sales".to_string(), sales);

    let mut forecasting = Table::new("vendor_forecasting");
    forecasting.write_disposition = WriteDisposition::Append;
    schema
        .tables
        .insert("vendor_forecasting".to_string(), forecasting);
    schema
}

/// Seed this workspace's row through the store, as a real run would.
async fn seed(
    db: &Arc<DatabaseConnection>,
    workspace_id: Uuid,
    name: &str,
    cursors: &[(&str, &str)],
) -> AirwayPgStateStore {
    let store = AirwayPgStateStore::new(Arc::clone(db), workspace_id, name);
    store
        .save(&state_with(cursors), &amazon_vc_schema(name), 0)
        .await
        .expect("seed the workspace state row");
    store
}

/// **The reason this exists.** Clearing a cursor must leave the stored schema
/// exactly where it was — the schema is what the executor's drop path reads to
/// decide which destination tables to destroy, so a cursor reset that
/// tombstones it has quietly armed the very thing it was built to avoid.
#[tokio::test(flavor = "multi_thread")]
async fn cursor_reset_preserves_the_stored_schema() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let db = Arc::new(db);
    let workspace_id = Uuid::new_v4();
    let name = format!("amazon_vc_{}", Uuid::new_v4().simple());
    let store = seed(&db, workspace_id, &name, &[("vendor_sales", "2026-09-20")]).await;

    let before = store.load().await.unwrap();
    let before_tables = before.schema.as_ref().expect("seeded schema").tables.len();

    clear_pipeline_cursors(&db, workspace_id, &name, &CursorScope::AllResources)
        .await
        .expect("clearing cursors must succeed");

    let after = store.load().await.unwrap();
    let after_schema = after
        .schema
        .as_ref()
        .expect("a cursor reset must NOT tombstone the schema — that is Reset schema's job");
    assert_eq!(
        after_schema.tables.len(),
        before_tables,
        "the stored schema must survive a cursor reset intact"
    );
    assert!(
        after_schema.tables.contains_key("vendor_forecasting"),
        "the append-only table must still be described by the stored schema"
    );
    assert!(
        after.state.resource_states.is_empty(),
        "…while the cursors are gone: {:?}",
        after.state.resource_states
    );
}

/// Per-resource granularity: rewinding `vendor_sales` must not disturb
/// `vendor_forecasting`'s position. Without this, "reset one resource" is a
/// pipeline-wide re-pull wearing a narrower label — which on `amazon_vc` means
/// eight days of forecasts pulled again into an append table.
#[tokio::test(flavor = "multi_thread")]
async fn per_resource_reset_leaves_sibling_cursors_alone() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let db = Arc::new(db);
    let workspace_id = Uuid::new_v4();
    let name = format!("amazon_vc_{}", Uuid::new_v4().simple());
    let store = seed(
        &db,
        workspace_id,
        &name,
        &[
            ("vendor_sales", "2026-09-20"),
            ("vendor_forecasting", "2026-09-21"),
            ("vendor_inventory", "2026-09-19"),
        ],
    )
    .await;

    let cleared = clear_pipeline_cursors(
        &db,
        workspace_id,
        &name,
        &CursorScope::Resources(vec!["vendor_sales".into()]),
    )
    .await
    .expect("clearing one resource's cursor must succeed");
    assert_eq!(cleared.cleared, vec!["vendor_sales".to_string()]);

    let after = store.load().await.unwrap().state;
    assert_eq!(
        high_water_of(&after, "vendor_sales"),
        None,
        "the named resource must rewind to its `default_start`"
    );
    assert_eq!(
        high_water_of(&after, "vendor_forecasting").as_deref(),
        Some("2026-09-21"),
        "an append-only sibling must keep its cursor — re-pulling it duplicates"
    );
    assert_eq!(
        high_water_of(&after, "vendor_inventory").as_deref(),
        Some("2026-09-19"),
        "every unnamed sibling keeps its cursor"
    );
}

/// A workspace that has never run this pipeline has **no row**, yet it does
/// have a cursor: its first load adopts the legacy name-keyed
/// `airway_pipeline_state`. A reset that writes straight to
/// `airway_workspace_pipeline_state` matches nothing, reports success, and the
/// next run resumes from exactly the cursor the operator believed they had
/// discarded. The same trap `clear_pipeline_state` writes a tombstone to avoid.
#[tokio::test(flavor = "multi_thread")]
async fn cursor_reset_defeats_the_legacy_row_it_would_otherwise_adopt() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let db = Arc::new(db);
    let workspace_id = Uuid::new_v4();
    let name = format!("amazon_vc_{}", Uuid::new_v4().simple());

    // The shape a pre-deploy pod left behind: a name-keyed row, no workspace row.
    let legacy = pipeline_state::ActiveModel {
        pipeline_name: ActiveValue::Set(name.clone()),
        state: ActiveValue::Set(
            serde_json::to_value(state_with(&[("vendor_sales", "2026-09-20")])).unwrap(),
        ),
        schema_json: ActiveValue::Set(serde_json::to_value(amazon_vc_schema(&name)).unwrap()),
        version: ActiveValue::Set(7),
        updated_at: ActiveValue::Set(chrono::Utc::now()),
    };
    pipeline_state::Entity::insert(legacy)
        .exec(db.as_ref())
        .await
        .expect("seed the legacy airway_pipeline_state row");

    // Straight to the reset — deliberately NOT reading the state first. A read
    // is what performs the adoption, so a test that reads first would pass
    // against an implementation that writes blind, which is the bug.
    let cleared = clear_pipeline_cursors(&db, workspace_id, &name, &CursorScope::AllResources)
        .await
        .expect("clearing cursors must succeed");
    assert_eq!(
        cleared.cleared,
        vec!["vendor_sales".to_string()],
        "the reset must see, and report, the cursor this workspace would have \
         resumed from — not the empty state of its own absent row"
    );

    // …and a later load must not get it back.
    let store = AirwayPgStateStore::new(Arc::clone(&db), workspace_id, name.clone());
    let after = store.load().await.unwrap();
    assert_eq!(
        high_water_of(&after.state, "vendor_sales"),
        None,
        "the legacy row must not hand the discarded cursor back on the next load"
    );
    assert!(
        after.schema.is_some(),
        "…and the adopted schema still survives the cursor reset"
    );
}

/// A reset takes no pipeline lease, so a run can be mid-flight with a
/// `state_version` read before the reset. The version bump is what stops its
/// `save` — `WHERE version = $expected` matches nothing — so the cursor it was
/// about to persist is dropped rather than restored.
#[tokio::test(flavor = "multi_thread")]
async fn cursor_reset_bumps_the_version_so_an_in_flight_run_cannot_write_it_back() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let db = Arc::new(db);
    let workspace_id = Uuid::new_v4();
    let name = format!("amazon_vc_{}", Uuid::new_v4().simple());
    let store = seed(&db, workspace_id, &name, &[("vendor_sales", "2026-09-20")]).await;

    // A run loads, and is now holding this version.
    let in_flight = store.load().await.unwrap();

    clear_pipeline_cursors(&db, workspace_id, &name, &CursorScope::AllResources)
        .await
        .expect("clearing cursors must succeed");

    let err = store
        .save(
            &state_with(&[("vendor_sales", "2026-09-21")]),
            &amazon_vc_schema(&name),
            in_flight.version,
        )
        .await
        .expect_err("a run holding the pre-reset version must not be able to save");
    assert!(
        err.to_string().contains("optimistic concurrency conflict"),
        "{err}"
    );
    assert_eq!(
        high_water_of(&store.load().await.unwrap().state, "vendor_sales"),
        None,
        "the reset must stick"
    );
}

/// Naming a resource that holds no cursor is not an error — it never ran, so
/// there is nothing to rewind — but it is reported, because at the call site a
/// typo and a never-run resource look identical.
#[tokio::test(flavor = "multi_thread")]
async fn clearing_an_absent_cursor_reports_it_rather_than_failing() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let db = Arc::new(db);
    let workspace_id = Uuid::new_v4();
    let name = format!("amazon_vc_{}", Uuid::new_v4().simple());
    let store = seed(&db, workspace_id, &name, &[("vendor_sales", "2026-09-20")]).await;
    let before = store.load().await.unwrap().version;

    let cleared = clear_pipeline_cursors(
        &db,
        workspace_id,
        &name,
        &CursorScope::Resources(vec!["vendor_sails".into()]),
    )
    .await
    .expect("naming a resource with no cursor is a no-op, not an error");
    assert!(cleared.cleared.is_empty(), "{cleared:?}");
    assert_eq!(cleared.not_held, vec!["vendor_sails".to_string()]);
    assert_eq!(
        store.load().await.unwrap().version,
        before,
        "a no-op must not bump the version — that would fail a legitimate \
         in-flight run's save for nothing"
    );
}

/// Listing what a reset could target has the same blind spot, and the same
/// fix: a workspace with no row of its own still holds the legacy cursor, so
/// an operator choosing a resource must be shown it.
#[tokio::test(flavor = "multi_thread")]
async fn stored_resource_cursors_sees_the_legacy_cursor() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let db = Arc::new(db);
    let workspace_id = Uuid::new_v4();
    let name = format!("amazon_vc_{}", Uuid::new_v4().simple());
    let legacy = pipeline_state::ActiveModel {
        pipeline_name: ActiveValue::Set(name.clone()),
        state: ActiveValue::Set(
            serde_json::to_value(state_with(&[("vendor_sales", "2026-09-20")])).unwrap(),
        ),
        schema_json: ActiveValue::Set(serde_json::to_value(amazon_vc_schema(&name)).unwrap()),
        version: ActiveValue::Set(7),
        updated_at: ActiveValue::Set(chrono::Utc::now()),
    };
    pipeline_state::Entity::insert(legacy)
        .exec(db.as_ref())
        .await
        .expect("seed the legacy airway_pipeline_state row");

    let held = stored_resource_cursors(&db, workspace_id, &name)
        .await
        .expect("listing cursors must succeed");
    assert_eq!(held, vec!["vendor_sales".to_string()]);
}

/// …but seeing it must not *take* it. Adoption copies the legacy row into
/// this workspace's, and mid-deploy that copy is the store's one documented
/// cost: an old pod that runs afterwards advances the legacy row, the adopted
/// copy never sees that progress, and the window is re-read as duplicate rows.
/// A listing serves a picker an operator opens just to look; opening it must
/// not be what spends that.
#[tokio::test(flavor = "multi_thread")]
async fn listing_cursors_does_not_adopt_the_legacy_row() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let db = Arc::new(db);
    let workspace_id = Uuid::new_v4();
    let name = format!("amazon_vc_{}", Uuid::new_v4().simple());
    let legacy = pipeline_state::ActiveModel {
        pipeline_name: ActiveValue::Set(name.clone()),
        state: ActiveValue::Set(
            serde_json::to_value(state_with(&[("vendor_sales", "2026-09-20")])).unwrap(),
        ),
        schema_json: ActiveValue::Set(serde_json::to_value(amazon_vc_schema(&name)).unwrap()),
        version: ActiveValue::Set(7),
        updated_at: ActiveValue::Set(chrono::Utc::now()),
    };
    pipeline_state::Entity::insert(legacy)
        .exec(db.as_ref())
        .await
        .expect("seed the legacy airway_pipeline_state row");

    let held = stored_resource_cursors(&db, workspace_id, &name)
        .await
        .expect("listing cursors must succeed");
    assert_eq!(
        held,
        vec!["vendor_sales".to_string()],
        "the legacy cursor is still shown"
    );

    let adopted = workspace_pipeline_state::Entity::find_by_id((workspace_id, name.clone()))
        .one(db.as_ref())
        .await
        .unwrap();
    assert!(
        adopted.is_none(),
        "listing must leave this workspace with no state row of its own: {adopted:?}"
    );
}

/// A workspace whose own row is a tombstone (a schema reset) holds no cursor,
/// and the legacy row behind it must not show through: the load that matters
/// never re-adopts over a real row.
#[tokio::test(flavor = "multi_thread")]
async fn listing_cursors_prefers_the_workspace_row_over_the_legacy_one() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let db = Arc::new(db);
    let workspace_id = Uuid::new_v4();
    let name = format!("amazon_vc_{}", Uuid::new_v4().simple());
    let legacy = pipeline_state::ActiveModel {
        pipeline_name: ActiveValue::Set(name.clone()),
        state: ActiveValue::Set(
            serde_json::to_value(state_with(&[("vendor_sales", "2026-09-20")])).unwrap(),
        ),
        schema_json: ActiveValue::Set(serde_json::to_value(amazon_vc_schema(&name)).unwrap()),
        version: ActiveValue::Set(7),
        updated_at: ActiveValue::Set(chrono::Utc::now()),
    };
    pipeline_state::Entity::insert(legacy)
        .exec(db.as_ref())
        .await
        .expect("seed the legacy airway_pipeline_state row");
    agentic_airway::reset::clear_pipeline_state(&db, workspace_id, &name)
        .await
        .expect("tombstone this workspace's row");

    let held = stored_resource_cursors(&db, workspace_id, &name)
        .await
        .expect("listing cursors must succeed");
    assert!(
        held.is_empty(),
        "a tombstoned workspace holds nothing: {held:?}"
    );
}

/// A pipeline that has never run anywhere: no workspace row, no legacy row.
/// Nothing to clear, and nothing to fail on.
#[tokio::test(flavor = "multi_thread")]
async fn clearing_cursors_for_a_pipeline_that_never_ran_is_a_no_op() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let db = Arc::new(db);
    let workspace_id = Uuid::new_v4();
    let name = format!("never_ran_{}", Uuid::new_v4().simple());

    let cleared = clear_pipeline_cursors(&db, workspace_id, &name, &CursorScope::AllResources)
        .await
        .expect("a never-provisioned pipeline must not error");
    assert_eq!(cleared, agentic_airway::reset::ClearedCursors::default());
}
