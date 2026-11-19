//! What a custom app's functions declare they need of the platform itself
//! (`internal-docs/proposals/app-platform-access.md`).
//!
//! A function entry in `oxy-app.json` may ask for platform actions, all under
//! one `oxygen` block, as the function calls them under `ctx.oxygen`:
//!
//! ```json
//! "tablets": { "oxygen": { "frontline": { "devices": "manage" } } }
//! "ubereats-report-webhook": { "oxygen": { "sourceUploads": { "pipelines": ["ubereats_transactions"] } } }
//! ```
//!
//! **The declaration grants nothing.** The manifest ships inside the bundle and
//! is not a trust boundary. An org admin approves what the live build declares
//! (`oxy_auth::token::app_access`); the function host refuses the op until the
//! approval exists. This module is the one reader of those blocks: the runtime
//! reads a function's own entry with it, and the approval page reads every
//! function of the live build with it, so the two cannot disagree.

use std::collections::{BTreeMap, BTreeSet};

use oxy_authz::Action;
use serde::Deserialize;
use serde_json::Value;

/// The function-entry key every platform action is asked under, as the
/// function calls them under `ctx.oxygen`.
pub const OXYGEN_KEY: &str = "oxygen";

/// `"oxygen": { "frontline": { "devices": "manage" } }`. One level, `manage`, because the
/// platform has one door for the org's kiosks (`Action::FrontlineDeviceManage`).
#[derive(Debug, Deserialize, Default, Clone, PartialEq, Eq)]
pub struct FrontlineSpec {
    #[serde(default)]
    pub devices: Option<String>,
}

/// `"oxygen": { "sourceUploads": { "pipelines": ["ubereats_transactions"] } }`: the
/// pipelines, by name, whose landing zones the function uploads reports into.
/// Write only: the landing zone has no read route, so there is nothing else to
/// ask for. A pipeline is named, never a workspace: the host finds which
/// workspace of the org holds it.
#[derive(Debug, Deserialize, Default, Clone, PartialEq, Eq)]
pub struct SourceUploadsSpec {
    #[serde(default)]
    pub pipelines: Vec<String>,
}

/// The longest pipeline name an ask may carry.
pub const MAX_PIPELINE_NAME: usize = 128;

/// Whether `name` can be a pipeline's name in an ask: 1–128 characters of
/// `[A-Za-z0-9_.-]`. Anything else is not asked for (fail closed), and
/// `oxyc publish` says so before it ships.
pub fn valid_pipeline_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_PIPELINE_NAME
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// The `oxygen` block of one function entry.
#[derive(Debug, Deserialize, Default, Clone, PartialEq, Eq)]
pub struct PlatformActionSpecs {
    #[serde(default)]
    pub frontline: Option<FrontlineSpec>,
    #[serde(default, rename = "sourceUploads")]
    pub source_uploads: Option<SourceUploadsSpec>,
}

impl PlatformActionSpecs {
    /// The actions these blocks ask for, in `Action::APP_ACTIONS` order. A block
    /// with any other value asks for nothing: fail closed, as every capability.
    pub fn actions(&self) -> Vec<Action> {
        let frontline =
            self.frontline.as_ref().and_then(|f| f.devices.as_deref()) == Some("manage");
        let uploads = !self.pipelines().is_empty();
        Action::APP_ACTIONS
            .into_iter()
            .filter(|action| match action {
                Action::FrontlineDeviceManage => frontline,
                Action::SourceUploadReport => uploads,
                _ => false,
            })
            .collect()
    }

    /// Whether the function asked for `action`.
    pub fn declares(&self, action: Action) -> bool {
        self.actions().contains(&action)
    }

    /// The pipelines the function uploads into, by name: the valid names of
    /// `sourceUploads.pipelines`, sorted and without repeats.
    pub fn pipelines(&self) -> BTreeSet<String> {
        self.source_uploads
            .iter()
            .flat_map(|s| s.pipelines.iter())
            .filter(|name| valid_pipeline_name(name))
            .cloned()
            .collect()
    }

    /// What `action` is asked for on: the pipelines for an upload, nothing for
    /// an action on the whole org.
    pub fn targets(&self, action: Action) -> BTreeSet<String> {
        match action {
            Action::SourceUploadReport => self.pipelines(),
            _ => BTreeSet::new(),
        }
    }
}

/// One action a build asks for: the functions that ask, and what it is asked
/// on (the pipelines, for an upload; empty for an action on the whole org).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Asked {
    pub functions: Vec<String>,
    pub targets: BTreeSet<String>,
}

/// One function entry's `oxygen` block. Absent or malformed asks for nothing.
pub fn function_specs(entry: &Value) -> PlatformActionSpecs {
    entry
        .get(OXYGEN_KEY)
        .and_then(|block| serde_json::from_value::<PlatformActionSpecs>(block.clone()).ok())
        .unwrap_or_default()
}

/// The actions one function entry declares in its `oxygen` block. Absent,
/// malformed or anything else declares nothing.
pub fn function_declares(entry: &Value) -> Vec<Action> {
    function_specs(entry).actions()
}

/// Every action a manifest's functions declare, each with the functions that
/// declare it (sorted) and what it is asked on. What an org admin is asked to
/// approve.
pub fn declared(manifest: Option<&Value>) -> BTreeMap<Action, Asked> {
    let mut out: BTreeMap<Action, Asked> = BTreeMap::new();
    let Some(functions) = manifest
        .and_then(|m| m.get("functions"))
        .and_then(Value::as_object)
    else {
        return out;
    };
    for (name, entry) in functions {
        let specs = function_specs(entry);
        for action in specs.actions() {
            let asked = out.entry(action).or_default();
            asked.functions.push(name.clone());
            asked.targets.extend(specs.targets(action));
        }
    }
    for asked in out.values_mut() {
        asked.functions.sort();
    }
    out
}

/// A plain-words description of an app action, for the approval page and the
/// inventory. The platform's own words, never the app's.
pub fn describe(action: Action) -> &'static str {
    match action {
        Action::FrontlineDeviceManage => {
            "Manage the organization's kiosk devices: list them, add one and get its \
             enrol link, give one a new link, and revoke one."
        }
        Action::SourceUploadReport => {
            "Upload source reports into the landing zones of the pipelines named below. \
             Each report is checked by the pipeline's own source before it is stored."
        }
        _ => "",
    }
}

#[cfg(test)]
#[path = "platform_access_tests.rs"]
mod tests;
