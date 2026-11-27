//! What `GET …/builds/{build}/checks` answers: one attempt's checks, the
//! build's roll-up, and every attempt's state. The contract the console and
//! `oxyc checks show` read; its shape is fixed so later phases add kinds and
//! advisory notes, not fields.
//!
//! **A stalled attempt is answered, not rewritten.** When an attempt got no
//! answer in time ([`super::stalled`]), its checks still open are answered
//! as `error` with a summary saying so, and its state is `failed`: the
//! console stops polling and *Run again* works. The stored rows are left as
//! they were.
//!
//! **So is an attempt nothing was started for.** Where the deployment starts
//! no runner (`stalled::NotStarted`), the attempt's open checks are answered
//! as `skipped` at once, saying what is missing and the command that runs
//! them by hand; `stalled` stays null. The stored rows stay open, so a claim
//! by hand takes them as usual.
//!
//! **A skip nobody decided by judging the build is no pass**
//! ([`not_checked`]): a check of an attempt nothing was started for, and a
//! check a claim closed because its environment no longer served the build.
//! Both read `skipped` on the wire; an attempt with one, and nothing that
//! failed, rolls up as `none` ([`NOT_CHECKED`]), never `passed`, and the
//! promote checks gate counts the check as unmet.

use std::collections::BTreeMap;

use chrono::{DateTime, FixedOffset, Utc};
use entity::{app_build_checks, app_builds};
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::kind::{CheckKind, CheckStatus, NOT_CHECKED_KEY};
use super::stalled::{self, Runner, Stalled};

/// The kind of the row that holds a build's advisory summary (a later
/// phase's): not a check, so it is lifted out of `checks`.
pub const SUMMARY_KIND: &str = "summary";

#[derive(Debug, Serialize)]
pub struct BuildRef {
    pub id: Uuid,
    pub build_id: String,
    pub created_at: DateTime<FixedOffset>,
}

#[derive(Debug, Default, Serialize, PartialEq, Eq)]
pub struct Counts {
    pub total: usize,
    pub queued: usize,
    pub running: usize,
    pub passed: usize,
    pub failed: usize,
    pub error: usize,
    pub skipped: usize,
}

#[derive(Debug, Serialize)]
pub struct CheckDto {
    pub kind: String,
    pub label: String,
    pub status: String,
    pub advisory: bool,
    pub summary: Option<String>,
    pub detail: Value,
    pub notes: Value,
    pub attempt: i32,
    pub run_id: Option<String>,
    pub created_at: DateTime<FixedOffset>,
    pub started_at: Option<DateTime<FixedOffset>>,
    pub finished_at: Option<DateTime<FixedOffset>>,
    pub duration_ms: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct AttemptDto {
    pub attempt: i32,
    pub state: &'static str,
    pub created_at: DateTime<FixedOffset>,
}

#[derive(Debug, Serialize)]
pub struct ChecksResponse {
    pub app_id: Uuid,
    pub build: BuildRef,
    pub attempt: Option<i32>,
    pub environment: String,
    pub state: &'static str,
    pub counts: Counts,
    pub checks: Vec<CheckDto>,
    pub summary: Option<Value>,
    pub attempts: Vec<AttemptDto>,
    /// Who runs the attempt shown; `null` until a runner claims it.
    pub runner: Option<Runner>,
    /// Set when the attempt shown got no answer in time.
    pub stalled: Option<Stalled>,
}

/// The roll-up over the non-advisory checks, by status alone: any still
/// queued or running is `running`; else any failed or unable to run is
/// `failed`; else `passed` (passes and skips); no checks at all is `none`.
/// An attempt's own roll-up also asks who decided each skip ([`rolled_up`]).
pub fn state_of<'a>(checks: impl Iterator<Item = (&'a str, bool)>) -> &'static str {
    let statuses: Vec<CheckStatus> = checks
        .filter(|(_, advisory)| !advisory)
        .filter_map(|(status, _)| CheckStatus::parse(status))
        .collect();
    if statuses.is_empty() {
        "none"
    } else if statuses.iter().any(|s| !s.is_terminal()) {
        "running"
    } else if statuses
        .iter()
        .any(|s| matches!(s, CheckStatus::Failed | CheckStatus::Error))
    {
        "failed"
    } else {
        "passed"
    }
}

fn counts(checks: &[CheckDto]) -> Counts {
    let mut counts = Counts {
        total: checks.len(),
        ..Counts::default()
    };
    for check in checks {
        match CheckStatus::parse(&check.status) {
            Some(CheckStatus::Queued) => counts.queued += 1,
            Some(CheckStatus::Running) => counts.running += 1,
            Some(CheckStatus::Passed) => counts.passed += 1,
            Some(CheckStatus::Failed) => counts.failed += 1,
            Some(CheckStatus::Error) => counts.error += 1,
            Some(CheckStatus::Skipped) => counts.skipped += 1,
            None => {}
        }
    }
    counts
}

/// One attempt's rows as they are answered as of `now`, and why when the
/// attempt is stalled: its checks still open read `error`, saying so. An
/// attempt nothing was started for is not stalled: its open checks read
/// `skipped`, saying how to run them, and none of them is a pass
/// ([`not_checked`]).
///
/// **Every check a runner still owes is answered** (`stalled::is_owed`), the
/// advisory ones included: `changes` never reads `queued` or `running` once
/// nobody will write it. That covers an attempt that is not stalled, whose
/// runner reported its last check that counts and died during `changes`
/// (`stalled::abandoned`): the row reads `error`, the state is what the
/// checks that count give, and `stalled` stays `None`.
pub fn answered(
    mut rows: Vec<app_build_checks::Model>,
    now: DateTime<Utc>,
) -> (Vec<app_build_checks::Model>, Option<Stalled>) {
    if let Some(marker) = stalled::not_started_of(&rows) {
        for row in rows.iter_mut().filter(|row| stalled::is_owed(row)) {
            row.status = CheckStatus::Skipped.as_str().to_string();
            row.summary = Some(marker.summary(row.app_id, row.build_id));
        }
        return (rows, None);
    }
    // Decided over the checks that count, before any row is answered.
    let stalled = stalled::stalled(&rows, now);
    if let Some(why) = stalled::unanswered(&rows, now) {
        for row in rows.iter_mut().filter(|row| stalled::is_owed(row)) {
            row.status = CheckStatus::Error.as_str().to_string();
            row.summary = Some(why.summary());
        }
    }
    (rows, stalled)
}

/// The roll-up of an attempt with a check nobody judged and nothing that
/// failed: `none`, the state of a build that was never checked. Such a check
/// reads `skipped`, and [`state_of`] counts a skip beside the passes, so
/// without this a build nobody checked would roll up as `passed`.
pub const NOT_CHECKED: &str = "none";

/// Whether a row reads `skipped` because nobody judged it, rather than
/// because the check itself found nothing to check:
///
/// - the platform closed it so (`Outcome::not_checked`: by the claim, its
///   environment no longer served the build), which its detail says; or
/// - nothing was started for its attempt, which its `runner` column says
///   ([`stalled::NotStarted`]): open and answered `skipped` by [`answered`],
///   or closed as `skipped` by a rerun.
///
/// Such a skip is no pass: it satisfies no requirement of the promote checks
/// gate, and its attempt never rolls up as `passed` ([`rolled_up`]).
pub fn not_checked(row: &app_build_checks::Model) -> bool {
    if row.status != CheckStatus::Skipped.as_str() {
        return false;
    }
    let marked = row
        .detail
        .as_ref()
        .and_then(|detail| detail.get(NOT_CHECKED_KEY));
    marked == Some(&Value::Bool(true))
        || stalled::not_started_of(std::slice::from_ref(row)).is_some()
}

/// The roll-up of one attempt's rows as [`answered`] answers them:
/// [`state_of`], but an attempt that would read `passed` while a check that
/// counts was judged by nobody ([`not_checked`]) reads [`NOT_CHECKED`]. A
/// verdict that did get written beside it (a failure, an error, a check still
/// open) decides the roll-up as it does for any attempt.
fn rolled_up(rows: &[app_build_checks::Model]) -> &'static str {
    let state = state_of(rows.iter().map(|r| (r.status.as_str(), r.advisory)));
    let unjudged = rows.iter().any(|row| !row.advisory && not_checked(row));
    if state == "passed" && unjudged {
        NOT_CHECKED
    } else {
        state
    }
}

/// The state of one attempt's rows as of `now`: [`rolled_up`] over what
/// [`answered`] answers.
pub fn attempt_state(rows: &[app_build_checks::Model], now: DateTime<Utc>) -> &'static str {
    let (rows, _) = answered(rows.to_vec(), now);
    rolled_up(&rows)
}

/// A stored detail as it is answered: always an object with `findings`.
fn detail_of(stored: Option<Value>) -> Value {
    let mut detail = match stored {
        Some(Value::Object(map)) => Value::Object(map),
        _ => json!({}),
    };
    if !detail["findings"].is_array() {
        detail["findings"] = json!([]);
    }
    detail
}

fn check_dto(row: app_build_checks::Model) -> CheckDto {
    let label = CheckKind::label_of(&row.kind);
    let duration_ms = row
        .started_at
        .zip(row.finished_at)
        .map(|(start, end)| (end - start).num_milliseconds());
    CheckDto {
        label,
        kind: row.kind,
        status: row.status,
        advisory: row.advisory,
        summary: row.summary,
        detail: detail_of(row.detail),
        notes: row.notes,
        attempt: row.attempt,
        run_id: row.run_id,
        created_at: row.created_at,
        started_at: row.started_at,
        finished_at: row.finished_at,
        duration_ms,
    }
}

/// The response for `build`'s rows in `environment` (every attempt, any
/// order), showing `attempt` — the newest when `None` — as of `now`.
pub fn response(
    build: &app_builds::Model,
    environment: &str,
    rows: Vec<app_build_checks::Model>,
    attempt: Option<i32>,
    now: DateTime<Utc>,
) -> ChecksResponse {
    let mut by_attempt: BTreeMap<i32, Vec<app_build_checks::Model>> = BTreeMap::new();
    for row in rows {
        by_attempt.entry(row.attempt).or_default().push(row);
    }
    let attempts = by_attempt
        .iter()
        .rev()
        .map(|(n, rows)| AttemptDto {
            attempt: *n,
            state: attempt_state(rows, now),
            created_at: rows
                .iter()
                .map(|r| r.created_at)
                .min()
                .unwrap_or(build.created_at),
        })
        .collect();
    let shown = attempt.or_else(|| by_attempt.keys().next_back().copied());
    let rows = shown
        .and_then(|n| by_attempt.remove(&n))
        .unwrap_or_default();
    let runner = stalled::runner_of(&rows);
    let (rows, stalled) = answered(rows, now);
    let state = rolled_up(&rows);
    let (summaries, mut checks): (Vec<_>, Vec<_>) =
        rows.into_iter().partition(|r| r.kind == SUMMARY_KIND);
    checks.sort_by(|a, b| CheckKind::list_key(&a.kind).cmp(&CheckKind::list_key(&b.kind)));
    let checks: Vec<CheckDto> = checks.into_iter().map(check_dto).collect();
    ChecksResponse {
        app_id: build.app_id,
        build: BuildRef {
            id: build.id,
            build_id: build.build_id.clone(),
            created_at: build.created_at,
        },
        attempt: shown.filter(|_| !checks.is_empty() || !summaries.is_empty()),
        environment: environment.to_string(),
        state,
        counts: counts(&checks),
        checks,
        summary: summaries.into_iter().next().and_then(|r| r.detail),
        attempts,
        runner,
        stalled,
    }
}

#[cfg(test)]
#[path = "dto_tests.rs"]
mod tests;
