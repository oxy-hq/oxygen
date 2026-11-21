//! The release decision and the request contract, without a database. The
//! delete itself, workspace isolation and the listing are driven against
//! Postgres in `tests/integration/leases.rs`.
//!
//! Ported from `oxy airway release-lease` (deleted with the rest of `oxy
//! airway`), whose zero-rows triage lived inline in the command: the three
//! answers below are the three messages it printed.

use axum::body::to_bytes;
use axum::http::StatusCode;
use chrono::{TimeZone, Utc};
use serde_json::Value;

use super::*;

fn view(run_id: &str) -> LeaseView {
    LeaseView {
        pipeline_name: "toast_orders".into(),
        run_id: run_id.into(),
        acquired_at: Utc.with_ymd_and_hms(2026, 11, 21, 1, 0, 0).unwrap(),
        expires_at: Utc.with_ymd_and_hms(2026, 11, 21, 7, 0, 0).unwrap(),
        lapsed: false,
    }
}

fn scoped(run_id: &str) -> ReleaseMode {
    ReleaseMode::Scoped {
        run_id: run_id.into(),
    }
}

#[test]
fn removed_rows_are_a_release_whatever_holds_it_now() {
    for mode in [scoped("r1"), ReleaseMode::Force] {
        assert_eq!(
            classify_release(&mode, 1, None),
            ReleaseOutcome::Released { removed: 1 }
        );
    }
}

/// The holder released on its own and nobody took over: the end state the
/// caller wanted, so not an error.
#[test]
fn nothing_removed_and_nothing_holding_is_already_free() {
    for mode in [scoped("r1"), ReleaseMode::Force] {
        assert_eq!(
            classify_release(&mode, 0, None),
            ReleaseOutcome::AlreadyFree
        );
    }
}

/// `retry` re-acquires under the original run id, so the reviewed run can be
/// the successor. Reporting it as "another run" would read "run X no longer
/// holds it, run X does now".
#[test]
fn the_reviewed_run_holding_it_again_is_reheld_not_another_run() {
    assert_eq!(
        classify_release(&scoped("r1"), 0, Some(view("r1"))),
        ReleaseOutcome::ReheldBySameRun
    );
}

/// A successor the caller was never shown: never widened to, on either path.
#[test]
fn a_different_holder_is_reported_never_released() {
    assert_eq!(
        classify_release(&scoped("r1"), 0, Some(view("r2"))),
        ReleaseOutcome::HeldByAnotherRun(view("r2"))
    );
    assert_eq!(
        classify_release(&ReleaseMode::Force, 0, Some(view("r2"))),
        ReleaseOutcome::HeldByAnotherRun(view("r2"))
    );
}

fn query(run_id: Option<&str>, force: bool) -> ReleaseQuery {
    ReleaseQuery {
        run_id: run_id.map(str::to_string),
        force,
    }
}

#[test]
fn a_release_names_the_run_or_says_force_and_never_both() {
    assert_eq!(mode_of(&query(Some("r1"), false)), Ok(scoped("r1")));
    assert_eq!(mode_of(&query(None, true)), Ok(ReleaseMode::Force));
    assert!(mode_of(&query(Some("r1"), true)).is_err(), "both");
    assert!(
        mode_of(&query(None, false)).is_err(),
        "a bare DELETE must not mean force"
    );
    assert!(
        mode_of(&query(Some("  "), false)).is_err(),
        "a blank run id"
    );
}

#[test]
fn the_name_rides_as_url_safe_base64() {
    let encoded = URL_SAFE_NO_PAD.encode("toast/orders?x".as_bytes());
    assert_eq!(decode_name(&encoded).as_deref(), Some("toast/orders?x"));
    assert_eq!(decode_name("not base64!"), None);
    assert_eq!(decode_name(&URL_SAFE_NO_PAD.encode(b"  ")), None, "blank");
}

#[test]
fn a_lease_is_lapsed_on_the_servers_clock() {
    let lease = Lease {
        workspace_id: Uuid::nil(),
        pipeline_name: "p".into(),
        run_id: "r".into(),
        acquired_at: Utc.with_ymd_and_hms(2026, 11, 21, 1, 0, 0).unwrap(),
        expires_at: Utc.with_ymd_and_hms(2026, 11, 21, 7, 0, 0).unwrap(),
    };
    let before = Utc.with_ymd_and_hms(2026, 11, 21, 6, 59, 59).unwrap();
    assert!(!LeaseView::of(&lease, before).lapsed);
    assert!(LeaseView::of(&lease, lease.expires_at).lapsed, "at expiry");
}

async fn body(response: Response) -> (StatusCode, Value) {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

/// The wire shapes `oxyc airway release-lease` branches on.
#[tokio::test]
async fn each_outcome_has_its_own_status_and_shape() {
    let (s, b) = body(outcome_response(
        "p",
        ReleaseOutcome::Released { removed: 1 },
    ))
    .await;
    assert_eq!(
        (s, b["outcome"].as_str()),
        (StatusCode::OK, Some("released"))
    );
    assert_eq!(b["removed"], 1);

    let (s, b) = body(outcome_response("p", ReleaseOutcome::AlreadyFree)).await;
    assert_eq!(
        (s, b["outcome"].as_str()),
        (StatusCode::OK, Some("already_free"))
    );

    let (s, b) = body(outcome_response("p", ReleaseOutcome::ReheldBySameRun)).await;
    assert_eq!(
        (s, b["error"].as_str()),
        (StatusCode::CONFLICT, Some("lease_reheld_by_same_run"))
    );

    let (s, b) = body(outcome_response(
        "p",
        ReleaseOutcome::HeldByAnotherRun(view("r2")),
    ))
    .await;
    assert_eq!(
        (s, b["error"].as_str()),
        (StatusCode::CONFLICT, Some("lease_held_by_another_run"))
    );
    assert_eq!(b["holder"]["run_id"], "r2");
    assert_eq!(b["pipeline_name"], "p");
}
