//! Fast schema/table discovery for the onboarding flow.
//!
//! Unlike `SchemaLoader::load_schema`, which fetches every column for every
//! table (and then writes thousands of `.dimension.yml` files), the inspector
//! issues a single GROUP-BY query per database to return just
//! `{ schema, table, column_count }`. This is cheap enough to surface to the
//! user immediately so they can pick which tables to actually sync. Full
//! column metadata is loaded later, scoped to selected tables only.
//!
//! Snowflake is the worst offender today: 243 schemas → 243
//! `INFORMATION_SCHEMA.COLUMNS` queries (capped at concurrency 10), which
//! takes 5+ minutes. With this module it's a single query.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use crate::adapters::secrets::SecretsManager;
use crate::config::ConfigManager;
use crate::config::ResolveWorkspaceFile;
use crate::config::model::{Database, DatabaseType};
use crate::connector::Connector;
use oxy_shared::errors::OxyError;
use serde::{Deserialize, Serialize};
use serde_arrow::from_record_batch;
use tokio::sync::mpsc;
use utoipa::ToSchema;

mod quoting;

pub use quoting::sql_string_literal;
use quoting::{bigquery_configured_scope, bigquery_dataset};

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DiscoveredTable {
    pub name: String,
    pub column_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DiscoveredSchema {
    pub schema: String,
    pub tables: Vec<DiscoveredTable>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct InspectionResult {
    pub schemas: Vec<DiscoveredSchema>,
    pub schema_count: u32,
    pub table_count: u32,
    pub elapsed_ms: u64,
}

/// Schema-only summary for the fast "schema-first" discovery flow. Returns
/// just schema names + per-schema table counts — no per-table column counts,
/// so the query is a simple `COUNT(*)` over `INFORMATION_SCHEMA.TABLES`
/// rather than a grouped scan of every column.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SchemaSummary {
    pub schema: String,
    pub table_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SchemaListResult {
    pub schemas: Vec<SchemaSummary>,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SchemaTablesResult {
    pub schema: String,
    pub tables: Vec<DiscoveredTable>,
    pub elapsed_ms: u64,
}

/// Streaming progress events for the inspect SSE endpoint. Mirrors the shape
/// of `ConnectionTestEvent` so the frontend can use the same handling pattern.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InspectEvent {
    Progress { message: String },
    Complete { result: InspectionResult },
    Error { message: String },
}

#[derive(Debug, Deserialize)]
struct InspectRecord {
    #[serde(alias = "table_schema", alias = "TABLE_SCHEMA")]
    schema: String,
    #[serde(alias = "table_name", alias = "TABLE_NAME")]
    table: String,
    #[serde(
        alias = "column_count",
        alias = "COLUMN_COUNT",
        deserialize_with = "deserialize_count"
    )]
    column_count: i64,
}

#[derive(Debug, Deserialize)]
struct SchemaSummaryRecord {
    #[serde(alias = "table_schema", alias = "TABLE_SCHEMA")]
    schema: String,
    #[serde(
        alias = "table_count",
        alias = "TABLE_COUNT",
        deserialize_with = "deserialize_count"
    )]
    table_count: i64,
}

#[derive(Debug, Deserialize)]
struct TableSummaryRecord {
    #[serde(alias = "table_name", alias = "TABLE_NAME")]
    table: String,
    #[serde(
        alias = "column_count",
        alias = "COLUMN_COUNT",
        deserialize_with = "deserialize_count"
    )]
    column_count: i64,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum AnyInt {
    I64(i64),
    U64(u64),
    I32(i32),
    U32(u32),
}

/// Tolerate the various integer widths warehouses return for `COUNT(*)`
/// (Snowflake/BigQuery → Int64, ClickHouse → UInt64, DuckDB → Int64).
fn deserialize_count<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: serde::de::Deserializer<'de>,
{
    let value = AnyInt::deserialize(deserializer)?;
    Ok(match value {
        AnyInt::I64(v) => v,
        AnyInt::U64(v) => v as i64,
        AnyInt::I32(v) => v as i64,
        AnyInt::U32(v) => v as i64,
    })
}

/// Inspect a database for its schemas and tables (with per-table column
/// counts). Emits one `Progress` event before the query, then returns the
/// aggregated result.
pub async fn inspect_database<S>(
    database: &Database,
    config: &ConfigManager<S>,
    secrets_manager: &SecretsManager,
    progress_tx: Option<mpsc::Sender<InspectEvent>>,
) -> Result<InspectionResult, OxyError>
where
    ConfigManager<S>: ResolveWorkspaceFile,
{
    crate::connector::reject_airhouse_managed_for_system_path(
        config,
        &database.name,
        "Schema inspection",
    )?;
    let start = Instant::now();
    let connector = Arc::new(
        Connector::from_database(
            &database.name,
            config,
            secrets_manager,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await?,
    );

    if let Some(tx) = &progress_tx {
        let _ = tx
            .send(InspectEvent::Progress {
                message: format!("Discovering schemas and tables for {}...", database.name),
            })
            .await;
    }

    let queries = build_inspect_queries(database)?;
    if queries.is_empty() {
        return Err(OxyError::ConfigurationError(format!(
            "Cannot inspect {}: no datasets are configured. \
             For BigQuery, add at least one `dataset` or `datasets` entry in config.yml.",
            database.name
        )));
    }

    let mut records: Vec<InspectRecord> = Vec::new();
    for query in queries {
        tracing::debug!("Running inspect query: {}", query);
        let (record_batches, _schema) = connector.run_query_with_limit(&query, None).await?;
        for record_batch in record_batches {
            let parsed: Vec<InspectRecord> = from_record_batch(&record_batch).map_err(|e| {
                OxyError::RuntimeError(format!("Failed to parse inspect results: {e}"))
            })?;
            records.extend(parsed);
        }
    }

    let mut by_schema: HashMap<String, Vec<DiscoveredTable>> = HashMap::new();
    for record in records {
        by_schema
            .entry(record.schema)
            .or_default()
            .push(DiscoveredTable {
                name: record.table,
                column_count: record.column_count.max(0) as u32,
            });
    }

    let mut schemas: Vec<DiscoveredSchema> = by_schema
        .into_iter()
        .map(|(schema, mut tables)| {
            tables.sort_by(|a, b| a.name.cmp(&b.name));
            DiscoveredSchema { schema, tables }
        })
        .collect();
    schemas.sort_by(|a, b| a.schema.cmp(&b.schema));

    let table_count: u32 = schemas.iter().map(|s| s.tables.len() as u32).sum();
    Ok(InspectionResult {
        schema_count: schemas.len() as u32,
        table_count,
        schemas,
        elapsed_ms: start.elapsed().as_millis() as u64,
    })
}

/// Fast schema-only discovery: returns `[{schema, table_count}]` via a single
/// `INFORMATION_SCHEMA.TABLES` scan per database. Used by the onboarding
/// table picker so the user can expand one schema at a time instead of
/// waiting for the whole warehouse to inspect.
pub async fn inspect_schemas<S>(
    database: &Database,
    config: &ConfigManager<S>,
    secrets_manager: &SecretsManager,
) -> Result<SchemaListResult, OxyError>
where
    ConfigManager<S>: ResolveWorkspaceFile,
{
    crate::connector::reject_airhouse_managed_for_system_path(
        config,
        &database.name,
        "Schema inspection",
    )?;
    let start = Instant::now();
    let connector = Arc::new(
        Connector::from_database(
            &database.name,
            config,
            secrets_manager,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await?,
    );

    let queries = build_schema_summary_queries(database)?;
    if queries.is_empty() {
        return Err(OxyError::ConfigurationError(format!(
            "Cannot inspect {}: no datasets are configured.",
            database.name
        )));
    }

    let mut by_schema: HashMap<String, i64> = HashMap::new();
    for query in queries {
        tracing::debug!("Running schema summary query: {}", query);
        let (record_batches, _schema) = connector.run_query_with_limit(&query, None).await?;
        for record_batch in record_batches {
            let rows: Vec<SchemaSummaryRecord> = from_record_batch(&record_batch).map_err(|e| {
                OxyError::RuntimeError(format!("Failed to parse schema summary: {e}"))
            })?;
            for row in rows {
                *by_schema.entry(row.schema).or_default() += row.table_count;
            }
        }
    }

    let mut schemas: Vec<SchemaSummary> = by_schema
        .into_iter()
        .map(|(schema, table_count)| SchemaSummary {
            schema,
            table_count: table_count.max(0) as u32,
        })
        .collect();
    schemas.sort_by(|a, b| a.schema.cmp(&b.schema));

    Ok(SchemaListResult {
        schemas,
        elapsed_ms: start.elapsed().as_millis() as u64,
    })
}

/// List tables (with column counts) for a single schema. Called lazily when
/// the user expands a schema in the onboarding picker.
pub async fn inspect_schema_tables<S>(
    database: &Database,
    schema_name: &str,
    config: &ConfigManager<S>,
    secrets_manager: &SecretsManager,
) -> Result<SchemaTablesResult, OxyError>
where
    ConfigManager<S>: ResolveWorkspaceFile,
{
    crate::connector::reject_airhouse_managed_for_system_path(
        config,
        &database.name,
        "Schema inspection",
    )?;
    let start = Instant::now();
    // Built before the connector: `schema_name` is a request parameter, and a
    // name the engine's grammar refuses must not cost a connection.
    let query = build_schema_tables_query(database, schema_name)?;
    let connector = Arc::new(
        Connector::from_database(
            &database.name,
            config,
            secrets_manager,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await?,
    );

    tracing::debug!("Running schema-tables query: {}", query);
    let (record_batches, _schema) = connector.run_query_with_limit(&query, None).await?;

    let mut tables: Vec<DiscoveredTable> = Vec::new();
    for record_batch in record_batches {
        let rows: Vec<TableSummaryRecord> = from_record_batch(&record_batch)
            .map_err(|e| OxyError::RuntimeError(format!("Failed to parse schema tables: {e}")))?;
        for row in rows {
            tables.push(DiscoveredTable {
                name: row.table,
                column_count: row.column_count.max(0) as u32,
            });
        }
    }
    tables.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(SchemaTablesResult {
        schema: schema_name.to_string(),
        tables,
        elapsed_ms: start.elapsed().as_millis() as u64,
    })
}

/// One query per database returning `(schema, table_count)` rows. Cheaper
/// than `build_inspect_queries` because it scans `INFORMATION_SCHEMA.TABLES`
/// (small) instead of `COLUMNS` (wide).
fn build_schema_summary_queries(database: &Database) -> Result<Vec<String>, OxyError> {
    match &database.database_type {
        DatabaseType::Snowflake(_) => {
            let datasets = database.datasets();
            let configured: Vec<&String> = datasets.keys().filter(|k| !k.is_empty()).collect();
            let where_clause = if configured.is_empty() {
                "TABLE_SCHEMA <> 'INFORMATION_SCHEMA'".to_string()
            } else {
                let in_list = configured
                    .iter()
                    .map(|s| sql_string_literal(&database.database_type, s))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("TABLE_SCHEMA IN ({in_list})")
            };
            Ok(vec![format!(
                "SELECT TABLE_SCHEMA, COUNT(*) AS TABLE_COUNT
                 FROM INFORMATION_SCHEMA.TABLES
                 WHERE {where_clause}
                 GROUP BY TABLE_SCHEMA"
            )])
        }
        DatabaseType::Bigquery(_) => database
            .datasets()
            .keys()
            .map(|dataset| {
                let scope = bigquery_configured_scope(dataset)?;
                Ok(format!(
                    "SELECT table_schema, COUNT(*) AS table_count
                     FROM `{scope}.INFORMATION_SCHEMA.TABLES`
                     GROUP BY table_schema"
                ))
            })
            .collect(),
        DatabaseType::ClickHouse(_) => {
            let datasets = database.datasets();
            let configured: Vec<&String> = datasets.keys().filter(|k| !k.is_empty()).collect();
            let where_clause = if configured.is_empty() {
                "database NOT IN ('system', 'INFORMATION_SCHEMA', 'information_schema')".to_string()
            } else {
                let in_list = configured
                    .iter()
                    .map(|s| sql_string_literal(&database.database_type, s))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("database IN ({in_list})")
            };
            Ok(vec![format!(
                "SELECT database AS table_schema, count() AS table_count
                 FROM system.tables
                 WHERE {where_clause}
                 GROUP BY database"
            )])
        }
        DatabaseType::DuckDB(_) | DatabaseType::MotherDuck(_) => {
            let datasets = database.datasets();
            let configured: Vec<&String> = datasets.keys().filter(|k| !k.is_empty()).collect();
            let where_clause = if configured.is_empty() {
                "schema_name NOT IN ('information_schema', 'pg_catalog', 'ducklake')".to_string()
            } else {
                let in_list = configured
                    .iter()
                    .map(|s| sql_string_literal(&database.database_type, s))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("schema_name IN ({in_list})")
            };
            Ok(vec![format!(
                "SELECT schema_name AS table_schema, COUNT(*) AS table_count
                 FROM duckdb_tables
                 WHERE {where_clause}
                 GROUP BY schema_name"
            )])
        }
        DatabaseType::Postgres(_)
        | DatabaseType::PostgresManaged(_)
        | DatabaseType::Redshift(_) => Ok(vec![
            "SELECT table_schema, COUNT(*) AS table_count
             FROM information_schema.tables
             WHERE table_schema NOT IN ('pg_catalog', 'information_schema')
             GROUP BY table_schema"
                .to_string(),
        ]),
        DatabaseType::Mysql(_) => Ok(vec![
            "SELECT table_schema, COUNT(*) AS table_count
             FROM information_schema.tables
             WHERE table_schema NOT IN ('information_schema', 'mysql', 'performance_schema', 'sys')
             GROUP BY table_schema"
                .to_string(),
        ]),
        _ => Err(OxyError::ConfigurationError(format!(
            "Schema discovery not yet supported for database type: {:?}",
            database.database_type
        ))),
    }
}

/// One query returning `(table_name, column_count)` rows for a single schema.
///
/// `schema` is a request parameter. Every engine but BigQuery compares it as a
/// string literal, escaped by that engine's rule. BigQuery's
/// `INFORMATION_SCHEMA` is addressed through the dataset, so there the name is
/// part of an identifier: it is a dataset ID or the request is refused.
fn build_schema_tables_query(database: &Database, schema: &str) -> Result<String, OxyError> {
    let lit = sql_string_literal(&database.database_type, schema);
    match &database.database_type {
        DatabaseType::Snowflake(_) => Ok(format!(
            "SELECT TABLE_NAME, COUNT(*) AS COLUMN_COUNT
             FROM INFORMATION_SCHEMA.COLUMNS
             WHERE TABLE_SCHEMA = {lit}
             GROUP BY TABLE_NAME"
        )),
        DatabaseType::Bigquery(_) => {
            let dataset = bigquery_dataset(schema)?;
            Ok(format!(
                "SELECT table_name, COUNT(*) AS column_count
             FROM `{dataset}.INFORMATION_SCHEMA.COLUMNS`
             GROUP BY table_name"
            ))
        }
        DatabaseType::ClickHouse(_) => Ok(format!(
            "SELECT table AS table_name, count() AS column_count
             FROM system.columns
             WHERE database = {lit}
             GROUP BY table"
        )),
        DatabaseType::DuckDB(_) | DatabaseType::MotherDuck(_) => Ok(format!(
            "SELECT table_name, COUNT(*) AS column_count
             FROM duckdb_columns
             WHERE schema_name = {lit}
             GROUP BY table_name"
        )),
        DatabaseType::Postgres(_)
        | DatabaseType::PostgresManaged(_)
        | DatabaseType::Redshift(_)
        | DatabaseType::Mysql(_) => Ok(format!(
            "SELECT table_name, COUNT(*) AS column_count
                 FROM information_schema.columns
                 WHERE table_schema = {lit}
                 GROUP BY table_name"
        )),
        _ => Err(OxyError::ConfigurationError(format!(
            "Schema discovery not yet supported for database type: {:?}",
            database.database_type
        ))),
    }
}

/// One GROUP BY query per database — collapses what the loader does in N
/// per-schema queries down to a single round-trip.
fn build_inspect_queries(database: &Database) -> Result<Vec<String>, OxyError> {
    match &database.database_type {
        DatabaseType::Snowflake(_) => {
            // INFORMATION_SCHEMA in Snowflake is per-database, so a single
            // ungrouped scan covers every schema/table the user can see.
            let datasets = database.datasets();
            let configured: Vec<&String> = datasets.keys().filter(|k| !k.is_empty()).collect();
            let where_clause = if configured.is_empty() {
                "TABLE_SCHEMA <> 'INFORMATION_SCHEMA'".to_string()
            } else {
                let in_list = configured
                    .iter()
                    .map(|s| sql_string_literal(&database.database_type, s))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("TABLE_SCHEMA IN ({in_list})")
            };
            Ok(vec![format!(
                "SELECT TABLE_SCHEMA, TABLE_NAME, COUNT(*) AS COLUMN_COUNT
                 FROM INFORMATION_SCHEMA.COLUMNS
                 WHERE {where_clause}
                 GROUP BY TABLE_SCHEMA, TABLE_NAME"
            )])
        }
        DatabaseType::Bigquery(_) => {
            // BigQuery's INFORMATION_SCHEMA is dataset-scoped, so we need one
            // query per configured dataset. `datasets()` always returns at
            // least `region-us` as a fallback for BigQuery, so this branch
            // never yields an empty vec.
            database
                .datasets()
                .keys()
                .map(|dataset| {
                    let scope = bigquery_configured_scope(dataset)?;
                    Ok(format!(
                        "SELECT table_schema, table_name, COUNT(*) AS column_count
                         FROM `{scope}.INFORMATION_SCHEMA.COLUMNS`
                         GROUP BY table_schema, table_name"
                    ))
                })
                .collect()
        }
        DatabaseType::ClickHouse(_) => {
            let datasets = database.datasets();
            let configured: Vec<&String> = datasets.keys().filter(|k| !k.is_empty()).collect();
            let where_clause = if configured.is_empty() {
                "database NOT IN ('system', 'INFORMATION_SCHEMA', 'information_schema')".to_string()
            } else {
                let in_list = configured
                    .iter()
                    .map(|s| sql_string_literal(&database.database_type, s))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("database IN ({in_list})")
            };
            Ok(vec![format!(
                "SELECT database AS table_schema, table AS table_name, count() AS column_count
                 FROM system.columns
                 WHERE {where_clause}
                 GROUP BY database, table"
            )])
        }
        DatabaseType::DuckDB(_) | DatabaseType::MotherDuck(_) => {
            let datasets = database.datasets();
            let configured: Vec<&String> = datasets.keys().filter(|k| !k.is_empty()).collect();
            let where_clause = if configured.is_empty() {
                "schema_name NOT IN ('information_schema', 'pg_catalog', 'ducklake')".to_string()
            } else {
                let in_list = configured
                    .iter()
                    .map(|s| sql_string_literal(&database.database_type, s))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("schema_name IN ({in_list})")
            };
            Ok(vec![format!(
                "SELECT schema_name AS table_schema, table_name, COUNT(*) AS column_count
                 FROM duckdb_columns
                 WHERE {where_clause}
                 GROUP BY schema_name, table_name"
            )])
        }
        DatabaseType::Postgres(_)
        | DatabaseType::PostgresManaged(_)
        | DatabaseType::Redshift(_) => {
            // Postgres / Redshift (via connectorx). `postgres_managed` is a real
            // Postgres, so it introspects identically — and the analyst role's
            // grants do the filtering for us: information_schema only exposes
            // objects the connected role may see, so `oxy_meta` stays hidden.
            //
            // `datasets()` is empty for
            // these since the config has no schemas list — a single
            // information_schema.columns scan covers the whole connected
            // database.
            Ok(vec![
                "SELECT table_schema, table_name, COUNT(*) AS column_count
                 FROM information_schema.columns
                 WHERE table_schema NOT IN ('pg_catalog', 'information_schema')
                 GROUP BY table_schema, table_name"
                    .to_string(),
            ])
        }
        DatabaseType::Mysql(_) => {
            // MySQL calls schemas "databases" at the connection level but
            // information_schema.columns still exposes them as table_schema.
            Ok(vec![
                "SELECT table_schema, table_name, COUNT(*) AS column_count
                 FROM information_schema.columns
                 WHERE table_schema NOT IN ('information_schema', 'mysql', 'performance_schema', 'sys')
                 GROUP BY table_schema, table_name"
                    .to_string(),
            ])
        }
        // DOMO is single-dataset and exposes metadata through its REST client,
        // not SQL — no inspect path today.
        _ => Err(OxyError::ConfigurationError(format!(
            "Schema inspection not yet supported for database type: {:?}",
            database.database_type
        ))),
    }
}

#[cfg(test)]
mod query_tests;
