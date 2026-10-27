//! Reads and the one write of the estimate job. Every read is scoped by ids the
//! run row already carries; the pool, the pins, the flips and the observations
//! come from the run itself (PR 4's `store`), and the design from the version it
//! is pinned to — insert-only, so it reads the same way years later.

use std::sync::Arc;

use entity::{experiment_results, experiment_runs, semantic_views, workspaces};
use oxy_experiment::config::ExperimentConfig;
use oxy_experiment::run::AbortLevers;
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveValue::Set, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, PaginatorTrait,
    QueryFilter, TryInsertResult,
};
use uuid::Uuid;

use super::compute::{Computed, RunInputs, ViewRow};
use super::plan::{FlipDay, design_ref, kept_levers_cut, measured_until};
use crate::server::experiment::proposal_store::{count_done_before, latest_refusal_before};
use crate::server::experiment::record::compliance_outcomes;
use crate::server::experiment::status::lever_revisions;
use crate::server::experiment::store as runs;

pub async fn result_exists(db: &DatabaseConnection, run_id: Uuid) -> Result<bool, DbErr> {
    let n = experiment_results::Entity::find()
        .filter(experiment_results::Column::RunId.eq(run_id))
        .count(db)
        .await?;
    Ok(n > 0)
}

/// The workspace's promoted revision, if it has one.
pub async fn promoted_revision(
    db: &DatabaseConnection,
    workspace_id: Uuid,
) -> Result<Option<Uuid>, DbErr> {
    Ok(workspaces::Entity::find_by_id(workspace_id)
        .one(db)
        .await?
        .and_then(|w| w.current_revision_id))
}

pub async fn load_inputs(
    db: &DatabaseConnection,
    run: &experiment_runs::Model,
    aborted: bool,
) -> Result<RunInputs, String> {
    load_inputs_for(db, run, aborted, true).await
}

/// [`load_inputs`] with the compliance record's `final_record` chosen by the caller: an
/// interim reads a run still in progress, where a flip not yet due is pending, not missed.
pub async fn load_inputs_for(
    db: &DatabaseConnection,
    run: &experiment_runs::Model,
    aborted: bool,
    final_record: bool,
) -> Result<RunInputs, String> {
    let err = |what: &'static str| move |e: DbErr| format!("{what}: {e}");
    // The frozen design the run is pinned to.
    let design = runs::pinned(db, run).await?;
    let hash = design.row.content_hash.clone();
    let adopted = design.row.from_proposal_id.is_some();
    let config = design.config;
    let flips = runs::flips_of(db, run.id)
        .await
        .map_err(err("load flips"))?;
    let pool = runs::control_pool(run);
    let membership = runs::group_membership(run);
    let fleet = runs::fleet(run);
    // PR 4's record (C4), final, no thresholds: per unit, per MEMBER under `assign_by`,
    // per unit-per-period for a switchback over the pinned fleet. `summarize` pools it;
    // nothing here weights a group, and a switchback that flipped nothing pools to
    // `n_treated == 0` — not measured.
    let outcomes = compliance_outcomes(
        &config,
        &flips,
        pool.as_deref(),
        membership.as_ref(),
        fleet.as_deref(),
        &runs::observations(run),
        final_record,
    );
    // `done` proposals only, and only those logged before the design was registered.
    let proposal_count =
        count_done_before(db, run.workspace_id, &config.experiment, run.registered_at)
            .await
            .map_err(err("count proposals"))? as usize;
    let mde_unavailable = match config.outcome.mde_at_proposal {
        Some(_) => None,
        None => latest_refusal_before(
            db,
            run.workspace_id,
            &config.experiment,
            run.registered_at,
            &hash,
        )
        .await
        .map_err(err("read the pricing refusal"))?,
    };
    let view = load_view(db, run, &config).await?;
    // An aborted run stops reading where the levers went off (`plan::measured_until`), or,
    // when the abort kept them, on the day it was aborted (`plan::kept_levers_cut`).
    let levers_kept = aborted && levers_kept(run);
    let cut = match run.aborted_on.filter(|_| levers_kept) {
        Some(day) => kept_levers_cut(&config, day),
        None => measured_until(&config, aborted, &flip_days(&flips)),
    };
    let carryover = super::carryover::carryover_window(&config, aborted, &flip_days(&flips));
    Ok(RunInputs {
        config: Arc::new(config),
        control_pool: pool,
        aborted,
        retrospective: run.retrospective,
        abort_reason: abort_reason(run),
        measured_until: cut,
        levers_kept,
        carryover,
        design_ref: design_ref(&hash),
        outcomes,
        proposal_count,
        hand_assigned: !adopted && proposal_count > 0,
        mde_unavailable,
        lever_revisions: lever_revisions(&flips).len(),
        view,
    })
}

/// Why the run was aborted, in the runner's own order (`run::abort_reason`): the guardrail
/// breach that tripped it — stored in `guardrail_breach`, naming the breaching unit — else
/// the owner's call (`abort_reason`).
fn abort_reason(run: &experiment_runs::Model) -> Option<String> {
    run.guardrail_breach
        .as_ref()
        .map(|b| format!("guardrail breached: {b}"))
        .or_else(|| run.abort_reason.clone())
}

/// The abort left the levers as they were: what the tick recorded on the way into `aborted`.
fn levers_kept(run: &experiment_runs::Model) -> bool {
    runs::abort_levers(run) == Some(AbortLevers::Keep)
}

/// The flip rows as the abort cut reads them. A row whose state does not parse records no
/// lever state and is skipped.
fn flip_days(flips: &[entity::experiment_flips::Model]) -> Vec<FlipDay> {
    flips
        .iter()
        .filter_map(|f| {
            Some(FlipDay {
                state: oxy_experiment::run::LeverState::parse(&f.target_state)?,
                due_at: oxy_experiment::Moment(f.due_at),
                wave: usize::try_from(f.wave).ok()?,
                cancelled: f.status == "cancelled",
                reached: f.status != "cancelled"
                    || f.reason.as_deref() == Some(crate::server::experiment::send::PERIOD_ENDED),
            })
        })
        .collect()
}

/// The compiled view that holds `outcome.target`'s measure — only when a driver is
/// registered. The row carries the parsed definition and a path, never the text.
/// Views stay in git and resolve by name, so it is read at the workspace's
/// promoted revision: that is the file a write-back edits.
pub async fn load_view(
    db: &DatabaseConnection,
    run: &experiment_runs::Model,
    cfg: &ExperimentConfig,
) -> Result<Option<ViewRow>, String> {
    if cfg.outcome.driver.is_none() {
        return Ok(None);
    }
    let Some((view_name, _)) = cfg.outcome.target.split_once('.') else {
        return Ok(None);
    };
    let promoted = promoted_revision(db, run.workspace_id)
        .await
        .map_err(|e| format!("load workspace: {e}"))?;
    if let Some(revision) = promoted {
        let row = semantic_views::Entity::find_by_id((revision, view_name.to_string()))
            .one(db)
            .await
            .map_err(|e| format!("load view: {e}"))?;
        if let Some(v) = row {
            return Ok(Some(ViewRow {
                file_path: v.file_path,
                definition: v.definition,
            }));
        }
    }
    Ok(None)
}

/// `false` when the unique `run_id` made this a conflict: the registered result
/// already exists and is left exactly as it was.
pub async fn insert_result(
    db: &DatabaseConnection,
    run_id: Uuid,
    c: &Computed,
    aborted: bool,
) -> Result<bool, DbErr> {
    let custom = |e: serde_json::Error| DbErr::Custom(e.to_string());
    let row = experiment_results::ActiveModel {
        id: Set(Uuid::new_v4()),
        run_id: Set(run_id),
        effect: Set(c.effect_raw.clone()),
        ratio: Set(c.ratio_raw.clone()),
        compliance: Set(c.compliance_json.clone()),
        // The same object as `report.prediction`, kept in its own column (C3).
        prediction: Set(c.prediction_json.clone()),
        series: Set(c.series_json.clone()),
        report: Set(serde_json::to_value(&c.report).map_err(custom)?),
        coefficient_patch: Set(c
            .edit
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(custom)?),
        aborted: Set(aborted),
        created_at: Set(chrono::Utc::now().fixed_offset()),
    };
    let res = experiment_results::Entity::insert(row)
        .on_conflict(
            OnConflict::column(experiment_results::Column::RunId)
                .do_nothing()
                .to_owned(),
        )
        .try_insert()
        .exec(db)
        .await?;
    Ok(!matches!(res, TryInsertResult::Conflicted))
}
