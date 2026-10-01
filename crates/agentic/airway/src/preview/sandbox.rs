//! A rotate-on-use source's sandbox company, and swapping it into a spec.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::SampleRefusal;
use crate::config::RESERVED_NAME_PREFIX;

/// QuickBooks config keys that carry or name a credential, or pick the company.
/// A sample removes every one before it writes the sandbox's in.
const QUICKBOOKS_CREDENTIAL_KEYS: [&str; 7] = [
    "realm_id",
    "client_secret",
    "client_secret_var",
    "refresh_token",
    "refresh_token_var",
    "access_token",
    "access_token_var",
];

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
                && !name.starts_with(RESERVED_NAME_PREFIX);
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

/// Replace a QuickBooks config's company and credentials with the sandbox's.
/// An explicit `base_url` is refused: the vendor's sandbox host comes from the
/// `sandbox` environment, and a URL the branch chose could be production's.
pub(super) fn swap_to_sandbox(
    config: &mut Value,
    sandbox: &SandboxSource,
) -> Result<(), SampleRefusal> {
    sandbox.validate().map_err(SampleRefusal::Invalid)?;
    if config.is_null() {
        *config = Value::Object(Map::new());
    }
    let obj = config
        .as_object_mut()
        .ok_or_else(|| SampleRefusal::Invalid("the source config is not a map".into()))?;
    if obj.contains_key("base_url") {
        return Err(SampleRefusal::ExplicitBaseUrl);
    }
    for key in QUICKBOOKS_CREDENTIAL_KEYS {
        obj.remove(key);
    }
    if sandbox.client_id.is_some() || sandbox.client_id_var.is_some() {
        obj.remove("client_id");
        obj.remove("client_id_var");
    }
    let mut set = |key: &str, value: &Option<String>| {
        if let Some(value) = value {
            obj.insert(key.to_string(), Value::String(value.clone()));
        }
    };
    set("realm_id", &Some(sandbox.realm_id.trim().to_string()));
    set("refresh_token_var", &sandbox.refresh_token_var);
    set("access_token_var", &sandbox.access_token_var);
    set("client_id", &sandbox.client_id);
    set("client_id_var", &sandbox.client_id_var);
    set("client_secret_var", &sandbox.client_secret_var);
    Ok(())
}
