use super::*;
use crate::compliance::ComplianceSummary;
use crate::config::{Direction, ExperimentConfig};
use crate::engine_in::{EffectIn, PowerIn, PreTrendIn, RatioIn, SizeBiasIn};
use crate::patch::{CoefficientEdit, PatchRefusal};
use crate::test_fixtures::{compliance, config, effect, power, ratio, refused, unbounded_ratio};

/// Owns every input so a test can change one and rebuild. Shared with `design_tests` and `prior_tests`.
pub(super) struct Case {
    pub(super) cfg: ExperimentConfig,
    pub(super) target: EffectIn,
    pub(super) sibling: Option<EffectIn>,
    pub(super) sibling_power: Option<PowerIn>,
    pub(super) ratio: Option<RatioIn>,
    pub(super) write_back: Option<Result<CoefficientEdit, PatchRefusal>>,
    pub(super) compliance: ComplianceSummary,
    pub(super) proposals: usize,
    pub(super) n_untreated: usize,
    pub(super) lever_revisions: usize,
    pub(super) aborted: bool,
    pub(super) retrospective: bool,
    pub(super) abort_reason: Option<String>,
    pub(super) measured_until: Option<chrono::NaiveDate>,
    pub(super) levers_kept: bool,
    pub(super) switchback: Option<SwitchbackFacts>,
    pub(super) carryover: Option<EffectIn>,
    pub(super) mde_unavailable: Option<String>,
    pub(super) hand_assigned: bool,
}

impl Case {
    pub(super) fn new() -> Self {
        Case {
            cfg: config(),
            target: effect(17.4, 3.9),
            sibling: Some(effect(-2.0, 30.0)),
            sibling_power: Some(power(86.0)),
            ratio: None,
            write_back: None,
            compliance: compliance(0.95, 0.0, Some(18.4), false),
            proposals: 1,
            n_untreated: 6,
            lever_revisions: 1,
            aborted: false,
            retrospective: false,
            abort_reason: None,
            measured_until: None,
            levers_kept: false,
            switchback: None,
            carryover: None,
            mde_unavailable: None,
            hand_assigned: false,
        }
    }

    pub(super) fn build(&self) -> ReportView {
        build_report(ReportInput {
            config: &self.cfg,
            target: &self.target,
            sibling: self.sibling.as_ref(),
            sibling_power: self.sibling_power.as_ref(),
            ratio: self.ratio.as_ref(),
            write_back: self.write_back.as_ref(),
            compliance: &self.compliance,
            proposal_count: self.proposals,
            aborted: self.aborted,
            retrospective: self.retrospective,
            abort_reason: self.abort_reason.as_deref(),
            measured_until: self.measured_until,
            levers_kept: self.levers_kept,
            n_treated: 6,
            n_untreated: self.n_untreated,
            distinct_lever_revisions: self.lever_revisions,
            switchback: self.switchback,
            carryover: self.carryover.as_ref(),
            post_days: 56,
            seeds: Seeds {
                effect: 1,
                placebo: 2,
                ratio: 3,
            },
            mde_unavailable: self.mde_unavailable.as_deref(),
            hand_assigned: self.hand_assigned,
        })
    }
}

fn size_bias(treated: f64, control: f64, significant: bool) -> SizeBiasIn {
    SizeBiasIn {
        correlation_treated: Some(treated),
        correlation_control: Some(control),
        significant,
    }
}

#[test]
fn headline_is_gross_estimate_with_ci_half_width() {
    let v = Case::new().build();
    let h = v.headline.expect("headline");
    assert_eq!(h.estimate, 17.4);
    assert!((h.half_width.expect("bounded") - 3.9).abs() < 1e-9);
    assert_eq!(
        h.text,
        "gross +17.40 per unit-day on store_days.beverages_revenue (±3.90); \
         substitution not excluded above 86.00 per unit-day on store_days.entree_revenue"
    );
}

#[test]
fn headline_is_never_net_even_when_the_sibling_moved() {
    let base = Case::new().build();
    let mut moved = Case::new();
    moved.sibling = Some(effect(-40.0, 5.0)); // a significant, large cannibalisation
    let v = moved.build();
    let h = v.headline.unwrap();
    // The sibling's estimate never touches the gross number; only its statement changes.
    assert_eq!(h.estimate, base.headline.unwrap().estimate);
    assert_eq!(h.estimate, 17.4);
    assert!(!h.text.contains(" net "), "{}", h.text);
    // shown, labelled as the sibling's own, never merged
    assert_eq!(v.substitution.sibling_effect.unwrap().estimate, Some(-40.0));
}

#[test]
fn substitution_floor_is_the_sibling_mde_and_nothing_is_subtracted() {
    let v = Case::new().build();
    assert_eq!(v.substitution.floor, Some(86.0));
    assert!(
        v.substitution
            .statement
            .starts_with("substitution not excluded above 86.00"),
        "{}",
        v.substitution.statement
    );
    assert_eq!(v.headline.unwrap().estimate, 17.4);
}

#[test]
fn a_significant_sibling_drop_reads_as_substitution_detected() {
    // Regression: a clear cannibalisation (the ads example's -$74 on the other
    // backpacks) read "substitution not excluded above <floor>", the wording for
    // a result that could not see one.
    let mut c = Case::new();
    c.sibling = Some(effect(-40.0, 5.0));
    let v = c.build();
    assert!(v.substitution.detected);
    assert_eq!(
        v.substitution.statement,
        "substitution detected: store_days.entree_revenue fell 40.00 per unit-day \
         (95% CI -45.00 to -35.00)"
    );
    let h = v.headline.unwrap();
    assert_eq!(h.estimate, 17.4, "the headline stays gross");
    assert!(h.text.ends_with(&v.substitution.statement), "{}", h.text);
}

#[test]
fn a_sibling_that_moved_with_the_target_is_not_substitution() {
    // Both rose: a halo, not cannibalisation, so the floor wording stands.
    let mut c = Case::new();
    c.sibling = Some(effect(30.0, 5.0));
    let v = c.build();
    assert!(!v.substitution.detected);
    assert!(
        v.substitution
            .statement
            .starts_with("substitution not excluded above 86.00")
    );
}

#[test]
fn an_insignificant_sibling_drop_keeps_the_floor_wording() {
    let mut c = Case::new();
    c.sibling = Some(effect(-20.0, 30.0));
    let v = c.build();
    assert!(!v.substitution.detected);
    assert!(
        v.substitution
            .statement
            .starts_with("substitution not excluded above 86.00")
    );
}

#[test]
fn substitution_check_unavailable_states_the_siblings_refusal() {
    let mut c = Case::new();
    c.sibling_power = Some(PowerIn {
        mde: None,
        refusal: Some("fewer than 2 non-overlapping windows in the bounded history".into()),
    });
    let v = c.build();
    assert_eq!(v.substitution.floor, None);
    assert_eq!(
        v.substitution.statement,
        "substitution check unavailable: the sibling's power computation was refused: \
         fewer than 2 non-overlapping windows in the bounded history"
    );
    assert!(
        v.headline
            .unwrap()
            .text
            .contains("substitution check unavailable")
    );
}

#[test]
fn no_registered_sibling_is_stated_not_omitted() {
    let mut c = Case::new();
    c.cfg.substitution_check = None;
    c.sibling = None;
    c.sibling_power = None;
    assert_eq!(
        c.build().substitution.statement,
        "no substitution check was registered"
    );
}

#[test]
fn treated_total_is_exact_arithmetic() {
    let mut c = Case::new();
    c.target = effect(12.5, 2.0);
    let t = c.build().treated_total.expect("always shown");
    assert_eq!((t.units, t.days), (6, 56));
    assert_eq!(t.total, 12.5 * 6.0 * 56.0); // 4200, exactly
    assert_eq!(
        t.interval,
        Interval {
            low: Some(10.5 * 336.0),
            high: Some(14.5 * 336.0)
        }
    );
    assert!(t.basis.contains("exact arithmetic"), "{}", t.basis);
}

#[test]
fn projection_is_always_labelled_an_assumption() {
    // With no size diagnostic, an insignificant one, and a significant one.
    for sb in [
        None,
        Some(size_bias(0.71, 0.05, false)),
        Some(size_bias(0.71, 0.05, true)),
    ] {
        let mut c = Case::new();
        c.target = EffectIn {
            size_bias: sb,
            ..effect(12.5, 2.0)
        };
        let p = c.build().projection.expect("there are untreated units");
        assert_eq!(p.label, PROJECTION_LABEL);
        assert!(p.assumption.contains("assumption"), "{}", p.assumption);
        assert_eq!(p.total, 12.5 * 6.0 * 56.0);
    }
}

#[test]
fn no_untreated_units_means_no_projection() {
    let mut c = Case::new();
    c.n_untreated = 0;
    assert!(c.build().projection.is_none());
}

#[test]
fn significant_size_bias_warns_and_names_the_direction() {
    let mut c = Case::new();
    c.target = EffectIn {
        size_bias: Some(size_bias(0.71, 0.05, true)),
        ..effect(12.5, 2.0)
    };
    let w = c
        .build()
        .projection
        .unwrap()
        .warning
        .expect("significant → warning");
    assert!(
        w.contains("smaller than the treated ones likely overstates"),
        "{w}"
    );

    // Reversed sign of (treated − control) reverses the direction named.
    c.target = EffectIn {
        size_bias: Some(size_bias(-0.6, 0.0, true)),
        ..effect(12.5, 2.0)
    };
    let w = c.build().projection.unwrap().warning.unwrap();
    assert!(
        w.contains("smaller than the treated ones likely understates"),
        "{w}"
    );
}

#[test]
fn a_missing_size_correlation_warns_without_numbers_or_direction() {
    let mut c = Case::new();
    let sb = SizeBiasIn {
        correlation_treated: None,
        correlation_control: Some(0.05),
        significant: true,
    };
    c.target = EffectIn {
        size_bias: Some(sb),
        ..effect(12.5, 2.0)
    };
    let w = c
        .build()
        .projection
        .unwrap()
        .warning
        .expect("significant -> warning");
    assert!(w.contains("direction is unknown"), "{w}");
    assert!(
        !w.contains("r = ") && !w.contains("overstates the effect;"),
        "{w}"
    );
}

#[test]
fn a_refused_ratio_is_not_reported_as_unbounded() {
    let mut c = Case::new();
    // As the engine refuses: no coefficient, so no set to call unbounded.
    c.ratio = Some(RatioIn {
        coefficient: None,
        ci_low: None,
        ci_high: None,
        refusal: Some("no variance".into()),
        ..ratio(5.0)
    });
    let r = c.build().ratio.unwrap();
    assert!(!r.unbounded);
    assert_eq!(r.refusal.as_deref(), Some("no variance"));
}

#[test]
fn insignificant_size_bias_carries_no_warning() {
    let mut c = Case::new();
    c.target = EffectIn {
        size_bias: Some(size_bias(0.1, 0.05, false)),
        ..effect(12.5, 2.0)
    };
    assert!(c.build().projection.unwrap().warning.is_none());
}

#[test]
fn a_wrong_sign_says_you_predicted_up_it_went_down() {
    let mut c = Case::new(); // registered direction: increase
    c.target = effect(-9.0, 2.0);
    let d = c.build().direction;
    assert!(d.mismatch);
    assert_eq!(d.message.as_deref(), Some("you predicted up, it went down"));

    c.cfg.outcome.direction = Direction::Decrease;
    c.target = effect(9.0, 2.0);
    assert_eq!(
        c.build().direction.message.as_deref(),
        Some("you predicted down, it went up")
    );

    c.target = effect(-9.0, 2.0); // predicted down, went down
    let d = c.build().direction;
    assert!(!d.mismatch && d.message.is_none());
}

#[test]
fn compliance_below_floor_warns_the_number_measures_the_attempt_and_restates_the_mde() {
    let mut c = Case::new();
    c.compliance = compliance(0.67, 0.0, Some(26.1), true);
    let v = c.build().compliance;
    let w = v.warning.expect("below floor → warning");
    assert!(w.contains("measures the attempt"), "{w}");
    assert!(w.contains("67%") && w.contains("80%"), "{w}");
    let r = v.restated.expect("restated");
    assert!(r.contains("26.10") && r.contains("17.50"), "{r}");
    assert_eq!(v.restated_mde, Some(26.1));
}

#[test]
fn compliance_above_floor_has_no_warning() {
    let v = Case::new().build().compliance;
    assert!(v.warning.is_none());
    assert!(!v.below_floor);
    assert!(v.not_measured.is_none());
}

#[test]
fn no_treated_entries_means_compliance_was_not_measured() {
    let mut c = Case::new();
    // What `summarize` returns for an empty list: shares of 0, below the floor, with no entries.
    c.compliance = ComplianceSummary {
        n_treated: 0,
        n_control: 0,
        ..compliance(0.0, 0.0, Some(99.0), true)
    };
    let v = c.build().compliance;
    let m = v.not_measured.expect("n_treated == 0 → not measured");
    assert!(m.contains("not measured"), "{m}");
    assert!(
        v.warning.is_none(),
        "no 'measures the attempt' warning over nothing"
    );
    assert!(
        v.restated.is_none() && v.restated_mde.is_none(),
        "no restated MDE over nothing"
    );
    assert!(!v.below_floor);
    assert_eq!((v.n_treated, v.n_control), (0, 0));
}

#[test]
fn dropped_waves_are_listed_with_their_reasons() {
    let mut c = Case::new();
    let line = "2026-03-16 (1 unit): fewer than 2 units in the wave".to_string();
    c.target = EffectIn {
        dropped_waves: vec![line.clone()],
        ..effect(12.5, 2.0)
    };
    assert_eq!(c.build().checks.dropped_waves, vec![line]);
}

#[test]
fn pre_trend_is_shown_as_a_check_not_proof() {
    let mut c = Case::new();
    c.target = EffectIn {
        pre_trend: Some(PreTrendIn {
            estimate: Some(4.2),
            t_stat: Some(3.1),
            diverged: true,
        }),
        ..effect(12.5, 2.0)
    };
    let ch = c.build().checks;
    assert!(ch.pre_trend.as_ref().unwrap().diverged);
    assert!(
        ch.pre_trend_note.contains("diverged") && ch.pre_trend_note.contains("not proof"),
        "{}",
        ch.pre_trend_note
    );

    c.target = effect(12.5, 2.0);
    assert!(c.build().checks.pre_trend_note.contains("not computed"));
}

#[test]
fn the_proposal_count_is_shown() {
    let mut c = Case::new();
    c.proposals = 5;
    let p = c.build().proposals;
    assert_eq!(p.count, 5);
    assert!(p.message.contains("5 design proposals"), "{}", p.message);
    c.proposals = 1;
    assert!(
        c.build()
            .proposals
            .message
            .contains("1 design proposal was")
    );
}

#[test]
fn a_lever_whose_definition_changed_mid_run_is_said_so() {
    let mut c = Case::new();
    c.lever_revisions = 2;
    let note = c.build().lever_note.expect("two revisions → a note");
    assert!(
        note.contains("the lever's definition changed during the experiment"),
        "{note}"
    );
    assert!(note.contains("2 distinct"), "{note}");
    for n in [0, 1] {
        c.lever_revisions = n; // none recorded, or one throughout: nothing to say
        assert!(
            c.build().lever_note.is_none(),
            "{n} revisions must not raise a note"
        );
    }
}

#[test]
fn an_aborted_report_is_labelled_and_never_the_registered_result() {
    let mut c = Case::new();
    c.aborted = true;
    c.abort_reason = Some("guardrail: store_days.refund_rate".into());
    let v = c.build();
    assert_eq!(
        v.status,
        ReportStatus::Aborted {
            reason: Some("guardrail: store_days.refund_rate".into()),
            levers_kept: false,
        }
    );
    assert!(v.label.starts_with("ABORTED RUN"), "{}", v.label);
    assert!(v.label.contains("not the registered result"), "{}", v.label);
    assert!(Case::new().build().label.starts_with("Registered result"));
}

#[test]
fn a_retrospective_report_is_labelled_as_not_pre_registered() {
    let mut c = Case::new();
    c.retrospective = true;
    let v = c.build();
    assert_eq!(v.status, ReportStatus::Retrospective);
    assert!(v.label.starts_with("RETROSPECTIVE"), "{}", v.label);
    assert!(v.label.contains("not pre-registered"), "{}", v.label);
    assert!(!v.label.contains("Registered result"), "{}", v.label);
    // The numbers are still computed: the label is what changes what may be claimed.
    assert!(v.headline.is_some());
}

#[test]
fn an_aborted_report_states_where_its_measurement_was_cut() {
    let mut c = Case::new();
    c.aborted = true;
    c.abort_reason = Some("owner call".into());
    c.measured_until = chrono::NaiveDate::from_ymd_opt(2026, 4, 2);
    let label = c.build().label;
    assert!(label.starts_with("ABORTED RUN"), "{label}");
    assert!(
        label.contains("the measurement stops before 2026-04-02"),
        "{label}"
    );
    // Read to its registered end (nothing to cut): no cut is claimed.
    c.measured_until = None;
    assert!(!c.build().label.contains("measurement stops"));
}

#[test]
fn an_abort_that_kept_its_levers_says_so_and_cuts_on_the_abort_day() {
    let mut c = Case::new();
    c.aborted = true;
    c.abort_reason = Some("owner call".into());
    c.levers_kept = true;
    c.measured_until = chrono::NaiveDate::from_ymd_opt(2026, 4, 2);
    let v = c.build();
    assert_eq!(
        v.status,
        ReportStatus::Aborted {
            reason: Some("owner call".into()),
            levers_kept: true,
        }
    );
    assert!(
        v.label
            .contains("the measurement stops before 2026-04-02, the day the run was aborted"),
        "{}",
        v.label
    );
    assert!(
        v.label.contains("levers were left as they were"),
        "{}",
        v.label
    );
    assert!(!v.label.contains("ended the run's switches"), "{}", v.label);
}

#[test]
fn an_aborted_status_read_before_levers_were_recorded_reads_as_turned_off() {
    let old: ReportStatus =
        serde_json::from_str(r#"{"kind":"aborted","reason":"owner call"}"#).unwrap();
    assert_eq!(
        old,
        ReportStatus::Aborted {
            reason: Some("owner call".into()),
            levers_kept: false,
        }
    );
}

#[test]
fn a_refused_target_renders_the_refusal_and_no_numbers() {
    let mut c = Case::new();
    c.target = refused("no unit stayed untreated");
    let v = c.build();
    assert_eq!(v.refusal.as_deref(), Some("no unit stayed untreated"));
    assert!(
        v.headline.is_none()
            && v.treated_total.is_none()
            && v.projection.is_none()
            && v.promise.is_none()
    );
    assert_eq!(v.direction.observed, "none");
    assert!(!v.direction.mismatch);
    assert_eq!(
        v.proposals.count, 1,
        "the operational sections still render"
    );
}

#[test]
fn an_estimate_that_is_not_a_number_is_a_refusal_not_a_printed_nan() {
    let mut c = Case::new();
    c.target = EffectIn {
        estimate: None,
        ..effect(1.0, 1.0)
    }; // engine wrote NaN → null → None
    let v = c.build();
    assert!(v.refusal.is_some() && v.headline.is_none());
}

#[test]
fn an_unbounded_interval_is_reported_not_invented() {
    let mut c = Case::new();
    c.target = EffectIn {
        ci_low: None,
        ci_high: None,
        ..effect(10.0, 2.0)
    };
    let h = c.build().headline.unwrap();
    assert_eq!(h.half_width, None);
    assert_eq!(
        h.interval,
        Interval {
            low: None,
            high: None
        }
    );
    assert!(h.text.contains("interval unbounded"), "{}", h.text);
}

#[test]
fn the_promise_is_compared_with_the_result() {
    let mut c = Case::new(); // mde_at_proposal = 17.5
    c.target = effect(9.0, 2.0);
    assert!(c.build().promise.unwrap().below_promise);
    c.target = effect(20.0, 2.0);
    assert!(!c.build().promise.unwrap().below_promise);
}

#[test]
fn ratio_and_write_back_are_carried_with_their_reasons() {
    let mut c = Case::new();
    c.ratio = Some(ratio(5.0));
    c.write_back = Some(Err(PatchRefusal::NonLinearForm {
        form: "log-log".into(),
    }));
    let v = c.build();
    assert_eq!(v.ratio.as_ref().unwrap().coefficient, Some(5.0));
    assert_eq!(
        v.write_back,
        Some(WriteBackView::Refused {
            reason: PatchRefusal::NonLinearForm {
                form: "log-log".into()
            }
            .to_string()
        })
    );
}

#[test]
fn an_unbounded_ratio_says_so() {
    let mut c = Case::new();
    c.ratio = Some(unbounded_ratio(5.0, None, None));
    let r = c.build().ratio.unwrap();
    assert!(r.unbounded);
    assert_eq!(
        r.interval,
        Interval {
            low: None,
            high: None
        }
    );
}

#[test]
fn the_report_round_trips_through_json() {
    let mut c = Case::new();
    c.ratio = Some(ratio(5.0));
    c.target = EffectIn {
        ci_high: None,
        ..effect(10.0, 2.0)
    };
    let v = c.build();
    let back: ReportView = serde_json::from_str(&serde_json::to_string(&v).unwrap()).unwrap();
    assert_eq!(back, v);
}

fn with_carryover(effect: EffectIn) -> Case {
    let mut c = Case::new();
    c.cfg.design.carryover_days = Some(14);
    c.carryover = Some(effect);
    c
}

#[test]
fn no_carryover_window_means_no_carryover_section() {
    assert!(Case::new().build().carryover.is_none());
}

#[test]
fn a_significant_effect_after_the_lever_turned_off_reads_as_lingering() {
    let v = with_carryover(effect(9.5, 4.0)).build();
    let c = v.carryover.expect("a carryover section");
    assert_eq!(c.days, 14);
    assert!(c.lingered);
    assert_eq!(
        c.statement,
        "the effect lingered after the lever turned off: +9.50 per unit-day over the next 14 days \
         (95% CI +5.50 to +13.50)"
    );
}

#[test]
fn an_undistinguishable_carryover_says_none_was_measurable() {
    let c = with_carryover(effect(2.0, 6.0)).build().carryover.unwrap();
    assert!(!c.lingered);
    assert!(
        c.statement
            .starts_with("no effect was measurable over the 14 days after the lever turned off"),
        "{}",
        c.statement
    );
}

#[test]
fn a_carryover_against_the_effect_reads_as_pull_forward() {
    // Bought during the campaign instead of after it.
    let c = with_carryover(effect(-7.0, 3.0)).build().carryover.unwrap();
    assert!(!c.lingered);
    assert!(c.statement.contains("pull-forward"), "{}", c.statement);
}

#[test]
fn a_refused_carryover_states_why() {
    let c = with_carryover(refused(
        "the carryover window's coverage 0.40 is below 0.90",
    ))
    .build()
    .carryover
    .unwrap();
    assert_eq!(
        c.statement,
        "carryover not measured: the carryover window's coverage 0.40 is below 0.90"
    );
}

#[test]
fn a_filtered_target_and_sibling_are_named_with_their_filters() {
    use crate::config::{FilterOp, MeasureFilter};
    let f = |dim: &str, op: FilterOp, v: &str| MeasureFilter {
        dimension: dim.into(),
        op,
        values: vec![v.into()],
    };
    let mut c = Case::new();
    c.cfg.outcome.target_filter = vec![f("store_days.sku", FilterOp::Equals, "TB-40")];
    c.cfg.substitution_check.as_mut().unwrap().filter = vec![
        f("store_days.category", FilterOp::Equals, "backpacks"),
        f("store_days.sku", FilterOp::NotEquals, "TB-40"),
    ];
    let v = c.build();
    assert_eq!(v.target, "store_days.beverages_revenue (sku = TB-40)");
    let h = v.headline.unwrap();
    assert!(
        h.text
            .starts_with("gross +17.40 per unit-day on store_days.beverages_revenue (sku = TB-40)"),
        "{}",
        h.text
    );
    assert!(
        v.substitution
            .statement
            .ends_with("on store_days.entree_revenue (category = backpacks, sku ≠ TB-40)"),
        "{}",
        v.substitution.statement
    );
}
