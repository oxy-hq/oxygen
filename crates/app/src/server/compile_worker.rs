//! Worker shape that wraps `oxy_compile::compile_workspace` into the
//! runtime's `ExecutingTask` channel pair so the existing worker pool
//! can drive it like any other queued task.
//!
//! Compile is atomic from the queue's perspective — one TaskSpec, one
//! `revisions` row, no per-step decisions, no fan-out at the
//! coordinator. The worker therefore emits exactly three events
//! (`compile_started`, `compile_progress` on each milestone,
//! `compile_finished`) and a single terminal `TaskOutcome`.

use std::path::PathBuf;
use std::sync::Arc;

use agentic_core::delegation::TaskOutcome;
use agentic_runtime::orchestrator::worker::ExecutingTask;
use sea_orm::DatabaseConnection;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use oxy_compile::{
    CompileError, CompileOutcome, CompileRequest, RevisionKind, RevisionStatus, compile_workspace,
    compiler_version,
};

const EVENT_BUFFER: usize = 16;
const OUTCOME_BUFFER: usize = 4;

/// Inputs the worker needs to drive one compile. Translated from the
/// `TaskSpec::Compile` payload by the executor before being handed to
/// this worker.
#[derive(Debug, Clone)]
pub struct CompileSpec {
    pub workspace_id: Uuid,
    pub workspace_path: PathBuf,
    pub git_sha: Option<String>,
    pub branch: Option<String>,
    pub promote: bool,
    pub kind: RevisionKind,
    pub owner_user_id: Option<Uuid>,
}

pub struct CompileWorker {
    db: Arc<DatabaseConnection>,
}

impl CompileWorker {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }

    /// Spawn a compile against the supplied spec. The returned
    /// `ExecutingTask` carries event + outcome receivers + a cancel
    /// token; the worker pool will plumb them through the normal
    /// queue lifecycle.
    pub fn execute(&self, spec: CompileSpec) -> ExecutingTask {
        let (event_tx, event_rx) = mpsc::channel::<(String, Value)>(EVENT_BUFFER);
        let (outcome_tx, outcome_rx) = mpsc::channel::<TaskOutcome>(OUTCOME_BUFFER);
        let cancel = CancellationToken::new();

        let db = self.db.clone();
        let cancel_clone = cancel.clone();
        // Outer task watches the inner one so a panic / abort always
        // synthesizes a terminal Failed outcome rather than leaving
        // the coordinator waiting on a dead channel. Mirrors the
        // `AirwayWorker` pattern.
        let outcome_tx_watch = outcome_tx.clone();
        tokio::spawn(async move {
            let handle = tokio::spawn(drive(spec, db, event_tx, cancel_clone, outcome_tx));
            if let Err(join_err) = handle.await {
                let msg = if join_err.is_panic() {
                    "compile worker panicked (internal error)".to_string()
                } else {
                    "compile worker task aborted".to_string()
                };
                let _ = outcome_tx_watch.send(TaskOutcome::Failed(msg)).await;
            }
        });

        ExecutingTask {
            events: event_rx,
            outcomes: outcome_rx,
            cancel,
            answers: None,
        }
    }
}

#[tracing::instrument(
    skip_all,
    fields(workspace_id = %spec.workspace_id, promote = spec.promote)
)]
async fn drive(
    spec: CompileSpec,
    db: Arc<DatabaseConnection>,
    event_tx: mpsc::Sender<(String, Value)>,
    cancel: CancellationToken,
    outcome_tx: mpsc::Sender<TaskOutcome>,
) {
    let started_ts = chrono::Utc::now();
    let _ = event_tx
        .send((
            "compile_started".to_string(),
            json!({
                "workspace_id": spec.workspace_id,
                "git_sha": spec.git_sha,
                "branch": spec.branch,
                "promote": spec.promote,
                "kind": spec.kind.as_str(),
                "started_at": started_ts.to_rfc3339(),
            }),
        ))
        .await;

    // Cancel is checked once up front rather than mid-compile; the
    // compile primitive itself is unit-of-work atomic — interrupting
    // it half-way through could leave a `compiling` row in Postgres
    // (the reaper sweep handles that path).
    if cancel.is_cancelled() {
        let _ = outcome_tx
            .send(TaskOutcome::Failed(
                "compile cancelled before start".to_string(),
            ))
            .await;
        return;
    }

    if !crate::server::role_manifest::process_can_compile() {
        // FAIL, do not defer.
        //
        // An earlier revision deferred here, reasoning that "cannot compile" is
        // a property of this process rather than of the task. The reasoning was
        // right and the mechanism was wrong: deferral hands the row back to the
        // queue, but only the process holding the run's DRIVER LEASE can claim
        // it (every `Worker::new` binds a run-scoped transport; there is no
        // unscoped pool). So the deferring process re-claims its own row every
        // `delay_secs` while its heartbeat keeps every other node excluded from
        // `find_pending_global_runs`, until dead-letter at `max_wait_secs`
        // leaves the run `task_status = 'running'` forever with a leaked lease.
        // That is strictly worse than failing: a fast, visible failure that
        // clears the enqueue dedup became a ten-minute stall that blocks it.
        //
        // The real gate is at SELECTION, in
        // `agentic_pipeline::recovery::recover_pending_global_runs`, which
        // drops `compile` runs before `try_acquire_driver` when this process
        // cannot compile. (`tick_cloud` also partitions them out of its
        // discovery probe, but that is an optimisation — an earlier revision
        // put the gate there and it did not hold, because `drive_pending`
        // re-selects per workspace.)
        //
        // Not claimed to be unreachable. On a diskless pod
        // `OxyCompileDispatcher::dispatch` fails on `!workspace_path.is_dir()`
        // before this runs, so the message an operator sees is the
        // dispatcher's, not this one. This arm is the backstop for a node that
        // lost the capability after taking the lease, where terminating loudly
        // is right.
        let _ = outcome_tx
            .send(TaskOutcome::Failed(format!(
                "compile requires a workspace working copy, which OXY_ROLE={} does not own. \
                 Route Compile tasks to an ide or all node.",
                crate::server::role_manifest::current_process_role().as_str()
            )))
            .await;
        return;
    }

    // The revision this workspace is serving RIGHT NOW, captured before the
    // compile can move the pointer. It is the baseline the post-promote
    // pre-aggregation nudge diffs against (`preagg_promote`), and the only
    // moment it can be read — afterwards the previous revision is not
    // recoverable from `workspaces` at all, and the `revisions` history cannot
    // say which of its rows was the one being served.
    let served_before = if spec.promote {
        crate::server::preagg_promote::promoted_revision(&db, spec.workspace_id)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(
                    target: "preagg",
                    error = %e,
                    workspace_id = %spec.workspace_id,
                    "could not read the served revision before compiling; the post-promote \
                     pre-aggregation nudge will treat this as a first promote"
                );
                None
            })
    } else {
        None
    };

    let outcome = compile_workspace(CompileRequest {
        db: &db,
        workspace_id: spec.workspace_id,
        workspace_path: &spec.workspace_path,
        git_sha: spec.git_sha.clone(),
        branch: spec.branch.clone(),
        compiler_version: compiler_version(),
        promote: spec.promote,
        kind: spec.kind,
        owner_user_id: spec.owner_user_id,
        // Reject a compiled config the stateless fleet couldn't read (#2520):
        // it fails the compile instead of promoting a revision that 503s.
        config_gate: Some(crate::server::compile_config_gate::runtime_config_gate()),
    })
    .await;

    match outcome {
        Ok(o) => {
            // Apply-then-promote. A revision carrying `schemas/*.sql` was
            // deliberately left unpromoted by the compiler; this is where
            // its DDL reaches the org database and the pointer moves.
            //
            // Must run BEFORE the reconciles below: both of them read "the
            // workspace's current compiled config", which is still the
            // PREVIOUS revision until this returns.
            let settled =
                crate::server::compile_oltp::settle_deferred_promotion(&db, spec.workspace_id, &o)
                    .await;

            let _ = event_tx
                .send((
                    "compile_finished".to_string(),
                    summarise_outcome(&o, &settled),
                ))
                .await;
            let mut answer = format!(
                "revision {} {} ({} files compiled, {} failed)",
                o.revision_id,
                o.status.as_str(),
                o.file_count_compiled,
                o.file_count_failed
            );
            let settled_note = settled.summary();
            if !settled_note.is_empty() {
                answer.push_str(" — ");
                answer.push_str(&settled_note);
            }

            let task_outcome = if !matches!(o.status, RevisionStatus::Ready) {
                TaskOutcome::Failed(format!(
                    "compile recorded {} failed file(s); revision_id={}",
                    o.file_count_failed, o.revision_id
                ))
            } else if !settled.compile_succeeded() {
                // The files all compiled; the DDL did not apply. Fail the
                // task so it is visible, and say which of the two it was —
                // the instinct on a failed compile is to go re-read the
                // YAML, and here the YAML is fine.
                TaskOutcome::Failed(format!("{settled_note}; revision_id={}", o.revision_id))
            } else {
                // config.yml is the source of truth for the per-workspace health
                // cadence. A promoted compile is the sync point: refresh the
                // workspace's `health_eval` schedule row from the freshly
                // compiled `health_check`. Best-effort — never fail the compile.
                if spec.promote {
                    reconcile_health_from_compiled(&db, spec.workspace_id).await;
                    // The reconcile settles the CADENCE and deliberately keeps
                    // the next fire slot; it is not a tick. A promote that
                    // re-hashed a rollup has work that would otherwise sit out
                    // the whole interval, so the nudge below asks for one —
                    // gated on the same opt-in the reconcile just resolved.
                    let preagg_enabled = reconcile_preagg_from_compiled(&db, spec.workspace_id)
                        .await
                        .unwrap_or_else(|| {
                            // `None` and `Some(false)` both mean "no tick", but
                            // they are different answers to "why didn't my
                            // promote build it?" — one is the tenant's opt-out,
                            // the other a read that failed. Only the second is
                            // worth a line.
                            tracing::debug!(
                                target: "preagg",
                                workspace_id = %spec.workspace_id,
                                "the preagg reconcile carried no statement of intent; \
                                 skipping the post-promote cycle"
                            );
                            false
                        });
                    crate::server::preagg_promote::nudge_after_promote(
                        &db,
                        spec.workspace_id,
                        served_before,
                        preagg_enabled,
                    )
                    .await;
                }
                queue_preview_check(&db, &spec, o.revision_id).await;
                TaskOutcome::Done {
                    answer,
                    metadata: Some(summarise_outcome(&o, &settled)),
                }
            };
            let _ = outcome_tx.send(task_outcome).await;
        }
        Err(e) => {
            let _ = event_tx
                .send((
                    "compile_finished".to_string(),
                    json!({ "error": format!("{e}") }),
                ))
                .await;
            let _ = outcome_tx
                .send(TaskOutcome::Failed(compile_error_to_string(&e)))
                .await;
        }
    }
}

/// *Why* a workspace's health-eval schedule landed where it did. Carried
/// alongside the cadence instead of a bare `bool` because the three disabled
/// cases are not interchangeable: one of them is a decision the tenant made and
/// must not be nagged about, and the other two need different wording from each
/// other. See [`HealthOptIn::inert_reconcile_warning`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HealthOptIn {
    /// A `health_check:` block that parsed and isn't switched off — opted in.
    Enabled,
    /// A block that says `enabled: false`. A **choice**, so nothing here should
    /// tell the tenant to write a block they already wrote.
    ExplicitlyDisabled,
    /// No `health_check:` block at all — the case the opt-in policy is aimed at,
    /// and the one far more likely to be an oversight than a decision.
    NoBlock,
    /// A block that failed to parse. Already warned about at the parse site, and
    /// the tenant *did* write a block, so it can't reuse `NoBlock`'s wording.
    Unparseable,
}

impl HealthOptIn {
    /// Whether the workspace's `health_eval` schedule row should be enabled.
    fn enabled(self) -> bool {
        matches!(self, Self::Enabled)
    }

    /// `(what we saw, how to fix it)` when this reason is worth pairing with the
    /// inert-`reconcile.yml` warning — see [`warn_if_reconcile_goes_inert`].
    /// `None` for the two reasons that aren't: `Enabled` (nothing goes inert)
    /// and `ExplicitlyDisabled` (a choice, not an oversight).
    fn inert_reconcile_warning(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::NoBlock => Some((
                "no `health_check:` block",
                "Add `health_check:` to config.yml to re-enable them",
            )),
            Self::Unparseable => Some((
                "a `health_check:` block that does not parse",
                "Fix the block (the parse error is logged above) to re-enable them",
            )),
            Self::Enabled | Self::ExplicitlyDisabled => None,
        }
    }
}

/// Derive the health-eval `(interval, opt-in reason)` from a **known** compiled
/// config value's `health_check` section. Absent / unparseable → default
/// cadence, **disabled**: health checks are opt-in per workspace, and a block we
/// couldn't read is not an opt-in.
///
/// Takes `&Value`, not `Option<&Value>`, on purpose: "I have no config" is not a
/// statement about intent and must never reach this function. That case is
/// [`health_reconcile_target`]'s to answer. Pure apart from the unparseable
/// warning, so it's directly unit-testable.
fn health_settings_from_config(
    config: &Value,
    workspace_id: uuid::Uuid,
) -> (std::time::Duration, HealthOptIn) {
    let raw = config.get("health_check");
    let mut parse_error = None;
    let hc = raw.and_then(|hc| {
        match serde_json::from_value::<oxy::config::health_check::HealthCheckConfig>(hc.clone()) {
            Ok(parsed) => Some(parsed),
            Err(e) => {
                parse_error = Some(e);
                None
            }
        }
    });
    // `health_check` rides the `other` JSONB catch-all, so compile never
    // validates it — `deny_unknown_fields` rejects a typo'd key only here. That
    // used to fail open to an hourly eval; now it disables the workspace, which
    // must not happen silently. Carry the serde error into the log like the
    // sibling reader (`smoke/config.rs`) does: "your block is broken" without
    // saying *how* leaves the operator diffing YAML by hand, and a bare
    // `health_check:` (YAML null) lands here too, which no "mistyped key"
    // wording would explain.
    if let Some(e) = parse_error {
        tracing::warn!(
            target: "health_eval",
            error = %e,
            %workspace_id,
            "config.yml has a `health_check:` block that could not be read (an unknown or \
             mistyped key, or an empty block — write `health_check: {{}}` for the defaults); \
             treating the workspace as opted out of health checks"
        );
    }
    let interval = oxy::config::health_check::resolve_interval(hc.as_ref());
    // `resolve_enabled` stays the one seam that decides on/off; this match only
    // classifies the *reason*, and must agree with it by construction.
    let opt_in = match (raw, hc.as_ref()) {
        (_, Some(parsed)) if oxy::config::health_check::resolve_enabled(Some(parsed)) => {
            HealthOptIn::Enabled
        }
        (_, Some(_)) => HealthOptIn::ExplicitlyDisabled,
        (Some(_), None) => HealthOptIn::Unparseable,
        (None, None) => HealthOptIn::NoBlock,
    };
    (interval, opt_in)
}

/// Decide what to write onto a workspace's `health_eval` row from a compiled
/// config read, or `None` to leave the row exactly as it is.
///
/// The distinction is load-bearing now that "no `health_check:` block" disables
/// the workspace. `resolve_workspace_config` returns three answers, and only one
/// of them is a statement of intent:
///
/// * `Ok(Some(cfg))` — the tenant's promoted config. Authoritative; reconcile.
/// * `Ok(None)` — no promoted revision yet. Says nothing; a workspace that wrote
///   `health_check:` but hasn't compiled would otherwise read as opted out.
/// * `Err(e)` — a DB error. Says nothing either, and this runs in a loop over
///   *every* workspace at startup, so collapsing it into "disabled" would switch
///   off healthy tenants on one bad read and leave no trace.
///
/// Mirrors `compiled_reader::resolve_workspace_config_typed`'s handling of the
/// same three cases.
fn health_reconcile_target(
    read: Result<Option<Value>, sea_orm::DbErr>,
    workspace_id: uuid::Uuid,
) -> Option<(std::time::Duration, HealthOptIn)> {
    match read {
        Ok(Some(config)) => Some(health_settings_from_config(&config, workspace_id)),
        Ok(None) => {
            tracing::debug!(
                target: "health_eval",
                %workspace_id,
                "no promoted compiled config; leaving the health schedule untouched"
            );
            None
        }
        Err(e) => {
            tracing::warn!(
                target: "health_eval",
                error = %e,
                %workspace_id,
                "compiled config read failed; leaving the health schedule untouched \
                 rather than reading the failure as an opt-out"
            );
            None
        }
    }
}

/// Read the workspace's promoted compiled config and reconcile its `health_eval`
/// schedule row to the configured cadence. Best-effort: a read that carries no
/// statement of intent leaves the row alone (see [`health_reconcile_target`]);
/// a reconcile error is logged.
pub(crate) async fn reconcile_health_from_compiled(
    db: &DatabaseConnection,
    workspace_id: uuid::Uuid,
) {
    // One revision for the whole pass: the schedule this writes and the
    // `reconcile.yml` the warning below looks for must describe the same
    // compile, or a promote landing mid-pass warns about the wrong one.
    let Some(revision_id) =
        crate::server::api::compiled_reader::resolve_request_revision(workspace_id, None).await
    else {
        tracing::debug!(
            target: "health_eval",
            %workspace_id,
            "no promoted compiled config; leaving the health schedule untouched"
        );
        return;
    };
    let read = crate::server::api::compiled_reader::resolve_workspace_config_at(revision_id).await;
    let Some((interval, opt_in)) = health_reconcile_target(read, workspace_id) else {
        return;
    };
    if let Some((cause, remedy)) = opt_in.inert_reconcile_warning() {
        warn_if_reconcile_goes_inert(workspace_id, revision_id, cause, remedy).await;
    }
    if let Err(e) = agentic_pipeline::scheduler::reconcile_health_schedule(
        db,
        workspace_id,
        interval,
        opt_in.enabled(),
    )
    .await
    {
        tracing::warn!(
            target: "health_eval",
            error = %e,
            %workspace_id,
            "failed to reconcile health schedule from compiled config"
        );
    }
}

/// Warn when disabling health checks also silences a `reconcile.yml` the tenant
/// deliberately wrote.
///
/// Reconciliation drift checks and Slack health alerting both run *inside* the
/// eval pass (`workspace_health::eval_pass::eval_and_persist`), so switching the
/// schedule off stops them too — and the workspace then vanishes from the admin
/// rollup, which is where an operator would otherwise notice. A `reconcile.yml`
/// is an explicit opt-in artifact, so the combination is much more likely to be
/// an oversight than a choice — which is exactly why the caller gates this on
/// [`HealthOptIn::inert_reconcile_warning`]: a tenant who wrote
/// `enabled: false` made the choice, and telling them to add a block they
/// already have is noise.
///
/// Deliberately **re-emitted on every reconcile** (startup and every promoted
/// compile), not only on the flip to disabled: there is no transition signal to
/// hang it off — the schedule upsert is idempotent and carries no previous
/// value — and `internal-docs/admin-surfaces.md` tells operators to grep
/// `target: "health_eval"` *after a deploy*, which needs the line to be present
/// then, not only in the logs of whichever instance first saw the change. The
/// repetition is the feature; don't "fix" it. Best-effort: a read failure is
/// not worth a log line of its own here.
async fn warn_if_reconcile_goes_inert(
    workspace_id: uuid::Uuid,
    revision_id: uuid::Uuid,
    cause: &str,
    remedy: &str,
) {
    if let Ok(Some(_)) =
        crate::server::api::compiled_reader::resolve_reconcile_config_at(revision_id).await
    {
        tracing::warn!(
            target: "health_eval",
            %workspace_id,
            "workspace has a compiled `reconcile.yml` but {cause}, so its drift checks and \
             Slack health alerts will not run — both ride inside the health eval pass. {remedy}"
        );
    }
}

/// Read a compiled config's `pre_aggregations` section and resolve it to a
/// `(interval, enabled)` pair, or `None` to leave the workspace's schedule row
/// untouched. Same three-case split as [`health_reconcile_target`]: only
/// `Ok(Some(cfg))` — a promoted config — is a statement of intent. `Ok(None)`
/// (no promoted revision yet) and `Err` (a DB read failure, and this runs in a
/// loop over every workspace at startup) both say nothing, so neither may be
/// read as "disable this workspace's pre-aggregations".
///
/// Unlike health, an unparseable `pre_aggregations:` block does not need its
/// own opt-in-reason enum: pre-aggregations were already off (no block parsed)
/// or already have a `PreaggConfig::default()` shape close enough that a stray
/// unknown key is the only realistic parse failure, caught by
/// `#[serde(deny_unknown_fields)]`. Warn and treat as absent either way.
fn preagg_reconcile_target(
    read: Result<Option<Value>, sea_orm::DbErr>,
    workspace_id: uuid::Uuid,
) -> Option<(std::time::Duration, bool)> {
    match read {
        Ok(Some(config)) => {
            let raw = config.get("pre_aggregations");
            let parsed = raw.and_then(|v| {
                match serde_json::from_value::<oxy::config::model::PreaggConfig>(v.clone()) {
                    Ok(cfg) => Some(cfg),
                    Err(e) => {
                        tracing::warn!(
                            target: "preagg",
                            error = %e,
                            %workspace_id,
                            "config.yml has a `pre_aggregations:` block that could not be read \
                             (an unknown or mistyped key); treating the workspace as opted out \
                             of the pre-aggregation cycle"
                        );
                        None
                    }
                }
            });
            let interval = oxy::config::preagg_check::resolve_interval(parsed.as_ref());
            let enabled = oxy::config::preagg_check::resolve_enabled(parsed.as_ref());
            Some((interval, enabled))
        }
        Ok(None) => {
            tracing::debug!(
                target: "preagg",
                %workspace_id,
                "no promoted compiled config; leaving the preagg schedule untouched"
            );
            None
        }
        Err(e) => {
            tracing::warn!(
                target: "preagg",
                error = %e,
                %workspace_id,
                "compiled config read failed; leaving the preagg schedule untouched rather \
                 than reading the failure as an opt-out"
            );
            None
        }
    }
}

/// Read the workspace's promoted compiled config and reconcile its
/// `preagg_cycle` schedule row to the configured cadence. Best-effort: a read
/// that carries no statement of intent leaves the row alone (see
/// [`preagg_reconcile_target`]); a reconcile error is logged.
///
/// Returns the opt-in it resolved — `Some(enabled)`, or `None` when the read
/// said nothing and the row was left alone. The promote path needs it so a
/// workspace with no `pre_aggregations:` block cannot be handed a cycle
/// through [`crate::server::preagg_promote::nudge_after_promote`]; the startup
/// reconcile has nothing to do with it and discards it.
pub(crate) async fn reconcile_preagg_from_compiled(
    db: &DatabaseConnection,
    workspace_id: uuid::Uuid,
) -> Option<bool> {
    let read =
        crate::server::api::compiled_reader::resolve_workspace_config(workspace_id, None).await;
    let (interval, enabled) = preagg_reconcile_target(read, workspace_id)?;
    if let Err(e) =
        agentic_pipeline::scheduler::reconcile_preagg_schedule(db, workspace_id, interval, enabled)
            .await
    {
        tracing::warn!(
            target: "preagg",
            error = %e,
            %workspace_id,
            "failed to reconcile preagg schedule from compiled config"
        );
        // The row is not where the config says it should be, so a tick seeded
        // against it would be attributed to a stale cadence at best. Say
        // nothing rather than yes.
        return None;
    }
    Some(enabled)
}

/// After a `Ready` compile: when it is a staging revision of a branch staff
/// preview, queue that revision's Airway change check. Best-effort; never
/// fails the compile.
async fn queue_preview_check(db: &DatabaseConnection, spec: &CompileSpec, revision_id: Uuid) {
    if !matches!(spec.kind, RevisionKind::Staging) {
        return;
    }
    crate::server::previews::analyze::enqueue_after_staging_compile(
        db,
        spec.workspace_id,
        spec.branch.as_deref(),
        spec.git_sha.as_deref(),
        revision_id,
    )
    .await;
}

fn summarise_outcome(o: &CompileOutcome, settled: &crate::server::compile_oltp::Settled) -> Value {
    json!({
        "revision_id": o.revision_id,
        // What happened to current_revision_id, and — when the revision
        // carries DDL — whether that DDL landed. Without these two an
        // operator looking at "why is this workspace serving the old
        // revision" has only the task's free-text answer to go on.
        //
        // `effective_promotion`, not `o.promotion`. The compiler's answer is
        // `Deferred` for any revision carrying `schemas/*.sql` — it defers
        // precisely so the migration can run first — and the settle step is what
        // decides whether the promotion then happened. Reporting the compiler's
        // half alone says "deferred" for a revision that is already live, which
        // is the one question this field exists to answer.
        "promotion": settled.effective_promotion(&o.promotion),
        "schema_migrations": settled.summary(),
        "status": o.status.as_str(),
        "git_sha": o.git_sha,
        "branch": o.branch,
        "started_at": o.started_at.to_rfc3339(),
        "finished_at": o.finished_at.to_rfc3339(),
        "file_count_seen": o.file_count_seen,
        "file_count_compiled": o.file_count_compiled,
        "file_count_failed": o.file_count_failed,
        // Truncate per-file failures to the first 10 here so the queue
        // event stream stays cheap; the full list is queryable via the
        // revisions.error_summary JSONB column.
        "failures_sample": o.failures.iter().take(10).collect::<Vec<_>>(),
    })
}

fn compile_error_to_string(e: &CompileError) -> String {
    format!("compile error: {e}")
}

/// Parse a `TaskSpec::Compile` payload into a `CompileSpec` the worker
/// can drive. Stays in this module so the executor stays clean of
/// payload-shape decisions.
pub fn spec_from_taskspec(
    workspace_id: Uuid,
    workspace_path: PathBuf,
    git_sha: Option<String>,
    branch: Option<String>,
    promote: bool,
    kind: Option<&str>,
    owner_user_id: Option<Uuid>,
) -> Result<CompileSpec, String> {
    let kind = match kind {
        None => RevisionKind::Main,
        Some(s) => RevisionKind::parse(s).ok_or_else(|| format!("unknown revision kind: {s}"))?,
    };
    if matches!(kind, RevisionKind::Draft) && owner_user_id.is_none() {
        return Err("draft revision requires owner_user_id".to_string());
    }
    Ok(CompileSpec {
        workspace_id,
        workspace_path,
        git_sha,
        branch,
        promote,
        kind,
        owner_user_id,
    })
}

#[cfg(test)]
#[path = "compile_worker_tests.rs"]
mod tests;

#[cfg(test)]
mod outcome_summary_tests {
    use super::*;
    use crate::server::compile_oltp::Settled;
    use oxy_compile::outcome::{CompileOutcome, Promotion, RevisionStatus};

    fn outcome(promotion: Promotion) -> CompileOutcome {
        let now = chrono::Utc::now();
        CompileOutcome {
            revision_id: uuid::Uuid::nil(),
            status: RevisionStatus::Ready,
            git_sha: "abc123".into(),
            branch: Some("main".into()),
            started_at: now,
            finished_at: now,
            file_count_seen: 1,
            file_count_compiled: 1,
            file_count_failed: 0,
            failures: Vec::new(),
            promotion,
        }
    }

    /// The worker is the production path, and it was reporting the COMPILER's
    /// half of a two-step answer.
    ///
    /// A revision carrying `schemas/*.sql` always compiles to `Deferred` — that
    /// is the point, the promotion waits for the migration — and the settle step
    /// is what decides whether it then happened. Emitting `o.promotion` alone
    /// said "deferred" for a revision that was already live, in both the
    /// `compile_finished` event and the task metadata.
    #[test]
    fn a_deferred_compile_that_settled_reports_promoted_not_deferred() {
        let o = outcome(Promotion::Deferred {
            schema_migration_count: 2,
        });
        let settled = Settled::Applied {
            applied: 2,
            already_applied: 0,
            promotion: Promotion::Promoted,
        };
        let v = summarise_outcome(&o, &settled);
        assert_eq!(
            v["promotion"],
            serde_json::to_value(Promotion::Promoted).unwrap(),
            "the worker reported the compiler's half of the answer"
        );
    }

    /// The distinction `Promotion::Skipped` exists for must survive the fold: a
    /// settle that ran but lost the causality race is not a promotion.
    #[test]
    fn a_settle_that_lost_the_race_still_reports_skipped() {
        let o = outcome(Promotion::Deferred {
            schema_migration_count: 1,
        });
        let settled = Settled::Applied {
            applied: 1,
            already_applied: 0,
            promotion: Promotion::Skipped,
        };
        let v = summarise_outcome(&o, &settled);
        assert_eq!(
            v["promotion"],
            serde_json::to_value(Promotion::Skipped).unwrap()
        );
    }

    /// When the migration never applied, the compiler's answer still stands —
    /// folding must not invent a promotion that did not happen.
    #[test]
    fn an_unsettled_deferral_still_reports_deferred() {
        let deferred = Promotion::Deferred {
            schema_migration_count: 3,
        };
        let v = summarise_outcome(&outcome(deferred.clone()), &Settled::Busy);
        assert_eq!(v["promotion"], serde_json::to_value(&deferred).unwrap());
    }
}

#[cfg(test)]
mod health_settings_tests {
    use super::*;
    use serde_json::json;

    fn settings(cfg: &Value) -> (std::time::Duration, HealthOptIn) {
        health_settings_from_config(cfg, uuid::Uuid::nil())
    }

    #[test]
    fn absent_health_check_section_is_disabled() {
        // The common case: a workspace whose config.yml never mentions health
        // checks gets a disabled schedule row, not an hourly eval.
        let (interval, opt_in) = settings(&json!({ "databases": [] }));
        assert_eq!(interval, std::time::Duration::from_secs(3600));
        assert_eq!(opt_in, HealthOptIn::NoBlock);
        assert!(!opt_in.enabled());
    }

    #[test]
    fn a_written_block_opts_in_without_naming_enabled() {
        let (interval, opt_in) = settings(&json!({ "health_check": { "interval": "45m" } }));
        assert_eq!(interval, std::time::Duration::from_secs(2700));
        assert_eq!(opt_in, HealthOptIn::Enabled);
    }

    #[test]
    fn unparseable_block_is_disabled_not_defaulted_on() {
        // A block we can't read is not evidence anyone asked for health checks;
        // `deny_unknown_fields` makes a typo'd key land here. The caller warns,
        // so this is not the silent off-switch it would otherwise be.
        let (_, opt_in) = settings(&json!({ "health_check": { "bogus_key": 1 } }));
        assert_eq!(opt_in, HealthOptIn::Unparseable);
        assert!(!opt_in.enabled());
    }

    #[test]
    fn a_bare_health_check_key_is_off_like_the_typed_path() {
        // `health_check:` with nothing under it is YAML null, and it reaches us
        // unvalidated through the `other` catch-all. Off is the only answer that
        // agrees with the typed readers, where `Option<HealthCheckConfig>`
        // deserialises null to `None` — `health_check: {}` is the empty opt-in.
        let (_, opt_in) = settings(&json!({ "health_check": Value::Null }));
        assert!(!opt_in.enabled());
        // And it warns rather than disappearing: the tenant did write the key.
        assert!(opt_in.inert_reconcile_warning().is_some());
    }

    #[test]
    fn reads_interval_and_enabled() {
        let (interval, opt_in) =
            settings(&json!({ "health_check": { "interval": "45m", "enabled": true } }));
        assert_eq!(interval, std::time::Duration::from_secs(2700));
        assert!(opt_in.enabled());
    }

    #[test]
    fn disabled_is_respected() {
        let (interval, opt_in) = settings(&json!({ "health_check": { "enabled": false } }));
        assert_eq!(interval, std::time::Duration::from_secs(3600));
        assert_eq!(opt_in, HealthOptIn::ExplicitlyDisabled);
        assert!(!opt_in.enabled());
    }

    #[test]
    fn an_explicit_opt_out_is_not_told_to_write_the_block_it_has() {
        // The inert-`reconcile.yml` warning names a *missing* block, so it must
        // not fire for a tenant who deliberately wrote `enabled: false` — that
        // is a decision, and the message would be false as well as noisy.
        let (_, opt_in) = settings(&json!({ "health_check": { "enabled": false } }));
        assert_eq!(opt_in.inert_reconcile_warning(), None);
        // Nor for an opted-in workspace: nothing goes inert there.
        let (_, opt_in) = settings(&json!({ "health_check": { "interval": "45m" } }));
        assert_eq!(opt_in.inert_reconcile_warning(), None);
    }

    #[test]
    fn the_two_oversight_shaped_reasons_warn_with_their_own_wording() {
        // Both are worth a warning, but an unparseable block must not be told to
        // add a `health_check:` block — it already has one, just a broken one.
        let (_, no_block) = settings(&json!({ "databases": [] }));
        let (_, unparseable) = settings(&json!({ "health_check": { "bogus_key": 1 } }));
        let (no_block_cause, no_block_remedy) = no_block.inert_reconcile_warning().unwrap();
        let (unparseable_cause, unparseable_remedy) =
            unparseable.inert_reconcile_warning().unwrap();
        assert!(no_block_cause.contains("no `health_check:` block"));
        assert!(no_block_remedy.contains("Add `health_check:`"));
        assert!(unparseable_cause.contains("does not parse"));
        assert!(unparseable_remedy.contains("Fix the block"));
    }

    #[test]
    fn a_read_failure_leaves_the_schedule_untouched() {
        // The regression that matters: this runs in a loop over every workspace
        // at startup, so a transient DB error must not write `enabled = false`
        // onto tenants that opted in.
        let read = Err(sea_orm::DbErr::Custom("connection reset".into()));
        assert_eq!(health_reconcile_target(read, uuid::Uuid::nil()), None);
    }

    #[test]
    fn an_unpromoted_workspace_leaves_the_schedule_untouched() {
        // No promoted revision is not an opt-out — the workspace may well have
        // written `health_check:` and simply not compiled yet.
        assert_eq!(health_reconcile_target(Ok(None), uuid::Uuid::nil()), None);
    }

    #[test]
    fn a_promoted_config_is_authoritative() {
        let read = Ok(Some(json!({ "health_check": { "interval": "45m" } })));
        assert_eq!(
            health_reconcile_target(read, uuid::Uuid::nil()),
            Some((std::time::Duration::from_secs(2700), HealthOptIn::Enabled))
        );
        // And a promoted config with no block is a real opt-out, unlike the two
        // "we don't know" cases above.
        let read = Ok(Some(json!({ "databases": [] })));
        assert_eq!(
            health_reconcile_target(read, uuid::Uuid::nil()),
            Some((std::time::Duration::from_secs(3600), HealthOptIn::NoBlock))
        );
    }
}
