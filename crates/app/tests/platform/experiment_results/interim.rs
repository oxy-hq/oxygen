//! The interim reading of a run in progress: the registered estimator over the days that
//! have finished, written to `experiment_interims` and nowhere else.

use std::sync::Arc;

use agentic_core::delegation::{TaskAssignment, TaskOutcome, TaskSpec};
use agentic_runtime::worker::TaskExecutor;
use chrono::{DateTime, Duration, NaiveDate, Utc};
use entity::{experiment_interims, experiment_runs};
use oxy_app::server::experiment::estimate::EstimateSettings;
use oxy_app::server::experiment::interim::read::{InterimError, read_interim, request_interim};
use oxy_app::server::experiment::interim::store::{
    INTERIM_EVERY_SECS, INTERIM_KIND, Reading, enqueue_interim, finish, interim_of,
};
use oxy_app::server::experiment::interim::{ExperimentInterimExecutor, interim_reading};
use oxy_experiment::report::{ReportStatus, ReportView};
use oxy_experiment::timeline::day_start_utc;
use sea_orm::{DatabaseConnection, EntityTrait};
use uuid::Uuid;

use super::fixtures::*;
use super::harness::*;

const SETTINGS: EstimateSettings = EstimateSettings {
    placebo_iterations: 60,
};

/// Every schema: the enqueue writes `agentic_runs` and the task queue, which the central
/// schema alone lacks.
async fn setup_db_with_queue() -> DatabaseConnection {
    let (db, _url) = crate::common::fresh_db(crate::common::Schema::All).await;
    db
}

fn sydney(y: i32, m: u32, d: u32) -> DateTime<Utc> {
    day_start_utc(
        NaiveDate::from_ymd_opt(y, m, d).unwrap(),
        chrono_tz::Australia::Sydney,
    ) + Duration::hours(10)
}

async fn workspace_of(db: &DatabaseConnection, run: Uuid) -> Uuid {
    experiment_runs::Entity::find_by_id(run)
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .workspace_id
}

async fn run_row(db: &DatabaseConnection, run: Uuid) -> experiment_runs::Model {
    experiment_runs::Entity::find_by_id(run)
        .one(db)
        .await
        .unwrap()
        .unwrap()
}

fn report(r: &Reading) -> ReportView {
    serde_json::from_value(r.report.clone()).expect("an interim report reads back")
}

/// Enqueue as the route does, then run the queued task through the executor as the worker would.
async fn run_queued_interim(db: &DatabaseConnection, run: Uuid) -> TaskOutcome {
    let row = run_row(db, run).await;
    assert!(
        enqueue_interim(db, &row, Utc::now(), Duration::zero())
            .await
            .unwrap()
    );
    let task = interim_of(db, run).await.unwrap().unwrap().task_id.unwrap();
    let executor = ExperimentInterimExecutor {
        db: db.clone(),
        panels: Arc::new(FixturePanels),
        settings: SETTINGS,
    };
    let mut running = executor
        .execute(TaskAssignment {
            task_id: task.to_string(),
            parent_task_id: None,
            run_id: task.to_string(),
            spec: TaskSpec::Custom {
                kind: INTERIM_KIND.to_string(),
                payload: serde_json::json!({ "run_id": run }),
            },
            policy: None,
            claim_epoch: agentic_core::NO_CLAIM,
        })
        .await
        .expect("the executor accepted the assignment");
    running.outcomes.recv().await.expect("an outcome")
}

#[tokio::test]
async fn an_interim_reads_a_live_run_and_seals_nothing_writes_back_nothing_and_moves_nothing() {
    let db = setup_db_with_queue().await;
    let run = seed(&db, "live", VIEW).await;
    let before = run_row(&db, run).await;

    let outcome = run_queued_interim(&db, run).await;
    assert!(is_done(&outcome), "{outcome:?}");

    let row = interim_of(&db, run).await.unwrap().expect("an interim row");
    assert!(row.task_id.is_none(), "the job cleared its mark");
    assert!(row.error.is_none(), "{:?}", row.error);
    let report: ReportView = serde_json::from_value(row.report.unwrap()).unwrap();
    assert!(matches!(report.status, ReportStatus::Interim { .. }));
    assert!(report.headline.is_some(), "{:?}", report.refusal);
    assert!(report.write_back.is_none() && report.treated_total.is_none());
    // Nothing sealed, and the run is exactly where it was.
    assert!(result_of(&db, run).await.is_none());
    let after = run_row(&db, run).await;
    assert_eq!(
        (after.state, after.estimate_task_id, after.next_check_at),
        (before.state, before.estimate_task_id, before.next_check_at)
    );
}

#[tokio::test]
async fn mid_run_the_interim_reads_only_the_finished_post_days() {
    let db = setup_db_with_queue().await;
    let run = seed(&db, "live", VIEW).await;
    // The wave switched 2026-03-02; washout 7 → the post window opens 03-09. On 03-25 the
    // days 03-09 .. 03-24 have finished.
    let r = interim_reading(&db, &FixturePanels, SETTINGS, run, sydney(2026, 3, 25))
        .await
        .expect("a reading");
    assert_eq!(r.through, NaiveDate::from_ymd_opt(2026, 3, 24).unwrap());
    let v = report(&r);
    assert_eq!(v.status, ReportStatus::Interim { through: r.through });
    let h = v.headline.expect("an estimate over sixteen post days");
    assert!(h.estimate > 0.0, "the planted lift shows: {}", h.estimate);
    let days = r.series.as_ref().unwrap()["days"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(
        days.last().unwrap()["date"],
        "2026-03-24",
        "no day after the last finished one is read"
    );
}

#[tokio::test]
async fn before_a_whole_post_day_has_finished_the_interim_says_when_it_can_read() {
    let db = setup_db_with_queue().await;
    let run = seed(&db, "live", VIEW).await;
    let why = interim_reading(&db, &FixturePanels, SETTINGS, run, sydney(2026, 3, 5))
        .await
        .unwrap_err();
    assert!(why.contains("2026-03-10"), "{why}");
}

#[tokio::test]
async fn only_a_live_or_closing_run_has_an_interim() {
    let db = setup_db_with_queue().await;
    for state in ["registered", "reported", "aborted"] {
        let run = seed(&db, state, VIEW).await;
        let ws = workspace_of(&db, run).await;
        assert!(
            matches!(
                read_interim(&db, ws, NAME, Utc::now()).await,
                Err(InterimError::NotInProgress { .. })
            ),
            "{state}"
        );
        assert!(
            matches!(
                request_interim(&db, ws, NAME, Utc::now()).await,
                Err(InterimError::NotInProgress { .. })
            ),
            "{state}"
        );
        assert!(
            interim_reading(&db, &FixturePanels, SETTINGS, run, Utc::now())
                .await
                .is_err(),
            "{state}"
        );
    }
}

#[tokio::test]
async fn one_job_at_a_time_and_only_the_job_the_row_names_writes_it() {
    let db = setup_db_with_queue().await;
    let run = seed(&db, "live", VIEW).await;
    let ws = workspace_of(&db, run).await;
    let row = run_row(&db, run).await;
    let now = Utc::now();
    let day = Duration::seconds(INTERIM_EVERY_SECS);

    let first = request_interim(&db, ws, NAME, now).await.unwrap();
    assert!(first.running && first.reading.is_none());
    assert!(
        !enqueue_interim(&db, &row, now, Duration::zero())
            .await
            .unwrap(),
        "a second request while one is in flight enqueues nothing"
    );
    let task = interim_of(&db, run)
        .await
        .unwrap()
        .unwrap()
        .task_id
        .unwrap();

    let failed: Result<Reading, String> = Err("no history".into());
    assert!(
        !finish(&db, run, Uuid::new_v4(), &failed, now)
            .await
            .unwrap(),
        "a job the row no longer names writes nothing"
    );
    assert!(finish(&db, run, task, &failed, now).await.unwrap());
    let r = read_interim(&db, ws, NAME, now).await.unwrap();
    assert!(!r.running);
    assert_eq!(r.error.as_deref(), Some("no history"));

    // On demand runs again at once; the schedule waits a day from the last request.
    assert!(!enqueue_interim(&db, &row, now, day).await.unwrap());
    assert!(enqueue_interim(&db, &row, now + day, day).await.unwrap());
}

#[tokio::test]
async fn a_failed_attempt_keeps_the_previous_reading() {
    let db = setup_db_with_queue().await;
    let run = seed(&db, "closing", VIEW).await;
    assert!(is_done(&run_queued_interim(&db, run).await));
    let row = run_row(&db, run).await;
    let now = Utc::now();
    assert!(
        enqueue_interim(&db, &row, now, Duration::zero())
            .await
            .unwrap()
    );
    let task = interim_of(&db, run)
        .await
        .unwrap()
        .unwrap()
        .task_id
        .unwrap();
    finish(&db, run, task, &Err("warehouse down".into()), now)
        .await
        .unwrap();
    let r = read_interim(&db, row.workspace_id, NAME, now)
        .await
        .unwrap();
    assert_eq!(r.error.as_deref(), Some("warehouse down"));
    assert!(r.reading.is_some(), "the earlier reading is still shown");
    let stored = experiment_interims::Entity::find_by_id(run)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert!(stored.report.is_some());
}
