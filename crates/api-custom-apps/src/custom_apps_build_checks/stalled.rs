//! An attempt that got no answer in time, and who was running it.
//!
//! The worker's task only starts a runner; nothing on the platform watches
//! that runner. So an attempt can stop two ways without anybody closing its
//! rows: the runner that was started never picks it up (GitHub never ran
//! it), or a runner claims it and dies. Both are read off the rows, never
//! written: the stored rows stay as they were, and every reader that would
//! otherwise wait forever (the console's poll, `oxyc checks show --wait`, a
//! rerun) asks [`stalled`].
//!
//! - [`Stalled::NoRunner`]: a check is still open, no runner claimed the
//!   attempt, and it was queued more than [`NO_RUNNER_AFTER`] ago.
//! - [`Stalled::RunnerStopped`]: a check is still open, and the runner that
//!   last claimed the attempt did so more than [`RUNNER_STOPPED_AFTER`] ago.
//!
//! **An attempt nothing was started for is not stalled.** A deployment that
//! is not configured to start a runner says so on the attempt ([`NotStarted`],
//! in the same `runner` column): nobody is late, so such an attempt never
//! reads as `no_runner`. Its open checks read `skipped` at once, saying how
//! to run them by hand, and a rerun may always start.
//!
//! **What the `runner` column holds**, the same value on every row of an
//! attempt: `null` (queued, a runner was started and has not claimed yet), a
//! [`Runner`] (`{label, url, started_at}`, and `by` for a run by hand,
//! written by a claim), or a [`NotStarted`] (`{not_started, reason, at}`,
//! written by the task). A claim replaces a `NotStarted`; a `NotStarted`
//! never replaces a runner.
//!
//! **Advisory rows never stall an attempt**, as they never decide its state:
//! [`stalled`] reads only the checks that count. **But a runner owes them a
//! verdict too** ([`is_owed`]: every open row that is not a model's,
//! `changes` included), and the same clocks say when it will never come
//! ([`unanswered`]): an advisory row of a stalled attempt reads `error` with
//! the others, and so does one a runner left behind after its last check
//! that counts (it reported `smoke` and died during `changes`). That second
//! attempt is not stalled, it is [`abandoned`]: one write short of settled.
//! Its next reader closes it (`settled::close_abandoned`), the one place a
//! reading writes, so the attempt settles and is handed on.

use chrono::{DateTime, Duration, FixedOffset, Utc};
use entity::app_build_checks;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::kind::{self, CheckStatus};

/// How long a queued attempt may wait for a runner to claim it.
pub const NO_RUNNER_AFTER: Duration = Duration::minutes(10);
/// How long a claimed attempt may stay open.
pub const RUNNER_STOPPED_AFTER: Duration = Duration::minutes(30);

/// Why an attempt is read as stalled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stalled {
    NoRunner,
    RunnerStopped,
}

impl Stalled {
    /// What an open check of a stalled attempt is answered with.
    pub fn summary(self) -> String {
        match self {
            Self::NoRunner => format!(
                "no runner picked the checks up within {} minutes of being queued; run them again",
                NO_RUNNER_AFTER.num_minutes()
            ),
            Self::RunnerStopped => format!(
                "the runner stopped before a verdict (it claimed the checks more than {} minutes \
                 ago); run them again",
                RUNNER_STOPPED_AFTER.num_minutes()
            ),
        }
    }
}

/// Who runs an attempt: the stored `runner` of its rows. **Only the platform
/// checker names itself** (`runner::claim::Claimant::runner`): a claim by
/// anyone else is named by the platform from who called, so a run by hand
/// cannot read as a GitHub Actions run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Runner {
    /// `GitHub Actions run #1482` as the checker's run names itself, or
    /// `<who>, by hand` for anyone else, whatever their request sent.
    pub label: String,
    /// The checker's run's page; never set for a run by hand.
    pub url: Option<String>,
    /// When the attempt was last claimed: every claim sets it, so the runner
    /// at work is the one [`RUNNER_STOPPED_AFTER`] is measured from.
    pub started_at: DateTime<FixedOffset>,
    /// Who ran it by hand, as every log names them; absent for the platform
    /// checker's run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
}

/// Why no runner was started for an attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotStartedWhy {
    /// This deployment is not configured to start one.
    NotConfigured,
}

/// The longest reason a [`NotStarted`] keeps.
const MAX_REASON_CHARS: usize = 160;

/// What the `runner` column holds for an attempt nothing was started for.
/// Never a [`Runner`]: it has no label and no start, so [`runner_of`] does
/// not read it as one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotStarted {
    pub not_started: NotStartedWhy,
    /// What is missing, in the configuration's own words: the variable's
    /// name, never a secret's value.
    pub reason: String,
    /// When the task found out.
    pub at: DateTime<FixedOffset>,
}

impl NotStarted {
    /// This deployment starts no runner, and `reason` is why.
    pub fn not_configured(reason: &str, at: DateTime<Utc>) -> Self {
        Self {
            not_started: NotStartedWhy::NotConfigured,
            reason: super::kind::cap(reason, MAX_REASON_CHARS),
            at: at.fixed_offset(),
        }
    }

    /// What an open check of such an attempt is answered with: what is
    /// missing, and the command that runs the checks by hand.
    pub fn summary(&self, app_id: Uuid, build_pk: Uuid) -> String {
        format!(
            "no runner is configured on this deployment ({}); run the checks by hand: oxyc \
             checks run {app_id} --build {build_pk} --report",
            self.reason
        )
    }
}

/// Whether a row is a check still without a verdict.
pub fn is_open(row: &app_build_checks::Model) -> bool {
    CheckStatus::parse(&row.status).is_some_and(|status| !status.is_terminal())
}

/// The runner of an attempt's rows: every row of a claimed attempt carries
/// the same one, so the first that parses is it.
pub fn runner_of(rows: &[app_build_checks::Model]) -> Option<Runner> {
    rows.iter()
        .filter_map(|row| row.runner.clone())
        .find_map(|stored| serde_json::from_value(stored).ok())
}

/// The record that nothing was started for an attempt; `None` once a runner
/// has claimed it, whose claim replaced the record.
pub fn not_started_of(rows: &[app_build_checks::Model]) -> Option<NotStarted> {
    if runner_of(rows).is_some() {
        return None;
    }
    rows.iter()
        .filter_map(|row| row.runner.clone())
        .find_map(|stored| serde_json::from_value(stored).ok())
}

/// Whether a row keeps its attempt waiting: a check still without a verdict
/// that is part of the build's verdict.
pub fn is_waiting(row: &app_build_checks::Model) -> bool {
    !row.advisory && is_open(row)
}

/// Whether a runner still owes a row its verdict: a check still open that is
/// not a model's row, an advisory one (`changes`) included. What keeps an
/// attempt unsettled (`store::is_settled`).
pub fn is_owed(row: &app_build_checks::Model) -> bool {
    is_open(row) && !kind::is_models(&row.kind)
}

/// Whether one attempt's `rows` are stalled as of `now`: a check that counts
/// is open, and its clock has run out.
pub fn stalled(rows: &[app_build_checks::Model], now: DateTime<Utc>) -> Option<Stalled> {
    out_of_time(rows, now, is_waiting)
}

/// Why the checks a runner still owes `rows`' attempt will get no answer, as
/// of `now`: the clocks of [`stalled`], asked of every owed row, the advisory
/// ones included. `Some` for every stalled attempt, and for an [`abandoned`]
/// one.
pub fn unanswered(rows: &[app_build_checks::Model], now: DateTime<Utc>) -> Option<Stalled> {
    out_of_time(rows, now, is_owed)
}

/// [`unanswered`] for an attempt that is not stalled: every check that counts
/// has its verdict, and a runner left only advisory ones behind.
pub fn abandoned(rows: &[app_build_checks::Model], now: DateTime<Utc>) -> Option<Stalled> {
    if rows.iter().any(is_waiting) {
        return None;
    }
    unanswered(rows, now)
}

/// The two clocks, over the open rows `counts` picks.
fn out_of_time(
    rows: &[app_build_checks::Model],
    now: DateTime<Utc>,
    counts: fn(&app_build_checks::Model) -> bool,
) -> Option<Stalled> {
    let queued_at = rows
        .iter()
        .filter(|row| counts(row))
        .map(|row| row.created_at)
        .min()?;
    match runner_of(rows) {
        Some(runner) => (runner.started_at.with_timezone(&Utc) < now - RUNNER_STOPPED_AFTER)
            .then_some(Stalled::RunnerStopped),
        // Nothing was started, and the rows say so: nobody is late.
        None if not_started_of(rows).is_some() => None,
        None => {
            (queued_at.with_timezone(&Utc) < now - NO_RUNNER_AFTER).then_some(Stalled::NoRunner)
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use serde_json::json;
    use uuid::Uuid;

    use super::*;

    fn queued_at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 20, 10, 0, 0).unwrap()
    }

    fn row(kind: &str, status: &str, runner: Option<DateTime<Utc>>) -> app_build_checks::Model {
        app_build_checks::Model {
            id: Uuid::new_v4(),
            app_id: Uuid::from_u128(1),
            build_id: Uuid::from_u128(10),
            environment: "staging".into(),
            kind: kind.into(),
            attempt: 1,
            status: status.into(),
            advisory: false,
            summary: None,
            detail: None,
            notes: json!([]),
            run_id: None,
            runner: runner.map(|at| json!({ "label": "run #1", "url": null, "started_at": at })),
            created_at: queued_at().fixed_offset(),
            started_at: None,
            finished_at: None,
        }
    }

    #[test]
    fn an_unclaimed_attempt_stalls_ten_minutes_after_it_was_queued() {
        let rows = vec![
            row("validation", "queued", None),
            row("loads", "queued", None),
        ];
        let at = |seconds: i64| stalled(&rows, queued_at() + Duration::seconds(seconds));
        assert_eq!(at(0), None);
        assert_eq!(at(600), None, "just inside the horizon");
        assert_eq!(at(601), Some(Stalled::NoRunner));
    }

    #[test]
    fn a_claimed_attempt_stalls_thirty_minutes_after_the_claim() {
        // Claimed nine minutes in: the queue's own horizon no longer applies.
        let claimed = queued_at() + Duration::minutes(9);
        let rows = vec![
            row("validation", "passed", Some(claimed)),
            row("loads", "running", Some(claimed)),
        ];
        let at = |minutes: i64| stalled(&rows, claimed + Duration::minutes(minutes));
        assert_eq!(at(5), None, "past the unclaimed horizon, but claimed");
        assert_eq!(at(30), None, "just inside the horizon");
        assert_eq!(
            stalled(&rows, claimed + Duration::seconds(30 * 60 + 1)),
            Some(Stalled::RunnerStopped)
        );
        assert_eq!(runner_of(&rows).unwrap().label, "run #1");
    }

    const MISSING: &str = "OXY_APP_CHECKS_GITHUB_TOKEN is not set";

    #[test]
    fn an_attempt_nothing_was_started_for_is_not_stalled_and_its_record_is_no_runner() {
        let record = NotStarted::not_configured(MISSING, queued_at());
        let stored = serde_json::to_value(&record).unwrap();
        assert_eq!(
            stored,
            json!({ "not_started": "not_configured", "reason": MISSING,
                "at": "2026-11-20T10:00:00Z" })
        );
        let mut rows = vec![
            row("validation", "queued", None),
            row("loads", "queued", None),
        ];
        for row in &mut rows {
            row.runner = Some(stored.clone());
        }
        let long_after = queued_at() + Duration::days(3);
        assert_eq!(runner_of(&rows), None, "the record is not a runner");
        assert_eq!(not_started_of(&rows), Some(record.clone()));
        assert_eq!(stalled(&rows, long_after), None, "nobody is late");

        // The same attempt with nothing recorded WAS given a runner: late.
        let dispatched = vec![
            row("validation", "queued", None),
            row("loads", "queued", None),
        ];
        assert_eq!(not_started_of(&dispatched), None);
        assert_eq!(stalled(&dispatched, long_after), Some(Stalled::NoRunner));

        // A claim replaces the record, and the attempt is a claimed one again.
        let claimed = vec![row("loads", "running", Some(queued_at()))];
        assert_eq!(not_started_of(&claimed), None);
        assert_eq!(stalled(&claimed, long_after), Some(Stalled::RunnerStopped));

        assert_eq!(
            record.summary(Uuid::from_u128(1), Uuid::from_u128(10)),
            "no runner is configured on this deployment (OXY_APP_CHECKS_GITHUB_TOKEN is not \
             set); run the checks by hand: oxyc checks run \
             00000000-0000-0000-0000-000000000001 --build \
             00000000-0000-0000-0000-00000000000a --report"
        );
        let long = NotStarted::not_configured(&"x".repeat(1000), queued_at());
        assert_eq!(long.reason.chars().count(), 160);
    }

    /// The runner reported its last check that counts and died during
    /// `changes`: the attempt is not stalled, and the advisory check it owes
    /// will get no answer once the clock of its claim has run out.
    #[test]
    fn an_advisory_check_a_runner_owes_runs_out_of_time_without_stalling_the_attempt() {
        let claimed = queued_at() + Duration::minutes(2);
        let mut changes = row("changes", "running", Some(claimed));
        changes.advisory = true;
        let rows = vec![
            row("validation", "passed", Some(claimed)),
            row("smoke", "passed", Some(claimed)),
            changes.clone(),
        ];
        assert!(is_owed(&changes) && !is_waiting(&changes));
        let inside = claimed + Duration::minutes(30);
        let past = claimed + Duration::seconds(30 * 60 + 1);
        assert_eq!(stalled(&rows, past), None, "advisory rows stall no attempt");
        assert_eq!(
            (unanswered(&rows, inside), abandoned(&rows, inside)),
            (None, None)
        );
        assert_eq!(unanswered(&rows, past), Some(Stalled::RunnerStopped));
        assert_eq!(abandoned(&rows, past), Some(Stalled::RunnerStopped));

        // With a check that counts still open the attempt is stalled, not
        // abandoned; the advisory row is unanswered with it.
        let mut stalled_rows = rows.clone();
        stalled_rows[1].status = "running".into();
        assert_eq!(stalled(&stalled_rows, past), Some(Stalled::RunnerStopped));
        assert_eq!(
            unanswered(&stalled_rows, past),
            Some(Stalled::RunnerStopped)
        );
        assert_eq!(abandoned(&stalled_rows, past), None);

        // A model's row is nobody's to wait for: it is written after the
        // attempt has settled.
        let mut audit = row("design_audit", "running", Some(claimed));
        audit.advisory = true;
        let settled = vec![row("smoke", "passed", Some(claimed)), audit.clone()];
        assert!(!is_owed(&audit));
        assert_eq!(
            (unanswered(&settled, past), abandoned(&settled, past)),
            (None, None)
        );
    }

    #[test]
    fn an_attempt_with_every_verdict_never_stalls_and_an_advisory_row_does_not_stall_one() {
        let long_after = queued_at() + Duration::days(3);
        let done = vec![
            row("validation", "passed", None),
            row("loads", "failed", None),
        ];
        assert_eq!(stalled(&done, long_after), None);
        let mut advisory = row("design_audit", "queued", None);
        advisory.advisory = true;
        let only_advisory_open = vec![row("validation", "passed", None), advisory];
        assert_eq!(stalled(&only_advisory_open, long_after), None);
        assert_eq!(stalled(&[], long_after), None);
    }
}
