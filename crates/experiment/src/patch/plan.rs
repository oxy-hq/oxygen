use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::{PriorCoefficient, PriorSource};
use crate::engine_in::RatioIn;

/// The slope the World Model held for this edge when pricing froze it
/// (`outcome.prior_coefficient`), and the coefficient the edge itself declares at the
/// registering revision, when it declares one. The reviewer reads the model's old belief
/// beside the new one — the feedback half of the control loop (spec § Predicted against measured).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplacedPrior {
    pub value: f64,
    pub source: PriorSource,
    pub edge_coefficient: Option<f64>,
}

/// A structured edit. Stored instead of a diff because no source text exists
/// where the estimate runs; the diff is rendered where the file is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoefficientEdit {
    pub file_path: String,
    pub target_measure: String,
    pub driver_measure: String,
    pub coefficient: f64,
    pub refs_entry: String,
    /// `None` when the run froze no `prior_coefficient`: there is nothing the patch can name.
    #[serde(default)]
    pub replaces: Option<ReplacedPrior>,
}

/// One variant per guard. Each `Display` states only what its own guard tested.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PatchRefusal {
    #[error("the run was aborted; an aborted estimate is not proposed as a coefficient")]
    Aborted,
    #[error(
        "the run is retrospective: its design was written after the data existed, so its slope \
         is a candidate to test, not a coefficient to write into the model"
    )]
    Retrospective,
    #[error(
        "the target or driver was filtered: the semantic model's edge is between the unfiltered \
         measures, so a slope read on a filtered one is not its coefficient"
    )]
    Filtered,
    #[error("the ratio computation was refused: {reason}")]
    RatioRefused { reason: String },
    #[error("the first stage (the lever's effect on the driver) was refused: {reason}")]
    FirstStageRefused { reason: String },
    #[error("the first stage is not significant ({}) at the registered alpha", p_text(.p_value))]
    FirstStageNotSignificant { p_value: Option<f64> },
    #[error("the ratio's coefficient is not a finite number")]
    NotFinite,
    #[error("`{reference}` is not a `<view>.<measure>` reference")]
    BadReference { reference: String },
    #[error("no compiled view named `{view}` exists at the registered revision")]
    NoView { view: String },
    #[error("view `{view}` declares no measure named `{measure}`")]
    NoMeasure { view: String, measure: String },
    #[error("measure `{measure}` declares no driver edge from `{driver}`")]
    NoEdge { measure: String, driver: String },
    #[error("measure `{measure}` declares the driver edge from `{driver}` more than once")]
    AmbiguousEdge { measure: String, driver: String },
    #[error("the edge declares `form: {form}`, not `linear`; the ratio is a linear slope")]
    NonLinearForm { form: String },
    #[error(
        "the edge declares `coefficients:`; writing `coefficient:` beside it would declare both"
    )]
    DeclaresCoefficients,
    #[error(
        "could not find the edge `{driver}` under measure `{measure}` as block-style YAML in the file text"
    )]
    EdgeNotInText { measure: String, driver: String },
    #[error("the file text uses a layout this editor does not edit: {what}")]
    UnsupportedLayout { what: String },
    #[error("the edit did not survive re-reading the file: {what}")]
    EditDidNotRoundTrip { what: String },
}

fn p_text(p: &Option<f64>) -> String {
    match p {
        Some(p) => format!("p = {p:.3}"),
        None => "no p-value was recorded".to_string(),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EdgeFacts {
    pub form: Option<String>,
    pub declares_coefficients: bool,
    pub coefficient: Option<f64>,
    pub refs: Vec<String>,
}

pub struct ViewSource<'a> {
    pub file_path: &'a str,
    pub definition: &'a Value,
}

pub struct WriteBackInputs<'a> {
    pub aborted: bool,
    pub retrospective: bool,
    /// `outcome.target_filter` or `outcome.driver_filter` is set.
    pub filtered: bool,
    pub ratio: &'a RatioIn,
    pub target: &'a str,
    pub driver: &'a str,
    pub view: Option<ViewSource<'a>>,
    pub experiment: &'a str,
    /// The registered design version, `v<n>`: what `refs` names after the experiment.
    pub design_ref: &'a str,
    /// Frozen at pricing; the edit names it as the prior it would replace.
    pub prior: Option<&'a PriorCoefficient>,
}

fn split_reference(reference: &str) -> Result<(&str, &str), PatchRefusal> {
    reference
        .split_once('.')
        .filter(|(v, m)| !v.is_empty() && !m.is_empty())
        .ok_or_else(|| PatchRefusal::BadReference {
            reference: reference.to_string(),
        })
}

/// What the compiled definition says about `measure`'s edge from `driver`.
/// Matching is exact string equality on `name` and `measure`, like every
/// other name in this feature.
pub fn edge_facts(view: &Value, measure: &str, driver: &str) -> Result<EdgeFacts, PatchRefusal> {
    let view_name = view
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let m = view
        .get("measures")
        .and_then(Value::as_array)
        .and_then(|ms| {
            ms.iter()
                .find(|m| m.get("name").and_then(Value::as_str) == Some(measure))
        })
        .ok_or_else(|| PatchRefusal::NoMeasure {
            view: view_name,
            measure: measure.to_string(),
        })?;
    let hits: Vec<&Value> = m
        .get("drivers")
        .and_then(Value::as_array)
        .map(|ds| {
            ds.iter()
                .filter(|d| d.get("measure").and_then(Value::as_str) == Some(driver))
                .collect()
        })
        .unwrap_or_default();
    let (measure, driver) = (measure.to_string(), driver.to_string());
    let edge = match hits.as_slice() {
        [] => return Err(PatchRefusal::NoEdge { measure, driver }),
        [one] => *one,
        _ => return Err(PatchRefusal::AmbiguousEdge { measure, driver }),
    };
    Ok(EdgeFacts {
        form: edge
            .get("form")
            .map(|f| f.as_str().unwrap_or("<not a string>").to_string()),
        declares_coefficients: edge.get("coefficients").is_some(),
        coefficient: edge.get("coefficient").and_then(Value::as_f64),
        refs: edge
            .get("refs")
            .and_then(Value::as_array)
            .map(|rs| {
                rs.iter()
                    .filter_map(|r| r.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// The guards, in the order the spec's table states them. A message is only
/// ever about the guard that produced it.
pub fn plan_write_back(i: WriteBackInputs<'_>) -> Result<CoefficientEdit, PatchRefusal> {
    if i.aborted {
        return Err(PatchRefusal::Aborted);
    }
    if i.retrospective {
        return Err(PatchRefusal::Retrospective);
    }
    if i.filtered {
        return Err(PatchRefusal::Filtered);
    }
    if let Some(reason) = &i.ratio.refusal {
        return Err(PatchRefusal::RatioRefused {
            reason: reason.clone(),
        });
    }
    if let Some(reason) = &i.ratio.first_stage.refusal {
        return Err(PatchRefusal::FirstStageRefused {
            reason: reason.clone(),
        });
    }
    if !i.ratio.first_stage.significant {
        return Err(PatchRefusal::FirstStageNotSignificant {
            p_value: i.ratio.first_stage.p_value,
        });
    }
    let Some(coefficient) = i.ratio.coefficient.filter(|c| c.is_finite()) else {
        return Err(PatchRefusal::NotFinite);
    };
    let (view_name, measure) = split_reference(i.target)?;
    let view = i.view.ok_or_else(|| PatchRefusal::NoView {
        view: view_name.to_string(),
    })?;
    let facts = edge_facts(view.definition, measure, i.driver)?;
    if facts.declares_coefficients {
        return Err(PatchRefusal::DeclaresCoefficients);
    }
    let edge_coefficient = facts.coefficient;
    if let Some(form) = facts.form.filter(|f| f != "linear") {
        return Err(PatchRefusal::NonLinearForm { form });
    }
    Ok(CoefficientEdit {
        file_path: view.file_path.to_string(),
        target_measure: i.target.to_string(),
        driver_measure: i.driver.to_string(),
        coefficient,
        refs_entry: format!("experiment:{}@{}", i.experiment, i.design_ref),
        // Named only when pricing froze one; the edge's own coefficient rides along so a drift is visible.
        replaces: i.prior.map(|p| ReplacedPrior {
            value: p.value,
            source: p.source,
            edge_coefficient,
        }),
    })
}

/// The branch's commit subject. Names the prior the edit would replace, when there is one,
/// so the reviewer sees what the model believed before the experiment.
pub fn commit_message(e: &CoefficientEdit) -> String {
    let base = format!(
        "experiments: propose {} from {} ({})",
        e.target_measure, e.driver_measure, e.refs_entry
    );
    let Some(r) = &e.replaces else {
        return base;
    };
    let source = match r.source {
        PriorSource::Declared => "declared",
        PriorSource::Fitted => "fitted",
    };
    let drift = match r.edge_coefficient {
        Some(c) if (c - r.value).abs() > 1e-9 => format!(" (the edge currently declares {c:.2})"),
        _ => String::new(),
    };
    format!("{base}; replaces the {source} prior {:.2}{drift}", r.value)
}
