//! Which Global-queue runs THIS process drives.
//!
//! One pure decision ([`drive_policy_for`]) over the process role and two
//! opt-in env gates, read in one place ([`IdeDeferral::from_env`]). The latency
//! worker reads the result once, at spawn, and hands the same value to its
//! probe and to the gate in front of the driver lease on every tick, so the
//! two cannot filter on different rules. The predicate itself is
//! `agentic_pipeline::recovery::may_drive`.

use agentic_pipeline::recovery::{DrivePolicy, STRANDED_GRACE_SECS};
use agentic_runtime::coordinator::{AIRWAY_SOURCE_TYPE, COMPILE_SOURCE_TYPE};

use crate::server::role_manifest::{Role, current_process_role};

/// Opt-in: make the `ide` singleton hand **airway** runs to the worker fleet
/// instead of driving them itself. Narrow on purpose — see
/// [`IDE_DEFER_QUEUE_WORK_ENV`] for the general form.
///
/// Default **off**, and that default is the safety property, not laziness. An
/// `ide` + `serve` deployment with no worker replicas has no other driver for
/// a `Global` airway run, so switching this on there costs every pipeline a
/// grace window before the `ide` takes it anyway. Off by default means such a
/// deployment behaves exactly as it does today; an operator who has a worker
/// fleet turns it on and gets the placement they deployed the fleet for.
const IDE_DEFER_AIRWAY_ENV: &str = "OXY_IDE_DEFER_AIRWAY";

/// Opt-in: make the `ide` singleton drive **only** the kinds that need it
/// ([`FACTORY_ONLY_KINDS`]) and leave every other Global run for the worker
/// fleet — automations, pre-aggregation cycles, scheduled custom-app
/// functions, health evals, airway syncs, and any kind added later.
///
/// **A temporary rollout gate.** The intended end state is that an `ide`
/// behaves this way by default; it is a flag only because turning it on moves
/// memory. Everything the Factory stops running lands on the workers, so the
/// fleet has to be sized for it first — a deployment whose workers were sized
/// to carry airway alone will OOM them on the rest. Flip the default, and
/// delete this gate, once worker sizing is settled.
///
/// Default **off**: a deployment that sets nothing behaves exactly as before.
/// Supersedes [`IDE_DEFER_AIRWAY_ENV`] when both are set (airway is one of the
/// kinds it defers).
///
/// Wants a worker fleet but does not hard-require one: a run nobody claims
/// within [`STRANDED_GRACE_SECS`] is driven here after all, so a missing or
/// wedged fleet costs latency, never the run. No effect on any role but `ide`.
const IDE_DEFER_QUEUE_WORK_ENV: &str = "OXY_IDE_DEFER_QUEUE_WORK";

/// The kinds a deferring `ide` keeps: work that cannot run anywhere else.
///
/// - **`compile`** reads the workspace working copy, which is a property of
///   the ROLE (#2822) — every other node declines it, so if the `ide` did too
///   nothing would ever compile.
///
/// Every other kind on the Global queue is one a worker already drives today
/// (a worker declines `compile` and nothing else), so no other *kind* belongs
/// here by construction. A kind that turns out to need the Factory's disk
/// joins this list **and** the workers' exclusion in [`drive_policy_for`] —
/// one without the other either strands it or runs it where it fails.
///
/// **`workflow` is deliberately not here, and it is the one to know about.**
/// No automation needs the Factory as a kind, but three step shapes read or
/// write the working copy and so behave differently on a worker:
///
/// - a `sql_file` step the compile boundary does not serve (`modeling/**`,
///   `schemas/**`) fails with "this node holds no workspace files";
/// - `export:` / `cache:` steps write under a workspace root that exists only
///   on that worker's ephemeral disk (the known violation recorded in
///   `tests/platform/workspace_path_needs_is_dir.rs`);
/// - an `agent_ref: __builder__` step runs the file-editing builder on a node
///   that has no working copy to edit.
///
/// None of that is new: a worker already drives any automation it wins the
/// race for. What this gate changes is the odds — from "whichever node polled
/// first" to "a worker, always" — so an automation that leans on one of these
/// and has been passing on the runs the `ide` happened to win will stop. Keeping
/// `workflow` here would hide that at the cost of the gate's main purpose
/// (automations are the heavy work), and a source-type gate cannot tell the
/// affected automations from the rest anyway.
const FACTORY_ONLY_KINDS: &[&str] = &[COMPILE_SOURCE_TYPE];

/// The two `ide` deferral gates, as set for this process.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct IdeDeferral {
    /// [`IDE_DEFER_AIRWAY_ENV`].
    pub airway: bool,
    /// [`IDE_DEFER_QUEUE_WORK_ENV`].
    pub queue_work: bool,
}

impl IdeDeferral {
    /// The only reader of either env var.
    fn from_env() -> Self {
        Self::read(|name| std::env::var(name).ok())
    }

    /// [`Self::from_env`] over an injected lookup, so a test can exercise the
    /// names and the parsing without mutating the process environment.
    fn read(var: impl Fn(&str) -> Option<String>) -> Self {
        let on = |name: &str| {
            var(name).is_some_and(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
        };
        Self {
            airway: on(IDE_DEFER_AIRWAY_ENV),
            queue_work: on(IDE_DEFER_QUEUE_WORK_ENV),
        }
    }
}

/// The policy **this** process applies when it selects Global work.
///
/// Read once, when the latency worker is spawned, and passed down every tick —
/// never re-read between the probe and the gate. They filter at different
/// layers on purpose (see the comment at the probe in `tick_cloud`), but they
/// must filter on the *same rule*: a probe that skips less than the gate
/// merely wastes a workspace visit, while a probe that skips **more** hides
/// work the gate would have driven, and nothing fails — it just silently stops
/// running.
pub(super) fn drive_policy() -> DrivePolicy {
    drive_policy_for(current_process_role(), IdeDeferral::from_env())
}

/// The decision behind [`drive_policy`], as a pure function.
///
/// Split out because `PROCESS_ROLE` is a `OnceLock` — a process has exactly one
/// role for its whole life, so a test cannot exercise the other three through
/// the real reader. Same reason `agentic_pipeline::recovery::may_drive` is
/// public: the test drives the predicate production uses instead of a copy that
/// keeps passing after the call site stops applying it.
///
/// Three different reasons to leave a run, and they must not be collapsed:
///
/// - **`compile` on `worker` / `serve`** — a node that *cannot* run it. Absolute
///   ([`DrivePolicy::Except`]): waiting does not produce a working copy.
/// - **`airway` on a deferring `ide`** — the one node that *can*, declining as
///   a placement preference. Airway submit routes are `IdeOnly`, so every
///   interactive pipeline is enqueued by the IDE singleton, whose own latency
///   worker polls the same queue at the same interval as the fleet's
///   (`OXY_LATENCY_WORKER_INTERVAL_MS`, default 1 s). Left to race, the pod
///   that just accepted the submit often wins and a memory-heavy pipeline
///   executes in the pod least able to afford it.
///   [`DrivePolicy::Defer`]: the deny-list form of the preference, fallback
///   included.
/// - **everything but `compile` on an `ide` deferring queue work** — the same
///   preference, generalised to an allow-list. [`DrivePolicy::Only`].
///
/// `Role::All` never defers, whatever is set: that is the single-process
/// deployment, where the ide *is* the fleet. Hence the arms key on `Role::Ide`
/// and not on `process_can_compile()`, which is true for both.
///
/// **Neither preference can strand a run**, and both fall back the same way:
/// `may_drive` opens for a deferred kind once it has gone unclaimed for
/// [`STRANDED_GRACE_SECS`], on the `unclaimed_secs` clock both selections
/// report. Every loop that drives Global work applies this one policy — the
/// latency worker's probe and gate, and the periodic stranded tick
/// (`recover_stranded_runs`), which could not stay outside it: that tick is
/// the pass that reaps a dead worker's claim and then selects the freed run,
/// so ungated it handed an OOM-killed airway pipeline to the very node
/// configured not to run it. The one-shot startup pass reaps too, and applies
/// the policy to the roots the latency worker can also see (a `queued` Global
/// row) — never to the rest, which no other loop would pick up. The tick
/// alone was also never a complete net:
/// `find_stuck_runs` selects `workflow` and `airway` **only**, so a declined
/// `preagg_cycle` or `app_function` was invisible to it at any age. Pinned by
/// `an_unclaimed_custom_run_is_invisible_to_the_periodic_tick` and
/// `the_periodic_tick_reads_the_same_clock_as_the_latency_worker` (runtime),
/// `a_deferring_node_takes_unclaimed_work_after_the_grace` and
/// `a_deferring_node_leaves_a_reaped_pipeline_for_the_fleet_first` (pipeline).
///
/// The fleet and the deferring node cannot fight over a normal submit: a
/// worker's latency loop claims within a tick, and both the claim and the
/// driver lease then exclude the run. What is left is exactly the case worth
/// a fallback — *nobody took this for thirty seconds* — so either gate on a
/// deployment whose worker fleet is missing or wedged degrades to slower
/// placement, not a stall.
pub(super) fn drive_policy_for(role: Role, defer: IdeDeferral) -> DrivePolicy {
    const AIRWAY_ONLY: &[&str] = &[AIRWAY_SOURCE_TYPE];
    const COMPILE_ONLY: &[&str] = &[COMPILE_SOURCE_TYPE];

    match role {
        // The broader gate wins: airway is among the kinds it defers.
        Role::Ide if defer.queue_work => DrivePolicy::Only(FACTORY_ONLY_KINDS),
        Role::Ide if defer.airway => DrivePolicy::Defer(AIRWAY_ONLY),
        Role::Ide | Role::All => DrivePolicy::ALL,
        // `Serve` reaches here only if something turned its driver on
        // explicitly (`role_runs_inprocess_workers` is false for it); the
        // compile exclusion is right for it either way, since it owns no
        // working copy.
        Role::Worker | Role::Serve => DrivePolicy::Except(COMPILE_ONLY),
    }
}

/// Say out loud, once at boot, what this node will not drive.
///
/// Every decline is silent by construction otherwise: work simply stops being
/// picked up here, and the only other trace is a per-tick DEBUG line. Both
/// deferral gates have a deployment prerequisite (a worker fleet should
/// exist), so an operator who set one on a fleetless install needs to be able
/// to find this in the boot log.
pub(super) fn announce(policy: DrivePolicy) {
    let role = current_process_role().as_str();
    match policy {
        DrivePolicy::Except([]) => {}
        DrivePolicy::Except(excluded) => tracing::info!(
            target: "recovery",
            role,
            excluded = ?excluded,
            "this node declines these run kinds at selection; another node must \
             drive them or they stay queued"
        ),
        DrivePolicy::Defer(deferred) => tracing::info!(
            target: "recovery",
            role,
            deferred = ?deferred,
            grace_secs = STRANDED_GRACE_SECS,
            "this node leaves these run kinds for the worker fleet at selection, \
             and drives one here only once it has gone unclaimed for the grace"
        ),
        DrivePolicy::Only(only) => tracing::info!(
            target: "recovery",
            role,
            only = ?only,
            grace_secs = STRANDED_GRACE_SECS,
            "this node drives only these run kinds at selection; every other \
             kind is left for the worker fleet, and driven here only once it \
             has gone unclaimed for the grace"
        ),
    }
}

#[cfg(test)]
mod tests;
