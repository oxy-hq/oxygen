//! The semantic model is checked when it is compiled, not when a pod first
//! builds an engine from it.
//!
//! Before this, a compile parsed each `.view.yml` and `.topic.yml` as YAML and
//! nothing more. A view naming a dimension that does not exist, or a topic
//! joining a view that is not there, was promoted, and the first user to ask a
//! question found out — on every pod, as a failed request. Looker's validator
//! and dbt's parse both refuse such a project before it serves, and so does
//! this: the gate builds the model the way the reader will
//! ([`crate::ConfigGate::check_semantic_model`], implemented in `oxy-app` over
//! `oxy::config::semantic_check`), and a model that does not build is not
//! promoted.
//!
//! ## The ratchet
//!
//! Refusing outright would freeze every workspace whose served model is ALREADY
//! broken: none of its other changes — an agent, an automation, a fix to some
//! other view — could ship until someone repaired the semantic model, and a
//! fix-forward that repairs half of it would be refused for the other half.
//! That trades one failure for a worse one, so the gate only refuses a compile
//! that would make things worse:
//!
//! | This compile's model | The served revision's verdict | Result |
//! | --- | --- | --- |
//! | builds | any | promoted; recorded as `validated = true` |
//! | does not build | `true` (it built) | **refused**: the revision fails and the working model keeps serving |
//! | does not build | `false`, or never checked, or nothing served | promoted with a warning; recorded as `validated = false` |
//!
//! So every workspace starts enforcing after its first compile whose model
//! builds, and never stops. The verdict lives on the revision's `semantic`
//! fingerprint ([`crate::manifest::Fingerprint::validated`]).

use std::sync::Arc;

use sea_orm::DatabaseConnection;
use uuid::Uuid;

use crate::compile::{CompiledRow, ConfigGate};
use crate::manifest::SourceEntry;
use crate::outcome::{FailureKind, FileFailure};

/// Run `gate` over this compile's semantic files and decide
/// ([`ratchet`]). Returns the verdict to record and the refusal, if any.
///
/// The served revision's verdict is read only when the model fails, so a
/// compile whose model builds costs no query here.
pub(crate) async fn judge(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    gate: Option<&Arc<dyn ConfigGate>>,
    rows: &[CompiledRow],
    sources: &[SourceEntry],
) -> (Option<bool>, Option<FileFailure>) {
    let Some(gate) = gate.cloned() else {
        return (None, None);
    };
    let config = rows.iter().find_map(|row| match row {
        CompiledRow::Config(cfg) => Some((**cfg).clone()),
        _ => None,
    });
    let of_kind = |kind: &str| -> Vec<(String, String)> {
        sources
            .iter()
            .filter(|entry| entry.kind == kind)
            .filter_map(|entry| Some((entry.file_path.clone(), entry.text.clone()?)))
            .collect()
    };
    let (views, topics) = (of_kind("view"), of_kind("topic"));
    // Building an engine re-validates the whole model and builds its join
    // graph: CPU work that must not hold a runtime thread, the same reason the
    // request path builds engines on the blocking pool.
    let answer = tokio::task::spawn_blocking(move || {
        gate.check_semantic_model(config.as_ref(), &as_sources(&views), &as_sources(&topics))
    })
    .await
    .unwrap_or_else(|e| {
        // A panic in the gate is a bug in the gate, not in the tenant's model:
        // record no verdict, which never refuses.
        tracing::error!(error = %e, "compile: the semantic gate did not finish; recording no verdict");
        None
    });
    let verdict = SemanticVerdict::from_gate(answer);
    let baseline = if verdict.needs_baseline() {
        crate::writer::served_semantic_verdict(db, workspace_id).await
    } else {
        None
    };
    ratchet(&verdict, baseline)
}

/// One semantic file, as the gate is handed it: the path it was compiled from
/// and the text the reader will parse.
#[derive(Debug, Clone, Copy)]
pub struct SemanticSource<'a> {
    pub file_path: &'a str,
    pub text: &'a str,
}

/// Why a semantic model did not build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticModelError {
    /// The file the failure is in, when one file is to blame (a parse error).
    /// `None` for a failure of the model as a whole (a join that cannot be
    /// resolved across files).
    pub file_path: Option<String>,
    pub message: String,
}

fn as_sources(files: &[(String, String)]) -> Vec<SemanticSource<'_>> {
    files
        .iter()
        .map(|(file_path, text)| SemanticSource { file_path, text })
        .collect()
}

/// What the gate said about this compile's model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SemanticVerdict {
    /// No gate, or a gate that does not check semantic models.
    Unchecked,
    Builds,
    Fails(SemanticModelError),
}

impl SemanticVerdict {
    pub(crate) fn from_gate(answer: Option<Result<(), SemanticModelError>>) -> Self {
        match answer {
            None => Self::Unchecked,
            Some(Ok(())) => Self::Builds,
            Some(Err(e)) => Self::Fails(e),
        }
    }

    /// Whether deciding needs the served revision's verdict at all — the only
    /// case that costs a query.
    pub(crate) fn needs_baseline(&self) -> bool {
        matches!(self, Self::Fails(_))
    }
}

/// The decision: what to record on this revision's `semantic` fingerprint, and
/// the failure that refuses it, if any. `baseline` is the served revision's
/// recorded verdict.
pub(crate) fn ratchet(
    verdict: &SemanticVerdict,
    baseline: Option<bool>,
) -> (Option<bool>, Option<FileFailure>) {
    let error = match verdict {
        SemanticVerdict::Unchecked => return (None, None),
        SemanticVerdict::Builds => return (Some(true), None),
        SemanticVerdict::Fails(error) => error,
    };
    if baseline != Some(true) {
        tracing::warn!(
            file = error.file_path.as_deref().unwrap_or("semantics/"),
            message = %error.message,
            "compile: the semantic model does not build. The served revision's did not either \
             (or was never checked), so this compile is not refused; it will be once a compile \
             whose model builds has been served"
        );
        return (Some(false), None);
    }
    let refusal = FileFailure {
        path: error
            .file_path
            .clone()
            .unwrap_or_else(|| "semantics/".to_string()),
        kind: FailureKind::Validation,
        message: format!(
            "the semantic model does not build: {}. The revision being served builds, so this \
             one is not promoted and that one keeps serving",
            error.message
        ),
    };
    (Some(false), Some(refusal))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fails(path: Option<&str>) -> SemanticVerdict {
        SemanticVerdict::Fails(SemanticModelError {
            file_path: path.map(str::to_string),
            message: "unknown dimension `region`".into(),
        })
    }

    #[test]
    fn a_model_that_builds_is_recorded_and_never_refused() {
        for baseline in [None, Some(false), Some(true)] {
            assert_eq!(
                ratchet(&SemanticVerdict::Builds, baseline),
                (Some(true), None)
            );
        }
    }

    #[test]
    fn breaking_a_working_model_is_refused_and_names_the_file() {
        let (validated, refusal) =
            ratchet(&fails(Some("semantics/views/orders.view.yml")), Some(true));
        assert_eq!(validated, Some(false));
        let refusal = refusal.expect("refused");
        assert_eq!(refusal.path, "semantics/views/orders.view.yml");
        assert_eq!(refusal.kind, FailureKind::Validation);
        assert!(refusal.message.contains("unknown dimension `region`"));
    }

    #[test]
    fn a_model_wide_failure_is_reported_against_the_semantics_directory() {
        let (_, refusal) = ratchet(&fails(None), Some(true));
        assert_eq!(refusal.expect("refused").path, "semantics/");
    }

    #[test]
    fn an_already_broken_or_unchecked_workspace_is_not_frozen() {
        for baseline in [None, Some(false)] {
            assert_eq!(ratchet(&fails(None), baseline), (Some(false), None));
        }
    }

    #[test]
    fn no_gate_records_nothing_and_needs_no_baseline() {
        assert_eq!(
            ratchet(&SemanticVerdict::Unchecked, Some(true)),
            (None, None)
        );
        assert!(!SemanticVerdict::Unchecked.needs_baseline());
        assert!(!SemanticVerdict::Builds.needs_baseline());
        assert!(fails(None).needs_baseline());
    }
}
