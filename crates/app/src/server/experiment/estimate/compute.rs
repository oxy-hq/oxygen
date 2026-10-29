//! Turning engine results into the stored record.

use std::sync::Arc;

use oxy_experiment::compliance::{FlipOutcome, summarize};
use oxy_experiment::config::ExperimentConfig;
use oxy_experiment::engine_in::{EffectIn, PowerIn, RatioIn, SyntheticIn};
use oxy_experiment::patch::{
    CoefficientEdit, PatchRefusal, ViewSource, WriteBackInputs, plan_write_back,
};
use oxy_experiment::report::{ReportInput, ReportView, Seeds, SwitchbackFacts, build_report};
use serde_json::{Value, json};

use super::mirror::Judged;
use super::series::DailySeries;

/// A compiled view row, as the write-back needs it.
pub struct ViewRow {
    pub file_path: String,
    pub definition: Value,
}

pub struct RunInputs {
    /// The run's **pinned** design (`experiment_runs.config`), not the definition row.
    pub config: Arc<ExperimentConfig>,
    /// The run's pinned control pool (control group names under `assign_by`); `None` when no
    /// check ever pinned it. A switchback pins none.
    pub control_pool: Option<Vec<String>>,
    pub aborted: bool,
    /// Created from past data (`experiment_runs.retrospective`): labelled not
    /// pre-registered, and never proposes a coefficient.
    pub retrospective: bool,
    pub abort_reason: Option<String>,
    /// An aborted run's cut (`plan::measured_until`): the exclusive end of every panel read,
    /// stated in the report. `None` reads to the registered end.
    pub measured_until: Option<chrono::NaiveDate>,
    /// The owner aborted keeping the levers as they were (`experiment_runs.abort_levers`).
    pub levers_kept: bool,
    /// The close `off`'s moment and the exclusive end date of the carryover read
    /// (`carryover::carryover_window`); `None` when it reads none.
    pub carryover: Option<(oxy_experiment::moment::Moment, chrono::NaiveDate)>,
    /// The provenance token written into `refs`: the registered design version, `v<n>`.
    pub design_ref: String,
    /// PR 4's `compliance_outcomes(.., final_record: true)`.
    pub outcomes: Vec<(String, FlipOutcome)>,
    pub proposal_count: usize,
    /// Proposals were logged but the analysed split is none of them.
    pub hand_assigned: bool,
    /// Pricing's refusal, when the design carries no MDE and pricing said why.
    pub mde_unavailable: Option<String>,
    /// Distinct non-null `lever_revision_id`s across the run's flips.
    pub lever_revisions: usize,
    pub view: Option<ViewRow>,
}

pub struct Engines {
    pub effect: Judged<EffectIn>,
    /// A synthetic control's fits (`design.comparison`); `None` for every other
    /// comparison, and when its estimate refused.
    pub synthetic: Option<Judged<SyntheticIn>>,
    /// The effect over `design.carryover_days` after the close `off`s; `None` when the
    /// design reads none or the run was aborted.
    pub carryover: Option<EffectIn>,
    pub sibling: Option<Judged<EffectIn>>,
    pub sibling_power: Option<Judged<PowerIn>>,
    pub ratio: Option<Judged<RatioIn>>,
    pub n_untreated: usize,
    /// A switchback's fleet size and on-days; `None` for every other design.
    pub switchback: Option<SwitchbackFacts>,
    /// The target panel day by day; `None` when it was never read (unavailable, or no pin).
    pub series: Option<DailySeries>,
}

pub struct Computed {
    pub effect_raw: Value,
    pub ratio_raw: Option<Value>,
    pub compliance_json: Value,
    pub report: ReportView,
    pub edit: Option<CoefficientEdit>,
    /// `report.prediction` serialised, for the `experiment_results.prediction` column.
    /// `None` when nothing was compared (no prior, no first stage, an abort).
    pub prediction_json: Option<Value>,
    /// [`DailySeries`] with the abort cut stated, for the `experiment_results.series` column.
    pub series_json: Option<Value>,
}

/// Every section refused with one reason. Used when there is nothing to build an
/// assignment from; a section the design does not have (no sibling, no driver)
/// stays absent.
pub fn refused_engines(cfg: &ExperimentConfig, why: &str) -> Engines {
    let refused = || why.to_string();
    Engines {
        effect: Judged::refused(refused()),
        synthetic: None,
        carryover: None,
        sibling: cfg
            .substitution_check
            .as_ref()
            .map(|_| Judged::refused(refused())),
        sibling_power: cfg
            .substitution_check
            .as_ref()
            .map(|_| Judged::refused(refused())),
        ratio: cfg
            .outcome
            .driver
            .as_ref()
            .map(|_| Judged::refused(refused())),
        n_untreated: 0,
        switchback: None,
        series: None,
    }
}

fn plan(inp: &RunInputs, e: &Engines) -> Option<Result<CoefficientEdit, PatchRefusal>> {
    let driver = inp.config.outcome.driver.as_deref()?;
    let ratio = e.ratio.as_ref()?;
    Some(plan_write_back(WriteBackInputs {
        aborted: inp.aborted,
        retrospective: inp.retrospective,
        filtered: crate::server::experiment::reads::is_filtered(&inp.config),
        ratio: &ratio.view,
        target: &inp.config.outcome.target,
        driver,
        view: inp.view.as_ref().map(|v| ViewSource {
            file_path: &v.file_path,
            definition: &v.definition,
        }),
        experiment: &inp.config.experiment,
        design_ref: &inp.design_ref,
        // Frozen at pricing: the patch names the prior it would replace.
        prior: inp.config.outcome.prior_coefficient.as_ref(),
    }))
}

pub fn assemble(inp: &RunInputs, e: &Engines, seeds: Seeds) -> Computed {
    let compliance = summarize(&inp.config, &inp.outcomes);
    let write_back = plan(inp, e);
    let report = build_report(ReportInput {
        config: &inp.config,
        target: &e.effect.view,
        sibling: e.sibling.as_ref().map(|j| &j.view),
        sibling_power: e.sibling_power.as_ref().map(|j| &j.view),
        ratio: e.ratio.as_ref().map(|j| &j.view),
        write_back: write_back.as_ref(),
        compliance: &compliance,
        proposal_count: inp.proposal_count,
        aborted: inp.aborted,
        retrospective: inp.retrospective,
        abort_reason: inp.abort_reason.as_deref(),
        measured_until: inp.measured_until,
        levers_kept: inp.levers_kept,
        n_treated: e.effect.view.n_treated,
        n_untreated: e.n_untreated,
        distinct_lever_revisions: inp.lever_revisions,
        // `post_days` is optional (C2): absent on a switchback, whose totals come from `switchback`.
        post_days: inp.config.design.post_days.unwrap_or(0) as usize,
        switchback: e.switchback,
        carryover: e.carryover.as_ref(),
        seeds,
        mde_unavailable: inp.mde_unavailable.as_deref(),
        hand_assigned: inp.hand_assigned,
        synthetic: e.synthetic.as_ref().map(|j| &j.view),
    });
    // Plain numbers, strings and options: serialising cannot fail. A silent drop
    // would break "column == report.prediction", so it must not be `.ok()`.
    let prediction_json = report
        .prediction
        .as_ref()
        .map(|p| serde_json::to_value(p).expect("a PredictionView (plain data) always serialises"));
    // A ratio's day by day would be its numerator alone, which reads as the outcome.
    let series = e
        .series
        .as_ref()
        .filter(|_| inp.config.outcome.denominator.is_none());
    let series_json = series.map(|s| {
        let s = DailySeries {
            measured_until: inp.measured_until,
            ..s.clone()
        };
        serde_json::to_value(&s).expect("a DailySeries (plain data) always serialises")
    });
    Computed {
        series_json,
        effect_raw: e.effect.raw.clone(),
        ratio_raw: e.ratio.as_ref().map(|j| j.raw.clone()),
        compliance_json: json!({
            "c_treated": compliance.c_treated,
            "c_control_leak": compliance.c_control_leak,
            "restated_mde": compliance.restated_mde,
            "below_floor": compliance.below_floor,
            // C2: `n_treated == 0` means compliance was NOT measured; the surface reads these two.
            "n_treated": compliance.n_treated,
            "n_control": compliance.n_control,
        }),
        edit: write_back.and_then(Result::ok),
        prediction_json,
        report,
    }
}
