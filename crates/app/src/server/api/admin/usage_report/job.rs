//! The weekly pass: write last week's report once, then email it.
//!
//! Driven by the global singleton worker's tick (`router::recovery`,
//! `OXY_INPROC_GLOBAL_WORKER`), beside the token sweep — not a loop of its
//! own, and never from a request.
//!
//! **Why this is not a `TaskSpec` on the queue.** The queue is per workspace:
//! every run carries a workspace id, and in cloud mode a run whose workspace
//! does not exist is retired. This report belongs to no workspace. What the
//! queue would have given it is here by other means — the state is two tables,
//! so a restart loses nothing; the work is claimed by unique rows
//! ([`store`]), so every node driving the tick writes one report and sends one
//! mail between them; and a week nobody was running for is simply not
//! reported, so missed runs collapse to the latest.
//!
//! A pass at `now`:
//! 1. names the last completed week, and waits until [`write_after`] past its
//!    end;
//! 2. writes that week's report unless one is there — on any day, so the
//!    console has a report from the first pass after a deploy;
//! 3. emails whoever is still owed it, but only until [`mail_until`] past the
//!    week's end. Mail goes out on the report's own Monday or not at all: a
//!    deploy on a Wednesday writes last week's report and mails nobody.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use futures::future::FutureExt;
use sea_orm::{DatabaseConnection, DbErr};

use crate::emails::usage_report::ReportMailer;

use super::collect;
use super::delivery::{self, Delivered};
use super::period::Period;
use super::store::{self, StoredReport};

/// The least time between two passes by one process.
const EVERY: Duration = Duration::from_secs(10 * 60);

/// How long after the week ends its report is written: Monday 01:00 UTC, which
/// is Monday morning in Asia and Sunday evening in the Americas — in the inbox
/// when the week starts, either way.
fn write_after() -> chrono::Duration {
    chrono::Duration::hours(1)
}

/// How long after the week ends its report may still be emailed: the whole of
/// that Monday. Long enough to ride out a mail outage or a late start; short
/// enough that a deploy mid-week, or someone made an admin on Thursday, does
/// not put a stale report in an inbox.
fn mail_until() -> chrono::Duration {
    chrono::Duration::hours(24)
}

static LAST_PASS: Mutex<Option<Instant>> = Mutex::new(None);

/// Whether a pass is due, claiming it if so. A poisoned lock passes: the work
/// is claimed in the database, and never running again is the worse failure.
fn due(now: Instant) -> bool {
    let mut last = LAST_PASS.lock().unwrap_or_else(|e| e.into_inner());
    let due = last.is_none_or(|at| now.duration_since(at) >= EVERY);
    if due {
        *last = Some(now);
    }
    due
}

/// The global driver's hook: run a pass if one is due, else return at once.
///
/// A panic stops here. This runs inside the loop that fires every schedule on
/// the deployment, and a report is not worth that loop.
pub(crate) async fn tick(db: &DatabaseConnection) {
    if !due(Instant::now()) {
        return;
    }
    let outcome = std::panic::AssertUnwindSafe(pass(db, Utc::now()))
        .catch_unwind()
        .await;
    match outcome {
        Ok(Ok(None)) => {}
        Ok(Ok(Some(done))) => tracing::info!(
            sent = done.sent,
            failed = done.failed,
            nothing_to_say = done.nothing_to_say,
            "usage report: emailed"
        ),
        Ok(Err(e)) => tracing::warn!(error = %e, "usage report: the pass failed"),
        Err(_) => tracing::error!("usage report: the pass panicked; the driver carries on"),
    }
}

/// One pass with this deployment's mailer. `None` when there was nothing to
/// send, nobody to send it to, or no way to send it.
async fn pass(db: &DatabaseConnection, now: DateTime<Utc>) -> Result<Option<Delivered>, DbErr> {
    let Some(report) = report_to_deliver(db, now).await? else {
        return Ok(None);
    };
    let readers = delivery::pending(db, &report).await?;
    if readers.is_empty() {
        return Ok(None);
    }
    let Some(mailer) = ReportMailer::for_schedule().await else {
        tracing::debug!("usage report: written, not emailed — this deployment sends no email");
        return Ok(None);
    };
    let console = oxy_app_core::custom_apps_host_dispatch::admin_base_url();
    let done = delivery::deliver(db, &report, readers, &mailer, console.as_deref()).await?;
    Ok(Some(done))
}

/// The report this pass should be emailing, writing it first if it is due and
/// missing. `None` before the week's report is due, and — the report written
/// all the same — once its Monday is over.
pub async fn report_to_deliver(
    db: &DatabaseConnection,
    now: DateTime<Utc>,
) -> Result<Option<StoredReport>, DbErr> {
    let period = Period::last_completed(now);
    if !is_due(&period, now) {
        return Ok(None);
    }
    let report = match store::for_period(db, period.start).await? {
        Some(report) => report,
        None => write(db, period).await?,
    };
    Ok(still_mailing(&period, now).then_some(report))
}

fn is_due(period: &Period, now: DateTime<Utc>) -> bool {
    now >= period.end + write_after()
}

fn still_mailing(period: &Period, now: DateTime<Utc>) -> bool {
    now < period.end + mail_until()
}

/// Read the week and store it. Whoever's insert lands, the row read back is the
/// report — two nodes writing at once agree on it.
async fn write(db: &DatabaseConnection, period: Period) -> Result<StoredReport, DbErr> {
    let snapshot = collect::collect(db, period).await?;
    if store::insert_if_absent(db, &snapshot).await? {
        tracing::info!(
            week = %period.label(),
            orgs = snapshot.orgs.len(),
            "usage report: written"
        );
        if let Err(e) = store::prune(db, period.start).await {
            tracing::warn!(error = %e, "usage report: old reports were not pruned");
        }
    }
    store::for_period(db, period.start)
        .await?
        .ok_or_else(|| DbErr::Custom("the usage report was not there after it was written".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(d: u32, h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, d, h, m, 0).unwrap()
    }

    #[test]
    fn a_pass_is_claimed_once_per_window() {
        // One process-wide throttle, so one test owns the whole sequence.
        let start = Instant::now();
        *LAST_PASS.lock().unwrap() = None;
        assert!(due(start), "the first tick passes");
        assert!(!due(start + Duration::from_secs(5)), "the next does not");
        assert!(!due(start + EVERY - Duration::from_secs(1)));
        assert!(due(start + EVERY), "a tick past the window passes again");
    }

    #[test]
    fn the_report_is_due_an_hour_into_monday() {
        // 2026-10-05 is a Monday; at 00:30 the week that just ended is named,
        // and it is not due yet.
        let just_ended = Period::last_completed(at(5, 0, 30));
        assert_eq!(just_ended.end, at(5, 0, 0));
        assert!(!is_due(&just_ended, at(5, 0, 30)));
        assert!(is_due(&just_ended, at(5, 1, 0)));
        // Any later day of the week still owes that same report.
        let thursday = at(8, 14, 0);
        assert!(is_due(&Period::last_completed(thursday), thursday));
    }

    #[test]
    fn a_report_is_mailed_on_its_own_monday_and_not_after() {
        let week = Period::last_completed(at(5, 1, 0));
        assert!(still_mailing(&week, at(5, 1, 0)));
        assert!(still_mailing(&week, at(5, 23, 59)));
        assert!(!still_mailing(&week, at(6, 0, 0)));
        // A deploy on Wednesday owes the report and mails nobody.
        let wednesday = at(7, 16, 0);
        let owed = Period::last_completed(wednesday);
        assert!(is_due(&owed, wednesday));
        assert!(!still_mailing(&owed, wednesday));
    }
}
