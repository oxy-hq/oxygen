//! The workspace-preview Airway sample's shared vocabulary: the task kind that
//! runs one, and a rotate-on-use source's registered sandbox company.
//!
//! Both are named by the preview platform (`oxy-previews`, which registers
//! sandboxes and seeds sample runs) and by `agentic-airway` (which applies a
//! sample at claim). They live here, at T1, so the preview platform names them
//! without depending on airway. `agentic_airway::preview` re-exports both for
//! one release; the sample's policy and its apply step stay in airway.

use serde::{Deserialize, Serialize};

use crate::preview_names::PREVIEW_PREFIX;

/// The `TaskSpec::Custom` kind that runs a sample, the `source_type` of its
/// `agentic_runs` row, and the key its Airway events are rendered under.
///
/// `agentic_airway::preview::PREVIEW_AIRWAY_SAMPLE` and
/// `agentic_pipeline::PREVIEW_AIRWAY_SAMPLE` re-export it.
pub const PREVIEW_AIRWAY_SAMPLE: &str = "preview_airway_sample";

/// A rotate-on-use source's sandbox company, as staff registered it
/// (`workspace_preview_sources.overrides`): identifiers and secret **names**,
/// never a secret.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxSource {
    pub realm_id: String,
    /// The sampler rotates this grant: it is the only writer of this var.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token_var: Option<String>,
    /// Someone else refreshes this grant; the sample only reads the token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_token_var: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id_var: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret_var: Option<String>,
}

impl SandboxSource {
    /// Well formed: a realm, exactly one token custody, the client secret a
    /// rotating grant needs, at most one way of naming the client id, and
    /// plain secret names.
    pub fn validate(&self) -> Result<(), String> {
        let realm = self.realm_id.trim();
        if realm.is_empty() || !realm.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err("`realm_id` must be the sandbox company's id".into());
        }
        match (&self.refresh_token_var, &self.access_token_var) {
            (Some(_), Some(_)) | (None, None) => {
                return Err(
                    "name exactly one of `refresh_token_var` (the sample rotates the \
                            sandbox grant) or `access_token_var` (someone else does)"
                        .into(),
                );
            }
            (Some(_), None) if self.client_secret_var.is_none() => {
                return Err("a rotating sandbox grant needs `client_secret_var`".into());
            }
            _ => {}
        }
        if self.client_id.is_some() && self.client_id_var.is_some() {
            return Err("name the client id once: `client_id` or `client_id_var`".into());
        }
        for name in self.var_names() {
            // No `/`: every app-scoped secret is `apps/<app_id>/<KEY>`, and a
            // custom app's Function (Pokehouse's `refresh-qb-token`) is the
            // one rotator of production's QuickBooks grant. A sandbox var is a
            // plain workspace secret name.
            if name.contains('/') {
                return Err(format!(
                    "`{name}` names an app-scoped secret; a sandbox var is a plain workspace \
                     secret name with no `/`"
                ));
            }
            let plain = !name.is_empty()
                && name.len() <= 200
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
                && !name.starts_with(PREVIEW_PREFIX);
            if !plain {
                return Err(format!("`{name}` is not a secret name"));
            }
        }
        Ok(())
    }

    /// Every secret name this source reads.
    pub fn var_names(&self) -> Vec<&str> {
        [
            &self.refresh_token_var,
            &self.access_token_var,
            &self.client_id_var,
            &self.client_secret_var,
        ]
        .into_iter()
        .filter_map(|v| v.as_deref())
        .collect()
    }

    /// The var the sample rotates, when it rotates one.
    pub fn rotating_var(&self) -> Option<&str> {
        self.refresh_token_var.as_deref()
    }
}
