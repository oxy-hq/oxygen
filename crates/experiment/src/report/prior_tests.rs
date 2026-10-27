use super::tests::Case;
use super::*;
use crate::config::{PriorCoefficient, PriorSource};
use crate::engine_in::{EffectIn, RatioIn};
use crate::test_fixtures::{effect, ratio, refused, unbounded_ratio};

/// A frozen prior and a strong first stage (5.0 ± 1.0, so the earned set is [4, 6]).
fn with_prior(value: f64, source: PriorSource) -> Case {
    let mut c = Case::new();
    c.cfg.outcome.prior_coefficient = Some(PriorCoefficient { value, source });
    c.ratio = Some(ratio(5.0));
    c
}

#[test]
fn a_registered_alpha_above_a_twentieth_calls_a_clearing_result_directional_never_significant() {
    let mut c = Case::new();
    c.cfg.outcome.alpha = 0.2;
    let v = c.build(); // effect(17.4, 3.9) clears its bar
    assert!(v.directional && v.alpha == 0.2);
    let h = v.headline.unwrap();
    assert_eq!(h.verdict, "directional at α = 0.2");
    assert!(
        h.text.starts_with("directional at α = 0.2: gross +17.40"),
        "{}",
        h.text
    );
    assert!(
        !h.text.contains("significant") && !h.verdict.contains("significant"),
        "{} / {}",
        h.text,
        h.verdict
    );
}

#[test]
fn an_alpha_of_exactly_a_twentieth_is_a_plain_significant_result() {
    let v = Case::new().build();
    assert!(!v.directional && v.alpha == 0.05);
    let h = v.headline.unwrap();
    assert_eq!(h.verdict, "significant at α = 0.05");
    assert!(h.text.starts_with("gross +17.40"), "{}", h.text);
}

#[test]
fn a_result_that_does_not_clear_even_a_loose_bar_is_not_called_directional() {
    let mut c = Case::new();
    c.cfg.outcome.alpha = 0.2;
    c.target = effect(0.3, 2.0); // interval spans zero
    let h = c.build().headline.unwrap();
    assert_eq!(h.verdict, "not distinguishable from zero at α = 0.2");
    assert!(!h.text.starts_with("directional"), "{}", h.text);
}

#[test]
fn the_prediction_is_the_prior_times_the_measured_first_stage_against_the_measured_target() {
    let p = with_prior(4.2, PriorSource::Fitted)
        .build()
        .prediction
        .expect("compared");
    assert_eq!(
        p.predicted,
        4.2 * 5.0,
        "prior × ITT(driver), the first stage the experiment measured"
    );
    assert_eq!(p.measured, 17.4);
    assert_eq!(
        p.measured_interval,
        Interval {
            low: Some(17.4 - 3.9),
            high: Some(17.4 + 3.9)
        }
    );
    assert_eq!(
        p.prior,
        PriorView {
            value: 4.2,
            source: PriorSource::Fitted
        }
    );
    // a stronger measured first stage moves the prediction: it is not a planned push
    let mut c = with_prior(4.2, PriorSource::Fitted);
    c.ratio = Some(RatioIn {
        first_stage: effect(8.0, 1.0),
        ..ratio(5.0)
    });
    assert_eq!(c.build().prediction.unwrap().predicted, 4.2 * 8.0);
}

#[test]
fn a_prior_inside_the_earned_set_agrees_and_one_outside_contradicts() {
    let inside = with_prior(4.2, PriorSource::Fitted)
        .build()
        .prediction
        .unwrap();
    assert_eq!(inside.prior_in_interval, Some(true));
    assert!(
        inside
            .statement
            .starts_with("the experiment agrees with the World Model's slope"),
        "{}",
        inside.statement
    );

    let outside = with_prior(9.0, PriorSource::Fitted)
        .build()
        .prediction
        .unwrap();
    assert_eq!(outside.prior_in_interval, Some(false));
    assert!(
        outside
            .statement
            .starts_with("the experiment contradicts the World Model's slope"),
        "{}",
        outside.statement
    );
    assert!(
        outside
            .statement
            .contains("observational history meeting a deliberate change"),
        "{}",
        outside.statement
    );
}

#[test]
fn the_statement_names_the_priors_source() {
    let fitted = with_prior(4.2, PriorSource::Fitted)
        .build()
        .prediction
        .unwrap();
    assert!(
        fitted.statement.contains("fitted prior 4.20"),
        "{}",
        fitted.statement
    );
    let declared = with_prior(9.0, PriorSource::Declared)
        .build()
        .prediction
        .unwrap();
    assert!(
        declared.statement.contains("declared prior 9.00"),
        "{}",
        declared.statement
    );
    assert!(
        !declared.statement.contains("observational history"),
        "that remark is about a fitted slope"
    );
}

#[test]
fn an_unbounded_earned_set_leaves_prior_in_interval_null_on_either_side() {
    for (lo, hi) in [(None, Some(6.0)), (Some(4.0), None), (None, None)] {
        let mut c = with_prior(4.2, PriorSource::Fitted);
        c.ratio = Some(unbounded_ratio(5.0, lo, hi));
        let v = c.build();
        assert!(v.ratio.as_ref().unwrap().unbounded, "{lo:?}..{hi:?}");
        let p = v
            .prediction
            .unwrap_or_else(|| panic!("the numbers still exist: {:?}", v.prediction_note));
        assert_eq!(p.prior_in_interval, None, "{lo:?}..{hi:?}");
        assert!(
            p.statement.contains("neither confirmed nor contradicted"),
            "{}",
            p.statement
        );
    }
}

#[test]
fn a_weak_first_stage_with_an_unbounded_set_is_unstated_not_refused() {
    // What the engine emits when the lever barely moved the driver: both reasons, joined.
    let mut c = with_prior(4.2, PriorSource::Fitted);
    let mut r = unbounded_ratio(5.0, None, None);
    r.refusal = Some(format!(
        "the lever did not measurably move the driver (first-stage p = 0.400 at alpha 0.05); {}",
        r.refusal.unwrap()
    ));
    c.ratio = Some(r);
    let v = c.build();
    let p = v.prediction.expect("compared, with no verdict");
    assert_eq!(p.prior_in_interval, None);
    assert!(v.prediction_note.is_none(), "{:?}", v.prediction_note);
    assert!(v.ratio.unwrap().unbounded);
}

#[test]
fn no_frozen_prior_means_no_prediction_and_no_note() {
    let mut c = Case::new();
    c.ratio = Some(ratio(5.0));
    let v = c.build();
    assert!(v.prediction.is_none() && v.prediction_note.is_none());
}

#[test]
fn a_frozen_prior_that_cannot_be_compared_says_why() {
    let note = |c: Case| c.build().prediction_note.expect("a note");

    let mut no_driver = with_prior(4.2, PriorSource::Fitted);
    no_driver.ratio = None;
    assert!(note(no_driver).contains("no driver measure was registered"));

    let mut ratio_refused = with_prior(4.2, PriorSource::Fitted);
    ratio_refused.ratio = Some(RatioIn {
        refusal: Some("the driver panel has no variance".into()),
        ..ratio(5.0)
    });
    assert!(
        note(ratio_refused)
            .contains("the first stage was refused: the driver panel has no variance")
    );

    let mut stage_refused = with_prior(4.2, PriorSource::Fitted);
    let mut r = ratio(5.0);
    r.first_stage.refusal = Some("no unit stayed untreated".into());
    stage_refused.ratio = Some(r);
    assert!(note(stage_refused).contains("no unit stayed untreated"));

    let mut target_refused = with_prior(4.2, PriorSource::Fitted);
    target_refused.target = refused("no unit stayed untreated");
    assert!(
        note(target_refused).contains("the target estimate was refused: no unit stayed untreated")
    );

    let mut target_nan = with_prior(4.2, PriorSource::Fitted);
    target_nan.target = EffectIn {
        estimate: None,
        ..effect(1.0, 1.0)
    };
    assert!(note(target_nan).contains("not a finite number"));
}

#[test]
fn an_aborted_run_is_compared_with_no_prior() {
    let mut c = with_prior(4.2, PriorSource::Fitted);
    c.aborted = true;
    let v = c.build();
    assert!(v.prediction.is_none());
    assert!(v.prediction_note.unwrap().contains("aborted"));
}

#[test]
fn the_prediction_round_trips_through_json() {
    let v = with_prior(9.0, PriorSource::Declared).build();
    let back: ReportView = serde_json::from_str(&serde_json::to_string(&v).unwrap()).unwrap();
    assert_eq!(back, v);
}
