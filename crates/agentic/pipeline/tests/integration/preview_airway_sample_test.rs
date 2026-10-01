//! I3 — a workspace preview's Airway sample leaves production's cursor, stored
//! schema, lease and load audit untouched — and I4's executor half: a
//! QuickBooks sample never asks for a production credential.
//!
//! Driven as `agentic-airway`'s `worker_integration` does: a filesystem source
//! (a temp JSONL file) into the in-process memory destination, against a real
//! Postgres. The one windowed-source case runs Toast against a local stand-in
//! (`toast_stub`); nothing here reaches a real source.
//!
//! Requires Docker (or `OXY_DATABASE_URL`); self-skips otherwise.

mod fences;
mod fixture;
mod toast_stub;

use agentic_airway::extension::pipeline_lease::{self, LeaseAcquisition};
use agentic_core::delegation::TaskOutcome;
use agentic_pipeline::PREVIEW_AIRWAY_SAMPLE;
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use crate::airway_run_test::test_db;
use fixture::{
    KEY, REF, SamplePlatform, audit_rows, drive, executor, preview_name, row_text, run_production,
    run_sample, sample, scope, seed_run, state_rows, users,
};

pub(super) fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", Uuid::new_v4().simple())
}

pub(super) fn is_done(outcome: &TaskOutcome) -> bool {
    matches!(outcome, TaskOutcome::Done { .. })
}

#[tokio::test(flavor = "multi_thread")]
async fn production_state_row_is_byte_identical_after_a_sample() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let ws = Uuid::new_v4();
    let name = unique("it_sample_prod");
    let (_dir, yaml) = users(&name);
    assert!(
        is_done(&run_production(&db, ws, &yaml).await),
        "production ran"
    );
    let state = state_rows(&db, ws, &name).await;
    let audit = audit_rows(&db, ws, &name).await;
    assert_eq!(
        (state.len(), audit.len()),
        (1, 1),
        "precondition: production's rows"
    );

    let (_, outcome, _) = run_sample(&db, ws, &yaml).await;
    assert!(is_done(&outcome), "{outcome:?}");

    assert_eq!(
        state_rows(&db, ws, &name).await,
        state,
        "production's cursor, stored schema, version and updated_at, byte for byte"
    );
    assert_eq!(
        audit_rows(&db, ws, &name).await,
        audit,
        "production's load audit"
    );
    assert_eq!(
        state_rows(&db, ws, &preview_name(&name)).await.len(),
        1,
        "the sample kept state of its own"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn production_lease_is_acquirable_while_a_sample_runs() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let ws = Uuid::new_v4();
    let name = unique("it_sample_lease");
    let (_dir, yaml) = users(&name);

    // A live production run holds production's lease: the sample still runs.
    let production = Uuid::new_v4().to_string();
    seed_run(&db, &production, "airway", ws).await;
    let taken = pipeline_lease::try_acquire(&db, ws, &name, &production, 3600)
        .await
        .unwrap();
    assert_eq!(taken, LeaseAcquisition::Acquired);
    let (_, outcome, _) = run_sample(&db, ws, &yaml).await;
    assert!(
        is_done(&outcome),
        "not deferred behind production: {outcome:?}"
    );
    pipeline_lease::release_by_run(&db, &production)
        .await
        .unwrap();

    // A live sample holds the preview's lease: production takes its own.
    let holder = Uuid::new_v4().to_string();
    seed_run(&db, &holder, PREVIEW_AIRWAY_SAMPLE, ws).await;
    let held = pipeline_lease::try_acquire(&db, ws, &preview_name(&name), &holder, 3600).await;
    assert_eq!(held.unwrap(), LeaseAcquisition::Acquired);
    let next = Uuid::new_v4().to_string();
    seed_run(&db, &next, "airway", ws).await;
    assert_eq!(
        pipeline_lease::try_acquire(&db, ws, &name, &next, 3600)
            .await
            .unwrap(),
        LeaseAcquisition::Acquired,
        "production is never blocked by a sample"
    );

    // …and a second sample of the same pipeline in the same preview waits.
    let second = Uuid::new_v4().to_string();
    seed_run(&db, &second, PREVIEW_AIRWAY_SAMPLE, ws).await;
    let platform = SamplePlatform::new(ws, Some(scope(&second)), yaml.clone());
    let task = executor(&db, platform)
        .execute_airway_preview_sample(&second, REF, &sample())
        .await
        .expect("dispatch");
    match drive(task).await.0 {
        TaskOutcome::Deferred { reason, .. } => {
            assert!(reason.contains(&preview_name(&name)), "{reason}")
        }
        other => panic!("two samples of one pipeline serialize on the preview key: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn sample_audit_rows_carry_the_preview_name() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let ws = Uuid::new_v4();
    let name = unique("it_sample_audit");
    let (_dir, yaml) = users(&name);
    let (run_id, outcome, _) = run_sample(&db, ws, &yaml).await;
    assert!(is_done(&outcome), "{outcome:?}");

    let audit = audit_rows(&db, ws, &preview_name(&name)).await;
    assert_eq!(audit.len(), 1, "{audit:?}");
    assert!(audit[0].contains("\"status\":\"completed\""), "{audit:?}");
    assert!(
        audit_rows(&db, ws, &name).await.is_empty(),
        "nothing under production's name"
    );

    let ext = agentic_airway::extension::run_extension::get_run_extension(&db, &run_id)
        .await
        .unwrap()
        .expect("the sample's run extension");
    assert_eq!(ext.pipeline_name, preview_name(&name));
    assert_eq!(ext.pipeline_ref, Some(format!("preview:{KEY}:{REF}")));
    assert!(ext.load_id.is_some(), "the worker stamped its load");
}

#[tokio::test(flavor = "multi_thread")]
async fn sample_starts_from_an_empty_cursor() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };
    let name = unique("it_sample_cursor");
    let (_dir, yaml) = users(&name);
    // A real cursor and schema for the name, in the legacy shared table that
    // a first run in a workspace adopts.
    let elsewhere = Uuid::new_v4();
    assert!(is_done(&run_production(&db, elsewhere, &yaml).await));
    fixture_exec(
        &db,
        "INSERT INTO airway_pipeline_state (pipeline_name, state, schema_json, version, updated_at) \
         SELECT pipeline_name, state, schema_json, 41, now() FROM airway_workspace_pipeline_state \
         WHERE workspace_id = $1 AND pipeline_name = $2",
        vec![elsewhere.into(), name.clone().into()],
    )
    .await;
    let legacy = legacy_rows(&db, &name).await;

    let ws = Uuid::new_v4();
    let (_, outcome, events) = run_sample(&db, ws, &yaml).await;
    assert!(is_done(&outcome), "{outcome:?}");
    let version = row_text(
        &db,
        "SELECT version::text AS t FROM airway_workspace_pipeline_state \
         WHERE workspace_id = $1 AND pipeline_name = $2",
        vec![ws.into(), preview_name(&name).into()],
    )
    .await;
    assert_eq!(
        version,
        vec!["1".to_string()],
        "saved once from an empty snapshot (version 0), not adopted at 41"
    );
    let extracted: u64 = events
        .iter()
        .filter(|(t, _)| t == "extract_completed")
        .filter_map(|(_, p)| p["rows_extracted"].as_u64())
        .sum();
    assert_eq!(extracted, 3, "every row read from the start");
    assert!(
        state_rows(&db, ws, &name).await.is_empty(),
        "nothing adopted under production's name"
    );
    assert_eq!(
        legacy_rows(&db, &name).await,
        legacy,
        "the legacy row untouched"
    );
}

async fn fixture_exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    use sea_orm::ConnectionTrait;
    db.execute_raw(sea_orm::Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .await
    .expect(sql);
}

async fn legacy_rows(db: &DatabaseConnection, name: &str) -> Vec<String> {
    row_text(
        db,
        "SELECT row_to_json(s)::text AS t FROM airway_pipeline_state s WHERE pipeline_name = $1",
        vec![name.into()],
    )
    .await
}
