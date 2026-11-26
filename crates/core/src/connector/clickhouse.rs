use arrow::datatypes::SchemaRef;
use arrow::ipc::reader::FileReader;
use arrow::record_batch::RecordBatch;
use clickhouse::Client;
use sqlparser::{dialect::ClickHouseDialect, parser::Parser};
use std::io::Cursor;
use uuid::Uuid;

use crate::{
    adapters::{
        secrets::SecretsManager,
        session_filters::{FilterProcessor, SessionFilters},
    },
    config::model::{
        ClickHouse as ConfigClickHouse, ClickHouseConnectionOverride, ConnectionOverride,
    },
};
use oxy_shared::errors::OxyError;

use super::constants::LOAD_RESULT;
use super::engine::Engine;
use super::utils::connector_internal_error;

#[derive(Debug)]
pub(super) struct ClickHouse {
    pub config: ConfigClickHouse,
    pub secret_manager: SecretsManager,
    pub filters: Option<SessionFilters>,
    pub overrides: Option<ClickHouseConnectionOverride>,
}

impl ClickHouse {
    pub fn new(config: ConfigClickHouse, secret_manager: SecretsManager) -> Self {
        ClickHouse {
            config,
            secret_manager,
            filters: None,
            overrides: None,
        }
    }

    pub fn with_filters(mut self, filters: SessionFilters) -> Self {
        self.filters = Some(filters);
        self
    }

    /// Apply connection overrides from the connections HashMap
    ///
    /// Extracts ClickHouse-specific overrides for the given database reference.
    /// Returns an error if a Snowflake override is provided for a ClickHouse database.
    pub fn with_overrides(
        mut self,
        connections: Option<ConnectionOverride>,
    ) -> Result<Self, OxyError> {
        if let Some(ovr) = connections {
            let ch: ClickHouseConnectionOverride = ovr.try_into()?;
            tracing::info!(
                has_host_override = ch.host.is_some(),
                has_database_override = ch.database.is_some(),
                "Applying ClickHouse connection overrides"
            );
            self.overrides = Some(ch);
        }
        Ok(self)
    }

    /// Escape a value for use in a ClickHouse SET statement
    /// Single quotes are escaped by doubling them: ' -> ''
    /// Backslashes are escaped: \ -> \\
    fn escape_for_set(value: &str) -> String {
        value.replace('\\', "\\\\").replace('\'', "''")
    }

    /// Apply filters and role to a query client
    ///
    /// Returns a tuple of:
    /// - Client with scalar filters applied via .with_setting() and role set
    /// - Vec of (session_key, csv_value) pairs for array filters that need SET commands
    ///
    /// Array filters are handled separately to avoid URI length limits when using sessions.
    fn apply_filters(&self, client: Client) -> (Client, Vec<(String, String)>) {
        let mut client = client;
        let mut array_filters = Vec::new();

        // Apply role if configured
        if let Some(role) = &self.config.role {
            tracing::debug!(
                role = %role,
                database = ?self.config.database,
                "Applying role to ClickHouse query"
            );
            client = client.with_setting("role", role);
        }

        // Apply filters as session variables
        if let Some(filters) = &self.filters {
            tracing::info!(
                database = ?self.config.database,
                filter_count = filters.len(),
                filters = ?filters.keys().collect::<Vec<_>>(),
                settings_prefix = ?self.config.settings_prefix,
                "Applying filters as session variables to ClickHouse query"
            );

            for (key, value) in filters.iter() {
                // Skip null values - optional filters that weren't provided
                if value.is_null() {
                    tracing::debug!(
                        filter_name = %key,
                        "Skipping null filter value - optional filter not provided"
                    );
                    continue;
                }

                let session_key = if let Some(prefix) = &self.config.settings_prefix {
                    format!("{}{}", prefix, key)
                } else {
                    key.clone()
                };

                let session_value = FilterProcessor::to_session_value(value);

                // Arrays are handled via SET commands in a session to avoid URI length limits
                if value.is_array() {
                    tracing::debug!(
                        filter_name = %key,
                        session_key = %session_key,
                        session_value_length = session_value.len(),
                        "Queuing array filter for SET command in session"
                    );
                    array_filters.push((session_key, session_value));
                } else {
                    // Scalar values use with_setting (added to URI/query params)
                    tracing::debug!(
                        filter_name = %key,
                        session_key = %session_key,
                        session_value_length = session_value.len(),
                        "Setting ClickHouse scalar filter via with_setting"
                    );
                    client = client.with_setting(&session_key, &session_value);
                }
            }
        } else {
            tracing::debug!(
                database = ?self.config.database,
                "No filters to apply to ClickHouse query"
            );
        }

        (client, array_filters)
    }

    pub fn strip_comments(query: &str) -> String {
        match Parser::parse_sql(&ClickHouseDialect {}, query) {
            Ok(ast) => {
                // Successfully parsed - strip comments by converting AST back to string
                ast.iter()
                    .map(|stmt| stmt.to_string())
                    .collect::<Vec<_>>()
                    .join("\n")
            }
            Err(err) => {
                // Parsing failed - likely due to dialect mismatch (e.g., backticks from another dialect)
                // ClickHouse can handle the query natively, so pass it through
                tracing::warn!(
                    error = %err,
                    "Failed to parse ClickHouse query for comment stripping, passing through unchanged."
                );
                query.to_string()
            }
        }
    }
}

impl Engine for ClickHouse {
    async fn run_query_with_limit(
        &self,
        query: &str,
        _dry_run_limit: Option<u64>,
    ) -> Result<(Vec<RecordBatch>, SchemaRef), OxyError> {
        // Determine host and database - use overrides if present, otherwise use config
        let host = match self.overrides.as_ref().and_then(|o| o.host.as_ref()) {
            Some(override_host) => {
                tracing::info!(
                    original_host = ?self.config.host,
                    override_host = %override_host,
                    "Using overridden ClickHouse host"
                );
                override_host.clone()
            }
            None => self.config.get_host(&self.secret_manager).await?,
        };

        let database = match self.overrides.as_ref().and_then(|o| o.database.as_ref()) {
            Some(override_database) => {
                tracing::info!(
                    original_database = ?self.config.database,
                    override_database = %override_database,
                    "Using overridden ClickHouse database"
                );
                override_database.clone()
            }
            None => self.config.get_database(&self.secret_manager).await?,
        };

        let base_client = Client::default()
            .with_url(normalize_clickhouse_url(&host))
            .with_user(self.config.get_user(&self.secret_manager).await?)
            .with_password(self.config.get_password(&self.secret_manager).await?)
            .with_database(&database);

        tracing::debug!("ClickHouse client created, applying filters");

        // Apply filters (role + session variables) - separates scalar and array filters
        let (client, array_filters) = self.apply_filters(base_client);
        let cleaned_query = ClickHouse::strip_comments(query);

        // Necessary when sending more than one query (i.e. for setting array filters)
        let session_id = format!("oxy-{}", Uuid::new_v4());

        tracing::info!(
            database = ?database,
            host = ?host,
            filter_names = ?self.filters.as_ref().map(|f| f.keys().collect::<Vec<_>>()),
            role = ?self.config.role,
            session_id = %session_id,
            has_array_filters = !array_filters.is_empty(),
            query_length = cleaned_query.len(),
            query_preview = %if cleaned_query.len() > 500 {
                format!("{}...", &cleaned_query[..500])
            } else {
                cleaned_query.clone()
            },
            "Executing ClickHouse query with session"
        );

        let session_client = client.with_setting("session_id", &session_id);

        // Execute SET commands for array filters (if any)
        for (session_key, session_value) in &array_filters {
            let escaped_value = Self::escape_for_set(session_value);
            let set_sql = format!("SET {} = '{}'", session_key, escaped_value);

            tracing::debug!(
                session_key = %session_key,
                value_length = session_value.len(),
                "Executing SET command for array filter in session"
            );

            session_client
                .query(&set_sql)
                .execute()
                .await
                .map_err(|err| {
                    tracing::error!(
                        session_key = %session_key,
                        error = %err,
                        "Failed to SET array filter in ClickHouse session"
                    );
                    OxyError::DBError(format!("Failed to SET {}: {}", session_key, err))
                })?;
        }

        // Execute the actual query in the session
        let mut cursor = session_client
            .query(&cleaned_query)
            .fetch_bytes("arrow")
            .map_err(|err| {
                tracing::error!(
                    database = ?self.config.database,
                    session_id = %session_id,
                    has_filters = self.filters.is_some(),
                    error = %err,
                    "ClickHouse query execution failed in session"
                );
                OxyError::DBError(format!("ClickHouse query error: {err}"))
            })?;
        let chunks = cursor
            .collect()
            .await
            .map_err(|e| OxyError::DBError(format!("Error fetching data: {}", e)))?;

        let cursor = Cursor::new(chunks);
        let reader = FileReader::try_new(cursor, None)
            .map_err(|e| OxyError::DBError(format!("Failed to create Arrow reader: {}", e)))?;
        let schema = reader.schema();
        let batches: Vec<RecordBatch> = reader
            .map(|result| result.map_err(|e| connector_internal_error(LOAD_RESULT, &e)))
            .collect::<Result<_, _>>()?;

        Ok((batches, schema))
    }
}

/// Normalize a user-supplied ClickHouse `host:` value into a full URL the
/// `clickhouse-rs` client accepts. The client's `with_url` requires a scheme
/// (e.g. `http://...`) and rejects bare hostnames or `host:port` forms with
/// "invalid format". Accept all three common shapes:
///
/// - `localhost`              → `http://localhost:8123`
/// - `localhost:9000`         → `http://localhost:9000`
/// - `https://x.cloud:8443`   → `https://x.cloud:8443`
fn normalize_clickhouse_url(host: &str) -> String {
    if host.contains("://") {
        host.to_string()
    } else if host.contains(':') {
        format!("http://{host}")
    } else {
        format!("http://{host}:8123")
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_clickhouse_url;

    #[test]
    fn passes_through_full_urls() {
        assert_eq!(
            normalize_clickhouse_url("http://localhost:8123"),
            "http://localhost:8123"
        );
        assert_eq!(
            normalize_clickhouse_url("https://x.clickhouse.cloud:8443"),
            "https://x.clickhouse.cloud:8443"
        );
    }

    #[test]
    fn prepends_scheme_to_host_port() {
        assert_eq!(
            normalize_clickhouse_url("localhost:8123"),
            "http://localhost:8123"
        );
        assert_eq!(
            normalize_clickhouse_url("myhost:9000"),
            "http://myhost:9000"
        );
    }

    #[test]
    fn defaults_port_for_bare_hostname() {
        assert_eq!(
            normalize_clickhouse_url("localhost"),
            "http://localhost:8123"
        );
        assert_eq!(
            normalize_clickhouse_url("clickhouse.internal"),
            "http://clickhouse.internal:8123"
        );
    }
}
