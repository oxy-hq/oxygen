//! The executor through PR 4's tick: a run that reaches `closing` is enqueued an
//! estimate, the executor runs the task exactly as it was queued, and the next tick
//! reports the run.

use std::sync::Arc;

use agentic_core::delegation::{TaskOutcome, TaskSpec};
use async_trait::async_trait;
use chrono::Days;
use oxy_app::server::experiment::panel::{PanelContext, PanelOutcome, PanelRequest};
use oxy_app::server::experiment::results_api::read_result;
use oxy_app::server::experiment::source::PanelSource;
use oxy_experiment::timeline::{day_start_utc, last_post_window_end_date};
use oxy_shared::errors::OxyError;
use sea_orm::{DatabaseBackend, FromQueryResult, Statement};
use serde_json::json;

use super::harness::{execute, is_done, report_of, result_of, synthetic};
use crate::experiment_runner::fixture::{DESIGN, Fx, NAME, switch_at};

/// The design's four treated stores and the two pinned controls; no lift.
pub(super) struct StorePanels;

#[async_trait]
impl PanelSource for StorePanels {
    async fn load(&self, _c: &PanelContext, req: &PanelRequest) -> Result<PanelOutcome, OxyError> {
        let units: Vec<String> = [
            "bondi",
            "newtown",
            "manly",
            "surry_hills",
            "glebe",
            "paddington",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(req.measure, "store_days.beverages_revenue");
        Ok(PanelOutcome::Panel(synthetic(
            &units, 0, 150.0, 0.0, 1.0, 5,
        )))
    }
}

#[derive(FromQueryResult)]
struct Queued {
    spec: serde_json::Value,
}

#[tokio::test]
async fn a_closed_run_is_estimated_from_its_queued_task_and_the_next_tick_reports_it() {
    let fx = Fx::seed(DESIGN).await;
    fx.register().await;
    fx.tick(switch_at("2026-03-02")).await;
    fx.pin(&["glebe", "paddington"], None).await;
    let end = last_post_window_end_date(&fx.design().await).expect("an end");
    fx.exec(
        "UPDATE experiment_runs SET state = 'closing', closing_since = $1, observations = $2 \
         WHERE workspace_id = $3",
        [end.into(), json!({ "coverage": 0.95 }).into(), fx.ws.into()],
    )
    .await;
    let sydney = |d: chrono::NaiveDate| day_start_utc(d, chrono_tz::Australia::Sydney);
    fx.tick(sydney(end + Days::new(1))).await;
    let run = fx.run().await;
    let task = run.estimate_task_id.expect("closing enqueued the estimate");

    // The spec exactly as PR 4 queued it: this pins one payload shape across the two PRs.
    let queued = Queued::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT spec FROM agentic_task_queue WHERE task_id = $1",
        [task.to_string().into()],
    ))
    .one(&fx.db)
    .await
    .unwrap()
    .expect("a queued task");
    let spec: TaskSpec = serde_json::from_value(queued.spec).expect("a task spec");
    let outcome = execute(&fx.db, &task.to_string(), spec, Arc::new(StorePanels)).await;
    assert!(is_done(&outcome), "{outcome:?}");
    // The answer lands in `agentic_runs.answer`, which is readable before `reported`:
    // it must not carry the effect.
    let TaskOutcome::Done { answer, .. } = &outcome else {
        unreachable!()
    };
    assert_eq!(answer, "estimated");
    let row = result_of(&fx.db, run.id)
        .await
        .expect("the result was written");
    assert!(!row.aborted);
    assert!(
        report_of(&row).headline.is_some(),
        "{:?}",
        report_of(&row).refusal
    );

    // The worker records the executor's Done on the agentic run; the next tick reads it.
    fx.exec(
        "UPDATE agentic_runs SET task_status = 'done' WHERE id = $1",
        [task.to_string().into()],
    )
    .await;
    assert_eq!(fx.run().await.state, "closing");
    fx.tick(sydney(end + Days::new(2))).await;
    assert_eq!(fx.run().await.state, "reported");
    // Reported: the sealed result is readable, with the report.
    let r = read_result(&fx.db, fx.ws, NAME)
        .await
        .expect("a reported run's result is readable");
    assert!(r.report.get("headline").is_some_and(|h| !h.is_null()));
    assert!(
        r.series.as_ref().is_some_and(|s| s["design"] == "waves"),
        "the result route returns the sealed series: {:?}",
        r.series
    );
}

#[tokio::test]
async fn a_live_run_is_enqueued_one_interim_reading_a_day() {
    use oxy_app::server::experiment::interim::store::{INTERIM_KIND, interim_of};

    #[derive(FromQueryResult)]
    struct Count {
        n: i64,
    }
    let count = |db: &sea_orm::DatabaseConnection| {
        let db = db.clone();
        async move {
            Count::find_by_statement(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT COUNT(*) AS n FROM agentic_runs WHERE source_type = $1",
                [INTERIM_KIND.into()],
            ))
            .one(&db)
            .await
            .unwrap()
            .map_or(0, |c| c.n)
        }
    };
    let fx = Fx::seed(DESIGN).await;
    fx.register().await;
    let switch = switch_at("2026-03-02");
    fx.tick(switch).await;
    let run = fx.run().await;
    assert_eq!(run.state, "live");
    let row = interim_of(&fx.db, run.id)
        .await
        .unwrap()
        .expect("scheduled");
    assert!(row.task_id.is_some());
    assert_eq!(count(&fx.db).await, 1);

    // The next tick the same day enqueues nothing more, even once the job has ended.
    fx.exec(
        "UPDATE experiment_interims SET task_id = NULL WHERE run_id = $1",
        [run.id.into()],
    )
    .await;
    fx.tick(switch + chrono::Duration::hours(2)).await;
    assert_eq!(count(&fx.db).await, 1);
    // A day later it does.
    fx.tick(switch + chrono::Duration::days(1) + chrono::Duration::minutes(1))
        .await;
    assert_eq!(count(&fx.db).await, 2);
}
