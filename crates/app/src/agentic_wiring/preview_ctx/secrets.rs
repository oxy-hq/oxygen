//! Production's QuickBooks token vars never resolve in a preview.
//!
//! Intuit voids a refresh token when it issues the next one, so exactly one
//! component may rotate a grant (see `product-context.md`). A preview that could
//! read the refresh token could rotate it — through an `http_request` step, an
//! agent, anything — and brick production's chain. So every var a `quickbooks`
//! pipeline names for its tokens or client secret resolves to nothing here.
//!
//! The names come from the pipelines of the workspace's **promoted** revision
//! (production's vars) and of the preview's own staging revision (a branch that
//! adds a QuickBooks pipeline over production's vars is exactly the case to
//! catch).

use std::collections::HashSet;

use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement};
use uuid::Uuid;

/// The source-config keys that name a QuickBooks credential.
const TOKEN_VAR_KEYS: &[&str] = &[
    "access_token_var",
    "refresh_token_var",
    "client_secret_var",
    "client_id_var",
];

const QUICKBOOKS_DEFINITIONS: &str = "\
    SELECT p.definition FROM airway_pipelines p \
    WHERE p.definition->'source'->>'kind' = 'quickbooks' \
      AND (p.revision_id = $2 \
           OR p.revision_id = (SELECT w.current_revision_id FROM workspaces w WHERE w.id = $1))";

pub(super) async fn production_token_vars(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    staging_revision_id: Uuid,
) -> Result<HashSet<String>, DbErr> {
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            QUICKBOOKS_DEFINITIONS,
            [workspace_id.into(), staging_revision_id.into()],
        ))
        .await?;
    let mut vars = HashSet::new();
    for row in rows {
        let definition: serde_json::Value = row.try_get("", "definition")?;
        vars.extend(token_vars(&definition));
    }
    Ok(vars)
}

/// The credential var names one pipeline definition declares.
pub(super) fn token_vars(definition: &serde_json::Value) -> Vec<String> {
    let config = &definition["source"]["config"];
    TOKEN_VAR_KEYS
        .iter()
        .filter_map(|key| config.get(*key).and_then(|v| v.as_str()))
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect()
}
