use chrono::{Duration, TimeZone, Utc};
use serde_json::json;

use super::*;

fn build() -> app_builds::Model {
    serde_json::from_value(json!({
        "id": Uuid::from_u128(10),
        "app_id": Uuid::from_u128(1),
        "build_id": "9087d7b",
        "s3_prefix": "p/",
        "manifest_json": null,
        "created_at": "2026-11-20T10:00:00Z",
        "published_by": null,
        "published_via": null,
        "source_repo": null,
        "commit_sha": null,
        "source_branch": null,
        "validation_status": "passed",
        "validation_detail": null,
        "semantic_revision_id": null,
        "published_token_id": null,
        "published_to": "staging",
        "sdk_version": "2.30.0",
    }))
    .expect("a build row")
}

fn row(kind: &str, attempt: i32, status: &str, advisory: bool) -> app_build_checks::Model {
    let at = Utc
        .with_ymd_and_hms(2026, 11, 20, 10, attempt as u32, 0)
        .unwrap()
        .fixed_offset();
    let finished = CheckStatus::parse(status).is_some_and(CheckStatus::is_terminal);
    app_build_checks::Model {
        id: Uuid::new_v4(),
        app_id: Uuid::from_u128(1),
        build_id: Uuid::from_u128(10),
        environment: "staging".into(),
        kind: kind.into(),
        attempt,
        status: status.into(),
        advisory,
        summary: Some(format!("{kind} {status}")),
        detail: finished.then(|| json!({ "findings": [] })),
        notes: json!([]),
        run_id: Some(format!("run-{attempt}")),
        runner: None,
        created_at: at,
        started_at: Some(at),
        finished_at: finished.then(|| at + Duration::milliseconds(1500)),
    }
}

/// A few minutes after the rows below were written.
fn soon() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 20, 10, 10, 0).unwrap()
}

/// `rows` claimed by a runner at `at`.
fn claimed(
    mut rows: Vec<app_build_checks::Model>,
    at: chrono::DateTime<Utc>,
) -> Vec<app_build_checks::Model> {
    for row in &mut rows {
        row.runner = Some(json!({
            "label": "GitHub Actions run #1482",
            "url": "https://github.com/oxy-hq/oxygen-internal/actions/runs/123",
            "started_at": at,
        }));
    }
    rows
}

/// When attempt 1's rows were queued (`row` puts attempt `n` at minute `n`).
fn queued_at() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 20, 10, 1, 0).unwrap()
}

#[test]
fn an_attempt_no_runner_claimed_reads_as_failed_after_ten_minutes() {
    let rows = vec![
        row("validation", 1, "queued", false),
        row("loads", 1, "queued", false),
    ];
    let inside = response(
        &build(),
        "staging",
        rows.clone(),
        None,
        queued_at() + Duration::minutes(10),
    );
    assert_eq!((inside.state, inside.stalled), ("running", None));
    assert!(inside.runner.is_none(), "null until claimed");
    assert_eq!(inside.checks[0].status, "queued");

    let after = queued_at() + Duration::seconds(601);
    let stale = response(&build(), "staging", rows, None, after);
    assert_eq!(stale.stalled, Some(Stalled::NoRunner));
    assert_eq!(stale.state, "failed", "the console stops polling");
    assert_eq!(stale.attempts[0].state, "failed");
    assert_eq!((stale.counts.error, stale.counts.queued), (2, 0));
    for check in &stale.checks {
        assert_eq!(check.status, "error");
        let summary = check.summary.as_deref().unwrap();
        assert!(summary.starts_with("no runner picked the checks up within 10 minutes"));
        assert_eq!(check.finished_at, None, "answered, not written");
    }
    let wire = serde_json::to_value(&stale).unwrap();
    assert_eq!(wire["stalled"], "no_runner");
    assert_eq!(wire["runner"], json!(null));
}

#[test]
fn an_attempt_whose_runner_stopped_reads_as_failed_after_thirty_minutes() {
    let at = queued_at() + Duration::minutes(2);
    let rows = claimed(
        vec![
            row("validation", 1, "passed", false),
            row("loads", 1, "running", false),
        ],
        at,
    );
    let inside = response(
        &build(),
        "staging",
        rows.clone(),
        None,
        at + Duration::minutes(30),
    );
    assert_eq!((inside.state, inside.stalled), ("running", None));
    let runner = inside.runner.expect("the runner that claimed it");
    assert_eq!(runner.label, "GitHub Actions run #1482");
    assert_eq!(runner.started_at, at);

    let after = at + Duration::seconds(30 * 60 + 1);
    let stale = response(&build(), "staging", rows, None, after);
    assert_eq!(stale.stalled, Some(Stalled::RunnerStopped));
    assert_eq!(stale.state, "failed");
    let by_kind = |kind: &str| stale.checks.iter().find(|c| c.kind == kind).unwrap();
    assert_eq!(by_kind("validation").status, "passed", "a verdict stands");
    assert_eq!(by_kind("loads").status, "error");
    assert!(
        by_kind("loads")
            .summary
            .as_deref()
            .unwrap()
            .starts_with("the runner stopped before a verdict")
    );
    let wire = serde_json::to_value(&stale).unwrap();
    assert_eq!(wire["stalled"], "runner_stopped");
    assert_eq!(
        wire["runner"],
        json!({
            "label": "GitHub Actions run #1482",
            "url": "https://github.com/oxy-hq/oxygen-internal/actions/runs/123",
            "started_at": "2026-11-20T10:03:00Z",
        })
    );
}

#[test]
fn an_attempt_nothing_was_started_for_reads_skipped_at_once_and_never_stalls() {
    let record = json!({ "not_started": "not_configured",
        "reason": "OXY_APP_CHECKS_GITHUB_TOKEN is not set", "at": "2026-11-20T10:01:00Z" });
    let mut rows: Vec<app_build_checks::Model> =
        ["validation", "functions", "loads", "design_system"]
            .iter()
            .map(|kind| row(kind, 1, "queued", false))
            .collect();
    for row in &mut rows {
        row.runner = Some(record.clone());
    }
    let says = "no runner is configured on this deployment (OXY_APP_CHECKS_GITHUB_TOKEN is not \
                set); run the checks by hand: oxyc checks run \
                00000000-0000-0000-0000-000000000001 --build \
                00000000-0000-0000-0000-00000000000a --report";
    for now in [queued_at(), queued_at() + Duration::days(2)] {
        let out = response(&build(), "staging", rows.clone(), None, now);
        // Never `passed`: nothing ran, not even the validation a claim writes.
        assert_eq!((out.state, out.stalled), ("none", None), "at {now}");
        assert!(out.runner.is_none(), "the record is not a runner");
        assert_eq!((out.counts.skipped, out.counts.queued), (4, 0));
        assert_eq!(out.attempts[0].state, "none");
        for check in &out.checks {
            assert_eq!(check.status, "skipped", "{}", check.kind);
            assert_eq!(check.summary.as_deref(), Some(says));
            assert_eq!(check.finished_at, None, "answered, not written");
        }
    }
    // A verdict written meanwhile stands beside the skips, and decides.
    rows[2].status = "failed".into();
    let out = response(&build(), "staging", rows, None, queued_at());
    assert_eq!((out.state, out.stalled), ("failed", None));
    let loads = out.checks.iter().find(|c| c.kind == "loads").unwrap();
    assert_eq!(loads.summary.as_deref(), Some("loads queued"));
}

#[test]
fn an_older_stalled_attempt_reads_as_failed_beside_a_newer_one() {
    let rows = vec![
        row("loads", 1, "queued", false),
        row("loads", 2, "passed", false),
    ];
    let later = queued_at() + Duration::hours(2);
    let out = response(&build(), "staging", rows, None, later);
    assert_eq!(
        (out.attempt, out.state, out.stalled),
        (Some(2), "passed", None)
    );
    let states: Vec<(i32, &str)> = out.attempts.iter().map(|a| (a.attempt, a.state)).collect();
    assert_eq!(states, [(2, "passed"), (1, "failed")]);
}

#[test]
fn a_detail_always_answers_findings() {
    let mut pending = row("functions", 1, "running", false);
    pending.detail = Some(json!({ "pending": [{ "name": "smoke", "run_id": "r1" }] }));
    let out = response(&build(), "staging", vec![pending], None, soon());
    assert_eq!(out.checks[0].detail["findings"], json!([]));
    assert_eq!(out.checks[0].detail["pending"][0]["name"], "smoke");
}

#[test]
fn the_roll_up_reads_only_what_is_not_advisory() {
    assert_eq!(state_of(std::iter::empty()), "none");
    assert_eq!(
        state_of([("passed", false), ("running", false)].into_iter()),
        "running"
    );
    assert_eq!(
        state_of([("passed", false), ("error", false)].into_iter()),
        "failed"
    );
    assert_eq!(
        state_of([("passed", false), ("skipped", false)].into_iter()),
        "passed"
    );
    // An advisory row never decides the build, running or failed.
    assert_eq!(
        state_of([("passed", false), ("failed", true)].into_iter()),
        "passed"
    );
    assert_eq!(
        state_of([("passed", false), ("running", true)].into_iter()),
        "passed"
    );
}

#[test]
fn the_newest_attempt_is_shown_in_kind_order_with_every_attempt_listed() {
    let rows = vec![
        row("loads", 1, "failed", false),
        row("design_system", 2, "passed", false),
        row("loads", 2, "running", false),
        row("validation", 2, "passed", false),
        row("functions", 2, "skipped", false),
    ];
    let out = response(&build(), "staging", rows, None, soon());
    assert_eq!(out.attempt, Some(2));
    assert_eq!(out.state, "running");
    let kinds: Vec<&str> = out.checks.iter().map(|c| c.kind.as_str()).collect();
    assert_eq!(kinds, ["validation", "functions", "loads", "design_system"]);
    assert_eq!(out.checks[0].label, "Builds and validates");
    assert_eq!(out.checks[0].duration_ms, Some(1500));
    assert_eq!(out.checks[2].duration_ms, None);
    assert_eq!(
        out.counts,
        Counts {
            total: 4,
            running: 1,
            passed: 2,
            skipped: 1,
            ..Counts::default()
        }
    );
    let attempts: Vec<(i32, &str)> = out.attempts.iter().map(|a| (a.attempt, a.state)).collect();
    assert_eq!(attempts, [(2, "running"), (1, "failed")]);
    let first = response(
        &build(),
        "staging",
        vec![row("loads", 1, "failed", false)],
        Some(1),
        soon(),
    );
    assert_eq!((first.attempt, first.state), (Some(1), "failed"));
}

#[test]
fn a_build_never_checked_answers_none_and_a_summary_row_is_lifted_out() {
    let none = response(&build(), "staging", Vec::new(), None, soon());
    assert_eq!(
        (none.attempt, none.state, none.checks.len()),
        (None, "none", 0)
    );
    assert_eq!(none.build.build_id, "9087d7b");
    let mut summary = row(SUMMARY_KIND, 1, "passed", true);
    summary.detail = Some(json!({ "author": "model", "text": "two screens changed" }));
    let mut later = row("design_audit", 1, "failed", true);
    later.notes = json!([{ "author": "model", "text": "a 13px label set by hand" }]);
    let rows = vec![row("validation", 1, "passed", false), summary, later];
    let out = response(&build(), "staging", rows, None, soon());
    assert_eq!(out.summary.as_ref().unwrap()["text"], "two screens changed");
    let kinds: Vec<&str> = out.checks.iter().map(|c| c.kind.as_str()).collect();
    assert_eq!(kinds, ["validation", "design_audit"]);
    assert_eq!(out.checks[1].label, "Design audit");
    assert_eq!(out.checks[1].notes[0]["author"], "model");
    assert_eq!(
        out.state, "passed",
        "an advisory failure does not fail the build"
    );
}

#[path = "dto_unanswered_tests.rs"]
mod unanswered;
