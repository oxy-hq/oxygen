//! A model's row never decides anything: with one still `queued` or
//! `running` an attempt is settled, reads as not running, never stalls, and
//! holds no promote. Pure: every reader of an attempt's rows is asked here.

use chrono::{Duration, TimeZone, Utc};
use entity::app_build_checks;
use serde_json::json;
use uuid::Uuid;

use super::super::dto::{self, SUMMARY_KIND};
use super::super::kind::CheckKind;
use super::super::settings::Settings;
use super::super::{stalled, store};
use super::screens::DESIGN_AUDIT_KIND;
use crate::custom_apps_promote::checks_gate;

fn queued_at() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 22, 10, 0, 0).unwrap()
}

/// One row of attempt 1, claimed by a runner when it was queued.
fn row(kind: &str, status: &str, advisory: bool) -> app_build_checks::Model {
    app_build_checks::Model {
        id: Uuid::new_v4(),
        app_id: Uuid::from_u128(1),
        build_id: Uuid::from_u128(10),
        environment: "staging".into(),
        kind: kind.into(),
        attempt: 1,
        status: status.into(),
        advisory,
        summary: None,
        detail: Some(json!({ "findings": [] })),
        notes: json!([]),
        run_id: None,
        runner: Some(json!({ "label": "run #1", "url": null, "started_at": queued_at() })),
        created_at: queued_at().fixed_offset(),
        started_at: None,
        finished_at: None,
    }
}

/// An attempt whose every check passed, with the model's two rows still open.
fn passed_with_open_model_rows() -> Vec<app_build_checks::Model> {
    let mut rows: Vec<_> = CheckKind::of_staging_attempt(true)
        .into_iter()
        .map(|kind| row(kind.as_str(), "passed", kind.advisory()))
        .collect();
    rows.push(row(DESIGN_AUDIT_KIND, "running", true));
    rows.push(row(SUMMARY_KIND, "queued", true));
    rows
}

#[test]
fn an_open_model_row_is_not_running_never_stalls_and_holds_no_promote() {
    let rows = passed_with_open_model_rows();
    // Long after any runner's clock: nothing here waits on a model's row.
    let later = queued_at() + Duration::days(3);
    assert!(store::is_settled(&rows), "the attempt is settled");
    assert!(rows.iter().all(|row| !stalled::is_waiting(row)));
    assert_eq!(stalled::stalled(&rows, later), None);
    assert_eq!(dto::attempt_state(&rows, later), "passed");

    let gate = checks_gate::evaluate(&Settings::default(), rows.clone(), later);
    assert_eq!(gate.state, "passed");
    assert!(gate.unmet.is_empty() && gate.running.is_empty(), "{gate:?}");
    assert!(!gate.holds());
    assert!(gate.blockers("b1").is_empty());
    // The audit is listed, as a row nobody can require; the summary is not a row.
    let audit = gate
        .rows
        .iter()
        .find(|r| r.kind == DESIGN_AUDIT_KIND)
        .unwrap();
    assert_eq!((audit.status.as_str(), audit.required), ("running", false));
    assert!(gate.rows.iter().all(|r| r.kind != SUMMARY_KIND));

    // With a person's look required too: the comparison ran, nothing changed.
    let seen_required = Settings {
        seen_required: true,
        ..Settings::default()
    };
    assert!(!checks_gate::evaluate(&seen_required, rows, later).holds());
}

/// The same rows with a check of the attempt's own still open: that one
/// does decide, so the readers above are not blind.
#[test]
fn a_check_of_the_attempts_own_still_open_does_decide() {
    let mut rows = passed_with_open_model_rows();
    let smoke = rows.iter_mut().find(|r| r.kind == "smoke").unwrap();
    smoke.status = "running".into();
    let soon = queued_at() + Duration::minutes(5);
    assert!(!store::is_settled(&rows));
    assert_eq!(dto::attempt_state(&rows, soon), "running");
    let gate = checks_gate::evaluate(&Settings::default(), rows, soon);
    assert_eq!(gate.running, ["smoke"]);
    assert!(gate.holds());
}
