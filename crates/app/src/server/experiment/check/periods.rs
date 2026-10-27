//! Switchback read-back (spec "Switchback"): `when_on` through on periods,
//! `when_off` through off periods, at every unit, per period. A row is the
//! flip of the period whose first day and state it carries — sent, or
//! cancelled because its period had ended (C2b). Task 2's `classify_period`
//! decides it; the outcome is the record, kept under `"<unit>@<period from>"`
//! (C4) in `observations.period_outcomes`, and a sent row is also settled for
//! the status route's progress.

use std::collections::BTreeMap;

use entity::experiment_flips;
use oxy_experiment::Moment;
use oxy_experiment::compliance::{FlipOutcome, PeriodSpan, Sampling, classify_period, period_key};
use oxy_experiment::config::SwitchbackConfig;
use oxy_experiment::expr::{Comparison, parse_comparison};
use oxy_experiment::run::LeverState;
use sea_orm::{DatabaseConnection, DbErr};

use super::read_back::{ReadBackInputs, grace_end, held};
use super::reader::UnitDays;
use crate::server::experiment::batch::period_span;
use crate::server::experiment::send::PERIOD_ENDED;
use crate::server::experiment::store::{self, Observations, Recorded};

pub async fn read_periods(
    db: &DatabaseConnection,
    inp: &ReadBackInputs<'_>,
    sb: &SwitchbackConfig,
    days: &dyn UnitDays,
    obs: &mut Observations,
) -> Result<(), DbErr> {
    let Some(rb) = inp.c.read_back() else {
        obs.problems
            .push("the design has no lever, so there is nothing to read back".into());
        return Ok(());
    };
    let (on, off) = match (
        parse_comparison(&rb.when_on),
        parse_comparison(&rb.when_off),
    ) {
        (Ok(on), Ok(off)) => (on, off),
        (Err(e), _) => {
            obs.problems.push(format!("lever.read_back.when_on: {e}"));
            return Ok(());
        }
        (_, Err(e)) => {
            obs.problems.push(format!("lever.read_back.when_off: {e}"));
            return Ok(());
        }
    };
    // Sent rows, and rows cancelled because their period had ended (C2b). A
    // row cancelled by an abort is a period the run abandoned: never read.
    for flip in inp.flips.iter().filter(|f| {
        f.status == "dispatched"
            || (f.status == "cancelled" && f.reason.as_deref() == Some(PERIOD_ENDED))
    }) {
        let Some(span) = period_span(sb, inp.c.design.grain, flip) else {
            continue;
        };
        let key = period_key(&flip.unit, span.from);
        if obs.period_outcomes.contains_key(&key) {
            continue; // decided: never re-read
        }
        let cmp = if span.state == LeverState::On {
            &on
        } else {
            &off
        };
        let held = match read_held(inp, &span, cmp, &flip.unit, days).await {
            Ok(Some(held)) => held,
            Ok(None) => continue,
            Err(e) => {
                obs.problems.push(e);
                continue;
            }
        };
        let Some(outcome) =
            classify_period(&span, rb.grace_days, Sampling::of(inp.c), inp.now, &held)
        else {
            continue;
        };
        if flip.status == "dispatched" {
            let rec = Recorded {
                run_id: inp.run.id,
                field: "period_outcomes",
                entries: BTreeMap::from([(key.as_str(), &outcome)]),
            };
            settle(db, flip, &span, &outcome, &rec).await?;
        }
        obs.period_outcomes.insert(key, outcome);
    }
    Ok(())
}

/// The expression per returned reading over what the period needs read: an on
/// period its grace window, an off period through its end. `None` before it starts.
async fn read_held(
    inp: &ReadBackInputs<'_>,
    span: &PeriodSpan,
    cmp: &Comparison,
    unit: &str,
    days: &dyn UnitDays,
) -> Result<Option<BTreeMap<Moment, bool>>, String> {
    let sampling = Sampling::of(inp.c);
    let grace = grace_end(span.from, inp.c.grace_days());
    // The last reading inside the period: the one before `until`.
    let last = span
        .until
        .plus_hours(match sampling {
            Sampling::Daily => -24,
            Sampling::Hourly(_) => -1,
        })
        .unwrap_or(span.until);
    let read_to = if span.state == LeverState::On {
        grace.min(last)
    } else {
        last
    }
    .min(inp.now);
    if read_to < span.from {
        return Ok(None);
    }
    let one = [unit.to_string()];
    Ok(Some(
        held(days, cmp, &one, (span.from, read_to), sampling).await?,
    ))
}

/// A sent row's progress for the status route. A landed on period is
/// confirmed on its landing day; an off period that leaked is unconfirmed on
/// the leak day (C3 `confirmed_on`), the reason naming it. The outcome is kept on the run in
/// the same statement (`rec`), so a later failure in the check cannot drop it.
async fn settle(
    db: &DatabaseConnection,
    flip: &experiment_flips::Model,
    span: &PeriodSpan,
    outcome: &FlipOutcome,
    rec: &Recorded<'_>,
) -> Result<(), DbErr> {
    let unit = &flip.unit;
    let id = flip.id;
    match outcome {
        FlipOutcome::Landed { on_day } => {
            store::settle_flip_recorded(db, id, "confirmed", Some(*on_day), None, rec).await
        }
        FlipOutcome::Clean => {
            store::settle_flip_recorded(db, id, "confirmed", None, None, rec).await
        }
        FlipOutcome::NeverLanded => {
            let why = format!(
                "`when_on` never held at `{unit}` within grace_days of the on period from {}",
                span.from
            );
            store::settle_flip_recorded(db, id, "unconfirmed", None, Some(&why), rec).await
        }
        FlipOutcome::LeakedToControl { on_day } => {
            let why = format!(
                "`when_off` failed at `{unit}` on {on_day}, inside the off period from {}",
                span.from
            );
            store::settle_flip_recorded(db, id, "unconfirmed", Some(*on_day), Some(&why), rec).await
        }
        FlipOutcome::Failed { .. } => Ok(()),
    }
}
