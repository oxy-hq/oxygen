//! Cross-tenant workspace-health sweep — the periodic eval pass driven by the
//! `health_eval` schedule. Gathers signals, evaluates each workspace, diffs the
//! result against the last-known state, pushes Slack on transitions (and on a
//! cadence while a workspace stays unhealthy), and upserts
//! `workspace_health_state`.

use sea_orm::sea_query::OnConflict;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, EntityTrait, Set, Statement};
use serde_json::json;
use std::collections::HashMap;

use super::SignalsRow;
use super::alert::{
    AlertDecision, AlertInput, HealthAlert, decide_transition, push_slack, reminder_interval,
};
use super::evaluator::{
    DimensionFailure, HealthStatus, HealthThresholds, WorkspaceHealth, WorkspaceSignals, evaluate,
};
use super::queries::{WorkspaceLabel, gather_signals, gather_workspace_labels};
use super::reconcile::{DriftVerdict, LiveReconcileRunner, ReconcileRunner};
use super::smoke::config::smoke_due;
use super::smoke::runner::resolve_smoke_settings;
use super::smoke::{LiveSmokeRunner, SmokeProbeStatus, SmokeRunner, SmokeVerdict, probe_statuses};
use crate::integrations::slack::client::SlackClient;
use oxy::config::health_check::SmokeTestConfig;

/// Everything an eval needs that doesn't vary per workspace. Grouped so the
/// per-workspace entry point stays a two-argument function as the pass grows.
struct EvalCtx<'a> {
    db: &'a DatabaseConnection,
    reconcile: &'a LiveReconcileRunner,
    /// Trait object rather than the concrete runner so tests can substitute a
    /// probe that inspects the database at the moment it is called — that is how
    /// the "stamp before probe" ordering below is pinned.
    smoke: &'a dyn SmokeRunner,
    slack_client: &'a SlackClient,
    slack: &'a Option<(String, String)>,
    labels: &'a HashMap<uuid::Uuid, WorkspaceLabel>,
    thresholds: &'a HealthThresholds,
    /// The pass's reference instant — also the `last_smoke_at` stamp when the
    /// smoke probes run, so the gate and the stamp can't disagree.
    now: chrono::DateTime<chrono::Utc>,
    /// Run the smoke probes on this pass regardless of the cadence — an operator
    /// pressing "Run smoke test". It overrides the *clock*, never the *config*:
    /// a workspace with `smoke_test: { enabled: false }` still runs nothing, so
    /// the button can't bill an opted-out workspace for warehouse queries and
    /// agent tokens.
    force_smoke: bool,
}

/// Fold reconciliation verdicts into the signals the evaluator consumes.
fn apply_reconciliation(signals: &mut WorkspaceSignals, verdicts: Vec<DriftVerdict>) {
    signals.reconciliation = verdicts;
}

/// The parts of the previous state row this pass needs: the status to diff
/// against, when it last changed and was last alerted on, and the smoke verdicts
/// + stamp + config to decide whether the probes are due.
#[derive(Default)]
struct PrevState {
    status: Option<HealthStatus>,
    /// When the status last transitioned — the "unhealthy for 3h 20m" suffix on
    /// reminder messages.
    changed_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    /// When Slack was last paged about this workspace, and the failure set that
    /// page carried. Together they decide whether a still-unhealthy workspace is
    /// due for a reminder or has picked up a new failure.
    last_alerted_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    alerted_failures: Option<Vec<DimensionFailure>>,
    last_smoke_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    smoke: Vec<SmokeVerdict>,
    /// The smoke config the stored verdicts were produced under. `None` for a row
    /// written before this was persisted, which reads as "we don't know what ran"
    /// and makes the probes due — the safe direction: one extra smoke run, versus
    /// serving verdicts that may not match the config the UI is showing.
    smoke_config: Option<SmokeTestConfig>,
    /// The smoke config the last *attempt* ran under, written by the claim before
    /// the probes start — as opposed to [`Self::smoke_config`], which records what
    /// the stored **verdicts** were produced under and is only written once a pass
    /// completes.
    ///
    /// The two are the same thing on any pass that finishes. They diverge exactly
    /// when a probe kills the process, and that is the case the cadence gate has
    /// to reason about: see [`PrevState::smoke_gate_config`].
    smoke_attempted_config: Option<SmokeTestConfig>,
}

impl PrevState {
    /// The config the cadence gate compares against.
    ///
    /// `smoke_due` treats "config changed since last time" as due-now regardless
    /// of the clock, which is what keeps the tab honest when a tenant enables a
    /// probe kind. But "last time" has to mean *last attempted*, not *last
    /// succeeded*: a probe that SIGSEGVs never persists a produced-under config,
    /// so comparing against that one makes the workspace due on every pass
    /// forever, and the stamp the claim just wrote is never even consulted. That
    /// is the crash loop this module's claim exists to break.
    ///
    /// Falls back to the produced-under config for rows written before the
    /// attempted key existed, so an upgrade doesn't force one extra smoke run
    /// across the fleet.
    fn smoke_gate_config(&self) -> Option<&SmokeTestConfig> {
        self.smoke_attempted_config
            .as_ref()
            .or(self.smoke_config.as_ref())
    }
}

/// Evaluate one workspace's signals, push Slack on a status transition (or a
/// re-alert while it stays unhealthy), and upsert state. Returns `true` if a
/// message was pushed. Shared by the fleet sweep and the single-workspace path so
/// both behave identically.
async fn eval_and_persist(ctx: &EvalCtx<'_>, signals: &mut WorkspaceSignals) -> bool {
    let workspace_id = signals.workspace_id;
    let verdicts = ctx.reconcile.run_checks(workspace_id).await;
    apply_reconciliation(signals, verdicts);

    let prev = load_prev_state(ctx.db, workspace_id).await;
    let smoke = resolve_smoke(ctx, workspace_id, &prev).await;
    signals.smoke = smoke.verdicts.clone();
    // Availability is read from the wide-event stream, not probed — see
    // `custom_apps`. Gathered every pass rather than on the smoke cadence
    // because it is a ClickHouse aggregate per app, not third-party warehouse
    // code running in-process: it cannot take the server down, so it does not
    // need the attempt-stamping the smoke probes have.
    signals.custom_apps = super::app_availability::gather(ctx.db, workspace_id).await;

    let health = evaluate(signals, ctx.thresholds);
    let failures = health.failures();
    let decision = decide_transition(&AlertInput {
        prev: prev.status,
        next: health.status,
        alerted_failures: prev.alerted_failures.as_deref(),
        next_failures: &failures,
        last_alerted_at: prev.last_alerted_at,
        now: ctx.now,
        reminder_after: reminder_interval(),
    });

    let alerted = notify(ctx, &health, &prev, decision).await;
    // The trail of status changes. Never in the way of the page or the state
    // row: a history row that cannot be written must not stop the evaluation
    // that found the change, and the next pass writes the one that was missed.
    let recorded = super::history::record(
        ctx.db,
        workspace_id,
        prev.status.map(HealthStatus::as_str),
        health.status.as_str(),
        serde_json::to_value(&failures).unwrap_or_else(|_| serde_json::json!([])),
        ctx.now,
    )
    .await;
    if let Err(e) = recorded {
        tracing::warn!(target: "health_eval", %workspace_id, error = %e, "health history not recorded");
    }
    upsert_state(
        ctx.db,
        &StateWrite {
            health: &health,
            signals,
            prev_status: prev.status,
            smoke: &smoke,
            alert: next_alert_state(&health, &prev, alerted, ctx.now),
        },
    )
    .await;
    alerted
}

/// Stamp `last_smoke_at` *before* the probes run, making the smoke cadence
/// attempt-driven rather than success-driven.
///
/// A smoke probe runs third-party native warehouse code in-process, and that code
/// can take the whole server down: the MotherDuck connector has SIGSEGV'd here,
/// which is uncatchable — no `Result`, no unwind, so `upsert_state` at the end of
/// the pass never runs. Stamping only on success therefore leaves `last_smoke_at`
/// untouched, the workspace is due again on the very next pass, and every restart
/// re-runs the same fatal probe. That is a permanent crash loop: one prod
/// workspace sat at `last_smoke_at = NULL` for two weeks while restarting the pod
/// ~72×/day. Committing the attempt first bounds the damage to one crash per
/// smoke interval (6h by default) instead of one per pass.
///
/// The stamp alone is not enough. `smoke_due` short-circuits to "due now"
/// whenever the live config differs from the one it is comparing against, and
/// never reaches the clock — so a claim that recorded only *when* it ran would be
/// written and then ignored, because the produced-under config is itself written
/// only by the pass that never completes. The claim therefore records **what it
/// is about to run** as well, under its own key, and the gate compares against
/// that (see [`PrevState::smoke_gate_config`]).
///
/// Deliberately a **narrow** write: `ON CONFLICT` touches `last_smoke_at` and one
/// key inside `payload`, leaving `status`, `reasons`, `changed_at` and every other
/// payload key exactly as they were. The surrounding eval is still mid-flight, so
/// anything else this pass has read is stale by definition and must not be
/// restated. In particular it does **not** write `smoke_config`: that means "the
/// config the stored verdicts were produced under", and moving it here would make
/// verdicts from an older run read as current — the precise dishonesty the config
/// arm of `smoke_due` exists to prevent.
///
/// The insert arm fires only for a workspace with no state row at all, and
/// records `degraded` / "smoke test did not complete" rather than `healthy`: on
/// the normal path the pass overwrites it with the real rollup microseconds
/// later, and on the crash path it is simply true. A fabricated `healthy` would
/// be a state that could not previously exist — a workspace with no row is absent
/// from the admin table, not well.
///
/// That arm writes the reason into the payload as well as the column, because
/// `payload_parts` reads `reasons` from the payload whenever one is present and
/// only falls back to the column when it is `NULL` — and this claim always writes
/// a payload. Column alone would render as `degraded` with no reason at all.
async fn claim_smoke_attempt(
    db: &DatabaseConnection,
    ws: uuid::Uuid,
    now: chrono::DateTime<chrono::FixedOffset>,
    attempted: &SmokeTestConfig,
) {
    let attempted = match serde_json::to_value(attempted) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                target: "health_eval", error = %e, %ws,
                "could not serialize the attempted smoke config; skipping the claim"
            );
            return;
        }
    };
    let res = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "INSERT INTO workspace_health_state \
               (workspace_id, status, reasons, changed_at, updated_at, payload, last_smoke_at) \
             VALUES ($1, $2, $3, $4, $4, \
                     jsonb_build_object('smoke_attempted_config', $5::jsonb, 'reasons', $3::jsonb), \
                     $4) \
             ON CONFLICT (workspace_id) DO UPDATE SET \
               last_smoke_at = EXCLUDED.last_smoke_at, \
               payload = jsonb_set( \
                 coalesce(workspace_health_state.payload, '{}'::jsonb), \
                 '{smoke_attempted_config}', $5::jsonb)",
            [
                ws.into(),
                HealthStatus::Degraded.as_str().into(),
                json!(["smoke test did not complete"]).into(),
                now.into(),
                attempted.into(),
            ],
        ))
        .await;
    if let Err(e) = res {
        // A failed claim only costs us the crash-loop guard for this pass; the
        // probes are still worth running, so this warns rather than returns.
        tracing::warn!(
            target: "health_eval", error = %e, %ws,
            "smoke attempt claim failed; a fatal probe would be retried next pass"
        );
    }
}

/// Push the decided message, if there is one and Slack is configured. Returns
/// whether a message actually reached Slack — a failed push must not stamp
/// `last_alerted_at`, or the reminder clock restarts on a message nobody saw.
async fn notify(
    ctx: &EvalCtx<'_>,
    health: &WorkspaceHealth,
    prev: &PrevState,
    decision: AlertDecision,
) -> bool {
    if decision == AlertDecision::Silent {
        return false;
    }
    let Some((token, channel)) = ctx.slack else {
        return false;
    };
    let alert = HealthAlert {
        workspace_id: health.workspace_id,
        label: ctx.labels.get(&health.workspace_id),
        status: health.status,
        reasons: &health.reasons,
        decision,
        since: prev.changed_at,
        now: ctx.now,
    };
    match push_slack(ctx.slack_client, token, channel, &alert).await {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(target: "health_eval", error = %e, "slack push failed");
            false
        }
    }
}

/// What to persist about alerting after this pass.
///
/// Leaving unhealthy clears both fields, so the *next* outage pages immediately
/// rather than inheriting a stale reminder clock. While unhealthy the values only
/// advance on a message that actually went out; otherwise they carry forward
/// untouched, which is what keeps the reminder measured from the last page rather
/// than from the last eval pass (10 minutes apart — it would never fire).
#[derive(Default)]
struct AlertState {
    last_alerted_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    /// The failing dimensions the message covered — not its reason text. Reason
    /// strings carry counts that move every pass, so storing them would make the
    /// next pass read normal churn as a new failure and page again.
    failures: Option<Vec<DimensionFailure>>,
}

fn next_alert_state(
    health: &WorkspaceHealth,
    prev: &PrevState,
    alerted: bool,
    now: chrono::DateTime<chrono::Utc>,
) -> AlertState {
    if health.status != HealthStatus::Unhealthy {
        return AlertState::default();
    }
    if alerted {
        return AlertState {
            last_alerted_at: Some(now.fixed_offset()),
            failures: Some(health.failures()),
        };
    }
    AlertState {
        last_alerted_at: prev.last_alerted_at,
        failures: prev.alerted_failures.clone(),
    }
}

/// The smoke half of an eval pass: the verdicts to roll up, when they were
/// produced, which probe kinds are enabled, and the config that produced them.
/// `probes` is persisted so the UI can distinguish a disabled probe from an
/// enabled one with no results; `config` so the next pass can tell whether the
/// stored verdicts still describe the config the UI is showing.
struct SmokeOutcome {
    verdicts: Vec<SmokeVerdict>,
    last_smoke_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    probes: Vec<SmokeProbeStatus>,
    /// `None` only when smoke is switched off entirely — which also clears the
    /// stamp, so re-enabling probes immediately.
    config: Option<SmokeTestConfig>,
    /// What the last *attempt* ran under, mirroring
    /// [`PrevState::smoke_attempted_config`].
    ///
    /// A field of its own rather than reusing [`Self::config`], because the two
    /// differ on exactly the arm that matters: a pass that skips the probes
    /// carries the *produced-under* config forward, which is `None` for a
    /// workspace whose probes have never completed. Writing `config` to both
    /// payload keys therefore erases the claim on the first pass that finishes,
    /// and the pass after that re-runs the fatal probe — a crash every couple of
    /// eval passes instead of once per smoke interval.
    attempted_config: Option<SmokeTestConfig>,
}

/// Decide whether this pass runs the smoke probes, and run them if so.
///
/// `probes` reflects the *current* config every pass, even the ones that don't
/// re-run the probes, so toggling a probe kind shows up on the next pass rather
/// than the next smoke run. That is also why the carry-forward arm is gated on
/// the config and not just the clock: the enabled flags and the verdicts are
/// rendered side by side, so verdicts from a run under a *different* config make
/// the tab contradict itself (a newly-enabled `agent` probe, which always
/// produces a verdict, would show none — "No targets found"). `smoke_due` treats
/// a changed config as due now, so the two can never drift apart by more than
/// one eval pass.
///
/// Most passes still fall through the carry-forward arm: the smoke cadence
/// (default 6h) is far slower than the eval cadence (default 10m), so the
/// previous run's verdicts are reused verbatim and the dimension holds its value
/// instead of flapping to Healthy between smoke runs.
async fn resolve_smoke(
    ctx: &EvalCtx<'_>,
    workspace_id: uuid::Uuid,
    prev: &PrevState,
) -> SmokeOutcome {
    // An absent `smoke_test:` block resolves to the cheap default (connections
    // only), so most workspaces land here with `explicit: false` and still get
    // probed. Only an explicit `enabled: false` switches it off — and that clears
    // the stamp and the stored config, so re-enabling probes immediately rather
    // than waiting out an interval measured from a run under a different config.
    let settings = resolve_smoke_settings(ctx.db, workspace_id).await;
    if !settings.config.enabled {
        return SmokeOutcome {
            verdicts: Vec::new(),
            last_smoke_at: None,
            probes: Vec::new(),
            config: None,
            // Switching smoke off clears the attempted config too, so re-enabling
            // it probes immediately rather than waiting out an interval measured
            // against a config nobody is running.
            attempted_config: None,
        };
    }
    let probes = probe_statuses(&settings.config);

    if !ctx.force_smoke
        && !smoke_due(
            prev.smoke_gate_config(),
            &settings.config,
            prev.last_smoke_at,
            ctx.now,
        )
    {
        return SmokeOutcome {
            verdicts: prev.smoke.clone(),
            last_smoke_at: prev.last_smoke_at,
            probes,
            config: prev.smoke_config.clone(),
            attempted_config: prev.smoke_gate_config().cloned(),
        };
    }

    tracing::info!(target: "health_eval", %workspace_id, "running workspace smoke test");
    // Commit the attempt before handing control to the probes — see
    // `claim_smoke_attempt`. A probe that kills the process must still have moved
    // the cadence forward, or the next pass re-runs it and the server never
    // stays up.
    claim_smoke_attempt(
        ctx.db,
        workspace_id,
        ctx.now.fixed_offset(),
        &settings.config,
    )
    .await;
    let verdicts = ctx.smoke.run_smoke(workspace_id, &settings).await;
    SmokeOutcome {
        verdicts,
        last_smoke_at: Some(ctx.now.fixed_offset()),
        probes,
        attempted_config: Some(settings.config.clone()),
        config: Some(settings.config),
    }
}

/// Single-workspace eval: gather this workspace's signals (synthesizing an empty
/// set when it has no activity), evaluate, push Slack on a transition, and upsert
/// `workspace_health_state`. Same logic as the fleet sweep, scoped to one id.
///
/// `force_smoke` runs the smoke probes on this pass even if their cadence has not
/// elapsed — the "Run smoke test" button. Scheduled fires pass `false` and keep
/// the 6h cadence, so the button is the only thing that can make the probes cost
/// more than the config asked for.
pub(crate) async fn run_eval_pass_single(
    db: &DatabaseConnection,
    workspace_id: uuid::Uuid,
    force_smoke: bool,
) -> Result<String, String> {
    let thresholds = HealthThresholds::from_env();
    let mut signals = gather_signals(db, &thresholds, Some(workspace_id))
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .next()
        .unwrap_or_else(|| WorkspaceSignals::empty(workspace_id));

    let now = chrono::Utc::now();
    let reconcile = LiveReconcileRunner::from_env(now).with_db(db.clone());
    let smoke = LiveSmokeRunner::from_env().with_db(db.clone());
    let slack = ops_slack_target();
    let labels = match gather_workspace_labels(db).await {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!(target: "health_eval", error = %e, "workspace label fetch failed");
            Default::default()
        }
    };
    let client = SlackClient::new();

    let ctx = EvalCtx {
        db,
        reconcile: &reconcile,
        smoke: &smoke,
        slack_client: &client,
        slack: &slack,
        labels: &labels,
        thresholds: &thresholds,
        now,
        force_smoke,
    };
    let alerted = eval_and_persist(&ctx, &mut signals).await;
    Ok(format!("evaluated=1 alerted={}", alerted as usize))
}

/// Load the previous state row: the last-known status (mapped back into the
/// enum), when the smoke probes last ran, and the verdicts they produced. All
/// fields default when there's no prior row (first eval), the read fails, or the
/// stored value is unrecognized — a missing prior state is not an error.
async fn load_prev_state(db: &DatabaseConnection, ws: uuid::Uuid) -> PrevState {
    let Ok(Some(row)) = entity::workspace_health_state::Entity::find_by_id(ws)
        .one(db)
        .await
    else {
        return PrevState::default();
    };
    let status = match row.status.as_str() {
        "unhealthy" => Some(HealthStatus::Unhealthy),
        "degraded" => Some(HealthStatus::Degraded),
        "healthy" => Some(HealthStatus::Healthy),
        _ => None,
    };
    PrevState {
        status,
        changed_at: Some(row.changed_at),
        last_alerted_at: row.last_alerted_at,
        // A malformed or legacy value reads as "we don't know what we paged
        // about", which is not an escalation — the reminder clock still governs.
        alerted_failures: row
            .alerted_failures
            .and_then(|v| serde_json::from_value(v).ok()),
        last_smoke_at: row.last_smoke_at,
        smoke: cached_smoke_verdicts(row.payload.as_ref()),
        smoke_config: cached_smoke_config(row.payload.as_ref()),
        smoke_attempted_config: cached_smoke_attempted_config(row.payload.as_ref()),
    }
}

/// The smoke config the stored verdicts were produced under. Absent (a row from
/// before this key existed) or malformed both yield `None`, which reads as "we
/// can't vouch for what produced these verdicts" and re-runs the probes — one
/// unnecessary smoke run, once, rather than an indefinitely stale tab.
fn cached_smoke_config(payload: Option<&serde_json::Value>) -> Option<SmokeTestConfig> {
    payload
        .and_then(|p| p.get("smoke_config"))
        .and_then(|v| serde_json::from_value(v.clone()).ok())
}

/// The config the last smoke *attempt* ran under. Written by `claim_smoke_attempt`
/// before the probes start, so unlike `smoke_config` it survives a probe that
/// kills the process. `None` on a row written before this key existed, which the
/// gate falls back out of rather than treating as a config change.
fn cached_smoke_attempted_config(payload: Option<&serde_json::Value>) -> Option<SmokeTestConfig> {
    payload
        .and_then(|p| p.get("smoke_attempted_config"))
        .and_then(|v| serde_json::from_value(v.clone()).ok())
}

/// Read the previous run's smoke verdicts back out of the stored payload. A
/// payload written before this dimension existed has no `smoke` key, and a
/// malformed one must not wedge the pass — both yield no verdicts, which reads
/// as "smoke test hasn't run yet" and makes the next pass run it.
fn cached_smoke_verdicts(payload: Option<&serde_json::Value>) -> Vec<SmokeVerdict> {
    payload
        .and_then(|p| p.get("smoke"))
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

/// Everything one state-row write needs. Grouped so [`upsert_state`] stays a
/// two-argument call as the row grows columns.
struct StateWrite<'a> {
    health: &'a WorkspaceHealth,
    signals: &'a WorkspaceSignals,
    /// Status recorded by the previous pass — decides whether `changed_at` moves.
    prev_status: Option<HealthStatus>,
    smoke: &'a SmokeOutcome,
    alert: AlertState,
}

/// Upsert the state row. `status` / `reasons` / `updated_at` are always
/// refreshed; `changed_at` is only bumped to now() when the status actually
/// changed vs the prior value (so it records "since when" the workspace has
/// held this status, not when the row was last touched).
async fn upsert_state(db: &DatabaseConnection, w: &StateWrite<'_>) {
    let StateWrite {
        health,
        signals,
        prev_status: prev,
        smoke,
        alert,
    } = w;
    let ws = health.workspace_id;
    let status = health.status;
    let now = chrono::Utc::now().fixed_offset();
    // Full rollup the read endpoint returns verbatim (labels + timestamps are
    // joined/read separately, so they are intentionally not in the payload).
    // `smoke` is carried forward from the previous run on passes where the smoke
    // cadence hasn't elapsed, so it is always current with `last_smoke_at`;
    // `smoke_probes` reflects the current config so the UI can name a disabled
    // probe even between runs. `smoke_config` records what the verdicts were
    // produced under — the next pass compares it against the live config and
    // re-probes on a change, so the two never contradict each other for longer
    // than one eval pass. It is internal bookkeeping; the UI ignores it.
    let payload = json!({
        "workspace_id": ws,
        "status": status.as_str(),
        "reasons": health.reasons,
        "dimensions": health.dimensions,
        "signals": SignalsRow::from(*signals),
        "reconciliation": signals.reconciliation,
        "smoke": signals.smoke,
        "smoke_probes": smoke.probes,
        "smoke_config": smoke.config,
        // Carried, not recomputed: a completed pass rebuilds `payload` wholesale,
        // so omitting this would erase the claim's key and reopen the retry hole
        // on the very next config change.
        "smoke_attempted_config": smoke.attempted_config,
    });
    let model = entity::workspace_health_state::ActiveModel {
        workspace_id: Set(ws),
        status: Set(status.as_str().to_string()),
        reasons: Set(json!(health.reasons)),
        changed_at: Set(now),
        updated_at: Set(now),
        payload: Set(Some(payload)),
        last_smoke_at: Set(smoke.last_smoke_at),
        last_alerted_at: Set(alert.last_alerted_at),
        alerted_failures: Set(alert.failures.as_ref().map(|f| json!(f))),
    };
    // On conflict, refresh status/reasons/payload/updated_at but NOT changed_at
    // — a re-eval with the same status must preserve the original transition time.
    let res = entity::workspace_health_state::Entity::insert(model)
        .on_conflict(
            OnConflict::column(entity::workspace_health_state::Column::WorkspaceId)
                .update_columns([
                    entity::workspace_health_state::Column::Status,
                    entity::workspace_health_state::Column::Reasons,
                    entity::workspace_health_state::Column::Payload,
                    entity::workspace_health_state::Column::UpdatedAt,
                    entity::workspace_health_state::Column::LastSmokeAt,
                    entity::workspace_health_state::Column::LastAlertedAt,
                    entity::workspace_health_state::Column::AlertedFailures,
                ])
                .to_owned(),
        )
        .exec(db)
        .await;
    if let Err(e) = res {
        tracing::warn!(target: "health_eval", error = %e, "state upsert failed");
        return;
    }
    // Only when the status transitioned: stamp changed_at = now(). On a brand
    // new row the INSERT above already set it correctly, so this is a no-op
    // there (prev == None and a fresh status is still a change → harmless reset
    // to the same now()). The targeted update keeps the steady-state path from
    // clobbering the transition time.
    if *prev != Some(status)
        && let Err(e) = db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "UPDATE workspace_health_state SET changed_at = $1 WHERE workspace_id = $2",
                [now.into(), ws.into()],
            ))
            .await
    {
        tracing::warn!(target: "health_eval", error = %e, "changed_at update failed");
    }
}

/// Ops Slack bot token + channel from env. `None` disables Slack (the dashboard
/// read endpoint still works). No established ops-alert Slack mechanism exists
/// in the codebase today — the per-org `slack_installations` tokens are
/// customer-scoped, so this internal alert path uses dedicated ops env vars.
fn ops_slack_target() -> Option<(String, String)> {
    let token = std::env::var("OXY_OPS_SLACK_BOT_TOKEN").ok()?;
    let channel = std::env::var("OXY_OPS_SLACK_CHANNEL").ok()?;
    Some((token, channel))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::api::admin::workspace_health::evaluator::{
        DimensionResult, HealthDimension,
    };
    use crate::server::api::admin::workspace_health::reconcile::{
        VerdictMeta, unreachable_verdict,
    };
    use crate::server::test_support::{SKIP_MSG, test_db};
    use sea_orm::EntityTrait;

    #[tokio::test]
    async fn single_eval_persists_healthy_row_for_idle_workspace() {
        let Some(db) = test_db().await else {
            eprintln!("{SKIP_MSG}");
            return;
        };
        let ws = uuid::Uuid::new_v4();
        let summary = run_eval_pass_single(&db, ws, false).await.unwrap();
        assert_eq!(summary, "evaluated=1 alerted=0");
        let row = entity::workspace_health_state::Entity::find_by_id(ws)
            .one(&db)
            .await
            .unwrap()
            .expect("a state row should be persisted for the idle workspace");
        assert_eq!(row.status, "healthy");
    }

    /// A [`SmokeRunner`] that records the workspace's persisted `last_smoke_at`
    /// *at the moment the probe is invoked*. That is the only observable that
    /// distinguishes "stamp committed before the probe" from "stamp committed
    /// after it", and the former is what stops a probe that kills the process
    /// from being retried forever.
    struct StampObservingRunner {
        db: DatabaseConnection,
        seen:
            std::sync::Arc<std::sync::Mutex<Option<Option<chrono::DateTime<chrono::FixedOffset>>>>>,
    }

    #[async_trait::async_trait]
    impl SmokeRunner for StampObservingRunner {
        async fn run_smoke(
            &self,
            workspace_id: uuid::Uuid,
            _settings: &super::super::smoke::runner::SmokeSettings,
        ) -> Vec<SmokeVerdict> {
            let stamp = entity::workspace_health_state::Entity::find_by_id(workspace_id)
                .one(&self.db)
                .await
                .unwrap()
                .and_then(|r| r.last_smoke_at);
            *self.seen.lock().unwrap() = Some(stamp);
            Vec::new()
        }
    }

    /// A smoke probe that hard-crashes the process (the MotherDuck connector
    /// SIGSEGV) must not leave the workspace due forever. The only thing that
    /// survives a crash is a committed row, so the stamp has to be written
    /// *before* the probe is handed control — otherwise every restart re-runs the
    /// same fatal probe and the pod never stays up.
    #[tokio::test]
    async fn smoke_stamp_is_committed_before_the_probe_runs() {
        let Some(db) = test_db().await else {
            eprintln!("{SKIP_MSG}");
            return;
        };
        let ws = uuid::Uuid::new_v4();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(None));
        let runner = StampObservingRunner {
            db: db.clone(),
            seen: seen.clone(),
        };
        let reconcile = LiveReconcileRunner::from_env(chrono::Utc::now());
        let slack = None;
        let labels = HashMap::new();
        let thresholds = HealthThresholds::from_env();
        let client = SlackClient::new();
        let ctx = EvalCtx {
            db: &db,
            reconcile: &reconcile,
            smoke: &runner,
            slack_client: &client,
            slack: &slack,
            labels: &labels,
            thresholds: &thresholds,
            now: chrono::Utc::now(),
            force_smoke: true,
        };

        let _ = resolve_smoke(&ctx, ws, &PrevState::default()).await;

        let observed = seen
            .lock()
            .unwrap()
            .expect("the smoke runner should have been invoked");
        assert!(
            observed.is_some(),
            "last_smoke_at must already be persisted when the probe is invoked, \
             or a probe that crashes the process is retried on every restart"
        );
    }

    /// Guard on the *narrowness* of the claim write, not on new behavior: the
    /// claim runs mid-pass, when the status and payload it could restate are
    /// already stale. Two regressions are pinned here: writing `status`/`reasons`/
    /// `changed_at` (which would resurrect a stale status on every smoke pass),
    /// and replacing `payload` wholesale rather than merging one key into it
    /// (which would drop the verdicts the UI carries forward between smoke runs).
    #[tokio::test]
    async fn smoke_claim_touches_only_the_stamp_on_an_existing_row() {
        let Some(db) = test_db().await else {
            eprintln!("{SKIP_MSG}");
            return;
        };
        let ws = uuid::Uuid::new_v4();
        // A fixed instant with no sub-microsecond component, not `Utc::now()`:
        // Postgres `timestamptz` resolves to microseconds while `chrono` carries
        // nanoseconds, so a clock-derived fixture fails the round-trip comparison
        // below on any host whose clock is nanosecond-granular (Linux CI) while
        // passing on one that is microsecond-granular (macOS).
        let earlier = chrono::DateTime::from_timestamp(1_760_000_000, 0)
            .expect("valid fixed timestamp")
            .fixed_offset();
        entity::workspace_health_state::Entity::insert(
            entity::workspace_health_state::ActiveModel {
                workspace_id: Set(ws),
                status: Set("unhealthy".to_string()),
                reasons: Set(json!(["warehouse unreachable"])),
                changed_at: Set(earlier),
                updated_at: Set(earlier),
                payload: Set(Some(json!({"marker": "original"}))),
                last_smoke_at: Set(None),
                last_alerted_at: Set(Some(earlier)),
                alerted_failures: Set(None),
            },
        )
        .exec(&db)
        .await
        .unwrap();

        let now = chrono::Utc::now().fixed_offset();
        claim_smoke_attempt(&db, ws, now, &SmokeTestConfig::default()).await;

        let row = entity::workspace_health_state::Entity::find_by_id(ws)
            .one(&db)
            .await
            .unwrap()
            .expect("row should still exist");
        assert!(row.last_smoke_at.is_some(), "the stamp must be claimed");
        assert_eq!(row.status, "unhealthy", "claim must not restate status");
        assert_eq!(
            row.reasons,
            json!(["warehouse unreachable"]),
            "claim must not restate reasons"
        );
        assert_eq!(
            row.changed_at, earlier,
            "claim must not disturb the transition time"
        );

        let payload = row.payload.expect("payload should still be present");
        assert_eq!(
            payload.get("marker"),
            Some(&json!("original")),
            "claim must merge into the payload, never replace it"
        );
        assert!(
            payload.get("smoke_attempted_config").is_some(),
            "claim must record the config it is about to run, or the cadence gate \
             short-circuits on a config difference and the probe repeats forever"
        );
        assert_eq!(
            payload.as_object().map(|o| o.len()),
            Some(2),
            "claim must add exactly one key — every other part of the payload \
             belongs to the pass that completes"
        );
    }

    /// Counts invocations; the probe itself does nothing. Stands in for a probe
    /// that killed the process: it returns without the surrounding pass ever
    /// reaching `upsert_state`.
    struct CountingRunner {
        calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl SmokeRunner for CountingRunner {
        async fn run_smoke(
            &self,
            _workspace_id: uuid::Uuid,
            _settings: &super::super::smoke::runner::SmokeSettings,
        ) -> Vec<SmokeVerdict> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Vec::new()
        }
    }

    /// The actual regression, on the **scheduled** path.
    ///
    /// `force_smoke: true` bypasses `smoke_due` entirely, so a forced pass cannot
    /// observe this: the crash loop lives in the cadence gate. A probe that kills
    /// the process leaves `upsert_state` unrun, so nothing it would have written
    /// survives — only what the claim committed. The next pass must therefore see
    /// enough state to decline, or every restart re-runs the same fatal probe.
    #[tokio::test]
    async fn a_crashed_probe_is_not_retried_on_the_next_scheduled_pass() {
        let Some(db) = test_db().await else {
            eprintln!("{SKIP_MSG}");
            return;
        };
        let ws = uuid::Uuid::new_v4();
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let runner = CountingRunner {
            calls: calls.clone(),
        };
        let reconcile = LiveReconcileRunner::from_env(chrono::Utc::now());
        let slack = None;
        let labels = HashMap::new();
        let thresholds = HealthThresholds::from_env();
        let client = SlackClient::new();
        let ctx = EvalCtx {
            db: &db,
            reconcile: &reconcile,
            smoke: &runner,
            slack_client: &client,
            slack: &slack,
            labels: &labels,
            thresholds: &thresholds,
            now: chrono::Utc::now(),
            // The scheduled path — this is the whole point of the test.
            force_smoke: false,
        };

        // Pass 1: nothing persisted yet, so the probes are due and run. The pass
        // then "crashes": we simply never call `upsert_state`.
        let prev1 = load_prev_state(&db, ws).await;
        let _ = resolve_smoke(&ctx, ws, &prev1).await;
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the first scheduled pass should run the probes"
        );

        // Pass 2: a fresh process re-reads state from the database — only the
        // claim survived.
        let prev2 = load_prev_state(&db, ws).await;
        let _ = resolve_smoke(&ctx, ws, &prev2).await;
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "a probe that killed the process must not be retried on the next \
             scheduled pass — that is the crash loop"
        );

        // Pass 3: a *completing* pass. This is the one that rebuilds `payload`
        // wholesale, so it is where the claim's key is at risk of being erased —
        // and a skipped smoke run carries the produced-under config, which is
        // still null here because no pass has ever finished the probes.
        let mut signals = WorkspaceSignals::empty(ws);
        let _ = eval_and_persist(&ctx, &mut signals).await;
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the completing pass is inside the cadence and must not probe either"
        );

        // Pass 4: the loop closes here or it doesn't. If pass 3 dropped the
        // attempted config, the gate sees a config difference again and re-runs
        // the fatal probe — a crash every couple of eval passes rather than once
        // per smoke interval.
        let prev4 = load_prev_state(&db, ws).await;
        let _ = resolve_smoke(&ctx, ws, &prev4).await;
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "a completing pass must not erase the attempted config the claim \
             wrote, or the next pass re-runs the probe that killed the process"
        );
    }

    /// A rollup whose failing dimensions are `dims`, each with a reason string
    /// carrying the kind of live count that drifts between passes.
    fn health_of(
        status: HealthStatus,
        dims: &[(HealthDimension, HealthStatus)],
    ) -> WorkspaceHealth {
        let dimensions: Vec<DimensionResult> = dims
            .iter()
            .map(|(dimension, status)| DimensionResult {
                dimension: *dimension,
                status: *status,
                reason: Some(format!("{dimension:?} is {}", status.as_str())),
            })
            .collect();
        WorkspaceHealth {
            workspace_id: uuid::Uuid::nil(),
            status,
            reasons: dimensions.iter().filter_map(|d| d.reason.clone()).collect(),
            dimensions,
        }
    }

    fn failures(dims: &[(HealthDimension, HealthStatus)]) -> Vec<DimensionFailure> {
        dims.iter()
            .map(|(dimension, status)| DimensionFailure {
                dimension: *dimension,
                status: *status,
            })
            .collect()
    }

    fn alerted_prev(hours_ago: i64, dims: &[(HealthDimension, HealthStatus)]) -> PrevState {
        PrevState {
            status: Some(HealthStatus::Unhealthy),
            last_alerted_at: Some(
                (chrono::Utc::now() - chrono::Duration::hours(hours_ago)).fixed_offset(),
            ),
            alerted_failures: Some(failures(dims)),
            ..PrevState::default()
        }
    }

    #[test]
    fn a_silent_pass_carries_the_alert_clock_forward() {
        // The reminder must be measured from the last page, not from the last eval
        // pass — those are 10 minutes apart, so re-stamping here would mean the 6h
        // reminder never comes due.
        let broken = [(HealthDimension::JobLiveness, HealthStatus::Unhealthy)];
        let prev = alerted_prev(2, &broken);
        let next = next_alert_state(
            &health_of(HealthStatus::Unhealthy, &broken),
            &prev,
            false,
            chrono::Utc::now(),
        );
        assert_eq!(next.last_alerted_at, prev.last_alerted_at);
        assert_eq!(next.failures, prev.alerted_failures);
    }

    #[test]
    fn a_pushed_message_stamps_the_clock_and_the_failing_dimensions() {
        let now = chrono::Utc::now();
        let broken = [
            (HealthDimension::JobLiveness, HealthStatus::Unhealthy),
            (HealthDimension::Queue, HealthStatus::Unhealthy),
        ];
        let next = next_alert_state(
            &health_of(HealthStatus::Unhealthy, &broken),
            &alerted_prev(7, &broken[..1]),
            true,
            now,
        );
        assert_eq!(next.last_alerted_at, Some(now.fixed_offset()));
        assert_eq!(next.failures, Some(failures(&broken)));
    }

    #[test]
    fn what_we_persist_is_dimensions_not_reason_text() {
        // The alert-storm regression, at the storage end: persisting reason
        // strings would make the next pass compare against text that has already
        // moved on ("3/10 runs failed (30%)" → "4/12 … (33%)") and page again.
        let broken = [(HealthDimension::JobLiveness, HealthStatus::Unhealthy)];
        let next = next_alert_state(
            &health_of(HealthStatus::Unhealthy, &broken),
            &alerted_prev(7, &broken),
            true,
            chrono::Utc::now(),
        );
        let stored = json!(next.failures.unwrap());
        assert_eq!(
            stored,
            json!([{ "dimension": "job_liveness", "status": "unhealthy" }])
        );
        // And it round-trips back through the column into the same value.
        let back: Vec<DimensionFailure> = serde_json::from_value(stored).unwrap();
        assert_eq!(back, failures(&broken));
    }

    #[test]
    fn leaving_unhealthy_clears_the_alert_clock() {
        // So the next outage pages immediately instead of inheriting a clock that
        // says "we already told them 20 minutes ago".
        for status in [HealthStatus::Healthy, HealthStatus::Degraded] {
            let next = next_alert_state(
                &health_of(status, &[]),
                &alerted_prev(
                    1,
                    &[(HealthDimension::JobLiveness, HealthStatus::Unhealthy)],
                ),
                false,
                chrono::Utc::now(),
            );
            assert_eq!(next.last_alerted_at, None);
            assert_eq!(next.failures, None);
        }
    }

    fn empty_signals() -> WorkspaceSignals {
        WorkspaceSignals {
            workspace_id: uuid::Uuid::nil(),
            failed_runs: 0,
            timed_out_runs: 0,
            total_runs: 0,
            airway_last_run_failed: false,
            airway_completed_with_errors: false,
            open_high_anomalies: 0,
            open_medium_anomalies: 0,
            dead_letter_count: 0,
            reconciliation: Vec::new(),
            smoke: Vec::new(),
            custom_apps: Vec::new(),
        }
    }

    #[test]
    fn apply_reconciliation_sets_verdicts() {
        let mut s = empty_signals();
        let meta = VerdictMeta {
            check: "c".to_string(),
            description: None,
            actual_label: "Actual".to_string(),
            expected_label: "Expected".to_string(),
            window: super::super::reconcile::ResolvedWindow {
                dates: ["2026-07-12".to_string(), "2026-07-18".to_string()],
                timezone: "UTC".to_string(),
            },
        };
        apply_reconciliation(&mut s, vec![unreachable_verdict(&meta, "toast")]);
        assert_eq!(s.reconciliation.len(), 1);
        assert_eq!(s.reconciliation[0].status, HealthStatus::Degraded);
    }

    #[test]
    fn cached_smoke_verdicts_survive_a_payload_round_trip() {
        // This is the carry-forward path: on the ~35 eval passes between two 6h
        // smoke runs, the dimension's value comes from here.
        use crate::server::api::admin::workspace_health::smoke::{SmokeProbeKind, failed};
        let stored = json!({
            "smoke": [ failed(SmokeProbeKind::Semantic, "orders", "boom".into(), 7) ]
        });
        let back = cached_smoke_verdicts(Some(&stored));
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].check, "semantic:orders");
        assert_eq!(back[0].status, HealthStatus::Unhealthy);
    }

    #[test]
    fn missing_or_malformed_smoke_payload_yields_no_verdicts() {
        // A row written before the smoke dimension existed, and a corrupted one:
        // both must read as "hasn't run yet" rather than wedging the pass.
        assert!(cached_smoke_verdicts(None).is_empty());
        assert!(cached_smoke_verdicts(Some(&json!({ "reconciliation": [] }))).is_empty());
        assert!(cached_smoke_verdicts(Some(&json!({ "smoke": "not-an-array" }))).is_empty());
    }

    #[test]
    fn cached_smoke_config_round_trips_and_fails_safe() {
        let cfg = SmokeTestConfig {
            semantic: oxy::config::health_check::SemanticProbeConfig::Sweep(true),
            ..SmokeTestConfig::default()
        };
        let back = cached_smoke_config(Some(&json!({ "smoke_config": cfg })));
        assert_eq!(back.as_ref().map(|c| c.semantic.enabled()), Some(true));

        // A row from before this key existed, a null (smoke switched off), and a
        // corrupted value all read as "unknown" → the probes re-run. Never as an
        // empty config, which would silently match nothing and freeze the tab.
        assert!(cached_smoke_config(None).is_none());
        assert!(cached_smoke_config(Some(&json!({ "smoke": [] }))).is_none());
        assert!(cached_smoke_config(Some(&json!({ "smoke_config": null }))).is_none());
        assert!(cached_smoke_config(Some(&json!({ "smoke_config": "nonsense" }))).is_none());
    }
}
