//! What a sandbox source may never name: production's QuickBooks grant, and
//! any secret a custom app owns. Checked at save and again when a sample's
//! platform is built.

use std::collections::HashSet;

use agentic_airway::preview::SandboxSource;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use serde_json::Value;
use uuid::Uuid;

use super::SourceRequestError;

/// Refuse `overrides` if it names a reserved var (app-scoped, or declared by a
/// custom app in the workspace), a production QuickBooks var or realm, or is
/// malformed — in that order, so an `apps/…` var answers `reserved_var`.
pub async fn check_sandbox(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    overrides: &SandboxSource,
) -> Result<(), SourceRequestError> {
    // `apps/<app_id>/<KEY>` and every other name with `/` is app-scoped.
    if let Some(var) = overrides.var_names().into_iter().find(|v| v.contains('/')) {
        return Err(SourceRequestError::ReservedVar(var.to_string()));
    }
    let declared = app_declared_keys(db, workspace_id).await?;
    if let Some(var) = overrides
        .var_names()
        .into_iter()
        .find(|v| declared.contains(*v))
    {
        return Err(SourceRequestError::ReservedVar(var.to_string()));
    }
    let production = production_quickbooks(db, workspace_id).await?;
    refuse_production(overrides, &production)?;
    overrides.validate().map_err(SourceRequestError::BadRequest)
}

/// Every key a custom-app manifest in the workspace declares, in any build
/// (and the app's manifest override): each `env` block's keys and each
/// `webhook.secretVar` (comma-split — a rotation holds two), wherever they sit
/// in the manifest.
pub async fn app_declared_keys(
    db: &DatabaseConnection,
    workspace_id: Uuid,
) -> Result<HashSet<String>, SourceRequestError> {
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT b.manifest_json AS m FROM app_builds b JOIN apps a ON a.id = b.app_id \
               WHERE a.project_id = $1 AND b.manifest_json IS NOT NULL \
             UNION ALL SELECT a.manifest_override FROM apps a \
               WHERE a.project_id = $1 AND a.manifest_override IS NOT NULL \
             UNION ALL SELECT f.manifest_json FROM app_functions f JOIN apps a ON a.id = f.app_id \
               WHERE a.project_id = $1 AND f.manifest_json IS NOT NULL",
            [workspace_id.into()],
        ))
        .await?;
    let mut keys = HashSet::new();
    for row in rows {
        let manifest: Value = row.try_get("", "m")?;
        declared_in(&manifest, &mut keys);
    }
    Ok(keys)
}

/// The keys `value` declares (see [`app_declared_keys`]).
pub(super) fn declared_in(value: &Value, keys: &mut HashSet<String>) {
    match value {
        Value::Object(map) => {
            for (key, inner) in map {
                match (key.as_str(), inner) {
                    ("env", Value::Object(env)) => keys.extend(env.keys().cloned()),
                    ("secretVar", Value::String(vars)) => keys.extend(
                        vars.split(',')
                            .map(str::trim)
                            .filter(|v| !v.is_empty())
                            .map(str::to_string),
                    ),
                    _ => {}
                }
                declared_in(inner, keys);
            }
        }
        Value::Array(items) => items.iter().for_each(|v| declared_in(v, keys)),
        _ => {}
    }
}

/// Production's QuickBooks credential var names and company ids: every
/// `*_var` and `realm_id` of the promoted revision's `quickbooks` pipelines.
#[derive(Debug, Default)]
pub struct ProductionQuickBooks {
    pub vars: HashSet<String>,
    pub realms: HashSet<String>,
}

pub async fn production_quickbooks(
    db: &DatabaseConnection,
    workspace_id: Uuid,
) -> Result<ProductionQuickBooks, SourceRequestError> {
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT p.definition FROM airway_pipelines p \
             JOIN workspaces w ON w.current_revision_id = p.revision_id \
             WHERE w.id = $1 AND p.definition->'source'->>'kind' = 'quickbooks'",
            [workspace_id.into()],
        ))
        .await?;
    let mut production = ProductionQuickBooks::default();
    for row in rows {
        let definition: Value = row.try_get("", "definition")?;
        let Some(config) = definition["source"]["config"].as_object() else {
            continue;
        };
        for (key, value) in config {
            match (key.as_str(), value) {
                ("realm_id", Value::String(realm)) => {
                    production.realms.insert(realm.trim().to_string());
                }
                ("realm_id", Value::Number(realm)) => {
                    production.realms.insert(realm.to_string());
                }
                (key, Value::String(var)) if key.ends_with("_var") => {
                    production.vars.insert(var.clone());
                }
                _ => {}
            }
        }
    }
    Ok(production)
}

pub(super) fn refuse_production(
    overrides: &SandboxSource,
    production: &ProductionQuickBooks,
) -> Result<(), SourceRequestError> {
    if let Some(var) = overrides
        .var_names()
        .into_iter()
        .find(|v| production.vars.contains(*v))
    {
        return Err(SourceRequestError::ProductionVar(var.to_string()));
    }
    let realm = overrides.realm_id.trim();
    if production.realms.contains(realm) {
        return Err(SourceRequestError::ProductionRealm(realm.to_string()));
    }
    Ok(())
}
