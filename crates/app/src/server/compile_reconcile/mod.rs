//! Keep a remote-backed workspace serving what its default branch has.
//!
//! Until this loop, a commit only reached what a workspace serves if someone
//! pulled in the IDE or clicked Compile. A workspace nobody touched went on
//! serving whatever was compiled last — on dev, one remote-backed workspace
//! was found serving a disk snapshot three and a half months old while its
//! files had long since been renamed.
//!
//! Each tick, for every workspace with a github.com remote and a recorded
//! default branch, this asks GitHub where the branch is (one small request)
//! and, when that is not the commit being served, queues a commit compile of
//! it (`server::compile_git`). It is the periodic half of the triggers in
//! `internal-docs/factory-retirement.md`, phase 1; there is no webhook yet.
//!
//! **Off unless `OXY_COMPILE_RECONCILE` is set.** Turning it on changes what
//! a deployment serves: a push to a default branch is promoted without anyone
//! clicking Compile. It is a rollout gate — dev, then staging — not a setting.
//!
//! Three properties the loop is built around:
//!
//! * **Once per workspace per interval, whatever the replica count.** A check
//!   is claimed with a compare-and-set on `workspace_compile_checks`
//!   ([`state`]); the replica that loses does not ask GitHub.
//! * **A budget on GitHub, and on the driver it runs in.** One request per
//!   workspace per [`CHECK_INTERVAL`], at most [`MAX_CHECKS_PER_TICK`] per
//!   tick, and a rate limit stops this process asking anything for
//!   [`RATE_LIMIT_PAUSE`]. A tick also ends at its first unanswered check and
//!   never runs past [`TICK_BUDGET`], because it shares a loop with schedules
//!   and health checks.
//! * **A broken repository cannot loop.** The compile goes through the same
//!   deduped enqueue as every automatic trigger, so one is never queued
//!   behind another and repeated failures back off.

mod check;
mod state;

use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use sea_orm::DatabaseConnection;

pub use check::Outcome;

/// The rollout gate. Read by [`enabled`] and nowhere else.
const GATE_ENV: &str = "OXY_COMPILE_RECONCILE";

/// How often one workspace's branch head is compared with what it serves.
///
/// Five minutes bounds how stale a pushed commit can be before it is noticed,
/// and costs twelve requests an hour per workspace against a GitHub App
/// allowance of 5,000 an hour per installation.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// The most workspaces one tick checks. A tick runs on the periodic driver's
/// cadence, far more often than [`CHECK_INTERVAL`], so in steady state only a
/// few are due at once; this bounds the burst after an outage, when all are.
const MAX_CHECKS_PER_TICK: i64 = 25;

/// How long this process stops checking after GitHub rate-limits it.
const RATE_LIMIT_PAUSE: Duration = Duration::from_secs(15 * 60);

/// The most time one tick may take. The tick runs inline on the periodic
/// driver, in the same pass that fires schedules, monitor scans, health checks
/// and pre-aggregation — so however slow GitHub is, this is all the delay it
/// can add to them. No check starts after the budget is spent, and a check
/// in flight is cut off when it runs out.
const TICK_BUDGET: Duration = Duration::from_secs(20);

/// Unix seconds before which this process asks GitHub nothing. Process-local
/// on purpose: a rate limit belongs to a token, and every replica that hits it
/// pauses itself.
static PAUSED_UNTIL: AtomicI64 = AtomicI64::new(0);

/// Whether the loop is switched on for this process.
pub fn enabled() -> bool {
    gate_is_on(|name| std::env::var(name).ok())
}

/// [`enabled`] over an injected lookup. Same truthy values as the drive
/// policy's gates.
fn gate_is_on(var: impl Fn(&str) -> Option<String>) -> bool {
    var(GATE_ENV).is_some_and(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
}

/// Say at boot whether this node will move what workspaces serve on its own.
pub fn announce() {
    if enabled() {
        tracing::info!(
            target: "compile_reconcile",
            interval_secs = CHECK_INTERVAL.as_secs(),
            "{GATE_ENV} is set: this node compares each remote-backed workspace's default \
             branch with the revision it serves, and queues a commit compile when they differ"
        );
    }
}

/// What one tick did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Tick {
    /// Checks this replica claimed and ran.
    pub checked: usize,
    /// Of those, how many handed a compile to the enqueue path.
    pub enqueued: usize,
    /// Whether the tick ended early on a rate limit.
    pub rate_limited: bool,
}

/// One pass of the loop. Does nothing at all — no query, no request — unless
/// the gate is on.
pub async fn tick(db: &DatabaseConnection) -> Tick {
    if !enabled() || paused() {
        return Tick::default();
    }
    let interval = CHECK_INTERVAL.as_secs() as i64;
    if let Err(e) = state::seed(db, interval).await {
        tracing::warn!(target: "compile_reconcile", error = %e, "seeding checks failed");
    }
    let due = match state::due(db, MAX_CHECKS_PER_TICK).await {
        Ok(due) => due,
        Err(e) => {
            tracing::warn!(target: "compile_reconcile", error = %e, "reading due checks failed");
            return Tick::default();
        }
    };

    let mut tick = Tick::default();
    let started = std::time::Instant::now();
    for due in due {
        if started.elapsed() >= TICK_BUDGET {
            // The rest are still due and are taken up on the next tick.
            break;
        }
        if !state::claim(db, &due, interval).await {
            // Another replica took this check, and will ask GitHub.
            continue;
        }
        // What is left of the budget, not a fresh one: a check that starts
        // late must not get to run the tick to twice its length.
        let remaining = TICK_BUDGET.saturating_sub(started.elapsed());
        let checking = check::check(db, due.workspace_id, due.last_head_sha.as_deref());
        let outcome = tokio::time::timeout(remaining, checking)
            .await
            .unwrap_or_else(|_| Outcome::Unavailable("the check ran out of time".into()));
        report(due.workspace_id, &outcome);
        let backoff = outcome.backoff().as_secs() as i64;
        state::record(
            db,
            due.workspace_id,
            outcome.label(),
            outcome.head(),
            backoff,
        )
        .await;
        tick.checked += 1;
        match outcome {
            Outcome::Enqueued { .. } => tick.enqueued += 1,
            Outcome::RateLimited => {
                pause();
                tick.rate_limited = true;
                break;
            }
            // GitHub (or Postgres) is struggling. Asking about the next
            // workspace would most likely wait out another timeout, on a
            // loop other work is queued behind. No pause: the next tick tries
            // again, one check at a time, until an answer comes back.
            Outcome::Unavailable(_) => break,
            _ => {}
        }
    }
    tick
}

fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

fn paused() -> bool {
    now_secs() < PAUSED_UNTIL.load(Ordering::Relaxed)
}

fn pause() {
    PAUSED_UNTIL.store(
        now_secs() + RATE_LIMIT_PAUSE.as_secs() as i64,
        Ordering::Relaxed,
    );
    tracing::warn!(
        target: "compile_reconcile",
        pause_secs = RATE_LIMIT_PAUSE.as_secs(),
        "GitHub rate-limited a branch check; this node asks nothing more until the pause ends"
    );
}

/// One line per check, at the level its outcome deserves. The outcomes a
/// person has to act on are warnings, and each is followed by a backoff, so a
/// workspace warns once per backoff window rather than once per tick.
fn report(workspace_id: uuid::Uuid, outcome: &Outcome) {
    match outcome {
        Outcome::UpToDate { .. } | Outcome::AlreadyCompiled { .. } => tracing::debug!(
            target: "compile_reconcile", %workspace_id, outcome = outcome.label(), "checked"
        ),
        Outcome::Enqueued { head } => tracing::info!(
            target: "compile_reconcile", %workspace_id, %head,
            "the default branch moved; a commit compile of its head was handed to the queue"
        ),
        // Logged once, by `pause`.
        Outcome::RateLimited => {}
        Outcome::Uncheckable(why) => tracing::warn!(
            target: "compile_reconcile", %workspace_id, why,
            backoff_secs = outcome.backoff().as_secs(),
            "skipping this workspace: it cannot be checked"
        ),
        Outcome::NoToken | Outcome::NotFound | Outcome::Denied => tracing::warn!(
            target: "compile_reconcile", %workspace_id, outcome = outcome.label(),
            backoff_secs = outcome.backoff().as_secs(),
            "skipping this workspace: GitHub cannot be asked about its branch — check its \
             GitHub connection and that the repository and branch still exist"
        ),
        Outcome::Unavailable(error) => tracing::warn!(
            target: "compile_reconcile", %workspace_id, %error,
            "a branch check failed; it is retried on the next interval"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gate(value: Option<&str>) -> bool {
        gate_is_on(|name| {
            assert_eq!(name, "OXY_COMPILE_RECONCILE");
            value.map(str::to_string)
        })
    }

    #[test]
    fn the_gate_is_off_unless_it_is_set_to_something_true() {
        for off in [
            None,
            Some(""),
            Some("0"),
            Some("false"),
            Some("no"),
            Some("TRUE"),
        ] {
            assert!(!gate(off), "{off:?}");
        }
        for on in ["1", "true", "yes", "on"] {
            assert!(gate(Some(on)), "{on}");
        }
    }
}
