//! Manifest schema, caching, and resolution for custom apps.
//!
//! `oxy-app.json` is the source of truth for a bundle's identity (name, slug,
//! org, project). This module owns:
//!
//! - The Rust mirror of that schema (`OxyAppManifest`) — identity fields only.
//! - `resolve_manifest` — the single entry point handlers call; it checks
//!   the DB override first, then falls back to the manifest captured with
//!   the channel's build.
//! - `pick_channel_for` — which channel a request is served from.

use std::collections::{BTreeMap, BTreeSet};

use entity::{app_builds, apps};
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_oltp::schema::MAX_NAME_LEN;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};

use crate::storage::{RetentionPolicy, RetentionRule};
use crate::sync::Channel;

// ── Manifest schema ──────────────────────────────────────────────────────────

/// Optional Ask-binding declared by the app bundle: which agent the
/// global Ask overlay should bind to on this app's surfaces, plus the
/// suggested-question chips shown on the launcher card.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OxyAppAskConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suggested_questions: Vec<String>,
}

/// Where the bundle keeps its schema migrations, from `oxy-app.json`.
///
/// **Absent by default, and that must stay the common case** — an app with no
/// tables of its own is the norm, so declaring nothing means nothing runs and
/// nothing is resolved (no OLTP writer lookup, no tenant connection).
///
/// A struct rather than a bare string so a later `strategy`/`role` field is an
/// additive change, matching [`OxyAppStorageConfig`].
///
/// Notably absent: any way to name the *schema*. The schema is derived
/// host-side from the app's slug (`oxy_oltp::schema::app_writer_name`), exactly
/// as `ctx.oltp` derives it. A manifest that could name its own schema is a
/// manifest that can migrate another app's.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OxyAppMigrationsConfig {
    /// Directory inside the bundle holding `*.sql` files, relative to the
    /// bundle root (e.g. `"migrations"`). Required when the block is present:
    /// a `migrations: {}` that silently meant "nowhere" would ship code without
    /// its schema, which is the failure this feature exists to prevent.
    pub dir: String,
}

/// Read the `migrations` block out of a raw `oxy-app.json`.
///
/// **Strict, unlike [`retention_policy_from_build_manifest`], and deliberately
/// so.** A retention policy that fails to parse degrades to "nothing expires" —
/// the safe direction, because the failure costs storage. A migrations block
/// that failed to parse would degrade to "no migrations", which ships an app's
/// code without its tables and lands on a user as `relation does not exist`.
/// The two blocks want opposite lenience, so they do not share a reader.
///
/// `Ok(None)` means the manifest genuinely declares no migrations. `Err` means
/// it declares something we could not understand, which fails the promote.
pub fn migrations_config(
    manifest_json: Option<&serde_json::Value>,
) -> Result<Option<OxyAppMigrationsConfig>, String> {
    let Some(raw) = manifest_json.and_then(|m| m.get("migrations")) else {
        return Ok(None);
    };
    // An explicit `null` is a declaration of nothing, not a malformed block —
    // `JSON.stringify` emits it for an optional field a generator left unset.
    if raw.is_null() {
        return Ok(None);
    }
    serde_json::from_value::<OxyAppMigrationsConfig>(raw.clone())
        .map(Some)
        .map_err(|e| {
            format!(
                "the `migrations` block in oxy-app.json is not usable ({e}); it must be an \
                 object with a `dir` naming a directory inside the bundle, e.g. \
                 \"migrations\": {{ \"dir\": \"migrations\" }}"
            )
        })
}

/// Read the `airhouseMigrations` block. Strict, for the reason
/// [`migrations_config`] gives: a block that failed to parse and meant "none"
/// would ship functions whose facts land in tables that were never created.
pub fn airhouse_migrations_config(
    manifest_json: Option<&serde_json::Value>,
) -> Result<Option<OxyAppMigrationsConfig>, String> {
    let Some(raw) = manifest_json.and_then(|m| m.get("airhouseMigrations")) else {
        return Ok(None);
    };
    if raw.is_null() {
        return Ok(None);
    }
    serde_json::from_value::<OxyAppMigrationsConfig>(raw.clone())
        .map(Some)
        .map_err(|e| {
            format!(
                "the `airhouseMigrations` block in oxy-app.json is not usable ({e}); it must be an \
                 object with a `dir` naming a directory inside the bundle, e.g. \
                 \"airhouseMigrations\": {{ \"dir\": \"airhouse-migrations\" }}"
            )
        })
}

/// The `oltpChanges` block: the tables of the app's OLTP schema whose every
/// committed change a production promote installs capture on, keyed by table
/// name (design `2026-11-22-oltp-change-stream-design.md` §5.1).
///
/// Like [`OxyAppMigrationsConfig`], no way to name a schema: the table lives in
/// the app's own `app_<writer>`, derived from its slug.
///
/// `deny_unknown_fields`, unlike the rest of the manifest: the one optional key
/// here is `exclude`, and a misspelt one would be read as "exclude nothing",
/// which captures the very columns the author meant to keep out of the log and
/// Airhouse. Refusing it costs a republish; ignoring it costs a purge.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OltpChangesConfig {
    pub tables: BTreeMap<String, OltpChangesTable>,
}

/// One declared table under `oltpChanges.tables`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OltpChangesTable {
    /// Columns never captured: they reach neither the change log nor Airhouse,
    /// so they keep no history. Personal data goes here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
}

/// Read the `oltpChanges` block out of a raw `oxy-app.json`.
///
/// **Strict**, for the reason [`migrations_config`] gives: a block that failed
/// to parse and meant "none" would ship an app whose declared tables are
/// silently not captured, and the history it was promised would start
/// whenever someone noticed. So it refuses, naming the field and the fix:
///
/// - a block that is not `{ "tables": { <table>: { "exclude"?: [..] } } }`;
/// - an empty `tables`: declaring nothing is leaving the block out;
/// - a table or column name that `oxy_oltp::schema::validate_name` would
///   refuse (`is_capturable_name`): it reaches generated capture SQL;
/// - a column listed twice in one `exclude`;
/// - the block without `migrations`: a captured table must be the app's own,
///   created by its migrations, which run before capture is installed.
///
/// `sdk/typescript/src/custom-app/manifest-validate.ts` (`oltpChangesRule`)
/// says the same refusals in the same words, so `oxyc validate` catches them
/// before a build. `Ok(None)` means the manifest declares no capture.
pub fn oltp_changes_from_manifest(
    raw: &serde_json::Value,
) -> Result<Option<OltpChangesConfig>, ManifestError> {
    let Some(block) = raw.get("oltpChanges") else {
        return Ok(None);
    };
    // As for `migrations`: an explicit `null` declares nothing.
    if block.is_null() {
        return Ok(None);
    }
    let refuse = ManifestError::InvalidOltpChanges;
    let config = match serde_json::from_value::<OltpChangesConfig>(block.clone()) {
        Ok(config) => config,
        Err(e) => {
            return Err(refuse(format!(
                "the `oltpChanges` block in oxy-app.json is not usable ({e}); it must be an \
                 object with a `tables` map naming each table to capture, e.g. \
                 \"oltpChanges\": {{ \"tables\": {{ \"leases\": {{}}, \
                 \"landlords\": {{ \"exclude\": [\"contact_email\"] }} }} }}"
            )));
        }
    };
    if config.tables.is_empty() {
        return Err(refuse(
            "`oltpChanges.tables` is empty: name at least one table to capture, or remove the \
             `oltpChanges` block"
                .to_string(),
        ));
    }
    for (table, spec) in &config.tables {
        check_declared_table(table, spec)?;
    }
    if matches!(raw.get("migrations"), None | Some(serde_json::Value::Null)) {
        return Err(refuse(
            "`oltpChanges` needs a `migrations` block: a captured table must be created by the \
             app's own migrations"
                .to_string(),
        ));
    }
    Ok(Some(config))
}

/// One `oltpChanges.tables` entry: a name the platform accepts, and an
/// `exclude` of such names, none twice.
fn check_declared_table(table: &str, spec: &OltpChangesTable) -> Result<(), ManifestError> {
    let refuse = ManifestError::InvalidOltpChanges;
    if !is_capturable_name(table) {
        return Err(refuse(format!(
            "`oltpChanges.tables` names {table:?}, which is not a table name the platform \
             accepts: use {}",
            capturable_name_rule()
        )));
    }
    let mut seen = BTreeSet::new();
    for column in &spec.exclude {
        if !is_capturable_name(column) {
            return Err(refuse(format!(
                "`oltpChanges.tables.{table}.exclude` names {column:?}, which is not a column \
                 name the platform accepts: use {}",
                capturable_name_rule()
            )));
        }
        if !seen.insert(column.as_str()) {
            return Err(refuse(format!(
                "`oltpChanges.tables.{table}.exclude` lists {column:?} more than once"
            )));
        }
    }
    Ok(())
}

/// `oxy_oltp::schema::validate_name`'s rule, restated because that function is
/// `pub(crate)` to `oxy-oltp`: a lowercase ASCII letter, then lowercase
/// letters, digits or `_`, at most [`MAX_NAME_LEN`] bytes. The bound is the
/// oltp crate's own constant, so the length cannot drift; the character rule
/// is the original's, line for line. A declared name reaches the capture SQL
/// `oxy_oltp::changes` generates, which accepts only what that rule passes, so
/// refusing the same set here tells the author at publish, not at the install.
fn is_capturable_name(name: &str) -> bool {
    if name.is_empty() || name.len() > MAX_NAME_LEN {
        return false;
    }
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// [`is_capturable_name`] as a sentence. `manifest-validate.ts` writes the same.
fn capturable_name_rule() -> String {
    format!(
        "1-{MAX_NAME_LEN} characters, a lowercase letter then lowercase letters, digits or \
         underscores"
    )
}

/// App-level storage config from `oxy-app.json`.
///
/// Deliberately **not** the same block as the per-function `storage: { read,
/// write }` capability. Those gate what one function may call; retention governs
/// the per-app silo that every function shares, so it can only be stated once,
/// app-wide. Putting it per-function would let two functions declare conflicting
/// lifetimes for the same object.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OxyAppStorageConfig {
    /// Prefix → TTL-class rules. Longest matching prefix wins; an unmatched key
    /// never expires. See `custom_apps_storage::retention`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retention: Vec<RetentionRule>,
}

/// Server-side mirror of the bundle's `oxy-app.json` — identity + launcher-card fields.
///
/// The producer/writer struct soup that existed here was dead code: the
/// executor that consumed it was deleted in commit 3b3dfea. The remaining
/// consumer (the debug endpoint) now exposes the raw manifest JSON, so this
/// type only needs to carry the fields that gate routing and display.
///
/// Schema version 2 is required. Version 1 bundles must be re-built with an
/// updated SDK.
/// `pub(crate)` so the seeded example bundle's manifest is validated against
/// THIS type (`cli::commands::seed_apps`). A hand-rolled check in the seed
/// would let the example drift out of schema without anything failing.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OxyAppManifest {
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub slug: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_slug: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// One-line purpose shown on the launcher card.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask: Option<OxyAppAskConfig>,
    /// Card art for the launcher — a path RELATIVE to the bundle root
    /// (e.g. "card.png"), served through the app's own public URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub art: Option<String>,
    /// Shell-rail icon — a path RELATIVE to the bundle root (e.g.
    /// "icon.svg"), served through the app's own public URL. Small square
    /// mark; the rail falls back to a name-initial tile when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Status line shown on the launcher card (e.g.
    /// "23 stores · sales +33.5% YoY · live"). A plain display string —
    /// static for now; a live data binding can replace it later.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// App-level storage policy — currently just asset retention.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage: Option<OxyAppStorageConfig>,
    /// Where the bundle keeps its `*.sql` schema migrations. Absent for an app
    /// with no tables of its own, which is the common case and must stay
    /// zero-config. See [`OxyAppMigrationsConfig`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub migrations: Option<OxyAppMigrationsConfig>,
    /// Where the bundle keeps the `*.sql` migrations for its Airhouse schema —
    /// the tables `ctx.airhouse` appends facts to. Absent for most apps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub airhouse_migrations: Option<OxyAppMigrationsConfig>,
    // `oltpChanges` is deliberately not a field: serving and the launcher
    // parse this type, and the block is strict. Publish and promote read it
    // from the raw manifest with [`oltp_changes_from_manifest`].
}

/// Resolve an app's asset-retention policy from the `oxy-app.json` captured in
/// its build row (`app_builds.manifest_json`).
///
/// Reads the manifest that shipped with the **running build**, so a policy change
/// takes effect on publish alongside the code that writes the assets — not the
/// moment someone edits a file.
///
/// Every failure here degrades to "no policy", i.e. nothing expires. A manifest
/// that won't parse must never be read as permission to start deleting; the
/// warnings are logged so a misspelled rule is diagnosable rather than silent.
pub fn retention_policy_from_build_manifest(
    manifest_json: Option<&serde_json::Value>,
    app_id: uuid::Uuid,
) -> RetentionPolicy {
    let Some(raw) = manifest_json else {
        return RetentionPolicy::default();
    };
    let config: OxyAppStorageConfig = match raw.get("storage") {
        Some(v) => match serde_json::from_value(v.clone()) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    %app_id,
                    "oxy-app.json `storage` block could not be parsed ({e}); \
                     assets for this app will not expire"
                );
                return RetentionPolicy::default();
            }
        },
        None => return RetentionPolicy::default(),
    };
    let (policy, warnings) = RetentionPolicy::from_rules(&config.retention);
    for warning in warnings {
        tracing::warn!(%app_id, "oxy-app.json storage.retention: {warning}");
    }
    policy
}

// ── Manifest error ───────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("no oxy-app.json recorded for this app's build")]
    NotFound,
    #[error("oxy-app.json could not be read: {0}")]
    Io(String),
    #[error("oxy-app.json is not valid JSON: {0}")]
    Parse(String),
    #[error("oxy-app.json schemaVersion {0} is not supported; expected 2")]
    UnsupportedSchema(u32),
    /// The `oltpChanges` block declares something the platform cannot
    /// capture; the message names the field and the fix
    /// ([`oltp_changes_from_manifest`]).
    #[error("{0}")]
    InvalidOltpChanges(String),
}

// ── Manifest resolution ──────────────────────────────────────────────────────

fn parse_manifest_value(raw: serde_json::Value) -> Result<OxyAppManifest, ManifestError> {
    let manifest: OxyAppManifest =
        serde_json::from_value(raw).map_err(|e| ManifestError::Parse(e.to_string()))?;
    if manifest.schema_version != 2 {
        return Err(ManifestError::UnsupportedSchema(manifest.schema_version));
    }
    Ok(manifest)
}

/// Resolve the manifest for an app on a given channel. Precedence:
///
/// 1. `apps.manifest_override` (per-deployment override).
/// 2. The manifest captured in the channel's current build row
///    (`app_builds.manifest_json`, written at publish from the bundle's
///    `oxy-app.json`). No build pointer → `NotFound`.
///
/// Called by the customer-apps debug endpoint and the workspace custom-apps
/// list (launcher card metadata); the live serve path injects identity via
/// `window.__OXY_APP__` and serves the bundle's `oxy-app.json` directly.
pub async fn resolve_manifest(
    db: &DatabaseConnection,
    app: &apps::Model,
    channel: Channel,
) -> Result<OxyAppManifest, ManifestError> {
    if let Some(raw) = &app.manifest_override {
        return parse_manifest_value(raw.clone());
    }
    // The draft channel is the staging environment's build; the published
    // channel is production's.
    let environment = match channel {
        Channel::Draft => AppEnvironment::Staging,
        Channel::Published => AppEnvironment::Production,
    };
    let build_pk = crate::env_resolve::resolve_environment(db, app, &environment)
        .await
        .map_err(|e| ManifestError::Io(e.to_string()))?
        .build_id
        .ok_or(ManifestError::NotFound)?;
    let build = app_builds::Entity::find_by_id(build_pk)
        .one(db)
        .await
        .map_err(|e| ManifestError::Io(e.to_string()))?
        .ok_or(ManifestError::NotFound)?;
    parse_manifest_value(build.manifest_json.ok_or(ManifestError::NotFound)?)
}

/// Resolve manifests for MANY apps with a **single** `app_builds` query — the
/// batched counterpart to [`resolve_manifest`], for paginated list endpoints
/// (avoids an N+1 per-page). `environments` is the page's builds, read by the
/// caller with `load_environment_builds_batch` because it needs them for more
/// than the manifests (production's build pins the picture URLs). Precedence
/// per app mirrors `resolve_manifest`: `manifest_override` → production's
/// build's `manifest_json` (falling back to staging's) → none. Returns a map
/// keyed by app id; apps with no resolvable manifest are simply absent.
/// Metadata — individual failures are skipped, never fatal. See the
/// `oxy-app-visual-identity` skill.
pub async fn resolve_manifests_batch(
    db: &DatabaseConnection,
    apps: &[apps::Model],
    environments: &std::collections::HashMap<uuid::Uuid, crate::env_resolve::EnvironmentBuilds>,
) -> std::collections::HashMap<uuid::Uuid, OxyAppManifest> {
    use std::collections::{HashMap, HashSet};
    // 1. Collect the build ids we might read (production's, staging's as the
    //    fallback), skipping apps that carry an inline override (no build
    //    lookup needed).
    let mut build_ids: HashSet<uuid::Uuid> = HashSet::new();
    for app in apps {
        if app.manifest_override.is_some() {
            continue;
        }
        if let Some(env) = environments.get(&app.id) {
            build_ids.extend(env.production);
            build_ids.extend(env.staging);
        }
    }
    // 2. One query for every build manifest on the page.
    let builds: HashMap<uuid::Uuid, serde_json::Value> = if build_ids.is_empty() {
        HashMap::new()
    } else {
        app_builds::Entity::find()
            .filter(app_builds::Column::Id.is_in(build_ids))
            .all(db)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter_map(|b| Some((b.id, b.manifest_json?)))
            .collect()
    };
    // 3. Resolve each app from the pre-fetched map (or its override).
    let none = crate::env_resolve::EnvironmentBuilds::default();
    apps.iter()
        .filter_map(|app| {
            let env = environments.get(&app.id).unwrap_or(&none);
            Some((app.id, manifest_from_prefetched(app, env, &builds)?))
        })
        .collect()
}

/// Per-app resolution against a pre-fetched `build_id → manifest_json` map — the
/// same precedence as [`resolve_manifest`], minus the DB round-trip.
fn manifest_from_prefetched(
    app: &apps::Model,
    environments: &crate::env_resolve::EnvironmentBuilds,
    builds: &std::collections::HashMap<uuid::Uuid, serde_json::Value>,
) -> Option<OxyAppManifest> {
    if let Some(raw) = &app.manifest_override {
        return parse_manifest_value(raw.clone()).ok();
    }
    let from_build = |id: Option<uuid::Uuid>| {
        id.and_then(|i| builds.get(&i))
            .and_then(|v| parse_manifest_value(v.clone()).ok())
    };
    from_build(environments.production).or_else(|| from_build(environments.staging))
}

// ── Channel selection ────────────────────────────────────────────────────────

/// `on_staging` is the caller's whole staging decision — a request on the app's
/// staging host from a viewer who may open it (`may_open_non_production`).
/// Everywhere else the published channel answers, or the draft for an app that
/// was never published.
pub fn pick_channel_for(app: &apps::Model, on_staging: bool) -> crate::sync::Channel {
    use crate::sync::Channel;
    if on_staging {
        return Channel::Draft;
    }
    if app.published_at.is_some() {
        Channel::Published
    } else {
        Channel::Draft
    }
}

/// Art must be a plain relative path inside the bundle — reject anything
/// that could escape the bundle dir or point off-origin.
///
/// Public so the seeded example bundle is checked by THIS predicate
/// rather than a copy of it (`cli::commands::seed_apps`): a copy would drift,
/// and then the example would either break silently or fail on a rule the
/// launcher doesn't actually apply.
pub fn safe_relative_art_path(p: &str) -> bool {
    !p.is_empty()
        && !p.starts_with('/')
        && !p.contains("..")
        && !p.contains("://")
        && !p.contains('\\')
        && !p.contains('?')
        && !p.contains('#')
}

#[cfg(test)]
mod card_metadata_tests {
    use super::*;

    #[test]
    fn manifest_parses_description_and_ask_block() {
        let json = serde_json::json!({
            "schemaVersion": 2,
            "slug": "site-scout",
            "description": "Find your next location",
            "art": "card.png",
            "ask": {
                "agent": "agents/restaurant_analyst.agentic.yml",
                "suggestedQuestions": ["Why does Pleasanton rank #1?"]
            }
        });
        let m: OxyAppManifest = serde_json::from_value(json).unwrap();
        assert_eq!(m.description.as_deref(), Some("Find your next location"));
        assert_eq!(m.art.as_deref(), Some("card.png"));
        let ask = m.ask.expect("ask block");
        assert_eq!(
            ask.agent.as_deref(),
            Some("agents/restaurant_analyst.agentic.yml")
        );
        assert_eq!(
            ask.suggested_questions,
            vec!["Why does Pleasanton rank #1?"]
        );
    }

    #[test]
    fn manifest_without_card_metadata_still_parses() {
        let json = serde_json::json!({ "schemaVersion": 2, "slug": "bare" });
        let m: OxyAppManifest = serde_json::from_value(json).unwrap();
        assert!(m.description.is_none());
        assert!(m.ask.is_none());
        assert!(m.art.is_none());
    }
}

/// The `oltpChanges` reader. `manifest-validate.test.ts` has the same cases,
/// under the same names, and expects the same sentences.
#[cfg(test)]
mod oltp_changes_tests {
    use super::*;
    use serde_json::json;

    /// A manifest with `migrations`, declaring `oltp_changes`.
    fn manifest(oltp_changes: serde_json::Value) -> serde_json::Value {
        json!({
            "schemaVersion": 2,
            "slug": "leasing",
            "migrations": { "dir": "migrations" },
            "oltpChanges": oltp_changes,
        })
    }

    /// `oltpChanges` declaring the one table `table`, with `spec`.
    fn one_table(table: &str, spec: serde_json::Value) -> serde_json::Value {
        let mut tables = serde_json::Map::new();
        tables.insert(table.to_string(), spec);
        manifest(json!({ "tables": tables }))
    }

    fn refusal(raw: &serde_json::Value) -> String {
        oltp_changes_from_manifest(raw)
            .expect_err(&format!("must refuse {raw}"))
            .to_string()
    }

    #[test]
    fn absent_is_none() {
        let bare = json!({ "schemaVersion": 2, "slug": "leasing" });
        assert_eq!(oltp_changes_from_manifest(&bare).expect("reads"), None);
        // An explicit `null` declares nothing, as for `migrations`, and needs
        // no `migrations` beside it.
        let declared_null = json!({ "schemaVersion": 2, "slug": "leasing", "oltpChanges": null });
        assert_eq!(
            oltp_changes_from_manifest(&declared_null).expect("reads"),
            None
        );
    }

    /// The typed manifest, which serving and the launcher parse, never reads
    /// the block: a key this binary does not know there (a later release's, or
    /// a misspelt override) must not fail the whole parse. Only publish and
    /// promote read it, strictly, through `oltp_changes_from_manifest`.
    #[test]
    fn the_typed_manifest_ignores_the_block() {
        let raw = manifest(json!({
            "tables": { "leases": { "exclude": ["a"], "mode": "from-a-later-release" } }
        }));
        let typed: Result<OxyAppManifest, _> = serde_json::from_value(raw.clone());
        assert!(typed.is_ok(), "{typed:?}");
        assert!(oltp_changes_from_manifest(&raw).is_err());
    }

    #[test]
    fn a_table_with_exclude_parses() {
        let raw = manifest(json!({
            "tables": {
                "leases": {},
                "landlords": { "exclude": ["contact_email", "contact_phone"] }
            }
        }));
        let config = oltp_changes_from_manifest(&raw)
            .expect("reads")
            .expect("declares");
        assert_eq!(
            config.tables.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["landlords", "leases"]
        );
        assert!(config.tables["leases"].exclude.is_empty());
        assert_eq!(
            config.tables["landlords"].exclude,
            vec!["contact_email", "contact_phone"]
        );
        // The longest name the oltp crate accepts is accepted here too.
        let longest = "a".repeat(MAX_NAME_LEN);
        assert!(oltp_changes_from_manifest(&one_table(&longest, json!({}))).is_ok());
    }

    #[test]
    fn empty_tables_is_refused() {
        assert_eq!(
            refusal(&manifest(json!({ "tables": {} }))),
            "`oltpChanges.tables` is empty: name at least one table to capture, or remove the \
             `oltpChanges` block"
        );
        // No `tables` at all, or not a map, is the shape error.
        for shape in [json!({}), json!({ "tables": ["leases"] }), json!("leases")] {
            let message = refusal(&manifest(shape));
            assert!(
                message.starts_with("the `oltpChanges` block in oxy-app.json is not usable"),
                "{message}"
            );
        }
    }

    #[test]
    fn a_bad_identifier_is_refused() {
        assert_eq!(
            refusal(&one_table("Leases", json!({}))),
            "`oltpChanges.tables` names \"Leases\", which is not a table name the platform \
             accepts: use 1-56 characters, a lowercase letter then lowercase letters, digits \
             or underscores"
        );
        assert_eq!(
            refusal(&one_table(
                "landlords",
                json!({ "exclude": ["contact-email"] })
            )),
            "`oltpChanges.tables.landlords.exclude` names \"contact-email\", which is not a \
             column name the platform accepts: use 1-56 characters, a lowercase letter then \
             lowercase letters, digits or underscores"
        );
        let too_long = "a".repeat(MAX_NAME_LEN + 1);
        for table in [
            "",
            "1leases",
            "_seq",
            "lea-ses",
            "app_leasing.leases",
            "a\"b",
            "leasés",
            too_long.as_str(),
        ] {
            let message = refusal(&one_table(table, json!({})));
            assert!(
                message.starts_with("`oltpChanges.tables` names "),
                "{table:?}: {message}"
            );
            let message = refusal(&one_table("leases", json!({ "exclude": [table] })));
            assert!(
                message.starts_with("`oltpChanges.tables.leases.exclude` names "),
                "{table:?}: {message}"
            );
        }
    }

    #[test]
    fn a_duplicate_exclude_is_refused() {
        let raw = one_table(
            "landlords",
            json!({ "exclude": ["contact_email", "contact_phone", "contact_email"] }),
        );
        assert_eq!(
            refusal(&raw),
            "`oltpChanges.tables.landlords.exclude` lists \"contact_email\" more than once"
        );
    }

    #[test]
    fn oltp_changes_without_migrations_is_refused() {
        let block = json!({ "tables": { "leases": {} } });
        for raw in [
            json!({ "schemaVersion": 2, "slug": "leasing", "oltpChanges": block }),
            json!({
                "schemaVersion": 2, "slug": "leasing", "migrations": null, "oltpChanges": block
            }),
            // Airhouse migrations create no OLTP table.
            json!({
                "schemaVersion": 2,
                "slug": "leasing",
                "airhouseMigrations": { "dir": "airhouse-migrations" },
                "oltpChanges": block
            }),
        ] {
            assert_eq!(
                refusal(&raw),
                "`oltpChanges` needs a `migrations` block: a captured table must be created by \
                 the app's own migrations"
            );
        }
    }

    /// A misspelt `exclude` would otherwise capture what it meant to leave out.
    #[test]
    fn an_unknown_key_is_refused() {
        let message = refusal(&one_table(
            "landlords",
            json!({ "exlude": ["contact_email"] }),
        ));
        assert!(message.contains("unknown field `exlude`"), "{message}");
        let message = refusal(&manifest(
            json!({ "tables": { "leases": {} }, "mode": "all" }),
        ));
        assert!(message.contains("unknown field `mode`"), "{message}");
    }
}

/// A tar entry path is safe iff it's relative and contains no `..`
/// component. The check is the kernel's, since a compile's commit tarball
/// applies it too; re-exported here under its old name.
pub use oxy_shared::utils::relative_path::is_safe_relative_path;
