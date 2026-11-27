//! Swapping a rotate-on-use source's sandbox company into a spec.
//!
//! [`SandboxSource`] itself is `agentic_runtime::preview_sample`'s, so the
//! preview platform (`oxy-previews`) names it without depending on airway.

use serde_json::{Map, Value};

use super::SampleRefusal;
pub use agentic_runtime::preview_sample::SandboxSource;

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
