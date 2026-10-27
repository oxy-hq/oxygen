//! How far the result can be trusted: compliance against its floor, the
//! pre-trend check, the proposals logged before registration, and a lever that
//! changed mid-run.

use crate::engine_in::EffectIn;

use super::build::{finite, num, pct};
use super::types::*;

pub(super) fn compliance(i: &ReportInput<'_>) -> ComplianceView {
    let c = i.compliance;
    let floor = i.config.design.compliance_floor;
    let net = c.c_treated - c.c_control_leak;
    // Nothing was treated (an aborted switchback that flipped nothing): the shares are
    // vacuous, so say so — never "0%", never the attempt warning, never a restated MDE.
    if c.n_treated == 0 {
        return ComplianceView {
            c_treated: c.c_treated,
            c_control_leak: c.c_control_leak,
            net,
            floor,
            below_floor: false,
            restated_mde: None,
            warning: None,
            restated: None,
            n_treated: 0,
            n_control: c.n_control,
            not_measured: Some(
                "Compliance was not measured: the lever was never switched on for any unit."
                    .to_string(),
            ),
        };
    }
    let warning = c.below_floor.then(|| {
        format!(
            "Treated compliance {} is below the registered floor of {}: this number measures the attempt \
             to treat, not the lever's own effect.",
            pct(c.c_treated),
            pct(floor)
        )
    });
    let restated = match (c.restated_mde, i.config.outcome.mde_at_proposal) {
        (Some(r), Some(m)) => Some(format!(
            "At {} net compliance this design could only have read a lever effect of about {}, not {}.",
            pct(net),
            num(r),
            num(m)
        )),
        _ => None,
    };
    ComplianceView {
        c_treated: c.c_treated,
        c_control_leak: c.c_control_leak,
        net,
        floor,
        below_floor: c.below_floor,
        restated_mde: c.restated_mde,
        warning,
        restated,
        n_treated: c.n_treated,
        n_control: c.n_control,
        not_measured: None,
    }
}

pub(super) fn checks(t: &EffectIn) -> ChecksView {
    let (pre_trend, note) = match &t.pre_trend {
        Some(p) => {
            let view = PreTrendView {
                estimate: finite(p.estimate),
                t_stat: finite(p.t_stat),
                diverged: p.diverged,
            };
            let note = if p.diverged {
                "the pre-period trends diverged: parallel trends is doubtful. This is a check, not proof."
            } else {
                "no divergence was detected in the pre-period. This is a check, not proof of parallel trends."
            };
            (Some(view), note.to_string())
        }
        None => (None, "the pre-trend check was not computed".to_string()),
    };
    ChecksView {
        pre_trend,
        pre_trend_note: note,
        dropped_waves: t.dropped_waves.clone(),
        dropped_units: t
            .dropped_units
            .iter()
            .map(|d| format!("{}: {}", d.unit, d.reason))
            .collect(),
    }
}

/// Treated and control were not drawn: no count of draws can vouch for the split.
const OBSERVATIONAL: &str = "Observational: the assignment was declared, not randomised, so \
                             treated and control units may differ in ways other than the change";

pub(super) fn proposals(count: usize, observed: bool, hand_assigned: bool) -> ProposalsView {
    if observed {
        return ProposalsView {
            count,
            message: OBSERVATIONAL.to_string(),
            observational: true,
            hand_assigned: false,
        };
    }
    let message = match count {
        n if n > 0 && hand_assigned => format!(
            "the analysed split was written by hand, not drawn by the platform: none of the {n} \
             logged proposal{} is this split",
            if n == 1 { "" } else { "s" }
        ),
        0 => {
            "no proposal was logged before registration: this split was not priced by the platform"
                .to_string()
        }
        1 => "1 design proposal was logged before registration".to_string(),
        n => format!(
            "{n} design proposals were logged before registration; {} of them were not the registered split",
            n - 1
        ),
    };
    ProposalsView {
        count,
        message,
        observational: false,
        hand_assigned: hand_assigned && count > 0,
    }
}

/// The automation a `via` names is resolved per flip and its revision recorded;
/// more than one revision means the lever itself changed under the experiment.
pub(super) fn lever_note(distinct_revisions: usize) -> Option<String> {
    (distinct_revisions > 1).then(|| {
        format!(
            "the lever's definition changed during the experiment: its flips ran {distinct_revisions} distinct \
             revisions of the automation, so \"on\" did not mean one thing throughout"
        )
    })
}
