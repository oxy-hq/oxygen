//! `ctx.platform.facts()`'s data: ids and counts about every org on the
//! deployment, for an internal app (the accounts view staff read).
//!
//! **Ids and counts only.** No email, message or thread text, SQL, customer row,
//! error text or file name ever leaves here: every query below selects ids,
//! names, timestamps and `COUNT`s, and nothing else.
//!
//! **The gate is the deployment's internal orgs**
//! (`oxy_app_core::internal_orgs`, `OXY_INTERNAL_ORGS`): comma-separated org
//! UUIDs, unset meaning off everywhere. Only an app whose org (taken from the
//! host, never from the function) is internal may read the facts; the host op
//! adds the manifest capability on top. The same list defines **staff**: anyone
//! who is a member of an internal org. Staff are left out of every `*People` count
//! (an app's `people7d` included), so our own opens of a customer's app never
//! read as the customer's use, and out of `lastActivityAt` and an app's
//! `customerLastActiveAt`, which are the customer's own activity only. They
//! still count in the event totals (`appViews`, `threads`, …), which say how
//! much happened, not who did it, and in an app's `lastActiveAt`, its last open.
//!
//! **Only production counts**, as in the usage report
//! (`admin::usage_report::collect`): a view or a function invocation in staging
//! or a sandbox is a developer trying a build, not usage.
//!
//! **Weeks are completed ISO weeks**, Monday 00:00 UTC to the next, oldest
//! first; the week in progress is never one of them. `last7d` is the rolling
//! window `now - 7 days .. now` beside them.
//!
//! **LLM cost is `agentRuns`' cost** (`llm_cost`): the runs a window counts in
//! `agentRuns`, each with every run it delegated to, priced the way Admin →
//! Metrics prices them. Dollars, not text: no prompt or answer is read.

mod app_use;
mod assemble;
mod cache;
mod llm_cost;
mod queries;
mod serve;

pub use serve::{
    BAD_WEEKS, CAPABILITY_MISSING, UNAVAILABLE, admit, admit_org, cached_reply, load_reply,
    platform_facts_gate, requested_weeks,
};

use chrono::{DateTime, Duration, NaiveDate, NaiveTime, Utc};
use oxy_shared::errors::OxyError;
use sea_orm::DatabaseConnection;
use serde::Serialize;
use uuid::Uuid;

use oxy_app_core::usage_period::Period;

/// Weeks returned when the caller names none.
pub const DEFAULT_WEEKS: u32 = 8;

/// The most weeks one call returns: half a year, which bounds every windowed
/// scan below.
pub const MAX_WEEKS: u32 = 26;

/// How far back `lastActivityAt` and an app's `customerLastActiveAt` look, in
/// days. Customer activity older than this reads as none (`null`). It is what
/// bounds the probes behind them: each
/// skips staff's and unattended rows, so without a floor a churned customer
/// whose webhooks keep arriving would have every build walk every invocation
/// since their last attributed one, a count that only grows.
pub const LAST_ACTIVITY_HORIZON_DAYS: i64 = 180;

/// The gate: may an app owned by `org_id` read the facts? Only when the org is
/// one of `internal` (`oxy_app_core::internal_orgs`). Fails closed: a
/// deployment with no internal orgs refuses everyone.
pub fn facts_enabled_for(org_id: Uuid, internal: &[Uuid]) -> bool {
    internal.contains(&org_id)
}

/// `weeks` held to `1..=MAX_WEEKS`.
pub fn clamp_weeks(weeks: u32) -> u32 {
    weeks.clamp(1, MAX_WEEKS)
}

/// The Monday (UTC) of each of the last `n` weeks that had fully ended at
/// `now`, oldest first.
pub fn completed_week_starts(now: DateTime<Utc>, n: u32) -> Vec<NaiveDate> {
    // The usage report's week, so the two never disagree on where one starts.
    let last = Period::last_completed(now).start.date_naive();
    (0..n)
        .rev()
        .map(|back| last - Duration::weeks(i64::from(back)))
        .collect()
}

/// Everything `ctx.platform.facts()` returns. Field names and nesting are a
/// contract with the app that reads them (`OxyPlatformFacts` in the SDK).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OxyPlatformFacts {
    pub generated_at: DateTime<Utc>,
    /// The Monday of each completed week, oldest first.
    pub weeks: Vec<NaiveDate>,
    /// Every org on the deployment, by name.
    pub orgs: Vec<OxyPlatformOrg>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OxyPlatformOrg {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
    /// One of the deployment's internal orgs (`OXY_INTERNAL_ORGS`).
    pub internal: bool,
    /// The partner that manages this org (`partner_orgs`).
    pub partner_org_id: Option<Uuid>,
    /// Every `org_members` row, staff who belong to the org included: billing's
    /// seat count (`oxy_billing::billable_seats`), not the customer's people.
    pub members: u64,
    pub workspaces: u64,
    pub billing: OxyPlatformBilling,
    /// The latest sign-in of any of the org's members, staff included. A
    /// member who never signed in reads as their user row's creation.
    pub last_sign_in_at: Option<DateTime<Utc>>,
    /// The latest customer activity within [`LAST_ACTIVITY_HORIZON_DAYS`]: a
    /// production app view or a thread by a non-staff person, or a production
    /// function invocation a non-staff person made. Staff, unattended runs (no
    /// user) and threads nobody is attributed to never move it; `None` when
    /// the customer did nothing inside the horizon.
    pub last_activity_at: Option<DateTime<Utc>>,
    pub apps: Vec<OxyPlatformApp>,
    pub milestones: OxyPlatformMilestones,
    /// One entry per [`OxyPlatformFacts::weeks`], in the same order.
    pub weekly: Vec<OxyPlatformWeek>,
    /// The rolling seven days up to `generatedAt`.
    #[serde(rename = "last7d")]
    pub last7d: OxyPlatformWeek,
}

/// The org's `org_billing` row; every field null when it has none.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OxyPlatformBilling {
    pub status: Option<String>,
    pub seats_paid: Option<i64>,
    pub grace_ends_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OxyPlatformApp {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub published_at: Option<DateTime<Utc>>,
    /// The latest production view, staff's included, as the admin apps list
    /// reports it: the app's last open, by anyone.
    pub last_active_at: Option<DateTime<Utc>>,
    /// The latest production view by a non-staff person within
    /// [`LAST_ACTIVITY_HORIZON_DAYS`]: the customer's own last use. Staff
    /// opening the app never move it; `None` when no customer opened it inside
    /// the horizon. The org's `last_activity_at` is at least this.
    pub customer_last_active_at: Option<DateTime<Utc>>,
    /// Distinct non-staff people with a production view in `last7d`'s window
    /// (`now - 7 days .. now`).
    #[serde(rename = "people7d")]
    pub people7d: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OxyPlatformMilestones {
    pub first_workspace_at: Option<DateTime<Utc>>,
    pub first_app_published_at: Option<DateTime<Utc>>,
    pub first_thread_at: Option<DateTime<Utc>>,
}

/// One window's counts: a completed week, or the last seven days.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OxyPlatformWeek {
    /// Distinct non-staff people with a production app view or a thread.
    pub active_people: u64,
    /// Distinct non-staff people with a production app view.
    pub app_people: u64,
    /// Production app views, staff included.
    pub app_views: u64,
    /// Distinct non-staff people who started a thread.
    pub chat_people: u64,
    pub threads: u64,
    /// Top-level customer agent runs (no platform daemon, no staff preview).
    pub agent_runs: u64,
    pub agent_run_failures: u64,
    /// US dollars of LLM usage by this window's `agent_runs` and every run they
    /// delegated to, priced as Admin → Metrics prices it. So the platform's
    /// own runs and staff's previews and dry runs are left out, as they are
    /// from `agent_runs`, while a staff member's ordinary run in a customer's
    /// workspace counts, as it does there. A model the price table lacks adds
    /// nothing; see `llm_unpriced_runs`.
    pub llm_cost_usd: f64,
    /// Runs among those that made LLM calls on a model the price table lacks,
    /// so their tokens are not in `llm_cost_usd`. A delegated run is a run of
    /// its own here, as Admin → Metrics counts runs, so this is not a share of
    /// `agent_runs`; non-zero means the cost reads low.
    pub llm_unpriced_runs: u64,
    /// Production function invocations.
    pub function_calls: u64,
    pub function_failures: u64,
    /// Promotions to production.
    pub releases: u64,
}

/// The time windows every count is taken over: one per completed week, then
/// the last seven days. A row's position here is its `idx` in the queries.
#[derive(Clone, Debug, PartialEq)]
struct Windows {
    starts: Vec<DateTime<Utc>>,
    ends: Vec<DateTime<Utc>>,
}

impl Windows {
    fn new(week_starts: &[NaiveDate], now: DateTime<Utc>) -> Self {
        let mut starts: Vec<DateTime<Utc>> = week_starts
            .iter()
            .map(|day| day.and_time(NaiveTime::MIN).and_utc())
            .collect();
        let mut ends: Vec<DateTime<Utc>> = starts.iter().map(|s| *s + Duration::weeks(1)).collect();
        starts.push(now - Duration::days(7));
        ends.push(now);
        Self { starts, ends }
    }

    fn len(&self) -> usize {
        self.starts.len()
    }

    /// The earliest instant any window covers: the bound every scan is cut to.
    fn lo(&self) -> DateTime<Utc> {
        self.starts.iter().min().copied().unwrap_or_default()
    }

    fn hi(&self) -> DateTime<Utc> {
        self.ends.iter().max().copied().unwrap_or_default()
    }
}

/// The facts for `weeks` completed weeks (held to `1..=MAX_WEEKS`) as of `now`.
/// `internal` is the deployment's internal orgs: it marks orgs `internal` and
/// defines staff.
pub async fn build_platform_facts(
    db: &DatabaseConnection,
    internal: &[Uuid],
    weeks: u32,
    now: DateTime<Utc>,
) -> Result<OxyPlatformFacts, OxyError> {
    let weeks = completed_week_starts(now, clamp_weeks(weeks));
    let windows = Windows::new(&weeks, now);
    let activity_since = now - Duration::days(LAST_ACTIVITY_HORIZON_DAYS);
    let sources = queries::read_all(db, internal, &windows, activity_since)
        .await
        .map_err(|e| OxyError::DBError(format!("platform facts: {e}")))?;
    Ok(OxyPlatformFacts {
        generated_at: now,
        orgs: assemble::orgs(sources, internal, windows.len()),
        weeks,
    })
}

#[cfg(test)]
mod assemble_tests;
#[cfg(test)]
mod serve_tests;
#[cfg(test)]
mod tests;
