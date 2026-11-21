//! The `.databases/` files a warehouse sync produces, rendered in memory.
//!
//! `POST /api/{workspace_id}/databases/sync` returns these and `oxyc sync`
//! writes them into the caller's checkout, so the file format lives here and
//! nowhere else: `oxyc` never reimplements it. Nothing in this module touches
//! the disk, which is what lets a fleet node with no working copy render a
//! sync from the served revision's compiled config.
//!
//! Layout, relative to the workspace root:
//!
//! - `.databases/<database>/<dataset>.sql`: the dataset's DDL, where the
//!   warehouse exposes it;
//! - `.databases/<database>/<dataset>/models/<table>.schema.yml`: one semantic
//!   model per table, serialized from [`SemanticModels`].
//!
//! A `models/` directory belongs to the sync wholly (a stale model in it is
//! removed by an overwriting sync); the database directory does not.

use std::collections::HashMap;

use serde::Serialize;

use crate::{
    adapters::secrets::SecretsManager,
    config::{
        ConfigManager, ResolveWorkspaceFile,
        constants::{DATABASE_SEMANTIC_PATH, SEMANTIC_MODEL_PATH},
        model::{Database, DatabaseType, SemanticModels},
    },
    semantic::loader::SchemaLoader,
    service::sync::SyncFilter,
};
use oxy_shared::errors::OxyError;

/// One file a sync writes, `path` relative to the workspace root (always under
/// `.databases/`, `/`-separated).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct RenderedFile {
    pub path: String,
    pub content: String,
}

/// The DDL file for one dataset.
pub fn ddl_path(database: &str, dataset: &str) -> String {
    format!("{DATABASE_SEMANTIC_PATH}/{database}/{dataset}.sql")
}

/// The semantic model file for one table.
pub fn model_path(database: &str, dataset: &str, table: &str) -> String {
    format!(
        "{DATABASE_SEMANTIC_PATH}/{database}/{dataset}/{SEMANTIC_MODEL_PATH}/{table}.schema.yml"
    )
}

/// Whether a sync can introspect this database at all. `airhouse_managed`,
/// `airhouse` and `mysql` have no schema loader: a sync of one is refused by
/// design, which a caller reports as skipped rather than as a failure.
pub fn is_syncable(database: &Database) -> bool {
    matches!(
        database.database_type,
        DatabaseType::ClickHouse(_)
            | DatabaseType::Bigquery(_)
            | DatabaseType::Snowflake(_)
            | DatabaseType::DuckDB(_)
            | DatabaseType::MotherDuck(_)
            | DatabaseType::Postgres(_)
            | DatabaseType::PostgresManaged(_)
            | DatabaseType::Redshift(_)
            | DatabaseType::DOMO(_)
    )
}

/// Turn what a [`SchemaLoader`] read into files. Pure: the warehouse has
/// already been asked.
pub fn render_files(
    database: &str,
    models: HashMap<String, HashMap<String, SemanticModels>>,
    ddls: HashMap<String, String>,
) -> Result<Vec<RenderedFile>, OxyError> {
    let mut files = Vec::new();
    for (dataset, tables) in models {
        for (table, model) in tables {
            let content = serde_yaml::to_string(&model).map_err(|err| {
                OxyError::SerializerError(format!("Failed to serialize semantic model: {err}"))
            })?;
            files.push(RenderedFile {
                path: model_path(database, &dataset, &table),
                content,
            });
        }
    }
    for (dataset, ddl) in ddls {
        files.push(RenderedFile {
            path: ddl_path(database, &dataset),
            content: ddl,
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// Introspect one database and render its files.
pub async fn render_database<S>(
    database: &Database,
    config: &ConfigManager<S>,
    secrets_manager: &SecretsManager,
) -> Result<Vec<RenderedFile>, OxyError>
where
    ConfigManager<S>: ResolveWorkspaceFile,
{
    let loader = SchemaLoader::from_database(database, config, secrets_manager).await?;
    let models = loader.load_schema().await?;
    let ddls = loader.load_ddl().await?;
    render_files(&database.name, models, ddls)
}

/// The databases a filter selects: the one it names (narrowed to its datasets
/// or tables), or every configured database. An unknown name is an error
/// before any warehouse is asked.
pub fn select_databases<S>(
    config: &ConfigManager<S>,
    filter: SyncFilter,
) -> Result<Vec<Database>, OxyError> {
    let (db_filter, schema_tables) = filter.into_filter();
    match db_filter {
        Some(name) => {
            let database = config.resolve_database(&name)?;
            if schema_tables.is_empty() {
                Ok(vec![database])
            } else {
                Ok(vec![database.with_schema_tables(schema_tables)])
            }
        }
        None => Ok(config.list_databases()),
    }
}

/// Render every database the filter selects, one after another. All or
/// nothing: the first database that fails fails the whole render, named in
/// the error, so a caller never writes half a sync believing it whole. The
/// route asks for one database per request; `oxyc sync` fans out itself.
pub async fn render_databases<S>(
    config: &ConfigManager<S>,
    secrets_manager: &SecretsManager,
    filter: SyncFilter,
) -> Result<Vec<RenderedFile>, OxyError>
where
    ConfigManager<S>: ResolveWorkspaceFile,
{
    let mut files = Vec::new();
    for database in select_databases(config, filter)? {
        let rendered = render_database(&database, config, secrets_manager)
            .await
            .map_err(|err| with_database(&database.name, err))?;
        files.extend(rendered);
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// Prefix the database's name onto an error, keeping its kind (the route maps
/// the kind to a status).
fn with_database(database: &str, err: OxyError) -> OxyError {
    let prefix = format!("database '{database}': ");
    match err {
        OxyError::ConfigurationError(m) => OxyError::ConfigurationError(format!("{prefix}{m}")),
        OxyError::ArgumentError(m) => OxyError::ArgumentError(format!("{prefix}{m}")),
        OxyError::DBError(m) => OxyError::DBError(format!("{prefix}{m}")),
        OxyError::RuntimeError(m) => OxyError::RuntimeError(format!("{prefix}{m}")),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::Dimension;

    fn model(table: &str) -> SemanticModels {
        SemanticModels {
            table: table.to_string(),
            database: "warehouse".to_string(),
            description: String::new(),
            entities: vec![],
            dimensions: vec![Dimension {
                name: "id".to_string(),
                description: None,
                synonyms: None,
                sample: vec![],
                data_type: Some("INTEGER".to_string()),
                is_partition_key: None,
            }],
            measures: vec![],
            database_name: String::new(),
        }
    }

    /// The layout `oxyc sync` writes and the old `oxy sync` wrote, pinned:
    /// a caller (and every customer checkout) depends on these paths.
    #[test]
    fn renders_the_databases_layout() {
        let models = HashMap::from([(
            "analytics".to_string(),
            HashMap::from([("orders".to_string(), model("analytics.orders"))]),
        )]);
        let ddls = HashMap::from([(
            "analytics".to_string(),
            "CREATE TABLE orders (id INTEGER);\n".to_string(),
        )]);
        let files = render_files("warehouse", models, ddls).unwrap();
        let paths: Vec<_> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                ".databases/warehouse/analytics.sql",
                ".databases/warehouse/analytics/models/orders.schema.yml",
            ]
        );
        assert_eq!(files[0].content, "CREATE TABLE orders (id INTEGER);\n");
        let parsed: SemanticModels = serde_yaml::from_str(&files[1].content).unwrap();
        assert_eq!(parsed.table, "analytics.orders");
        assert_eq!(parsed.dimensions[0].name, "id");
    }

    #[test]
    fn database_name_prefixes_the_error_and_keeps_its_kind() {
        let err = with_database("bq", OxyError::ConfigurationError("bad key".into()));
        assert!(
            matches!(err, OxyError::ConfigurationError(ref m) if m == "database 'bq': bad key")
        );
    }
}
