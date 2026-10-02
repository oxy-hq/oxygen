use arrow::datatypes::SchemaRef;
use arrow::json::ReaderBuilder;
use arrow::json::reader::infer_json_schema;
use arrow::record_batch::RecordBatch;
use df_interchange::Interchange;
use itertools::Itertools;
use snowflake_api::{QueryResult, SnowflakeApi};
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::adapters::secrets::SecretsManager;
use crate::adapters::session_filters::{FilterProcessor, SessionFilters};
use crate::config::model::{ConnectionOverride, SnowflakeAuthType};
use crate::config::{
    ConfigManager,
    model::{Snowflake as SnowflakeConfig, SnowflakeConnectionOverride},
};
use oxy_shared::errors::OxyError;

use super::constants::{CREATE_CONN, EXECUTE_QUERY, SNOWFLAKE_SESSION_VAR_LIMIT};
use super::engine::Engine;
use super::utils::connector_internal_error;
use crate::config::WorkingCopy;

#[derive(Debug)]
pub(super) struct Snowflake {
    pub config: SnowflakeConfig,
    pub secret_manager: SecretsManager,
    /// `None` on a process with no working copy. Only the private-key auth
    /// types need it, so the failure surfaces there instead of blocking every
    /// Snowflake connection.
    pub config_manager: Option<ConfigManager<WorkingCopy>>,
    pub filters: Option<SessionFilters>,
    pub overrides: Option<SnowflakeConnectionOverride>,
    pub sso_url_sender: Option<mpsc::Sender<String>>,
}

impl Snowflake {
    pub fn new(
        config: SnowflakeConfig,
        secret_manager: SecretsManager,
        config_manager: Option<ConfigManager<WorkingCopy>>,
    ) -> Self {
        Self {
            config,
            secret_manager,
            config_manager,
            filters: None,
            overrides: None,
            sso_url_sender: None,
        }
    }

    pub fn with_filters(mut self, filters: SessionFilters) -> Self {
        self.filters = Some(filters);
        self
    }

    /// Apply connection overrides from the connections HashMap
    ///
    /// Extracts Snowflake-specific overrides for the given database reference.
    /// Returns an error if a ClickHouse override is provided for a Snowflake database.
    pub fn with_overrides(
        mut self,
        connections: Option<ConnectionOverride>,
    ) -> Result<Self, OxyError> {
        if let Some(ovr) = connections {
            let sf: SnowflakeConnectionOverride = ovr.try_into()?;
            tracing::info!(
                has_database_override = sf.database.is_some(),
                has_schema_override = sf.schema.is_some(),
                has_warehouse_override = sf.warehouse.is_some(),
                has_account_override = sf.account.is_some(),
                "Applying Snowflake connection overrides"
            );
            self.overrides = Some(sf);
        }
        Ok(self)
    }

    /// Set the SSO URL sender for browser authentication
    pub fn with_sso_url_sender(mut self, sender: mpsc::Sender<String>) -> Self {
        self.sso_url_sender = Some(sender);
        self
    }

    /// Build SET statements for session variables from filters
    /// Returns error if total size exceeds Snowflake's limit
    fn build_filter_statements(&self, database_name: &str) -> Result<Vec<String>, OxyError> {
        let Some(filters) = &self.filters else {
            return Ok(Vec::new());
        };

        let mut statements = Vec::new();
        let mut total_size = 0;

        for (key, value) in filters.iter() {
            // Skip null values - optional filters that weren't provided
            if value.is_null() {
                tracing::debug!(
                    filter_name = %key,
                    "Skipping null filter value - optional filter not provided"
                );
                continue;
            }

            // Convert filter name to uppercase (Snowflake convention)
            let var_name = key.to_uppercase();

            // Serialize the value to a session variable string
            let var_value = FilterProcessor::to_session_value(value);

            // Build the SET statement
            let statement = format!("SET {} = {}", var_name, session_var_literal(&var_value));

            // Track size (the value size is what counts toward the limit)
            total_size += var_value.len();

            tracing::debug!(
                filter_name = %key,
                var_name = %var_name,
                value_size = var_value.len(),
                total_size = total_size,
                "Building Snowflake SET statement for filter"
            );

            statements.push(statement);
        }

        // Check if we exceeded the limit
        if total_size > SNOWFLAKE_SESSION_VAR_LIMIT {
            tracing::error!(
                database = %database_name,
                total_size = total_size,
                limit = SNOWFLAKE_SESSION_VAR_LIMIT,
                filter_count = statements.len(),
                "Filter size exceeds Snowflake session variable limit"
            );
            return Err(OxyError::FilterSizeLimitExceeded {
                database: database_name.to_string(),
                size_bytes: total_size,
                limit_bytes: SNOWFLAKE_SESSION_VAR_LIMIT,
            });
        }

        tracing::info!(
            database = %database_name,
            statement_count = statements.len(),
            total_size = total_size,
            limit = SNOWFLAKE_SESSION_VAR_LIMIT,
            "Successfully built Snowflake filter statements"
        );

        Ok(statements)
    }

    async fn get_connection(&self) -> Result<(SnowflakeApi, String, Vec<String>), OxyError> {
        // Determine warehouse, database, schema, account from config with potential overrides
        let warehouse = self
            .overrides
            .as_ref()
            .and_then(|o| o.warehouse.as_ref())
            .map(|w| w.as_str())
            .unwrap_or(self.config.warehouse.as_str());

        let database = self
            .overrides
            .as_ref()
            .and_then(|o| o.database.as_ref())
            .map(|d| d.as_str())
            .unwrap_or(self.config.database.as_str());

        let schema = match &self.overrides {
            Some(o) if o.schema.is_some() => o.schema.as_deref(),
            _ => self.config.schema.as_deref(),
        };

        let account = self
            .overrides
            .as_ref()
            .and_then(|o| o.account.as_ref())
            .map(|a| a.as_str())
            .unwrap_or(self.config.account.as_str());

        if self.overrides.is_some() {
            tracing::info!(
                original_account = %self.config.account,
                original_warehouse = %self.config.warehouse,
                original_database = %self.config.database,
                original_schema = ?self.config.schema,
                override_account = %account,
                override_warehouse = %warehouse,
                override_database = %database,
                override_schema = ?schema,
                "Using connection overrides for Snowflake"
            );
        }

        let config = self.config.clone();
        let api = match &config.auth_type {
            SnowflakeAuthType::BrowserAuth {
                browser_timeout_secs,
                cache_dir,
            } => {
                tracing::info!("🔐 Snowflake: Using browser-based authentication");

                let sso_sender = self.sso_url_sender.clone();
                let callback = sso_sender.map(|sender| {
                    Arc::new(move |url: String| {
                        tracing::info!("📡 Snowflake: SSO URL generated: {}", url);
                        // Use try_send which is non-blocking and synchronous
                        if let Err(e) = sender.try_send(url) {
                            tracing::error!("Failed to send SSO URL: {}", e);
                        }
                    }) as Arc<dyn Fn(String) + Send + Sync>
                });

                SnowflakeApi::with_externalbrowser_auth_full(
                    account,
                    Some(warehouse),
                    Some(database),
                    schema,
                    &config.username,
                    config.role.as_deref(),
                    *browser_timeout_secs,
                    true,
                    cache_dir.clone(),
                    callback,
                )
                .map_err(|err| {
                    tracing::error!(
                        "❌ Snowflake: Failed to create connection with browser auth: {}",
                        err
                    );
                    connector_internal_error(CREATE_CONN, &err)
                })?
            }
            SnowflakeAuthType::PrivateKey { private_key_path } => {
                tracing::info!(
                    "🔐 Snowflake: Using private key authentication from: {}",
                    private_key_path.display()
                );
                let resolver = self.config_manager.as_ref().ok_or_else(|| {
                    OxyError::ConfigurationError(
                        "this Snowflake connection authenticates with a private key \
                         file in the workspace, which this process does not have. \
                         Run it on the filesystem-owning node, or switch to an auth \
                         type that does not read a local file."
                            .to_string(),
                    )
                })?;
                let private_key_path =
                    resolver
                        .resolve_file(private_key_path)
                        .await
                        .map_err(|err| {
                            OxyError::ConfigurationError(format!(
                                "Failed to resolve private key path: {}",
                                err
                            ))
                        })?;
                let private_key_content =
                    std::fs::read_to_string(private_key_path).map_err(|err| {
                        OxyError::ConfigurationError(format!(
                            "Failed to read private key file: {}",
                            err
                        ))
                    })?;

                SnowflakeApi::with_certificate_auth(
                    account,
                    Some(warehouse),
                    Some(database),
                    schema,
                    &config.username,
                    config.role.as_deref(),
                    &private_key_content,
                )
                .map_err(|err| {
                    tracing::error!(
                        "❌ Snowflake: Failed to create connection with private key: {}",
                        err
                    );
                    connector_internal_error(CREATE_CONN, &err)
                })?
            }
            SnowflakeAuthType::Password { .. } | SnowflakeAuthType::PasswordVar { .. } => {
                tracing::info!("🔑 Snowflake: Using password authentication");
                // Use password authentication
                SnowflakeApi::with_password_auth(
                    account,
                    Some(warehouse),
                    Some(database),
                    schema,
                    &config.username,
                    config.role.as_deref(),
                    &config.get_password(&self.secret_manager).await?,
                )
                .map_err(|err| {
                    tracing::error!(
                        "❌ Snowflake: Failed to create connection with password: {}",
                        err
                    );
                    connector_internal_error(CREATE_CONN, &err)
                })?
            }
        };

        api.authenticate().await.map_err(|err| {
            tracing::error!(
                database = %database,
                account = %account,
                "❌ Snowflake: Authentication failed: {}",
                err
            );
            connector_internal_error(CREATE_CONN, &err)
        })?;
        tracing::debug!("✅ Snowflake: Connection established successfully");

        // Explicitly set role, warehouse, and database for the session
        // Even though these are passed during initialization, explicitly setting them
        // ensures they're active for the current session, especially with browser auth
        if let Some(role) = &config.role {
            let use_role_stmt = format!("USE ROLE {}", role);
            tracing::info!(
                role = %role,
                "⚡ Snowflake: Setting session role"
            );
            api.exec(&use_role_stmt).await.map_err(|err| {
                tracing::error!(
                    role = %role,
                    error = %err,
                    "❌ Snowflake: Failed to set role"
                );
                connector_internal_error(EXECUTE_QUERY, &err)
            })?;
        }

        let use_warehouse_stmt = format!("USE WAREHOUSE {}", warehouse);
        tracing::info!(
            warehouse = %warehouse,
            "⚡ Snowflake: Setting session warehouse"
        );
        api.exec(&use_warehouse_stmt).await.map_err(|err| {
            tracing::error!(
                warehouse = %warehouse,
                error = %err,
                "❌ Snowflake: Failed to set warehouse"
            );
            connector_internal_error(EXECUTE_QUERY, &err)
        })?;

        let use_database_stmt = format!("USE DATABASE {}", database);
        tracing::info!(
            database = %database,
            "⚡ Snowflake: Setting session database"
        );
        api.exec(&use_database_stmt).await.map_err(|err| {
            tracing::error!(
                database = %database,
                error = %err,
                "❌ Snowflake: Failed to set database"
            );
            connector_internal_error(EXECUTE_QUERY, &err)
        })?;

        if let Some(schema_name) = schema {
            let use_schema_stmt = format!("USE SCHEMA {}", schema_name);
            tracing::info!(
                schema = %schema_name,
                "⚡ Snowflake: Setting session schema"
            );
            api.exec(&use_schema_stmt).await.map_err(|err| {
                tracing::error!(
                    schema = %schema_name,
                    error = %err,
                    "❌ Snowflake: Failed to set schema"
                );
                connector_internal_error(EXECUTE_QUERY, &err)
            })?;
        }

        // Build filter SET statements if filters are present
        let filter_statements = self.build_filter_statements(database)?;

        // Execute each SET statement individually to establish session variables
        if !filter_statements.is_empty() {
            tracing::info!(
                database = %database,
                filter_count = filter_statements.len(),
                "⚡ Snowflake: Executing SET statements for session filters"
            );

            for (idx, statement) in filter_statements.iter().enumerate() {
                tracing::debug!(
                    statement_num = idx + 1,
                    total = filter_statements.len(),
                    statement = %statement,
                    "Executing SET statement"
                );

                api.exec(statement).await.map_err(|err| {
                    tracing::error!(
                        database = %database,
                        statement = %statement,
                        error = %err,
                        "❌ Snowflake: Failed to execute SET statement"
                    );
                    connector_internal_error(EXECUTE_QUERY, &err)
                })?;
            }

            tracing::debug!("✅ Snowflake: All SET statements executed successfully");
        } else {
            tracing::debug!("⚡ Snowflake: No filters to apply, executing query directly");
        }
        Ok((api, database.to_string(), filter_statements))
    }
}

impl Engine for Snowflake {
    async fn run_query_with_limit(
        &self,
        query: &str,
        _dry_run_limit: Option<u64>,
    ) -> Result<(Vec<RecordBatch>, SchemaRef), OxyError> {
        tracing::debug!("🔍 Snowflake query: {}", query);
        let (api, database, filter_statements) = self.get_connection().await?;

        // Execute the actual query (session variables are already set)
        tracing::debug!("⚡ Snowflake: Executing main query...");
        let res = api.exec(query).await.map_err(|err| {
            tracing::error!(
                database = %database,
                has_filters = !filter_statements.is_empty(),
                error = %err,
                "❌ Snowflake: Query execution failed"
            );
            connector_internal_error(EXECUTE_QUERY, &err)
        })?;
        let record_batches: Vec<RecordBatch>;
        match res {
            QueryResult::Arrow(batches) => {
                tracing::debug!(
                    "📊 Snowflake: Received Arrow result with {} batches",
                    batches.len()
                );
                if batches.is_empty() {
                    // `Interchange::from_arrow_58` panics on an empty
                    // chunk vec — fall through to the existing
                    // `record_batches.is_empty()` check below which
                    // returns a clean `"No record batches returned"`
                    // error instead.
                    record_batches = Vec::new();
                } else {
                    record_batches = Interchange::from_arrow_58(batches)
                        .map_err(|err| {
                            OxyError::DBError(format!("Failed to convert Arrow batches: {err}"))
                        })?
                        .to_arrow_58()
                        .map_err(|err| {
                            OxyError::DBError(format!("Failed to convert Arrow batches: {err}"))
                        })?;
                }
            }
            QueryResult::Json(json) => {
                tracing::debug!("📄 Snowflake: Received JSON result, converting to Arrow...");
                let batches = convert_json_result_to_arrow(&json)?;
                tracing::debug!(
                    "✅ Snowflake: Converted JSON to {} Arrow batches",
                    batches.len()
                );
                record_batches = batches;
            }
            QueryResult::Empty => {
                tracing::warn!("⚠️ Snowflake: Query returned empty result");
                return Err(OxyError::DBError("Empty result".to_string()));
            }
        }

        if record_batches.is_empty() {
            tracing::warn!("⚠️ Snowflake: No record batches returned");
            return Err(OxyError::DBError("No record batches returned".to_string()));
        }

        let total_rows: usize = record_batches.iter().map(|batch| batch.num_rows()).sum();
        tracing::debug!(
            "🎯 Snowflake: Query completed successfully - {} batches, {} total rows",
            record_batches.len(),
            total_rows
        );

        let schema = record_batches[0].schema();
        Ok((record_batches, schema))
    }
}

fn convert_json_result_to_arrow(
    json: &snowflake_api::JsonResult,
) -> Result<Vec<RecordBatch>, OxyError> {
    let json_objects = convert_to_json_objects(json);
    let infer_cursor = std::io::Cursor::new(json_objects[0].to_string());
    let (arrow_schema, _) = infer_json_schema(infer_cursor, None)
        .map_err(|err| OxyError::DBError(format!("Failed to infer JSON schema: {err}")))?;

    let json_string = json_objects.to_string();
    let json_stream_string = json_string[1..json_string.len() - 1]
        .to_string()
        .split(",")
        .join("");
    let cursor = std::io::Cursor::new(json_stream_string);
    let reader = ReaderBuilder::new(Arc::new(arrow_schema))
        .build(cursor)
        .map_err(|err| OxyError::DBError(format!("Failed to create JSON reader: {err}")))?;
    reader
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| OxyError::DBError(format!("Failed to convert JSON to Arrow: {err}")))
}

fn convert_to_json_objects(json: &snowflake_api::JsonResult) -> serde_json::Value {
    let mut rs: Vec<serde_json::Value> = vec![];
    if let serde_json::Value::Array(values) = &json.value {
        for value in values {
            let mut m: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
            if let serde_json::Value::Array(inner_values) = value {
                for field in &json.schema {
                    let field_name = field.name.clone();
                    let field_index = json
                        .schema
                        .iter()
                        .position(|x| x.name == field_name)
                        .unwrap();
                    let field_value = inner_values[field_index].clone();
                    m.insert(field_name, field_value);
                }
            }
            rs.push(serde_json::Value::Object(m));
        }
    }
    serde_json::Value::Array(rs)
}

/// A session-variable value as a single-quoted SQL literal for Snowflake.
///
/// Snowflake reads a backslash as an escape inside a `'…'` literal, so the
/// backslash is doubled first — a quote-only escape let a value ending in `\\`,
/// or carrying `\\'`, break out of the literal. A value with neither a quote
/// nor a backslash is spelled exactly as before.
fn session_var_literal(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "''"))
}

#[cfg(test)]
mod session_var_tests {
    use super::session_var_literal;

    #[test]
    fn a_value_cannot_break_out_of_a_snowflake_session_literal() {
        assert_eq!(session_var_literal("east"), "'east'");
        assert_eq!(session_var_literal("it's"), "'it''s'");
        assert_eq!(session_var_literal("a\\b"), "'a\\\\b'");
        // The breakout: a backslash before the quote, and a trailing backslash.
        assert_eq!(
            session_var_literal("x\\' OR 1=1 -- "),
            "'x\\\\'' OR 1=1 -- '"
        );
        assert_eq!(session_var_literal("C:\\"), "'C:\\\\'");
    }
}
