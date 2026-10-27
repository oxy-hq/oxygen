//! `GET /experiments`: one item per experiment record. A registered experiment
//! shows its run's state and pinned version; one with no run shows its latest
//! version and what registering it now would get.

use chrono::{DateTime, Utc};
use entity::{experiment_designs, experiment_runs, experiments};
use oxy_experiment::run::RunState;
use oxy_experiment::timeline::local_hour_in;
use sea_orm::{DatabaseConnection, DbErr};
use uuid::Uuid;

use super::design_store::{self, Design};
use super::lever::LeverChecks;
use super::register::{self, Registration};
use super::retrospective::{self, Retrospective};
use super::status::{ExperimentStatus, next_switch_on};
use super::store;
use super::summary::{ExperimentListItem, LatestRow, latest_proposals, next_switch};

/// What the item says about the version it shows.
#[derive(Debug, Clone, PartialEq)]
pub struct Standing {
    pub status: ExperimentStatus,
    pub registration_refusal: Option<String>,
    pub next_switch_on: Option<oxy_experiment::Moment>,
}

pub async fn list_items(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    now: DateTime<Utc>,
) -> Result<Vec<ExperimentListItem>, DbErr> {
    let latest = latest_proposals(db, workspace_id).await?;
    let experiments = design_store::list_experiments(db, workspace_id).await?;
    // One query each for every experiment's run and design, and one lever
    // lookup per distinct lever: the list's cost does not grow per experiment.
    let ids: Vec<Uuid> = experiments.iter().map(|e| e.id).collect();
    let mut runs = store::runs_of(db, &ids).await?;
    let mut designs = design_store::designs_of(db, &ids).await?;
    let mut levers = LeverChecks::default();
    let mut out = Vec::new();
    for e in experiments {
        let run = runs.remove(&e.id);
        let row = match &run {
            Some(run) => designs.remove(&run.design_id),
            None => designs
                .iter()
                .find(|(_, d)| d.experiment_id == e.id)
                .map(|(id, _)| *id)
                .and_then(|id| designs.remove(&id)),
        };
        match shown(db, &e, run, row, now, &mut levers).await? {
            Some((design, standing, retro)) => {
                let mut it = item(&e, &design, standing, latest.get(&e.name));
                (it.retrospective, it.retrospective_ready) = (retro.run, retro.ready);
                out.push(it);
            }
            None => tracing::warn!(
                target: "experiment", experiment = %e.name,
                "an experiment is omitted from the list: its design or run state cannot be read"
            ),
        }
    }
    Ok(out)
}

/// Whether the run is retrospective, and — with no run — whether one could be made now.
#[derive(Debug, Clone, Copy, Default)]
struct Retro {
    run: bool,
    ready: bool,
}

/// The version an item shows, and its standing: the run's, else the latest's.
/// `row` is the run's pinned design, else the experiment's draft. `None` when
/// that version or the run's state does not read.
async fn shown(
    db: &DatabaseConnection,
    e: &experiments::Model,
    run: Option<experiment_runs::Model>,
    row: Option<experiment_designs::Model>,
    now: DateTime<Utc>,
    levers: &mut LeverChecks,
) -> Result<Option<(Design, Standing, Retro)>, DbErr> {
    let Some(Ok(design)) = row.map(design_store::decode) else {
        if let Some(run) = &run {
            tracing::warn!(target: "experiment", run_id = %run.id, "the run's pinned design is gone or unreadable");
        }
        return Ok(None);
    };
    if let Some(run) = run {
        let Some(state) = RunState::parse(&run.state) else {
            return Ok(None);
        };
        let local = local_hour_in(design.config.design.timezone, now);
        let standing = Standing {
            status: state.into(),
            registration_refusal: None,
            next_switch_on: next_switch_on(&design.config, state, local),
        };
        let retro = Retro {
            run: run.retrospective,
            ready: false,
        };
        return Ok(Some((design, standing, retro)));
    }
    let c = &design.config;
    let next_switch_on = next_switch(c, local_hour_in(c.design.timezone, now));
    let (status, registration_refusal) =
        match register::decide_with(db, e.workspace_id, c, now, levers).await? {
            Registration::Draft { .. } => (ExperimentStatus::Draft, None),
            Registration::Refused { reason } => {
                (ExperimentStatus::RegistrationRefused, Some(reason))
            }
            Registration::Ready => (ExperimentStatus::Registrable, None),
        };
    let standing = Standing {
        status,
        registration_refusal,
        next_switch_on,
    };
    let retro = Retro {
        run: false,
        ready: retrospective::judge(c, now) == Retrospective::Ready,
    };
    Ok(Some((design, standing, retro)))
}

fn item(
    e: &experiments::Model,
    d: &Design,
    s: Standing,
    latest: Option<&LatestRow>,
) -> ExperimentListItem {
    ExperimentListItem {
        name: e.name.clone(),
        owner: d.config.owner.clone(),
        hypothesis: d.config.hypothesis.clone(),
        design_id: d.row.id,
        design_hash: d.row.content_hash.clone(),
        design_frozen_at: d.row.frozen_at,
        status: s.status,
        registration_refusal: s.registration_refusal,
        next_switch_on: s.next_switch_on.map(|day| day.to_string()),
        proposals_count: latest.map_or(0, LatestRow::count),
        latest_proposal: latest.map(LatestRow::summary),
        // The decoded config, not the stored JSON: a row written outside the API
        // can leave out keys the config defaults, and the page reads them all.
        definition: serde_json::to_value(&d.config).unwrap_or_else(|_| d.row.config.clone()),
        retrospective: false,
        retrospective_ready: false,
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::server::experiment::test_design::design;

    #[test]
    fn an_item_carries_the_whole_shown_version() {
        let at = chrono::Utc
            .with_ymd_and_hms(2026, 9, 30, 12, 0, 0)
            .unwrap()
            .fixed_offset();
        let c = design();
        let e = experiments::Model {
            id: Uuid::from_u128(1),
            workspace_id: Uuid::from_u128(2),
            name: c.experiment.clone(),
            hypothesis: "as created".into(),
            owner: "someone".into(),
            created_by: Uuid::from_u128(3),
            created_at: at,
        };
        let row = experiment_designs::Model {
            id: Uuid::from_u128(4),
            experiment_id: e.id,
            from_proposal_id: None,
            config: serde_json::to_value(&c).unwrap(),
            content_hash: "abc".into(),
            created_by: e.created_by,
            created_at: at,
            updated_by: e.created_by,
            updated_at: at,
            frozen_at: Some(at),
        };
        let d = Design { row, config: c };
        let s = Standing {
            status: ExperimentStatus::Live,
            registration_refusal: None,
            next_switch_on: Some("2026-03-16".parse().unwrap()),
        };
        let v = serde_json::to_value(item(&e, &d, s, None)).unwrap();
        assert_eq!(v["design_hash"], "abc");
        assert!(v["design_frozen_at"].is_string());
        assert_eq!(v["owner"], "ops", "the shown version's owner");
        assert_eq!(v["status"], "live");
        assert_eq!(v["next_switch_on"], "2026-03-16");
        assert_eq!(v["definition"]["lever"]["on"]["via"], "promo_on");
        assert_eq!(v["proposals_count"], 0);
    }

    /// A row written outside the API (a seed, or the records migration's
    /// backfill of an old run) can leave out keys the config defaults. The item
    /// still carries them, as the version decodes, so the page can rely on them.
    #[test]
    fn an_item_fills_in_what_the_stored_design_left_out() {
        let at = chrono::Utc
            .with_ymd_and_hms(2026, 9, 30, 12, 0, 0)
            .unwrap()
            .fixed_offset();
        let c = design();
        let mut stored = serde_json::to_value(&c).unwrap();
        stored.as_object_mut().unwrap().remove("guardrails");
        stored["lever"].as_object_mut().unwrap().remove("on_close");
        let e = experiments::Model {
            id: Uuid::from_u128(1),
            workspace_id: Uuid::from_u128(2),
            name: c.experiment.clone(),
            hypothesis: String::new(),
            owner: String::new(),
            created_by: Uuid::from_u128(3),
            created_at: at,
        };
        let row = experiment_designs::Model {
            id: Uuid::from_u128(4),
            experiment_id: e.id,
            from_proposal_id: None,
            config: stored,
            content_hash: String::new(),
            created_by: e.created_by,
            created_at: at,
            updated_by: e.created_by,
            updated_at: at,
            frozen_at: None,
        };
        let d = design_store::decode(row).unwrap();
        let s = Standing {
            status: ExperimentStatus::Draft,
            registration_refusal: None,
            next_switch_on: None,
        };
        let v = serde_json::to_value(item(&e, &d, s, None)).unwrap();
        assert_eq!(v["definition"]["guardrails"], serde_json::json!([]));
        assert_eq!(v["definition"]["lever"]["on_close"], "off");
    }
}
