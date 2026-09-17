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

use entity::{app_builds, apps};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};

use super::custom_apps_storage::{RetentionPolicy, RetentionRule};
use super::custom_apps_sync::Channel;

// ── Manifest schema ──────────────────────────────────────────────────────────

/// Optional Ask-binding declared by the app bundle: which agent the
/// global Ask overlay should bind to on this app's surfaces, plus the
/// suggested-question chips shown on the launcher card.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct OxyAppAskConfig {
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
pub(crate) struct OxyAppMigrationsConfig {
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
pub(crate) fn migrations_config(
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
pub(crate) fn airhouse_migrations_config(
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

/// App-level storage config from `oxy-app.json`.
///
/// Deliberately **not** the same block as the per-function `storage: { read,
/// write }` capability. Those gate what one function may call; retention governs
/// the per-app silo that every function shares, so it can only be stated once,
/// app-wide. Putting it per-function would let two functions declare conflicting
/// lifetimes for the same object.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OxyAppStorageConfig {
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
pub(crate) struct OxyAppManifest {
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
pub(crate) fn retention_policy_from_build_manifest(
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
pub(super) enum ManifestError {
    #[error("no oxy-app.json recorded for this app's build")]
    NotFound,
    #[error("oxy-app.json could not be read: {0}")]
    Io(String),
    #[error("oxy-app.json is not valid JSON: {0}")]
    Parse(String),
    #[error("oxy-app.json schemaVersion {0} is not supported; expected 2")]
    UnsupportedSchema(u32),
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
pub(super) async fn resolve_manifest(
    db: &DatabaseConnection,
    app: &apps::Model,
    channel: Channel,
) -> Result<OxyAppManifest, ManifestError> {
    if let Some(raw) = &app.manifest_override {
        return parse_manifest_value(raw.clone());
    }
    let build_pk = match channel {
        Channel::Draft => app.draft_build_id,
        Channel::Published => app.published_build_id,
    }
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
/// (avoids an N+1 per-page). Precedence per app mirrors `resolve_manifest`:
/// `manifest_override` → the published build's `manifest_json` (falling back to
/// the draft build) → local-folder disk → none. Returns a map keyed by app id;
/// apps with no resolvable manifest are simply absent. Metadata — individual
/// failures are skipped, never fatal. See the `oxy-app-visual-identity` skill.
pub(super) async fn resolve_manifests_batch(
    db: &DatabaseConnection,
    apps: &[apps::Model],
) -> std::collections::HashMap<uuid::Uuid, OxyAppManifest> {
    use std::collections::{HashMap, HashSet};
    // 1. Collect the build ids we might read (published + draft fallback),
    //    skipping apps that carry an inline override (no build lookup needed).
    let mut build_ids: HashSet<uuid::Uuid> = HashSet::new();
    for app in apps {
        if app.manifest_override.is_some() {
            continue;
        }
        build_ids.extend(app.published_build_id);
        build_ids.extend(app.draft_build_id);
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
    apps.iter()
        .filter_map(|app| Some((app.id, manifest_from_prefetched(app, &builds)?)))
        .collect()
}

/// Per-app resolution against a pre-fetched `build_id → manifest_json` map — the
/// same precedence as [`resolve_manifest`], minus the DB round-trip.
fn manifest_from_prefetched(
    app: &apps::Model,
    builds: &std::collections::HashMap<uuid::Uuid, serde_json::Value>,
) -> Option<OxyAppManifest> {
    if let Some(raw) = &app.manifest_override {
        return parse_manifest_value(raw.clone()).ok();
    }
    let from_build = |id: Option<uuid::Uuid>| {
        id.and_then(|i| builds.get(&i))
            .and_then(|v| parse_manifest_value(v.clone()).ok())
    };
    from_build(app.published_build_id).or_else(|| from_build(app.draft_build_id))
}

// ── Channel selection ────────────────────────────────────────────────────────

pub(super) fn pick_channel_for(
    app: &apps::Model,
    is_staff: bool,
    cookie_wants_draft: bool,
) -> super::custom_apps_sync::Channel {
    use super::custom_apps_sync::Channel;
    if is_staff && cookie_wants_draft {
        return Channel::Draft;
    }
    if app.published_at.is_some() {
        Channel::Published
    } else {
        Channel::Draft
    }
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
