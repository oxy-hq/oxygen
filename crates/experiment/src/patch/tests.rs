use super::fixtures::*;
use super::*;
use crate::config::{PriorCoefficient, PriorSource};
use crate::engine_in::RatioIn;
use crate::test_fixtures::{effect, ratio};

fn inputs<'a>(r: &'a RatioIn, view: Option<&'a serde_json::Value>) -> WriteBackInputs<'a> {
    WriteBackInputs {
        aborted: false,
        retrospective: false,
        filtered: false,
        ratio: r,
        target: TARGET,
        driver: DRIVER,
        view: view.map(|definition| ViewSource {
            file_path: "views/store_days.view.yml",
            definition,
        }),
        experiment: "beverage_promo_pilot",
        design_ref: "v3",
        prior: None,
    }
}

#[test]
fn a_qualifying_slope_produces_an_edit_with_git_provenance() {
    let v = view_value(VIEW);
    let r = ratio(4.25);
    let edit = plan_write_back(inputs(&r, Some(&v))).expect("qualifies");
    assert_eq!(
        edit,
        CoefficientEdit {
            file_path: "views/store_days.view.yml".into(),
            target_measure: TARGET.into(),
            driver_measure: DRIVER.into(),
            coefficient: 4.25,
            refs_entry: REFS_ENTRY.into(),
            replaces: None, // the run froze no prior: nothing to name
        }
    );
}

fn prior(value: f64, source: PriorSource) -> PriorCoefficient {
    PriorCoefficient { value, source }
}

#[test]
fn the_edit_names_the_prior_it_would_replace() {
    let v = view_value(VIEW);
    let r = ratio(4.25);
    let p = prior(4.2, PriorSource::Fitted);
    let edit = plan_write_back(WriteBackInputs {
        prior: Some(&p),
        ..inputs(&r, Some(&v))
    })
    .expect("qualifies");
    assert_eq!(
        edit.replaces,
        Some(ReplacedPrior {
            value: 4.2,
            source: PriorSource::Fitted,
            edge_coefficient: None
        }),
        "the qualitative edge declares no coefficient of its own"
    );
}

#[test]
fn the_edit_also_reports_what_the_edge_itself_declares() {
    // The edge was edited since pricing: it now declares 1.5, the frozen prior was 4.2.
    let text = VIEW.replace(
        "        direction: positive\n        strength: strong\n",
        "        coefficient: 1.5\n",
    );
    let v = view_value(&text);
    let r = ratio(4.25);
    let p = prior(4.2, PriorSource::Declared);
    let edit = plan_write_back(WriteBackInputs {
        prior: Some(&p),
        ..inputs(&r, Some(&v))
    })
    .expect("qualifies");
    assert_eq!(edit.replaces.unwrap().edge_coefficient, Some(1.5));
}

fn bare_edit() -> CoefficientEdit {
    CoefficientEdit {
        file_path: "views/store_days.view.yml".into(),
        target_measure: TARGET.into(),
        driver_measure: DRIVER.into(),
        coefficient: 4.25,
        refs_entry: REFS_ENTRY.into(),
        replaces: None,
    }
}

#[test]
fn the_commit_subject_names_the_prior_when_there_is_one() {
    let bare = format!("experiments: propose {TARGET} from {DRIVER} ({REFS_ENTRY})");
    assert_eq!(commit_message(&bare_edit()), bare);

    let fitted = CoefficientEdit {
        replaces: Some(ReplacedPrior {
            value: 4.2,
            source: PriorSource::Fitted,
            edge_coefficient: None,
        }),
        ..bare_edit()
    };
    assert_eq!(
        commit_message(&fitted),
        format!("{bare}; replaces the fitted prior 4.20")
    );

    // an edge that now declares something else is said, once; one that agrees is not repeated
    let drifted = CoefficientEdit {
        replaces: Some(ReplacedPrior {
            value: 4.2,
            source: PriorSource::Declared,
            edge_coefficient: Some(1.5),
        }),
        ..bare_edit()
    };
    assert_eq!(
        commit_message(&drifted),
        format!("{bare}; replaces the declared prior 4.20 (the edge currently declares 1.50)")
    );
    let agreeing = CoefficientEdit {
        replaces: Some(ReplacedPrior {
            value: 1.5,
            source: PriorSource::Declared,
            edge_coefficient: Some(1.5),
        }),
        ..bare_edit()
    };
    assert_eq!(
        commit_message(&agreeing),
        format!("{bare}; replaces the declared prior 1.50")
    );
}

#[test]
fn an_aborted_run_proposes_nothing() {
    let v = view_value(VIEW);
    let r = ratio(4.25);
    let i = WriteBackInputs {
        aborted: true,
        ..inputs(&r, Some(&v))
    };
    assert_eq!(plan_write_back(i), Err(PatchRefusal::Aborted));
}

#[test]
fn a_filtered_target_or_driver_proposes_nothing() {
    // The edge is between the unfiltered measures; a slope read on one product's
    // sales is not that edge's coefficient.
    let v = view_value(VIEW);
    let r = ratio(4.25);
    let i = WriteBackInputs {
        filtered: true,
        ..inputs(&r, Some(&v))
    };
    assert_eq!(plan_write_back(i), Err(PatchRefusal::Filtered));
}

#[test]
fn a_retrospective_run_proposes_nothing() {
    // Its design was written after the data existed: a slope it found is a
    // candidate to test, never a coefficient to write into the model.
    let v = view_value(VIEW);
    let r = ratio(4.25);
    let i = WriteBackInputs {
        retrospective: true,
        ..inputs(&r, Some(&v))
    };
    assert_eq!(plan_write_back(i), Err(PatchRefusal::Retrospective));
}

#[test]
fn a_refused_ratio_proposes_nothing_and_says_why() {
    let v = view_value(VIEW);
    let r = RatioIn {
        refusal: Some("the driver panel has no variance".into()),
        ..ratio(4.25)
    };
    assert_eq!(
        plan_write_back(inputs(&r, Some(&v))),
        Err(PatchRefusal::RatioRefused {
            reason: "the driver panel has no variance".into()
        })
    );
}

#[test]
fn a_refused_first_stage_proposes_nothing() {
    let v = view_value(VIEW);
    let mut r = ratio(4.25);
    r.first_stage.refusal = Some("no unit stayed untreated".into());
    assert!(matches!(
        plan_write_back(inputs(&r, Some(&v))),
        Err(PatchRefusal::FirstStageRefused { .. })
    ));
}

#[test]
fn a_first_stage_indistinguishable_from_zero_proposes_nothing() {
    let v = view_value(VIEW);
    let mut r = ratio(4.25);
    r.first_stage = effect(0.3, 2.0); // significant = false
    r.first_stage.p_value = Some(0.412);
    let err = plan_write_back(inputs(&r, Some(&v))).unwrap_err();
    assert_eq!(
        err,
        PatchRefusal::FirstStageNotSignificant {
            p_value: Some(0.412)
        }
    );
    assert_eq!(
        err.to_string(),
        "the first stage is not significant (p = 0.412) at the registered alpha"
    );
}

#[test]
fn a_coefficient_that_is_not_a_number_proposes_nothing() {
    let v = view_value(VIEW);
    let r = RatioIn {
        coefficient: None,
        ..ratio(4.25)
    }; // the engine wrote NaN → null
    assert_eq!(
        plan_write_back(inputs(&r, Some(&v))),
        Err(PatchRefusal::NotFinite)
    );
}

#[test]
fn a_missing_view_measure_or_edge_is_named() {
    let r = ratio(4.25);
    assert_eq!(
        plan_write_back(inputs(&r, None)),
        Err(PatchRefusal::NoView {
            view: "store_days".into()
        })
    );
    let v = view_value(VIEW);
    let i = WriteBackInputs {
        target: "store_days.nope",
        ..inputs(&r, Some(&v))
    };
    assert_eq!(
        plan_write_back(i),
        Err(PatchRefusal::NoMeasure {
            view: "store_days".into(),
            measure: "nope".into()
        })
    );
    let i = WriteBackInputs {
        driver: "store_days.someone_else",
        ..inputs(&r, Some(&v))
    };
    assert_eq!(
        plan_write_back(i),
        Err(PatchRefusal::NoEdge {
            measure: "beverages_revenue".into(),
            driver: "store_days.someone_else".into()
        })
    );
    let i = WriteBackInputs {
        target: "no_dot",
        ..inputs(&r, Some(&v))
    };
    assert!(matches!(
        plan_write_back(i),
        Err(PatchRefusal::BadReference { .. })
    ));
}

#[test]
fn a_non_linear_edge_is_refused_by_name() {
    let text = VIEW.replace(
        "        direction: positive\n        strength: strong\n",
        "        form: log-log\n        coefficient: 1.2\n",
    );
    let v = view_value(&text);
    let r = ratio(4.25);
    assert_eq!(
        plan_write_back(inputs(&r, Some(&v))),
        Err(PatchRefusal::NonLinearForm {
            form: "log-log".into()
        })
    );
}

#[test]
fn an_explicit_linear_form_qualifies() {
    let text = VIEW.replace("        strength: strong\n", "        form: linear\n");
    let v = view_value(&text);
    let r = ratio(4.25);
    assert!(plan_write_back(inputs(&r, Some(&v))).is_ok());
}

#[test]
fn an_edge_declaring_coefficients_is_refused() {
    let text = VIEW.replace(
        "        strength: strong\n",
        "        coefficients: [1.0, 2.0]\n        form: quadratic\n",
    );
    let v = view_value(&text);
    let r = ratio(4.25);
    // the vector is the more specific fact and is checked before the form
    assert_eq!(
        plan_write_back(inputs(&r, Some(&v))),
        Err(PatchRefusal::DeclaresCoefficients)
    );
}

#[test]
fn a_driver_declared_twice_under_one_measure_is_ambiguous() {
    let text = VIEW.replace(
        "- measure: store_days.foot_traffic",
        "- measure: store_days.promo_redemptions",
    );
    let v = view_value(&text);
    let r = ratio(4.25);
    assert!(matches!(
        plan_write_back(inputs(&r, Some(&v))),
        Err(PatchRefusal::AmbiguousEdge { .. })
    ));
}

#[test]
fn edge_facts_reads_the_declared_shape() {
    let v = view_value(VIEW);
    let f = edge_facts(&v, "entree_revenue", DRIVER).unwrap();
    assert_eq!(f.form.as_deref(), Some("linear"));
    assert_eq!(f.coefficient, Some(-1.0));
    assert!(!f.declares_coefficients && f.refs.is_empty());
}
