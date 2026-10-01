//! The manifest's `"shared": true` env keys
//! (`internal-docs/2026-09-10-custom-app-environments-design.md` §4.2, §4.4):
//! which keys a non-production `ctx.env` may read from production, and the
//! publish gate refusing `shared` on a key that each environment must hold its
//! own value of.
//!
//! **A key is shared only when both sides say so.** The running build marks it
//! `shared`, and so does the build **production serves** ([`effective_shared`]).
//! A staging bundle alone can therefore never reach a production credential:
//! marking production's `QB_REFRESH_TOKEN` shared in a staging build shares
//! nothing until production's own build — published through the same gate —
//! marks it too. An app never promoted shares nothing, and a
//! `webhook.secretVar` of either build is never shared.
//!
//! The publish gate ([`check_shared_env`]) checks a new build's `shared` keys
//! against its own functions **and** those of the build production serves, so
//! a key production writes or verifies webhooks with is refused wherever it is
//! marked.

mod production;
mod scan;
#[cfg(test)]
mod tests;

pub(crate) use production::{check_publish, effective_shared_env};
pub(crate) use scan::written_secret_keys;

use std::collections::BTreeSet;

use serde_json::Value;
use uuid::Uuid;

use super::declared::{declared_env, webhook_secret_vars};

/// One build's functions, as the conflict check reads them.
pub(crate) struct BuildFunctions {
    /// How a conflict names the build: empty for the build being published,
    /// `` in production's build `<id>` `` for the one production serves.
    label: String,
    /// `(name, manifest, bundled JS)` for each function.
    functions: Vec<(String, Value, String)>,
}

impl BuildFunctions {
    /// The build being published: each declared function with its
    /// `functions/<name>.js` from the bundle (empty when absent — the
    /// artifact check refuses that separately).
    pub(crate) fn published(fn_specs: &[(String, Value)], files: &[(String, Vec<u8>)]) -> Self {
        let functions = fn_specs
            .iter()
            .map(|(name, spec)| {
                let path = format!("functions/{name}.js");
                let source = files
                    .iter()
                    .find(|(p, _)| *p == path)
                    .map(|(_, bytes)| String::from_utf8_lossy(bytes).into_owned())
                    .unwrap_or_default();
                (name.clone(), spec.clone(), source)
            })
            .collect();
        Self {
            label: String::new(),
            functions,
        }
    }

    /// The build production serves, `build_id`.
    pub(crate) fn production(build_id: &str, functions: Vec<(String, Value, String)>) -> Self {
        Self {
            label: format!(" in production's build `{build_id}`"),
            functions,
        }
    }
}

/// The keys the manifest's `env` block marks `"shared": true`. Lenient like
/// [`declared_env`]: an unusable block shares nothing.
pub(crate) fn shared_env_keys(manifest_json: Option<&Value>) -> BTreeSet<String> {
    declared_env(manifest_json, Uuid::nil())
        .0
        .into_iter()
        .filter(|(_, decl)| decl.shared)
        .map(|(key, _)| key)
        .collect()
}

/// `(name, manifest)` for every function the manifest's `functions` block
/// declares.
pub(crate) fn manifest_functions(manifest_json: Option<&Value>) -> Vec<(String, Value)> {
    manifest_json
        .and_then(|m| m.get("functions"))
        .and_then(Value::as_object)
        .map(|obj| obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default()
}

/// The keys a non-production run's `ctx.env` may take from production: marked
/// `shared` by the running build **and** by `production` — the manifest of the
/// build production serves — and a `webhook.secretVar` of neither. No
/// production build: nothing.
pub(crate) fn effective_shared(
    running: Option<&Value>,
    production: Option<&Value>,
) -> BTreeSet<String> {
    let Some(production) = production else {
        return BTreeSet::new();
    };
    let production_shared = shared_env_keys(Some(production));
    let specs: Vec<(String, Value)> = [running, Some(production)]
        .into_iter()
        .flat_map(manifest_functions)
        .collect();
    let webhook_vars = webhook_secret_vars(specs.iter().map(|(_, spec)| spec));
    shared_env_keys(running)
        .into_iter()
        .filter(|key| production_shared.contains(key) && !webhook_vars.contains(key))
        .collect()
}

/// The publish gate over [`shared_key_conflicts`]: the manifest's `shared`
/// keys against the bundle's functions and, when there is one, the build
/// production serves. `Err` lists every conflict.
pub(crate) fn check_shared_env(
    manifest_json: Option<&Value>,
    function_specs: &[(String, Value)],
    files: &[(String, Vec<u8>)],
    production: Option<&BuildFunctions>,
) -> Result<(), Vec<String>> {
    let shared = shared_env_keys(manifest_json);
    if shared.is_empty() {
        return Ok(());
    }
    let published = BuildFunctions::published(function_specs, files);
    let builds: Vec<&BuildFunctions> = std::iter::once(&published).chain(production).collect();
    let conflicts = shared_key_conflicts(&shared, &builds);
    if conflicts.is_empty() {
        Ok(())
    } else {
        Err(conflicts)
    }
}

/// Why each `shared` key may not be shared: a key a function of any of
/// `builds` writes (a `ctx.secrets.set("<KEY>", …)` with that literal in its
/// bundled JS, [`written_secret_keys`]) or names as its `webhook.secretVar`.
/// Empty when the manifest is sound.
///
/// Both are values an environment must hold its own of. A written key is
/// rotated state — a refreshed OAuth token — and staging rotating production's
/// token would fork production's grant at the provider. A webhook key verifies
/// deliveries, and staging's route verifies with staging's own value.
///
/// The written-key reading is a scan of the bundled JS, so a key computed at
/// runtime escapes it; the runtime backstop is `ctx.secrets.set` refusing a key
/// the run read through the shared fallback.
pub(crate) fn shared_key_conflicts(
    shared: &BTreeSet<String>,
    builds: &[&BuildFunctions],
) -> Vec<String> {
    let mut conflicts = Vec::new();
    for build in builds {
        let webhook_vars = webhook_secret_vars(build.functions.iter().map(|(_, spec, _)| spec));
        for key in shared.intersection(&webhook_vars) {
            conflicts.push(format!(
                "`{key}` is a webhook.secretVar{}; each environment verifies with its own value",
                build.label
            ));
        }
        for (function, _, source) in &build.functions {
            for key in shared.intersection(&written_secret_keys(source)) {
                conflicts.push(format!(
                    "`{key}` is written by function `{function}`{} (ctx.secrets.set); a \
                     written key must be set per environment",
                    build.label
                ));
            }
        }
    }
    conflicts
}
