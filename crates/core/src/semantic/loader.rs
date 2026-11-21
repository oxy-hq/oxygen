use std::{collections::HashMap, sync::Arc};

use crate::adapters::secrets::SecretsManager;
use crate::connector::DOMO;
use crate::{
    config::{
        ConfigManager,
        model::{Database, DatabaseType, Dimension, SemanticModels},
    },
    connector::Connector,
};
use oxy_shared::errors::OxyError;

use crate::config::ResolveWorkspaceFile;
use futures::StreamExt;
use itertools::Itertools;
use serde::Deserialize;
use serde_arrow::from_record_batch;
use slugify::slugify;

pub struct ColumnNames {
    dataset: String,
    table: String,
    column: String,
    data_type: String,
    is_partitioning_column: String,
    ddl: String,
    description: String,
}

impl Default for ColumnNames {
    fn default() -> Self {
        Self {
            dataset: "table_schema".to_string(),
            table: "table_name".to_string(),
            column: "column_name".to_string(),
            data_type: "data_type".to_string(),
            is_partitioning_column: "is_partitioning_column".to_string(),
            ddl: "ddl".to_string(),
            description: "column_comment".to_string(),
        }
    }
}

impl ColumnNames {
    pub fn with_dataset(mut self, dataset: &str) -> Self {
        self.dataset = dataset.to_string();
        self
    }

    pub fn with_table(mut self, table: &str) -> Self {
        self.table = table.to_string();
        self
    }

    pub fn with_column(mut self, column: &str) -> Self {
        self.column = column.to_string();
        self
    }

    pub fn with_data_type(mut self, data_type: &str) -> Self {
        self.data_type = data_type.to_string();
        self
    }

    pub fn with_is_partitioning_column(mut self, is_partitioning_column: &str) -> Self {
        self.is_partitioning_column = is_partitioning_column.to_string();
        self
    }

    pub fn with_ddl(mut self, ddl: &str) -> Self {
        self.ddl = ddl.to_string();
        self
    }

    pub fn with_description(mut self, description: &str) -> Self {
        self.description = description.to_string();
        self
    }
}

pub struct GetSchemaQueryBuilder {
    columns_table: String,
    tables_table: String,
    filter_tables: Vec<String>,
    filter_dataset: Option<String>,
    column_names: ColumnNames,
}

impl Default for GetSchemaQueryBuilder {
    fn default() -> Self {
        Self {
            columns_table: "INFORMATION_SCHEMA.COLUMNS".to_string(),
            tables_table: "INFORMATION_SCHEMA.TABLES".to_string(),
            filter_dataset: None,
            filter_tables: vec![],
            column_names: Default::default(),
        }
    }
}

impl GetSchemaQueryBuilder {
    pub fn with_columns_table(mut self, columns_table: String) -> Self {
        self.columns_table = columns_table;
        self
    }

    pub fn with_tables_table(mut self, tables_table: String) -> Self {
        self.tables_table = tables_table;
        self
    }

    pub fn with_column_names(mut self, column_names: ColumnNames) -> Self {
        self.column_names = column_names;
        self
    }

    pub fn with_filter_dataset(mut self, dataset: String) -> Self {
        if dataset.is_empty() {
            return self;
        }
        self.filter_dataset = Some(dataset);
        self
    }

    pub fn with_filter_tables(mut self, tables: Vec<String>) -> Self {
        self.filter_tables = tables;
        self
    }

    pub fn build_ddl(&self) -> String {
        let mut query = format!(
            "SELECT {}, {} FROM {}",
            self.column_names.dataset, self.column_names.ddl, self.tables_table
        );
        let where_clause = self.get_where_clause();
        query.push_str(&where_clause);
        query
    }

    pub fn build(&self) -> String {
        let mut query = format!(
            "SELECT {}, {}, {}, {}, {}, {} FROM {}",
            self.column_names.dataset,
            self.column_names.table,
            self.column_names.column,
            self.column_names.data_type,
            self.column_names.is_partitioning_column,
            self.column_names.description,
            self.columns_table,
        );
        let where_clause = self.get_where_clause();
        query.push_str(&where_clause);
        query
    }

    fn get_table_filter(&self) -> Option<String> {
        if self.filter_tables.is_empty() {
            return None;
        }
        Some(
            self.filter_tables
                .iter()
                .map(|v| {
                    if v.contains("*") {
                        format!("{} LIKE '{}'", self.column_names.table, v.replace("*", "%"))
                    } else {
                        format!("{} = '{}'", self.column_names.table, v)
                    }
                })
                .join(" OR "),
        )
    }

    fn get_dataset_filter(&self) -> Option<String> {
        self.filter_dataset
            .as_ref()
            .map(|dataset| format!("{} = '{}'", self.column_names.dataset, dataset))
    }

    fn get_where_clause(&self) -> String {
        let dataset_filter = self.get_dataset_filter();
        let table_filter = self.get_table_filter();
        match (dataset_filter, table_filter) {
            (Some(dataset), Some(table)) => format!(" WHERE {dataset} AND ({table})"),
            (Some(dataset), None) => format!(" WHERE {dataset}"),
            (None, Some(table)) => format!(" WHERE {table}"),
            (None, None) => String::default(),
        }
    }
}

trait GetSchemaQuery {
    fn get_schemas_queries(&self) -> Result<Vec<String>, OxyError>;
    fn get_ddl_queries(&self) -> Result<Vec<String>, OxyError>;
}

impl GetSchemaQuery for Database {
    fn get_schemas_queries(&self) -> Result<Vec<String>, OxyError> {
        match &self.database_type {
            DatabaseType::Bigquery(_) => self
                .datasets()
                .iter()
                .map(|(dataset, tables)| {
                    let tables_filter = if tables.is_empty() {
                        String::default()
                    } else {
                        let table_conditions = tables
                            .iter()
                            .map(|v| {
                                if v.contains("*") {
                                    format!("c.table_name LIKE '{}'", v.replace("*", "%"))
                                } else {
                                    format!("c.table_name = '{v}'")
                                }
                            })
                            .join(" OR ");
                        format!(" WHERE {table_conditions}")
                    };

                    let query = format!(
                        "SELECT c.table_schema, c.table_name, c.column_name, c.data_type, c.is_partitioning_column, COALESCE(d.description, NULL) as description
                         FROM `{dataset}.INFORMATION_SCHEMA.COLUMNS` c
                         LEFT JOIN `{dataset}.INFORMATION_SCHEMA.COLUMN_FIELD_PATHS` d
                         ON c.table_name = d.table_name AND c.column_name = d.column_name{tables_filter}"
                    );
                    Ok(query)
                })
                .collect::<Result<Vec<_>, OxyError>>(),
            DatabaseType::ClickHouse(_) => self
                .datasets()
                .iter()
                .map(|(dataset, tables)| {
                    let query = GetSchemaQueryBuilder::default()
                        .with_column_names(
                            ColumnNames::default()
                                .with_dataset("database")
                                .with_table("table")
                                .with_column("name")
                                .with_data_type("type")
                                .with_is_partitioning_column("is_in_partition_key")
                                .with_description("comment"),
                        )
                        .with_filter_dataset(dataset.to_string())
                        .with_filter_tables(tables.clone())
                        .with_columns_table("system.columns".to_string())
                        .build();
                    Ok(query)
                })
                .collect::<Result<Vec<_>, OxyError>>(),
            DatabaseType::Snowflake(_) => self
                .datasets()
                .iter()
                .map(|(dataset, tables)| {
                    let tables_filter = if tables.is_empty() {
                        String::default()
                    } else {
                        let table_conditions = tables
                            .iter()
                            .map(|v| {
                                if v.contains("*") {
                                    format!("c.table_name LIKE '{}'", v.replace("*", "%"))
                                } else {
                                    format!("c.table_name = '{v}'")
                                }
                            })
                            .join(" OR ");
                        format!(" AND ({table_conditions})")
                    };

                    let schema_filter = if dataset.is_empty() {
                        "c.TABLE_SCHEMA <> 'INFORMATION_SCHEMA'".to_string()
                    } else {
                        format!("c.TABLE_SCHEMA = '{dataset}'")
                    };

                    let query = format!(
                        "SELECT c.TABLE_SCHEMA as table_schema,
                                c.TABLE_NAME as table_name,
                                c.COLUMN_NAME as column_name,
                                c.DATA_TYPE as data_type,
                                CASE WHEN c.IS_IDENTITY = 'YES' THEN TRUE ELSE FALSE END as is_partitioning_column,
                                c.COMMENT as description
                         FROM INFORMATION_SCHEMA.COLUMNS c
                         WHERE {schema_filter}{tables_filter}
                         ORDER BY c.TABLE_SCHEMA, c.TABLE_NAME, c.ORDINAL_POSITION"
                    );
                    tracing::debug!("Snowflake schema query for dataset '{}': {}", dataset, query);
                    Ok(query)
                })
                .collect::<Result<Vec<_>, OxyError>>(),

            DatabaseType::DuckDB(_) => {
                // DuckDB uses the information_schema structure
                let query = "SELECT schema_name as table_schema,
                            database_name,
                            table_name,
                            column_name,
                            data_type,
                            FALSE as is_partitioning_column,
                            comment
                     FROM duckdb_columns
                     WHERE schema_name NOT IN ('information_schema', 'pg_catalog', 'ducklake')
                     ORDER BY schema_name, table_name, column_index".to_string();
                tracing::debug!("DuckDB schema query: {}", query);
                Ok(vec![query])
            }

            DatabaseType::MotherDuck(_) => {
                // MotherDuck uses the same information_schema structure as DuckDB
                // but we filter by schemas specified in config
                self.datasets()
                    .iter()
                    .map(|(schema, tables)| {
                        let query = GetSchemaQueryBuilder::default()
                            .with_column_names(
                                ColumnNames::default()
                                    .with_is_partitioning_column("FALSE as is_partitioning_column")
                                    .with_description("NULL as description"),
                            )
                            .with_filter_dataset(schema.to_string())
                            .with_filter_tables(tables.clone())
                            .with_columns_table("information_schema.columns".to_string())
                            .build();
                        tracing::debug!("MotherDuck schema query for schema '{}': {}", schema, query);
                        Ok(query)
                    })
                    .collect::<Result<Vec<_>, OxyError>>()
            }

            DatabaseType::Postgres(_)
            | DatabaseType::PostgresManaged(_)
            | DatabaseType::Redshift(_) => {
                // One scan of information_schema covers the whole connected
                // database — Postgres configs carry no `datasets` list, so
                // `self.datasets()` is empty and the per-dataset builder used
                // by MotherDuck/Snowflake would emit zero queries.
                //
                // Column aliases are load-bearing: they must match
                // `SchemaRecord`'s field names / serde aliases.
                //
                // `NULL::text` rather than bare `NULL` — Postgres types an
                // untyped NULL as `unknown`, which the Arrow bridge cannot
                // map. Comments are skipped rather than joined from
                // pg_description, whose regclass cast fails on any table the
                // connected role cannot see (the analyst role hits exactly
                // that on `oxy_meta`).
                let query = "SELECT table_schema,
                            current_database() as database_name,
                            table_name,
                            column_name,
                            data_type,
                            FALSE as is_partitioning_column,
                            NULL::text as description
                     FROM information_schema.columns
                     WHERE table_schema NOT IN ('pg_catalog', 'information_schema')
                     ORDER BY table_schema, table_name, ordinal_position"
                    .to_string();
                tracing::debug!("Postgres schema query: {}", query);
                Ok(vec![query])
            }

            _ => Err(OxyError::ConfigurationError(
                "Unsupported database type".to_string(),
            )),
        }
    }
    fn get_ddl_queries(&self) -> Result<Vec<String>, OxyError> {
        match &self.database_type {
            DatabaseType::Bigquery(_) => self
                .datasets()
                .iter()
                .map(|(dataset, tables)| {
                    let query = GetSchemaQueryBuilder::default()
                        .with_tables_table(format!("{dataset}.INFORMATION_SCHEMA.TABLES"))
                        .with_filter_tables(tables.clone())
                        .build_ddl();
                    Ok(query)
                })
                .collect::<Result<Vec<_>, OxyError>>(),
            DatabaseType::ClickHouse(_) => self
                .datasets()
                .iter()
                .map(|(dataset, tables)| {
                    let query = GetSchemaQueryBuilder::default()
                        .with_column_names(
                            ColumnNames::default()
                                .with_dataset("database")
                                .with_ddl("create_table_query")
                                .with_table("name"),
                        )
                        .with_tables_table("system.tables".to_string())
                        .with_filter_dataset(dataset.to_string())
                        .with_filter_tables(tables.clone())
                        .build_ddl();
                    Ok(query)
                })
                .collect::<Result<Vec<_>, OxyError>>(),
            DatabaseType::Snowflake(_) => {
                // Snowflake's GET_DDL function requires constant arguments and cannot be used
                // in bulk queries with dynamic table names. DDL information is not critical
                // for semantic model generation, so we skip it for Snowflake.
                Ok(vec![])
            }

            DatabaseType::DuckDB(_) => {
                // DuckDB supports querying table DDL via information_schema when available
                let query = "SELECT schema_name as table_schema, sql as ddl
                    FROM duckdb_tables
                    WHERE
                        schema_name NOT IN ('information_schema', 'pg_catalog', 'ducklake')
                        AND sql IS NOT NULL"
                    .to_string();
                tracing::debug!("DuckDB DDL query: {}", query);
                Ok(vec![query])
            }

            DatabaseType::MotherDuck(_) => {
                // MotherDuck's information_schema doesn't expose DDL statements via sql column
                // DDL information is not critical for semantic model generation, so we skip it
                tracing::debug!(
                    "MotherDuck: Skipping DDL queries (not supported via information_schema)"
                );
                Ok(vec![])
            }

            DatabaseType::Postgres(_)
            | DatabaseType::PostgresManaged(_)
            | DatabaseType::Redshift(_) => {
                // information_schema exposes no DDL text. Not critical for
                // semantic-model generation — the same call MotherDuck makes.
                tracing::debug!("Postgres: skipping DDL queries (no information_schema source)");
                Ok(vec![])
            }

            _ => Err(OxyError::ConfigurationError(
                "Unsupported database type".to_string(),
            )),
        }
    }
}

async fn fetch_schema_models<T: for<'de> Deserialize<'de>>(
    queries: Vec<String>,
    connector: &Arc<Connector>,
) -> Result<Vec<T>, OxyError> {
    let datasets = async_stream::stream! {
        for (query_idx, query) in queries.into_iter().enumerate() {
            yield async move {
                tracing::debug!("Executing query #{}: {}", query_idx + 1, query);
                let (record_batches, schema) = connector.run_query_with_limit(&query, None).await?;
                tracing::debug!("Query #{} completed. Schema: {:?}", query_idx + 1, schema);

                let mut results = vec![];
                for (batch_idx, record_batch) in record_batches.iter().enumerate() {
                    tracing::debug!("Processing batch #{}: {} rows", batch_idx + 1, record_batch.num_rows());
                    let records: Vec<T> = from_record_batch(record_batch).map_err(|e| {
                        OxyError::RuntimeError(format!("Failed to parse schema information: {e}"))
                    })?;
                    tracing::debug!("Parsed {} records from batch #{}", records.len(), batch_idx + 1);
                    results.extend(records);
                }
                tracing::debug!("Total records from query #{}: {}", query_idx + 1, results.len());
                Ok::<_, OxyError>(results)
            };
        }
    }
    .buffered(10)
    .collect::<Vec<_>>()
    .await
    .into_iter()
    .try_collect::<Vec<T>, Vec<_>, _>()?
    .into_iter()
    .flatten()
    .collect();
    Ok(datasets)
}

pub struct SchemaLoader {
    database: Database,
    connector: Arc<Connector>,
    secrets_manager: SecretsManager,
}

#[derive(Debug, Deserialize)]
pub(super) struct SchemaRecord {
    #[serde(skip_serializing_if = "Option::is_none")]
    database_name: Option<String>,
    #[serde(alias = "table_schema", alias = "database", alias = "TABLE_SCHEMA")]
    dataset: String,
    #[serde(alias = "table", alias = "TABLE_NAME")]
    table_name: String,
    #[serde(alias = "name", alias = "COLUMN_NAME")]
    column_name: String,
    #[serde(alias = "type", alias = "DATA_TYPE")]
    data_type: String,
    #[serde(
        alias = "is_in_partition_key",
        alias = "IS_PARTITIONING_COLUMN",
        deserialize_with = "deserialize_bool"
    )]
    is_partitioning_column: bool,
    #[serde(alias = "column_comment", alias = "comment", alias = "DESCRIPTION")]
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum IsPartitionTypes {
    Bool(bool),
    U8(u8),
    Utf8(String),
}

fn deserialize_bool<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: serde::de::Deserializer<'de>,
{
    let value = IsPartitionTypes::deserialize(deserializer)?;
    match value {
        IsPartitionTypes::Bool(value) => Ok(value),
        IsPartitionTypes::U8(value) => Ok(value > 0),
        IsPartitionTypes::Utf8(value) => match value.to_lowercase().as_str() {
            "yes" => Ok(true),
            "no" => Ok(false),
            _ => Err(serde::de::Error::custom(
                "Expected 'yes', 'no', 'YES' or 'NO'",
            )),
        },
    }
}

#[derive(Debug, Deserialize)]
pub(super) struct DDLRecord {
    #[serde(alias = "table_schema", alias = "database", alias = "TABLE_SCHEMA")]
    dataset: String,
    #[serde(alias = "create_table_query")]
    ddl: String,
}

impl SchemaLoader {
    pub async fn from_database<S>(
        database: &Database,
        config: &ConfigManager<S>,
        secrets_manager: &SecretsManager,
    ) -> Result<Self, OxyError>
    where
        ConfigManager<S>: ResolveWorkspaceFile,
    {
        // System-side schema load has no user / workspace context —
        // `airhouse_managed` can't mint here. Bail with the canonical
        // friendly message before `from_db` produces its low-level
        // ConfigurationError.
        crate::connector::reject_airhouse_managed_for_system_path(
            config,
            &database.name,
            "Schema loading",
        )?;
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
        Ok(SchemaLoader {
            database: database.clone(),
            connector,
            secrets_manager: secrets_manager.clone(),
        })
    }

    pub async fn load_schema(
        &self,
    ) -> Result<HashMap<String, HashMap<String, SemanticModels>>, OxyError> {
        match &self.database.database_type {
            DatabaseType::DOMO(domo) => {
                let domo_client =
                    DOMO::from_config(self.secrets_manager.clone(), domo.clone()).await?;
                let domo_dataset = domo_client.dataset();
                let dataset = domo_dataset.info(&domo.dataset_id).await?;
                let dataset_info = domo_dataset.details(&domo.dataset_id).await?;
                let file_stem = slugify!(&dataset.name, separator = "_", max_length = 60);
                Ok(HashMap::from_iter([(
                    "domo".to_string(),
                    HashMap::from_iter([(
                        file_stem,
                        SemanticModels {
                            database: self.database.name.clone(),
                            table: dataset.name,
                            description: dataset.description,
                            dimensions: dataset_info
                                .tables
                                .into_iter()
                                .flat_map(|table| table.columns)
                                .map(|col| Dimension {
                                    name: col.name,
                                    description: col.description,
                                    synonyms: col.synonyms,
                                    sample: vec![],
                                    data_type: Some(col.r#type),
                                    is_partition_key: None,
                                })
                                .collect(),
                            entities: vec![],
                            measures: vec![],
                            database_name: "".to_owned(),
                        },
                    )]),
                )]))
            }
            DatabaseType::ClickHouse(_)
            | DatabaseType::Bigquery(_)
            | DatabaseType::Snowflake(_)
            | DatabaseType::DuckDB(_)
            | DatabaseType::MotherDuck(_)
            | DatabaseType::Postgres(_)
            | DatabaseType::PostgresManaged(_)
            | DatabaseType::Redshift(_) => {
                let db_type = match &self.database.database_type {
                    DatabaseType::ClickHouse(_) => "ClickHouse",
                    DatabaseType::Postgres(_) => "Postgres",
                    DatabaseType::PostgresManaged(_) => "PostgresManaged",
                    DatabaseType::Redshift(_) => "Redshift",
                    DatabaseType::Bigquery(_) => "BigQuery",
                    DatabaseType::Snowflake(_) => "Snowflake",
                    DatabaseType::MotherDuck(_) => "MotherDuck",
                    DatabaseType::DuckDB(_c) => "DuckDB",
                    _ => "Unknown",
                };
                tracing::debug!(
                    "Starting schema load for {} database: {}",
                    db_type,
                    self.database.name
                );

                let queries = self.database.get_schemas_queries()?;
                tracing::debug!("Generated {} schema queries for {}", queries.len(), db_type);
                for (i, query) in queries.iter().enumerate() {
                    tracing::trace!("Query {}: {}", i + 1, query);
                }

                let records: Vec<SchemaRecord> =
                    fetch_schema_models(queries, &self.connector).await?;
                tracing::debug!(
                    "Retrieved {} schema records from {}",
                    records.len(),
                    db_type
                );
                let datasets = records.into_iter().fold(HashMap::new(), |mut acc, record| {
                    let model: &mut HashMap<String, SemanticModels> =
                        acc.entry(record.dataset.clone()).or_default();
                    let table_name = match &self.database.database_type {
                        DatabaseType::Snowflake(sf) => {
                            // For Snowflake, use database.schema.table format
                            format!("{}.{}.{}", sf.database, record.dataset, record.table_name)
                        }
                        _ => {
                            // For other databases, use schema.table format
                            format!("{}.{}", record.dataset, record.table_name)
                        }
                    };

                    let entry: &mut SemanticModels = model
                        .entry(record.table_name.clone())
                        .or_insert(SemanticModels {
                            database: self.database.name.to_string(),
                            table: table_name,
                            description: Default::default(),
                            dimensions: vec![],
                            entities: vec![],
                            measures: vec![],
                            database_name: record.database_name.clone().unwrap_or_default(),
                        });
                    entry.dimensions.push(Dimension {
                        name: record.column_name.to_string(),
                        description: record.description,
                        synonyms: None,
                        sample: vec![],
                        data_type: Some(record.data_type.to_string()),
                        is_partition_key: if record.is_partitioning_column {
                            Some(record.is_partitioning_column)
                        } else {
                            None
                        },
                    });
                    acc
                });

                let total_tables = datasets.values().map(|tables| tables.len()).sum::<usize>();
                let total_dimensions = datasets
                    .values()
                    .flat_map(|tables| tables.values())
                    .map(|table| table.dimensions.len())
                    .sum::<usize>();

                tracing::debug!(
                    "Final results for {} database '{}': {} datasets, {} tables, {} dimensions",
                    db_type,
                    self.database.name,
                    datasets.len(),
                    total_tables,
                    total_dimensions
                );

                for (dataset_name, tables) in &datasets {
                    tracing::debug!("  Dataset '{}': {} tables", dataset_name, tables.len());
                    for (table_name, table) in tables {
                        tracing::debug!(
                            "    Table '{}': {} dimensions",
                            table_name,
                            table.dimensions.len()
                        );
                    }
                }

                Ok(datasets)
            }
            _ => Err(OxyError::ConfigurationError(
                "Unsupported database type".to_string(),
            )),
        }
    }

    pub async fn load_ddl(&self) -> Result<HashMap<String, String>, OxyError> {
        match &self.database.database_type {
            // Postgres-family queries are empty (information_schema exposes no
            // DDL text), so these render no `.sql` file rather than failing
            // the whole sync after their models loaded.
            DatabaseType::ClickHouse(_)
            | DatabaseType::DuckDB(_)
            | DatabaseType::MotherDuck(_)
            | DatabaseType::Bigquery(_)
            | DatabaseType::Snowflake(_)
            | DatabaseType::Postgres(_)
            | DatabaseType::PostgresManaged(_)
            | DatabaseType::Redshift(_) => {
                let queries = self.database.get_ddl_queries()?;
                let records: Vec<DDLRecord> = fetch_schema_models(queries, &self.connector).await?;
                let datasets = records.into_iter().fold(HashMap::new(), |mut acc, record| {
                    let entry: &mut String = acc.entry(record.dataset.clone()).or_default();
                    entry.push_str(&record.ddl);
                    entry.push('\n');
                    acc
                });
                Ok(datasets)
            }
            DatabaseType::DOMO(_) => Ok(HashMap::new()),
            _ => Err(OxyError::ConfigurationError(
                "Unsupported database type".to_string(),
            )),
        }
    }
}
