//! Which env keys an app *declares*, and reconciling those against the secrets
//! actually stored under `apps/<app_id>/`.
//!
//! Two sources of declarations, merged:
//!
//! - The app-level `env` block in `oxy-app.json` — what the author says the app
//!   needs. App-level rather than per-function because a secret is app-scoped by
//!   construction (`apps/<app_id>/`), so two functions sharing `STRIPE_API_KEY`
//!   must not be able to declare it twice with conflicting descriptions.
//! - Every function's `webhook.secretVar`, which already names a key. Folding
//!   these in costs the author nothing and closes the bootstrap footgun: a
//!   `webhook:` block whose secret was never set answers 401 to every delivery,
//!   and nothing in the product said which key to fill.
//!
//! Reconciliation is a pure function over (declarations, stored rows) so the
//! interesting cases — declared-but-missing, set-but-undeclared — are unit
//! testable without a database.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One key an app declares it needs.
///
/// No `value` field, ever. The manifest *names* secrets and never holds one —
/// the same rule `webhook.secretVar` follows, and the reason a bundle can be
/// committed to a public repo.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OxyAppEnvDecl {
    /// Surfaced as **Missing** rather than merely absent when unset.
    ///
    /// Defaults to false: a declaration is documentation first, and defaulting
    /// to required would turn every newly-declared key into an alarm on apps
    /// that are running fine.
    #[serde(default)]
    pub required: bool,
    /// Shown next to the key in the management UI — what it is and where to get
    /// one ("Stripe restricted key, dashboard → Developers → API keys").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Outside production, `ctx.env` falls back to production's value for this
    /// key when the environment holds none of its own (`scope`). Default false:
    /// an unshared key with no environment value reads as unset, so staging
    /// never holds a production credential nobody chose to give it.
    ///
    /// Only for a key nothing writes: a publish refuses `shared` on a key a
    /// function sets with `ctx.secrets.set` or names as a `webhook.secretVar`
    /// ([`super::shared_env::shared_key_conflicts`]).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub shared: bool,
}

/// Where a declaration came from. Drives how the UI labels a row, and whether
/// "undeclared" is worth flagging at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EnvSource {
    /// The `env` block in `oxy-app.json`.
    Manifest,
    /// A function's `webhook.secretVar`.
    Webhook,
    /// Stored, but nothing in the active build asks for it. Not an error — a
    /// key written by `ctx.secrets.set` (a refreshed OAuth token) is legitimately
    /// undeclared, and so is one left behind by a build that no longer reads it.
    Undeclared,
}

/// A declaration after merging both sources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeclaredKey {
    pub key: String,
    pub required: bool,
    pub description: Option<String>,
    pub source: EnvSource,
    /// Declared `"shared": true` (`OxyAppEnvDecl::shared`).
    pub shared: bool,
}

/// A secret that actually exists under `apps/<app_id>/`, with the prefix already
/// stripped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredSecret {
    pub key: String,
    pub secret_id: Uuid,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub updated_by_email: Option<String>,
}

/// One row in the reconciled view the API returns.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AppSecretEntry {
    /// Bare key (`STRIPE_API_KEY`), never the `apps/<uuid>/` storage name. The
    /// prefix is an implementation detail of where it lives; showing it is what
    /// made the existing settings table unreadable.
    pub key: String,
    /// A value is stored right now.
    pub is_set: bool,
    /// The active build asks for this key.
    pub declared: bool,
    /// Declared with `required: true`. Always false for an undeclared key.
    pub required: bool,
    pub source: EnvSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Only present when `is_set` — the row id, for reveal/delete by id on the
    /// existing project-secrets routes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_by_email: Option<String>,
    /// Declared `"shared": true`: outside production, `ctx.env` falls back to
    /// production's value while the environment holds none of its own.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub shared: bool,
    /// In a non-production view: nothing is stored here and the key is
    /// `shared`, so a run reads production's value. Not missing.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub inherits_production: bool,
}

impl AppSecretEntry {
    /// Needs attention: declared, required, and nothing a run would read.
    pub fn is_missing_required(&self) -> bool {
        self.required && !self.is_set && !self.inherits_production
    }
}

/// Read the app-level `env` block out of a raw `oxy-app.json`.
///
/// **Lenient, like [`retention_policy_from_build_manifest`] and unlike
/// [`migrations_config`].** A malformed `env` block degrades to "declares
/// nothing", which costs a display: the app still runs, `ctx.env` still resolves
/// whatever is stored, and no data moves. Failing the publish instead would
/// break shipping code over a documentation field.
///
/// It is not silent, though — the parse error comes back as the second half of
/// the tuple and is rendered in the UI, so a misspelled block is diagnosable
/// rather than looking like an app that declares nothing.
///
/// [`retention_policy_from_build_manifest`]: super::super::custom_apps_manifest::retention_policy_from_build_manifest
/// [`migrations_config`]: super::super::custom_apps_manifest::migrations_config
pub(crate) fn declared_env(
    manifest_json: Option<&serde_json::Value>,
    app_id: Uuid,
) -> (BTreeMap<String, OxyAppEnvDecl>, Option<String>) {
    let Some(raw) = manifest_json.and_then(|m| m.get("env")) else {
        return (BTreeMap::new(), None);
    };
    // An explicit `null` declares nothing — `JSON.stringify` emits it for an
    // optional field a generator left unset. Same reading as `migrations`.
    if raw.is_null() {
        return (BTreeMap::new(), None);
    }
    match serde_json::from_value::<BTreeMap<String, OxyAppEnvDecl>>(raw.clone()) {
        Ok(map) => (map, None),
        Err(e) => {
            let msg = format!(
                "the `env` block in oxy-app.json is not usable ({e}); it must be an object \
                 keyed by env-var name, e.g. \"env\": {{ \"STRIPE_API_KEY\": {{ \"required\": \
                 true, \"description\": \"…\" }} }}"
            );
            tracing::warn!(%app_id, "oxy-app.json `env`: {msg}");
            (BTreeMap::new(), Some(msg))
        }
    }
}

/// Every key named by a `webhook.secretVar` across a build's function manifests.
///
/// **Comma-split**, because `secretVar` may hold two live signing keys during a
/// provider rotation (Uber's `BASIC_HMAC` issues a pair). Both halves are real
/// keys someone has to set, so both belong in the view.
pub(crate) fn webhook_secret_vars<'a>(
    function_manifests: impl IntoIterator<Item = &'a serde_json::Value>,
) -> BTreeSet<String> {
    function_manifests
        .into_iter()
        .filter_map(|m| m.get("webhook")?.get("secretVar")?.as_str())
        .flat_map(|v| v.split(','))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Merge both declaration sources. A key in the `env` block keeps its
/// `required` / `description`; a webhook-only key is required by construction —
/// a declared `webhook:` with no resolvable secret is a 401 on every delivery,
/// which is exactly the "must be set" case.
pub(crate) fn merge_declared(
    env: BTreeMap<String, OxyAppEnvDecl>,
    webhook_vars: BTreeSet<String>,
) -> BTreeMap<String, DeclaredKey> {
    let mut out: BTreeMap<String, DeclaredKey> = webhook_vars
        .into_iter()
        .map(|key| {
            (
                key.clone(),
                DeclaredKey {
                    key,
                    required: true,
                    description: Some(
                        "Signing key for this app's webhook. Deliveries are rejected with 401 \
                         until it is set."
                            .to_string(),
                    ),
                    source: EnvSource::Webhook,
                    shared: false,
                },
            )
        })
        .collect();
    // The explicit block wins: an author who wrote a description for the key
    // said more about it than we can infer from the webhook block.
    for (key, decl) in env {
        out.insert(
            key.clone(),
            DeclaredKey {
                key,
                required: decl.required,
                description: decl.description,
                source: EnvSource::Manifest,
                shared: decl.shared,
            },
        );
    }
    out
}

/// Reconcile declarations against what is stored.
///
/// The union of both sides, so the three states are all visible: declared and
/// set, declared and **missing**, set but undeclared. Sorted so the rows that
/// need action come first — missing-required, then missing-optional, then the
/// rest — because a fresh deploy's whole question is "what do I still have to
/// fill in".
pub(crate) fn reconcile(
    declared: BTreeMap<String, DeclaredKey>,
    stored: Vec<StoredSecret>,
) -> Vec<AppSecretEntry> {
    let mut stored: BTreeMap<String, StoredSecret> =
        stored.into_iter().map(|s| (s.key.clone(), s)).collect();

    let mut entries: Vec<AppSecretEntry> = declared
        .into_values()
        .map(|d| {
            let row = stored.remove(&d.key);
            AppSecretEntry {
                key: d.key,
                is_set: row.is_some(),
                declared: true,
                required: d.required,
                source: d.source,
                description: d.description,
                secret_id: row.as_ref().map(|r| r.secret_id),
                updated_at: row.as_ref().map(|r| r.updated_at),
                updated_by_email: row.and_then(|r| r.updated_by_email),
                shared: d.shared,
                inherits_production: false,
            }
        })
        .collect();

    // Whatever is left is stored but unasked-for.
    entries.extend(stored.into_values().map(|row| AppSecretEntry {
        key: row.key,
        is_set: true,
        declared: false,
        required: false,
        source: EnvSource::Undeclared,
        description: None,
        secret_id: Some(row.secret_id),
        updated_at: Some(row.updated_at),
        updated_by_email: row.updated_by_email,
        shared: false,
        inherits_production: false,
    }));

    entries.sort_by(|a, b| {
        sort_rank(a)
            .cmp(&sort_rank(b))
            .then_with(|| a.key.cmp(&b.key))
    });
    entries
}

/// 0 = missing and required, 1 = missing, 2 = set. Ties break alphabetically.
fn sort_rank(e: &AppSecretEntry) -> u8 {
    match (e.is_set, e.required) {
        (false, true) => 0,
        (false, false) => 1,
        (true, _) => 2,
    }
}

#[cfg(test)]
#[path = "declared_tests.rs"]
mod tests;
