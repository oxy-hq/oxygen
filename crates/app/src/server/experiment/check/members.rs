//! Under `assign_by`: read-back per member (spec "Groups of units"). Each
//! pinned member of a dispatched group is read over the group's grace window
//! until decided, and kept, sticky, in `observations.member_outcomes`. Once
//! every member is decided the group's row is settled for the status route —
//! confirmed on the earliest landing if any member landed. The members, not
//! the row, are the compliance record (C4).

use entity::experiment_flips;
use oxy_experiment::Moment;
use oxy_experiment::compliance::{FlipOutcome, Sampling, classify_treated};
use oxy_experiment::expr::{Comparison, parse_comparison};
use sea_orm::{DatabaseConnection, DbErr};

use super::read_back::{ReadBackInputs, grace_end, held};
use super::reader::UnitDays;
use crate::server::experiment::panel::GroupMembership;
use crate::server::experiment::store::{self, Observations, Recorded};

pub async fn treated_groups(
    db: &DatabaseConnection,
    inp: &ReadBackInputs<'_>,
    groups: &GroupMembership,
    days: &dyn UnitDays,
    obs: &mut Observations,
) -> Result<(), DbErr> {
    let Some(rb) = inp.c.read_back() else {
        obs.problems
            .push("the design has no lever, so there is nothing to read back".into());
        return Ok(());
    };
    let cmp = match parse_comparison(&rb.when_on) {
        Ok(cmp) => cmp,
        Err(e) => {
            obs.problems.push(format!("lever.read_back.when_on: {e}"));
            return Ok(());
        }
    };
    for flip in inp
        .flips
        .iter()
        .filter(|f| f.target_state == "on" && f.status == "dispatched")
    {
        let Some(members) = groups.0.get(&flip.unit) else {
            continue;
        };
        read_members(inp, &cmp, Moment(flip.due_at), members, days, obs).await;
        settle_group(db, flip, members, obs).await?;
    }
    Ok(())
}

async fn read_members(
    inp: &ReadBackInputs<'_>,
    cmp: &Comparison,
    switch_on: Moment,
    members: &[String],
    days: &dyn UnitDays,
    obs: &mut Observations,
) {
    let grace = inp.c.grace_days();
    let sampling = Sampling::of(inp.c);
    let to = grace_end(switch_on, grace).min(inp.now);
    if to < switch_on {
        return;
    }
    for member in members {
        if obs.member_outcomes.contains_key(member) {
            continue;
        }
        let one = [member.clone()];
        match held(days, cmp, &one, (switch_on, to), sampling).await {
            Ok(held) => {
                if let Some(outcome) = classify_treated(switch_on, grace, sampling, inp.now, &held)
                {
                    obs.member_outcomes.insert(member.clone(), outcome);
                }
            }
            Err(e) => obs.problems.push(e),
        }
    }
}

async fn settle_group(
    db: &DatabaseConnection,
    flip: &experiment_flips::Model,
    members: &[String],
    obs: &Observations,
) -> Result<(), DbErr> {
    let decided: Vec<&FlipOutcome> = members
        .iter()
        .filter_map(|m| obs.member_outcomes.get(m))
        .collect();
    if members.is_empty() || decided.len() < members.len() {
        return Ok(()); // a member not yet read back: the group is undecided
    }
    let landings: Vec<Moment> = decided
        .iter()
        .filter_map(|o| match o {
            FlipOutcome::Landed { on_day } => Some(*on_day),
            _ => None,
        })
        .collect();
    let why = format!(
        "{} of {} members of `{}` landed within grace_days",
        landings.len(),
        members.len(),
        flip.unit
    );
    // The members' outcomes are kept on the run in the same statement: once settled, the
    // group is never read again, so a later failure in the check must not drop them.
    let rec = Recorded {
        run_id: flip.run_id,
        field: "member_outcomes",
        entries: members
            .iter()
            .filter_map(|m| Some((m.as_str(), obs.member_outcomes.get(m)?)))
            .collect(),
    };
    let (status, first) = match landings.iter().min() {
        Some(first) => ("confirmed", Some(*first)),
        None => ("unconfirmed", None),
    };
    store::settle_flip_recorded(db, flip.id, status, first, Some(&why), &rec).await
}
