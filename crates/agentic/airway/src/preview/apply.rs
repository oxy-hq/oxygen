//! [`PreviewSample::apply`]: a queued sample, made the spec's, at claim.

use airway::connector::Environment;
use chrono::Duration;
use serde::{Deserialize, Serialize};

use super::sandbox::swap_to_sandbox;
use super::{
    MAX_WINDOW_DAYS, SAMPLE_REFUSED_KINDS, SampleRefusal, SampleWindow, SandboxSource, is_windowed,
    rotates_on_use, scoped_pipeline_name,
};
use crate::AirwayAdmission;
use crate::config::{AirwayPipelineSpec, DestinationSpec, RESERVED_NAME_PREFIX};

/// A sample, as queued with its run: what [`PreviewSample::apply`] needs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewSample {
    pub preview_key: String,
    #[serde(default)]
    pub window: Option<SampleWindow>,
    #[serde(default)]
    pub resources: Vec<String>,
    /// The registered sandbox, for a rotate-on-use source.
    #[serde(default)]
    pub sandbox: Option<SandboxSource>,
}

/// What [`PreviewSample::apply`] did to a spec.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppliedSample {
    /// The pipeline's own name, which production's rows are keyed by.
    pub live_name: String,
    /// `preview:<key>:<live_name>`, which the sample's rows are keyed by.
    pub name: String,
}

impl PreviewSample {
    /// Make `spec` this sample's (module doc). Every check runs before the spec
    /// is touched, so a refusal leaves it as it was.
    pub fn apply(
        &self,
        spec: &mut AirwayPipelineSpec,
        admission: &mut AirwayAdmission,
    ) -> Result<AppliedSample, SampleRefusal> {
        let kind = spec.source.kind.clone();
        if SAMPLE_REFUSED_KINDS.contains(&kind.as_str()) {
            return Err(SampleRefusal::Refused { kind });
        }
        self.check_names(&spec.name)?;
        let sandbox = match (rotates_on_use(&kind), &self.sandbox) {
            (false, _) => None,
            (true, Some(sandbox)) => Some(sandbox),
            (true, None) => {
                return Err(SampleRefusal::SandboxRequired {
                    pipeline: spec.name.clone(),
                });
            }
        };
        self.check_scope(&kind, spec)?;
        if let DestinationSpec::Inline(inline) = &spec.destination
            && inline.kind != "memory"
        {
            return Err(SampleRefusal::InlineDestination {
                kind: inline.kind.clone(),
            });
        }
        if let Some(sandbox) = sandbox {
            swap_to_sandbox(&mut spec.source.config, sandbox)?;
            admission.environment = Environment::Sandbox;
        }
        if let DestinationSpec::Reference(reference) = &mut spec.destination {
            reference.schema_separator = None;
        }
        if !self.resources.is_empty() {
            spec.resources = self.resources.clone();
        }
        spec.allow_concurrent_runs = false;
        let name = scoped_pipeline_name(&self.preview_key, &spec.name);
        let live_name = std::mem::replace(&mut spec.name, name.clone());
        Ok(AppliedSample { live_name, name })
    }

    /// The key can scope a name, and the pipeline is not already scoped.
    fn check_names(&self, name: &str) -> Result<(), SampleRefusal> {
        let key = &self.preview_key;
        if key.is_empty() || key.contains(':') {
            return Err(SampleRefusal::Invalid(format!(
                "`{key}` is not a preview key"
            )));
        }
        if name.starts_with(RESERVED_NAME_PREFIX) {
            return Err(SampleRefusal::Invalid(format!(
                "pipeline `{name}` is already preview-scoped"
            )));
        }
        Ok(())
    }

    /// What was queued still holds at claim: a windowed source carries its
    /// window (at most [`MAX_WINDOW_DAYS`]), any other carries named resources
    /// and no window, and every named resource is one the pipeline reads.
    fn check_scope(&self, kind: &str, spec: &AirwayPipelineSpec) -> Result<(), SampleRefusal> {
        match (is_windowed(kind), self.window) {
            (true, None) => {
                return Err(SampleRefusal::WindowRequired(
                    "a sample of a windowed source runs only with its window; it was queued \
                     without one"
                        .into(),
                ));
            }
            (true, Some(w)) if w.to - w.from > Duration::days(MAX_WINDOW_DAYS) => {
                let days = (w.to - w.from).num_days() + 1;
                return Err(SampleRefusal::WindowTooLong { days });
            }
            (false, Some(_)) => {
                return Err(SampleRefusal::WindowNotSupported {
                    kind: kind.to_string(),
                });
            }
            (false, None) if self.resources.is_empty() => {
                return Err(SampleRefusal::ResourcesRequired {
                    advertised: spec.resources.clone(),
                });
            }
            _ => {}
        }
        let listed = &spec.resources;
        if let Some(unknown) = self
            .resources
            .iter()
            .find(|r| !listed.is_empty() && !listed.contains(r))
        {
            return Err(SampleRefusal::UnknownResource {
                name: unknown.clone(),
                advertised: listed.clone(),
            });
        }
        Ok(())
    }
}
