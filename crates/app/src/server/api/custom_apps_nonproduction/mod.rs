//! `nonProduction` in `oxy-app.json`: what a non-production environment of a
//! custom app writes instead of production
//! (`internal-docs/2026-09-10-custom-app-environments-design.md` §4.4).
//!
//! ```jsonc
//! "nonProduction": {
//!   "destinations": { "clickhouse": "clickhouse_staging" }
//! }
//! ```
//!
//! `destinations` maps a database a function writes in production to the one
//! staging writes instead. Outside production `ctx.warehouse.{insert,exec,
//! upsert}` and `ctx.tx` naming `clickhouse` run against `clickhouse_staging`
//! (`env_policy::Target::MappedDestination`); a database with no mapping is
//! held, never written.
//!
//! Two readers, with opposite lenience, as `custom_apps_manifest` has for
//! retention and migrations:
//!
//! - [`config`] is **strict** and runs at publish: a block that does not
//!   parse fails the publish, and [`check_mapping`] refuses a mapping whose
//!   database is not configured or whose credential resolves to production's
//!   same host and user.
//! - [`destinations_from_build_manifest`] is the runtime reader and degrades
//!   to "no mapping", which holds every staging write to a customer
//!   warehouse — the safe direction.

mod identity;
pub mod publish;
pub mod staging_task;
pub mod staging_task_executor;

use std::collections::BTreeMap;

use oxy::config::model::Database;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub use identity::{
    Identity, Unresolved, configured_database, configured_names, identity, mapping_refusal,
};

/// The `nonProduction` block. Unknown keys are ignored, so a later
/// `schedules` list is additive.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NonProductionConfig {
    /// Production database → the database a non-production write lands in.
    #[serde(default)]
    pub destinations: BTreeMap<String, String>,
}

/// Why a publish refuses the `nonProduction` block.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MappingRefusal {
    /// The author's to fix: the block, a name, or a mapping onto production.
    #[error("{0}")]
    Author(String),
    /// Nothing to check it against yet — the workspace has no readable
    /// compiled config. Worth retrying once it compiles.
    #[error("{0}")]
    Unchecked(String),
}

/// Read the `nonProduction` block strictly. `Ok(None)`: none declared.
pub fn config(
    manifest_json: Option<&serde_json::Value>,
) -> Result<Option<NonProductionConfig>, MappingRefusal> {
    let Some(raw) = manifest_json.and_then(|m| m.get("nonProduction")) else {
        return Ok(None);
    };
    if raw.is_null() {
        return Ok(None);
    }
    let config: NonProductionConfig = serde_json::from_value(raw.clone()).map_err(|e| {
        MappingRefusal::Author(format!(
            "the `nonProduction` block in oxy-app.json is not usable ({e}); `destinations` maps \
             a production database to the one staging writes instead, e.g. \"nonProduction\": \
             {{ \"destinations\": {{ \"clickhouse\": \"clickhouse_staging\" }} }}"
        ))
    })?;
    for (from, to) in &config.destinations {
        if from.trim().is_empty() || to.trim().is_empty() {
            return Err(MappingRefusal::Author(format!(
                "nonProduction.destinations maps {from:?} to {to:?}; both must name a database"
            )));
        }
    }
    Ok(Some(config))
}

/// The running build's mapping, for the host. A block that does not parse is
/// logged and read as none, which holds every mapped write.
pub fn destinations_from_build_manifest(
    manifest_json: Option<&serde_json::Value>,
    app_id: Uuid,
) -> BTreeMap<String, String> {
    match config(manifest_json) {
        Ok(config) => config.map(|c| c.destinations).unwrap_or_default(),
        Err(e) => {
            tracing::warn!(
                %app_id,
                "oxy-app.json nonProduction could not be read ({e}); staging writes to a customer \
                 warehouse are held"
            );
            BTreeMap::new()
        }
    }
}

/// Refuse a mapping the publish should not ship: a database the workspace does
/// not configure, a mapping onto itself or onto the workspace's own Airhouse,
/// or one whose credential resolves to production's same host and user
/// ([`mapping_refusal`]; the host repeats it on every staging write, failing
/// closed where publish compares an unresolved secret by name).
///
/// **A heuristic, not proof.** Two names can reach one database through
/// different hosts or users (a CNAME, a second role with the same grants), and
/// the owner of the mapping is responsible for the credential behind it. What
/// this catches is the likely mistake: a staging entry copied from
/// production's and never pointed anywhere else.
pub async fn check_mapping(
    project_id: Uuid,
    manifest_json: Option<&serde_json::Value>,
) -> Result<(), MappingRefusal> {
    let Some(config) = config(manifest_json)? else {
        return Ok(());
    };
    if config.destinations.is_empty() {
        return Ok(());
    }
    let databases = workspace_databases(project_id).await?;
    let secrets = oxy::adapters::secrets::SecretsManager::from_database_with_env_fallback(
        oxy::service::secret_manager::SecretManagerService::new(project_id),
    )
    .map_err(|e| {
        MappingRefusal::Unchecked(format!("could not read the workspace's secrets: {e}"))
    })?;
    for (from, to) in &config.destinations {
        configured(&databases, from)?;
        configured(&databases, to)?;
        let mapping = &config.destinations;
        if let Some(refusal) = chain_refusal(
            mapping,
            from,
            to,
            &databases,
            &secrets,
            Unresolved::CompareByName,
        )
        .await
        {
            return Err(MappingRefusal::Author(refusal));
        }
    }
    Ok(())
}

/// Why staging may not write `to` in place of `from` under the whole
/// `mapping`, or `None` — at publish and, with [`Unresolved::Refuse`], on every
/// staging write.
///
/// Refused: a `to` that is itself a key of the mapping (a chain: a production
/// database written as another's staging copy), and a `to` that
/// [`mapping_refusal`] refuses against **any** production database the mapping
/// names, not only `from` — a staging copy of one database must not be
/// another production database under a third name.
pub async fn chain_refusal(
    mapping: &BTreeMap<String, String>,
    from: &str,
    to: &str,
    databases: &[Database],
    secrets: &oxy::adapters::secrets::SecretsManager,
    unresolved: Unresolved,
) -> Option<String> {
    if to != from && mapping.contains_key(to) {
        return Some(format!(
            "nonProduction.destinations maps `{from}` to `{to}`, which the same block maps as a \
             production database of its own — staging would write a production database. Map \
             `{from}` to a database no mapping starts from"
        ));
    }
    let find = |name: &str| databases.iter().find(|d| d.name == name);
    let Some(staging) = find(to) else {
        return Some(format!("`{to}` is not configured for this project"));
    };
    for production_name in mapping.keys() {
        let Some(production) = find(production_name) else {
            return Some(format!(
                "`{production_name}` is not configured for this project"
            ));
        };
        let Some(refusal) = mapping_refusal(production, staging, secrets, unresolved).await else {
            continue;
        };
        if production_name == from {
            return Some(refusal);
        }
        return Some(format!(
            "`{to}`, staging's copy of `{from}`, is not separate from production database \
             `{production_name}`: {refusal}"
        ));
    }
    None
}

fn configured<'a>(databases: &'a [Database], name: &str) -> Result<&'a Database, MappingRefusal> {
    databases.iter().find(|d| d.name == name).ok_or_else(|| {
        MappingRefusal::Author(format!(
            "nonProduction.destinations names `{name}`, which is not a database of this \
             workspace's config.yml"
        ))
    })
}

/// The workspace's databases from its compiled config — the revision the
/// serve fleet reads.
async fn workspace_databases(project_id: Uuid) -> Result<Vec<Database>, MappingRefusal> {
    let unchecked = |why: String| {
        MappingRefusal::Unchecked(format!(
            "nonProduction.destinations could not be checked: {why}; publish again once the \
             workspace has compiled"
        ))
    };
    let config = crate::server::api::compiled_reader::resolve_workspace_config(project_id, None)
        .await
        .map_err(|e| unchecked(format!("the workspace config could not be read ({e})")))?
        .ok_or_else(|| unchecked("the workspace has no compiled config".to_string()))?;
    let databases = config
        .get("databases")
        .cloned()
        .unwrap_or(serde_json::Value::Array(vec![]));
    serde_json::from_value(databases)
        .map_err(|e| unchecked(format!("its databases do not parse ({e})")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_absent_or_null_block_maps_nothing() {
        assert_eq!(config(None), Ok(None));
        assert_eq!(config(Some(&json!({ "slug": "a" }))), Ok(None));
        assert_eq!(config(Some(&json!({ "nonProduction": null }))), Ok(None));
    }

    #[test]
    fn destinations_parse_and_other_keys_are_ignored() {
        let manifest = json!({ "nonProduction": {
            "destinations": { "clickhouse": "clickhouse_staging" },
            "schedules": ["refresh"],
        }});
        let parsed = config(Some(&manifest)).unwrap().unwrap();
        assert_eq!(
            parsed.destinations.get("clickhouse").map(String::as_str),
            Some("clickhouse_staging")
        );
    }

    #[test]
    fn a_malformed_block_fails_the_publish_and_maps_nothing_at_runtime() {
        for bad in [
            json!({ "nonProduction": { "destinations": ["clickhouse"] } }),
            json!({ "nonProduction": { "destinations": { "clickhouse": 1 } } }),
            json!({ "nonProduction": { "destinations": { "clickhouse": " " } } }),
            json!({ "nonProduction": "staging" }),
        ] {
            assert!(
                matches!(config(Some(&bad)), Err(MappingRefusal::Author(_))),
                "{bad}"
            );
            assert!(destinations_from_build_manifest(Some(&bad), Uuid::nil()).is_empty());
        }
    }
}
