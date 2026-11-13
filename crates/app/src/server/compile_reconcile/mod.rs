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
//! `internal-docs/factory-retirement.md`, phase 1. Its other half is the GitHub
//! `push` webhook ([`push`]), which reaches the same enqueue sooner.
//!
//! **Off unless the `compile_reconcile` feature flag is on** (or, for one more
//! release, `OXY_COMPILE_RECONCILE` is set: `feature_flags::rollout`), and the
//! webhook obeys the same gate. Turning it on changes what a deployment serves:
//! a push to a default branch is promoted without anyone clicking Compile. It is
//! a rollout gate — dev, then staging — not a setting, and the registry names
//! its sunset.
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
pub mod default_branch;
pub mod env_pinner;
pub mod env_pins;
pub mod push;
mod state;

use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use sea_orm::DatabaseConnection;

pub use check::Outcome;

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

/// The share of [`TICK_BUDGET`] the default-branch pass may spend. Each lookup
/// in it is asked with what is left of this, so one slow GitHub answer ends
/// the pass instead of the tick; the checks get the rest.
const DEFAULT_BRANCH_BUDGET: Duration = Duration::from_secs(5);

/// Unix seconds before which this process asks GitHub nothing. Process-local
/// on purpose: a rate limit belongs to a token, and every replica that hits it
/// pauses itself.
static PAUSED_UNTIL: AtomicI64 = AtomicI64::new(0);

/// Whether the loop is switched on for this process: the `compile_reconcile`
/// flag, or its deprecated environment override
/// ([`rollout::COMPILE_RECONCILE`](crate::server::feature_flags::rollout::COMPILE_RECONCILE)).
pub fn enabled() -> bool {
    use crate::server::feature_flags::rollout;
    rollout::is_on(&rollout::COMPILE_RECONCILE)
}

/// Say at boot whether this node will move what workspaces serve on its own.
pub fn announce() {
    if enabled() {
        tracing::info!(
            target: "compile_reconcile",
            interval_secs = CHECK_INTERVAL.as_secs(),
            "compile_reconcile is on: this node compares each remote-backed workspace's default \
             branch with the revision it serves, and queues a commit compile when they differ"
        );
    }
}

/// Ask for `workspace_id`'s branch to be checked at the next tick instead of
/// when its interval comes round. For a workspace a request found with nothing
/// compiled: its first compile is a commit compile of its default branch, and
/// the check is what finds the head and queues it.
///
/// Does nothing with the gate off, and what it does is bounded (see
/// [`state::pull_forward`]): it moves a due time, it does not ask GitHub.
///
/// Returns whether a check **will** look at the workspace. `false` means no
/// commit compile can come of it (the gate is off, the workspace is not ready,
/// or its last check found something only a person can fix), and the caller
/// has to compile some other way or the workspace waits for nothing.
pub async fn request_check(db: &DatabaseConnection, workspace_id: uuid::Uuid) -> bool {
    if !enabled() {
        return false;
    }
    match state::pull_forward(db, workspace_id).await {
        Ok(pull) => {
            if pull.moved {
                tracing::info!(
                    target: "compile_reconcile",
                    %workspace_id,
                    "a request found nothing compiled; the branch check is due at the next tick"
                );
            }
            pull.will_run
        }
        Err(e) => {
            tracing::warn!(
                target: "compile_reconcile", %workspace_id, error = %e,
                "bringing a branch check forward failed"
            );
            false
        }
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
    // The budget covers the whole tick, the default-branch pass included.
    let started = std::time::Instant::now();
    // A remote-backed workspace with no recorded default branch gets no check
    // row; learn the branch from GitHub first (the Factory's fetch sweep used
    // to record it from the checkout). The futures are lazy: the checks start
    // only once the pass is over.
    sequence(default_branch_pass(db), check_due(db, started)).await
}

/// The default-branch pass, then the checks — always both. Whatever the pass
/// came to (GitHub unwell, its budget spent, a rate limit), the checks of
/// workspaces that already have a branch still run: a repository the pass
/// keeps failing on must not stop every other workspace being checked. A rate
/// limit pauses this process, which the check loop honours before each check.
async fn sequence(
    pass: impl std::future::Future<Output = default_branch::Pass>,
    checks: impl std::future::Future<Output = Tick>,
) -> Tick {
    let pass = pass.await;
    if pass.rate_limited {
        pause();
    } else if pass.stopped {
        tracing::info!(
            target: "compile_reconcile",
            recorded = pass.recorded,
            "the default-branch pass stopped early; the rest are asked on a later tick"
        );
    }
    let mut tick = checks.await;
    tick.rate_limited |= pass.rate_limited;
    tick
}

/// [`default_branch::record_missing`] within [`DEFAULT_BRANCH_BUDGET`]. The
/// outer timeout is a backstop for Postgres (each lookup already ends inside
/// the budget), a second over so an in-flight lookup is cut off — and its
/// workspace stamped — by the pass itself.
async fn default_branch_pass(db: &DatabaseConnection) -> default_branch::Pass {
    let backstop = DEFAULT_BRANCH_BUDGET + Duration::from_secs(1);
    tokio::time::timeout(
        backstop,
        default_branch::record_missing(db, DEFAULT_BRANCH_BUDGET),
    )
    .await
    .unwrap_or_else(|_| {
        tracing::warn!(target: "compile_reconcile", "the default-branch pass ran out of time");
        default_branch::Pass {
            stopped: true,
            ..Default::default()
        }
    })
}

/// Seed check rows, then run the due checks with what is left of the tick's
/// budget since `started`.
async fn check_due(db: &DatabaseConnection, started: std::time::Instant) -> Tick {
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
    for due in due {
        if paused() {
            // A rate limit (here or in the default-branch pass): ask nothing.
            break;
        }
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

#[cfg(test)]
#[path = "tick_tests.rs"]
mod tick_tests;

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
