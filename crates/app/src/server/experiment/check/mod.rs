//! The `experiment_check` job's work: read the warehouse, write what it saw
//! onto the run. Never actuates — the tick does that on its next pass.

pub mod executor;
pub mod guardrails;
pub mod members;
pub mod periods;
pub mod pin;
pub mod read_back;
pub mod reader;

use chrono::{DateTime, Utc};
use entity::experiment_runs;
use oxy_experiment::run::RunState;
use oxy_experiment::timeline::local_hour_in;
use sea_orm::{DatabaseConnection, DbErr};

use self::guardrails::GuardrailInputs;
use self::read_back::ReadBackInputs;
use self::reader::{AnomalyScan, Membership, UnitDays};
use super::store::{self, Observations};

pub struct Readers<'a> {
    pub days: &'a dyn UnitDays,
    pub anomalies: &'a dyn AnomalyScan,
    pub groups: &'a dyn Membership,
}

/// Guardrails while `live` only: the effect is fixed once the run closes.
async fn watch(
    inp: &ReadBackInputs<'_>,
    readers: &Readers<'_>,
    obs: &mut Observations,
    now: DateTime<Utc>,
) -> Option<String> {
    if RunState::parse(&inp.run.state) != Some(RunState::Live) {
        return None;
    }
    let watched = guardrails::watched(inp.c, inp.flips, inp.membership, inp.fleet, inp.today);
    let inputs = GuardrailInputs {
        c: inp.c,
        watched: &watched,
        today: inp.today,
        now,
    };
    let (statuses, breach) = guardrails::evaluate(&inputs, readers).await;
    obs.guardrails = statuses;
    breach
}

pub async fn run_check(
    db: &DatabaseConnection,
    run: &experiment_runs::Model,
    readers: &Readers<'_>,
    now: DateTime<Utc>,
) -> Result<String, String> {
    let e = |e: DbErr| e.to_string();
    let c = store::pinned_config(db, run).await?;
    let local = local_hour_in(c.design.timezone, now);
    let today = local.date();
    let mut obs = Observations {
        problems: Vec::new(),
        ..store::observations(run)
    };
    let flips = store::flips_of(db, run.id).await.map_err(e)?;
    let pinned = pin::pin_all(db, run, &c, readers, &mut obs)
        .await
        .map_err(e)?;
    let inp = ReadBackInputs {
        run,
        c: &c,
        flips: &flips,
        today,
        now: local,
        membership: pinned.membership.as_ref(),
        fleet: pinned.fleet.as_deref(),
    };
    match (&c.design.switchback, &pinned.membership) {
        (Some(sb), _) => periods::read_periods(db, &inp, sb, readers.days, &mut obs)
            .await
            .map_err(e)?,
        (None, Some(groups)) => members::treated_groups(db, &inp, groups, readers.days, &mut obs)
            .await
            .map_err(e)?,
        (None, None) => read_back::treated(db, &inp, readers.days, &mut obs.problems)
            .await
            .map_err(e)?,
    }
    if let Some(pool) = &pinned.pool {
        // Under assign_by the controls are the control groups' pinned members.
        let controls: Vec<String> = pool
            .iter()
            .flat_map(|g| read_back::subjects(&inp, g))
            .map(str::to_string)
            .collect();
        read_back::controls(&inp, &controls, readers.days, &mut obs).await;
    }
    read_back::coverage(&inp, readers.days, &mut obs).await;
    // The same unreadable day is reported once per unit it hit, not once per read.
    let mut seen = std::collections::HashSet::new();
    obs.problems.retain(|p| seen.insert(p.clone()));
    let breach = watch(&inp, readers, &mut obs, now).await;
    obs.checked_at = Some(now);
    store::save_observations(db, run.id, &obs, breach.as_deref(), now)
        .await
        .map_err(e)?;
    Ok(format!(
        "checked `{}`: {} problem(s)",
        c.experiment,
        obs.problems.len()
    ))
}
