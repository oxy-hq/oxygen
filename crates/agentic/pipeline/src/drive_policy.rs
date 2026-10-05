//! Which runs a process takes off the Global queue, and when.
//!
//! One typed policy, one predicate. The host decides the policy once per
//! process (for Oxy: `oxy_app::server::router::drive_policy`); everything that
//! selects Global work — the latency worker's probe, the gate in
//! [`recover_pending_global_runs`](crate::recovery::recover_pending_global_runs)
//! the periodic stranded tick
//! [`recover_stranded_runs`](crate::recovery::recover_stranded_runs) and the
//! queued-Global share of the one-shot startup pass
//! [`recover_active_runs`](crate::recovery::recover_active_runs) — splits its
//! selection with [`partition_drivable`], so no two of them can filter on
//! different rules.

use agentic_runtime::crud::StuckRun;

/// How long a run may sit unclaimed before it is anyone's to drive.
///
/// Two fallbacks share the number, because they are the same promise made at
/// two layers — *work nobody took for this long is taken by whoever can*:
///
/// - the periodic stranded tick (`recover_stranded_runs`), where it is the
///   grace on `find_stuck_runs`: a run untouched this long with no live queue
///   entry is genuinely stranded, not a worker mid-commit between its state
///   write and its enqueue;
/// - [`DrivePolicy::Only`], where it is how long a node that would rather leave
///   a run for the fleet waits before concluding the fleet is not coming.
pub const STRANDED_GRACE_SECS: u64 = 30;

/// Which run kinds this process drives from the Global queue.
///
/// Matched on `agentic_runs.source_type`, which is open-ended: `compile`,
/// `airway`, `workflow`, `analytics`, the system kind `preagg_cycle`, every
/// `TaskSpec::Custom` kind a host registers, and rows with none at all. That
/// openness is why the placement form comes as both a deny-list and an
/// allow-list — a deny-list cannot say "only compile" without naming every
/// kind that exists or ever will.
///
/// Two of the three forms are *preferences* and carry a fallback; one is a
/// *capability* and does not. The split is the whole model: a preference left
/// unanswered must still be work somebody does, a missing capability never
/// becomes one by waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrivePolicy {
    /// Drive everything **except** these kinds, and never those — however long
    /// they wait.
    ///
    /// The absolute form, for a node that *cannot* run a kind (a worker has no
    /// working copy to compile from). Waiting does not grow a capability, so
    /// age is ignored: an hour-old compile is still not a worker's to take.
    Except(&'static [&'static str]),
    /// Drive everything except these kinds **while they are fresh**; take one
    /// of them too once it has gone unclaimed for [`STRANDED_GRACE_SECS`].
    ///
    /// The placement form as a deny-list, for a node that *can* run a kind but
    /// should not be where it lands by default (the `ide` singleton and
    /// `airway`). The fallback is what keeps a preference from becoming a
    /// stall: with no fleet to take the run it is driven here ~30 s late, not
    /// left queued forever.
    Defer(&'static [&'static str]),
    /// Drive **only** these kinds straight away. Every other kind is left for
    /// another node, and taken here only once it has gone unclaimed for
    /// [`STRANDED_GRACE_SECS`].
    ///
    /// The placement form as an allow-list — [`DrivePolicy::Defer`] turned
    /// around, for when the kinds to keep are the short list and the kinds to
    /// leave are "everything else, including what does not exist yet".
    ///
    /// The fallback lives in this predicate, for both placement forms, rather
    /// than on the periodic stranded tick, because that tick cannot provide it
    /// alone. `find_stuck_runs` is scoped to `workflow` + `airway` on purpose
    /// (it also selects runs with **no** queue row, and re-driving an analytics
    /// run from there double-spends its LLM calls), so a declined
    /// `preagg_cycle`, `app_function` or `health_eval_workspace` is invisible to
    /// it at any age. The tick applies this same predicate to what it does
    /// select — it is the pass that reaps a dead worker's claim, and it must not
    /// then drive what it freed on a node that defers it.
    Only(&'static [&'static str]),
}

impl DrivePolicy {
    /// No preference and no missing capability: drive whatever is queued.
    pub const ALL: DrivePolicy = DrivePolicy::Except(&[]);
}

/// May this process drive this run now?
///
/// Public and separate so the test suite exercises the predicate the driver
/// actually uses, rather than a copy of it. A test that re-implements the rule
/// it is checking passes just as happily when the production call site stops
/// applying it — which is the failure mode this gate has already had twice, in
/// two different layers.
///
/// `unclaimed_secs` is [`StuckRun::unclaimed_secs`]: how long the run has been
/// selectable with nobody taking it.
///
/// A `None` source_type is never *excluded* — absent is not on any deny-list —
/// and never *listed* either, so under [`DrivePolicy::Only`] it is left for the
/// fleet like any other unlisted kind and taken after the grace. It cannot be
/// a compile (those are always stamped), so nothing about it needs this node.
pub fn may_drive(source_type: Option<&str>, unclaimed_secs: u64, policy: DrivePolicy) -> bool {
    let listed = |kinds: &[&str]| source_type.is_some_and(|t| kinds.contains(&t));
    let waited = unclaimed_secs >= STRANDED_GRACE_SECS;
    match policy {
        DrivePolicy::Except(kinds) => !listed(kinds),
        DrivePolicy::Defer(kinds) => !listed(kinds) || waited,
        DrivePolicy::Only(kinds) => listed(kinds) || waited,
    }
}

/// Split a selection into `(drive now, leave for another node)`.
///
/// The one place a selection meets a policy. Both the probe that decides which
/// workspaces to visit and the gate in front of the driver lease call this, so
/// "the probe skipped more than the gate would have driven" — work that
/// silently stops running, with nothing failing — is not a state the code can
/// be in.
pub fn partition_drivable(
    pending: Vec<StuckRun>,
    policy: DrivePolicy,
) -> (Vec<StuckRun>, Vec<StuckRun>) {
    pending
        .into_iter()
        .partition(|r| may_drive(r.source_type.as_deref(), r.unclaimed_secs, policy))
}

/// Does `policy` drive this run **only** because it has gone unclaimed for
/// [`STRANDED_GRACE_SECS`] — a run it would have left for another node had it
/// been fresh?
///
/// Defined as the difference of two [`may_drive`] answers rather than a second
/// match over the policy, so it cannot drift from the gate: under
/// [`DrivePolicy::Except`] age changes nothing and this is always false; under
/// the two placement forms it is true exactly for the deferred kinds that have
/// waited.
fn drives_on_fallback(source_type: Option<&str>, unclaimed_secs: u64, policy: DrivePolicy) -> bool {
    may_drive(source_type, unclaimed_secs, policy) && !may_drive(source_type, 0, policy)
}

/// Log, at INFO, each run in `drivable` this process is taking only on the
/// fallback, and return them.
///
/// The rollout signal for the placement forms. The fallback cannot tell a
/// missing fleet from one that is up but too small or too slow to claim within
/// the grace, so heavy work can drift back onto the node that defers it with
/// nothing failing — the only trace is that node taking runs it would rather
/// leave. Each such take is logged with its run id, kind and wait. Quiet under
/// [`DrivePolicy::Except`] (a capability never yields to age) and for a kind
/// the node drives on its own account.
///
/// Called by every loop that drives Global work, on the side of
/// [`partition_drivable`] it is about to drive — not by the latency worker's
/// probe, which only picks the workspaces to visit and would log each take
/// twice. `pass` names the loop, as the surrounding `recovery` lines do.
pub fn report_fallback_takes<'a>(
    drivable: &'a [StuckRun],
    policy: DrivePolicy,
    pass: &'static str,
) -> Vec<&'a StuckRun> {
    let taken: Vec<&StuckRun> = drivable
        .iter()
        .filter(|r| drives_on_fallback(r.source_type.as_deref(), r.unclaimed_secs, policy))
        .collect();
    for r in &taken {
        tracing::info!(
            target: "recovery",
            run_id = %r.run_id,
            source_type = r.source_type.as_deref().unwrap_or("none"),
            unclaimed_secs = r.unclaimed_secs,
            ?policy,
            pass,
            "taking a run this node defers: nobody claimed it within the grace"
        );
    }
    taken
}

#[cfg(test)]
mod tests {
    use super::{
        DrivePolicy, STRANDED_GRACE_SECS, may_drive, partition_drivable, report_fallback_takes,
    };
    use agentic_runtime::crud::StuckRun;

    const FRESH: u64 = 0;
    const AGED: u64 = STRANDED_GRACE_SECS;
    const ANCIENT: u64 = 24 * 60 * 60;

    /// The gate itself. Two earlier attempts at this failed by being applied
    /// in the wrong place rather than by computing the wrong answer, so this
    /// pins the answer and the call site keeps the placement honest.
    #[test]
    fn excluded_kinds_are_declined_and_everything_else_is_driven() {
        let p = DrivePolicy::Except(&["compile"]);
        assert!(!may_drive(Some("compile"), FRESH, p));
        assert!(may_drive(Some("airway"), FRESH, p));
        assert!(may_drive(Some("workflow"), FRESH, p));
    }

    /// `ALL` is the `oxy serve` / `all` case: drive everything. Getting this
    /// wrong would strand every run on the node that CAN do the work.
    #[test]
    fn the_all_policy_drives_everything() {
        assert!(may_drive(Some("compile"), FRESH, DrivePolicy::ALL));
        assert!(may_drive(Some("preagg_cycle"), FRESH, DrivePolicy::ALL));
        assert!(may_drive(None, FRESH, DrivePolicy::ALL));
    }

    /// Absent is not excluded. A run row with a NULL `source_type` must still
    /// be driven — silently dropping it would strand it forever, since nothing
    /// else selects a run whose driver never claims it.
    #[test]
    fn a_missing_source_type_is_drivable_under_a_deny_list() {
        assert!(may_drive(None, FRESH, DrivePolicy::Except(&["compile"])));
    }

    /// The Phase 2 mirror image: the `ide` node declines `airway` so a worker
    /// takes the pipeline instead of the pod that accepted the submit.
    ///
    /// Pinned separately from the compile case because the two exclusions have
    /// opposite justifications — `compile` is a capability the decliner lacks,
    /// `airway` is a placement preference by a node that is perfectly capable.
    /// A future reader collapsing them into "things a node can't do" would
    /// break the airway rule without failing the compile test.
    #[test]
    fn the_ide_declines_airway_but_still_drives_compiles() {
        let p = DrivePolicy::Defer(&["airway"]);
        assert!(!may_drive(Some("airway"), FRESH, p));
        assert!(may_drive(Some("compile"), FRESH, p));
        assert!(may_drive(Some("workflow"), FRESH, p));
        assert!(may_drive(Some("analytics"), FRESH, p));
    }

    /// The deny form of the preference carries the same fallback as the allow
    /// form: an airway run nobody took for the grace is driven by the deferring
    /// `ide` after all. This used to be the periodic stranded tick's job alone;
    /// now that the tick applies the policy too, the fallback has to be in the
    /// predicate or an `ide` + `serve` deployment with the airway flag and no
    /// workers would strand every pipeline.
    #[test]
    fn a_deferred_kind_is_taken_once_it_has_waited() {
        let p = DrivePolicy::Defer(&["airway"]);
        assert!(!may_drive(Some("airway"), STRANDED_GRACE_SECS - 1, p));
        assert!(may_drive(Some("airway"), AGED, p));
        assert!(may_drive(Some("airway"), ANCIENT, p));
    }

    /// The two gates are disjoint sets held by different roles, never both by
    /// one process — but `may_drive` itself must not care, so that a future
    /// role needing both is a one-line change at the call site rather than a
    /// rewrite here.
    #[test]
    fn excluding_both_kinds_declines_both() {
        let p = DrivePolicy::Except(&["compile", "airway"]);
        assert!(!may_drive(Some("airway"), FRESH, p));
        assert!(!may_drive(Some("compile"), FRESH, p));
        assert!(may_drive(Some("workflow"), FRESH, p));
    }

    /// A deny-list is a capability statement, and waiting does not grow one.
    /// If age opened this arm, a compile queued while the `ide` was down would
    /// be claimed by a worker thirty seconds later and fail there, on a node
    /// with no working copy — spending its recovery budget until it retired.
    #[test]
    fn a_deny_list_never_yields_to_age() {
        let p = DrivePolicy::Except(&["compile"]);
        assert!(!may_drive(Some("compile"), AGED, p));
        assert!(!may_drive(Some("compile"), ANCIENT, p));
    }

    /// The allow form's first half: listed kinds are driven at once, and
    /// nothing else is. Every kind the open-ended `source_type` column can
    /// hold is deferred without being named — which is the property a
    /// deny-list cannot express.
    #[test]
    fn an_allow_list_drives_only_its_kinds_while_work_is_fresh() {
        let p = DrivePolicy::Only(&["compile"]);
        assert!(may_drive(Some("compile"), FRESH, p));
        for kind in [
            "airway",
            "workflow",
            "analytics",
            "preagg_cycle",
            "app_function",
            "health_eval_workspace",
            "a_kind_added_next_year",
        ] {
            assert!(
                !may_drive(Some(kind), FRESH, p),
                "{kind} must be left for the fleet while it is fresh"
            );
            assert!(
                !may_drive(Some(kind), STRANDED_GRACE_SECS - 1, p),
                "{kind} must still be left one second inside the grace"
            );
        }
    }

    /// The allow form's second half, and the reason it is safe to turn on:
    /// work nobody took for the grace is driven here after all. Without this
    /// arm a deferring `ide` beside a missing or wedged fleet leaves every
    /// non-compile run queued forever — the periodic stranded tick only ever
    /// selects `workflow` and `airway`.
    #[test]
    fn an_allow_list_takes_unlisted_work_once_it_has_gone_unclaimed() {
        let p = DrivePolicy::Only(&["compile"]);
        for kind in ["airway", "workflow", "preagg_cycle", "app_function"] {
            assert!(
                may_drive(Some(kind), AGED, p),
                "{kind} unclaimed for the grace must fall back to this node"
            );
            assert!(may_drive(Some(kind), ANCIENT, p));
        }
    }

    /// A run with no `source_type` under the allow form: not listed, so it is
    /// deferred like any other unlisted kind — and, like them, not stranded.
    #[test]
    fn a_missing_source_type_is_deferred_then_taken_under_an_allow_list() {
        let p = DrivePolicy::Only(&["compile"]);
        assert!(!may_drive(None, FRESH, p));
        assert!(!may_drive(None, STRANDED_GRACE_SECS - 1, p));
        assert!(may_drive(None, AGED, p));
    }

    /// An empty allow-list is "drive nothing until it has waited", not "drive
    /// everything" — the opposite reading of an empty deny-list. Pinned so the
    /// two forms are never unified by treating empty as a wildcard.
    #[test]
    fn an_empty_allow_list_drives_nothing_fresh() {
        let p = DrivePolicy::Only(&[]);
        assert!(!may_drive(Some("compile"), FRESH, p));
        assert!(!may_drive(None, FRESH, p));
        assert!(may_drive(Some("compile"), AGED, p));
    }

    fn run(id: &str, source_type: Option<&str>, unclaimed_secs: u64) -> StuckRun {
        StuckRun {
            run_id: id.to_string(),
            task_status: Some("running".to_string()),
            workspace_id: uuid::Uuid::nil(),
            source_type: source_type.map(str::to_string),
            unclaimed_secs,
        }
    }

    fn ids(runs: &[StuckRun]) -> Vec<&str> {
        runs.iter().map(|r| r.run_id.as_str()).collect()
    }

    /// The split reads each run's own age, not one age for the batch: a fresh
    /// and a long-unclaimed run of the same kind land on opposite sides.
    #[test]
    fn the_partition_applies_the_policy_per_run() {
        let pending = vec![
            run("compile-fresh", Some("compile"), FRESH),
            run("preagg-fresh", Some("preagg_cycle"), FRESH),
            run("preagg-aged", Some("preagg_cycle"), AGED),
            run("untyped-fresh", None, FRESH),
            run("untyped-aged", None, AGED),
        ];
        let (drive, leave) = partition_drivable(pending, DrivePolicy::Only(&["compile"]));
        assert_eq!(
            ids(&drive),
            ["compile-fresh", "preagg-aged", "untyped-aged"]
        );
        assert_eq!(ids(&leave), ["preagg-fresh", "untyped-fresh"]);
    }

    /// Nothing is dropped by the split: every selected run is on exactly one
    /// side, under either form.
    #[test]
    fn the_partition_loses_nothing() {
        for policy in [
            DrivePolicy::ALL,
            DrivePolicy::Except(&["compile"]),
            DrivePolicy::Only(&["compile"]),
        ] {
            let pending = vec![
                run("a", Some("compile"), FRESH),
                run("b", Some("airway"), AGED),
                run("c", None, FRESH),
            ];
            let (drive, leave) = partition_drivable(pending, policy);
            assert_eq!(drive.len() + leave.len(), 3, "{policy:?} dropped a run");
        }
    }

    fn reported(runs: &[StuckRun], policy: DrivePolicy) -> Vec<&str> {
        report_fallback_takes(runs, policy, "test")
            .into_iter()
            .map(|r| r.run_id.as_str())
            .collect()
    }

    /// The rollout signal under the allow form: what a deferring `ide` takes
    /// because nobody else did is reported; a listed kind is not, however long
    /// it waited, because the node drives it on its own account.
    #[test]
    fn an_allow_list_reports_only_unlisted_work_taken_after_the_grace() {
        let pending = vec![
            run("compile-fresh", Some("compile"), FRESH),
            run("compile-ancient", Some("compile"), ANCIENT),
            run("preagg-fresh", Some("preagg_cycle"), FRESH),
            run("preagg-aged", Some("preagg_cycle"), AGED),
            run("untyped-ancient", None, ANCIENT),
        ];
        let policy = DrivePolicy::Only(&["compile"]);
        let (drive, _) = partition_drivable(pending, policy);
        assert_eq!(reported(&drive, policy), ["preagg-aged", "untyped-ancient"]);
    }

    /// The deny form of the preference: only the deferred kind, and only once
    /// it has waited.
    #[test]
    fn a_deny_list_preference_reports_only_its_kind_taken_after_the_grace() {
        let pending = vec![
            run("airway-aged", Some("airway"), AGED),
            run("workflow-ancient", Some("workflow"), ANCIENT),
            run("workflow-fresh", Some("workflow"), FRESH),
            run("untyped-ancient", None, ANCIENT),
        ];
        let policy = DrivePolicy::Defer(&["airway"]);
        let (drive, _) = partition_drivable(pending, policy);
        assert_eq!(reported(&drive, policy), ["airway-aged"]);
    }

    /// A capability exclusion never yields to age, so a worker's loops — and
    /// every `all` deployment — never log a fallback take.
    #[test]
    fn a_capability_exclusion_never_reports() {
        for policy in [DrivePolicy::ALL, DrivePolicy::Except(&["compile"])] {
            let drivable = vec![
                run("airway-ancient", Some("airway"), ANCIENT),
                run("preagg-aged", Some("preagg_cycle"), AGED),
                run("untyped-ancient", None, ANCIENT),
                run("workflow-fresh", Some("workflow"), FRESH),
            ];
            assert!(
                reported(&drivable, policy).is_empty(),
                "{policy:?} reported a fallback take"
            );
        }
    }

    /// Only what the gate lets through is reported: a run still inside the
    /// grace is being left, not taken, whichever side it was passed from.
    #[test]
    fn a_run_the_gate_declines_is_never_reported() {
        let declined = vec![
            run("preagg-fresh", Some("preagg_cycle"), FRESH),
            run(
                "preagg-almost",
                Some("preagg_cycle"),
                STRANDED_GRACE_SECS - 1,
            ),
        ];
        assert!(reported(&declined, DrivePolicy::Only(&["compile"])).is_empty());
    }
}
