//! The route's one job: the effect is unreadable until the run is reported.

use axum::http::StatusCode;
use axum::response::IntoResponse;
use oxy_app::server::experiment::results_api::{ResultError, read_result};
use serde_json::{Value, json};
use uuid::Uuid;

use super::fixtures::*;

const NAME: &str = "pilot";

async fn body_of(e: ResultError) -> (StatusCode, Value) {
    let resp = e.into_response();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn seeded(state: &str) -> (sea_orm::DatabaseConnection, Uuid, Uuid) {
    let db = setup_db().await;
    let (ws, _rev) = seed_workspace(&db, "sealed").await;
    let run = seed_run(&db, RunSeed::new(ws, NAME, state)).await;
    (db, ws, run)
}

fn report() -> Value {
    json!({
        "label": "Registered result", "directional": false, "alpha": 0.05, "assumptions": [],
        "headline": { "estimate": 17.4 },
        "write_back": { "status": "refused", "reason": "the edge declares `form: log-log`" }
    })
}

#[tokio::test]
async fn result_is_sealed_while_the_run_is_live() {
    let (db, ws, _run) = seeded("live").await;
    let err = read_result(&db, ws, NAME).await.unwrap_err();
    let (status, body) = body_of(err).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body, json!({ "sealed": true, "state": "live" }));
}

#[tokio::test]
async fn result_is_sealed_even_when_a_row_exists_but_the_run_is_not_reported() {
    // The estimate job finished; the tick has not advanced the state yet. The row
    // exists — and must stay unreadable.
    let (db, ws, run) = seeded("closing").await;
    assert!(insert_result_row(&db, run, false, report(), None).await);
    let err = read_result(&db, ws, NAME).await.unwrap_err();
    assert!(
        matches!(err, ResultError::Sealed { ref state } if state == "closing"),
        "{err:?}"
    );
    let (status, body) = body_of(err).await;
    assert_eq!(
        (status, body),
        (
            StatusCode::CONFLICT,
            json!({ "sealed": true, "state": "closing" })
        )
    );
}

#[tokio::test]
async fn an_unrecognised_state_is_sealed_not_open() {
    let (db, ws, run) = seeded("mystery").await;
    assert!(insert_result_row(&db, run, false, report(), None).await);
    assert!(matches!(
        read_result(&db, ws, NAME).await,
        Err(ResultError::Sealed { .. })
    ));
}

#[tokio::test]
async fn a_reported_run_returns_the_stored_report_unchanged() {
    let (db, ws, run) = seeded("reported").await;
    assert!(insert_result_row(&db, run, false, report(), None).await);
    let r = read_result(&db, ws, NAME)
        .await
        .expect("readable once reported");
    assert_eq!(
        (r.experiment.as_str(), r.state.as_str(), r.aborted),
        (NAME, "reported", false)
    );
    assert_eq!(
        r.report,
        report(),
        "a re-read renders exactly the stored page"
    );
    assert!(!r.write_back.available);
    assert_eq!(
        r.write_back.reason.as_deref(),
        Some("the edge declares `form: log-log`")
    );
}

#[tokio::test]
async fn an_aborted_run_is_readable_and_says_aborted() {
    let (db, ws, run) = seeded("aborted").await;
    assert!(insert_result_row(&db, run, true, report(), None).await);
    let r = read_result(&db, ws, NAME).await.expect("readable");
    assert!(r.aborted);
    assert_eq!(r.state, "aborted");
    assert!(
        r.series.is_none(),
        "a row sealed before the series reads as none"
    );
}

#[tokio::test]
async fn a_terminal_run_whose_estimate_is_not_written_yet_is_pending_not_sealed() {
    let (db, ws, _run) = seeded("aborted").await;
    let err = read_result(&db, ws, NAME).await.unwrap_err();
    let (status, body) = body_of(err).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        body,
        json!({ "sealed": false, "state": "aborted", "pending": true })
    );
}

#[tokio::test]
async fn the_body_carries_directional_alpha_prediction_and_assumptions() {
    let (db, ws, run) = seeded("reported").await;
    let prediction = json!({ "predicted": 21.0, "measured": 17.4, "prior": { "value": 4.2, "source": "fitted" }, "prior_in_interval": false });
    let stored = json!({
        "label": "Registered result", "directional": true, "alpha": 0.2,
        "assumptions": ["Carryover …", "Pull-forward …"], "prediction": prediction, "headline": { "estimate": 17.4 }
    });
    assert!(
        insert_result_row_with(
            &db,
            run,
            false,
            stored.clone(),
            None,
            Some(prediction.clone())
        )
        .await
    );
    let r = read_result(&db, ws, NAME).await.expect("readable");
    assert!(r.directional);
    assert_eq!(r.alpha, 0.2);
    assert_eq!(
        r.assumptions,
        vec!["Carryover …".to_string(), "Pull-forward …".to_string()]
    );
    assert_eq!(
        r.prediction,
        Some(prediction),
        "the column is the same object as report.prediction"
    );
    assert_eq!(
        r.report["prediction"],
        r.prediction.clone().unwrap(),
        "the two copies cannot drift"
    );
}

#[tokio::test]
async fn a_stored_report_without_alpha_is_corrupt_not_defaulted() {
    // Never answer with a guessed alpha: a directional result would be mislabelled.
    let (db, ws, run) = seeded("reported").await;
    assert!(
        insert_result_row(
            &db,
            run,
            false,
            json!({ "label": "Registered result" }),
            None
        )
        .await
    );
    let err = read_result(&db, ws, NAME).await.unwrap_err();
    assert!(matches!(err, ResultError::Corrupt(_)), "{err:?}");
    assert_eq!(body_of(err).await.0, StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn the_read_is_scoped_to_the_workspace() {
    let (db, _ws, run) = seeded("reported").await;
    assert!(insert_result_row(&db, run, false, report(), None).await);
    let (other_ws, _rev) = seed_workspace(&db, "other").await;
    let err = read_result(&db, other_ws, NAME).await.unwrap_err();
    assert!(
        matches!(err, ResultError::NoSuchRun),
        "another tenant's run must not resolve: {err:?}"
    );
}
