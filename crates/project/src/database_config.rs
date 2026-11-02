use crate::models::{WarehouseConfig, WarehousesFormData};
use axum::http::StatusCode;
use oxy::{
    adapters::secrets::SecretsManager,
    config::model::{Database, DatabaseType, Snowflake, default_snowflake_browser_timeout},
    service::secret_manager::SecretManagerService,
};
use oxy_shared::errors::OxyError;
use sea_orm::DatabaseTransaction;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{error, warn};
use uuid::Uuid;

pub struct DatabaseConfigBuilder;

impl DatabaseConfigBuilder {
    /// Build database configs using DatabaseTransaction (backward compatibility)
    pub async fn build_database_configs(
        project_id: Uuid,
        user_id: Uuid,
        warehouses_form: &WarehousesFormData,
        _txn: &DatabaseTransaction,
        repo_path: &Path,
    ) -> Result<Vec<Database>, StatusCode> {
        let secret_manager = SecretManagerService::new(project_id);
        Self::build_configs(
            warehouses_form,
            repo_path,
            user_id,
            &SecretsManager::from_database(secret_manager)?,
        )
        .await
    }

    pub async fn build_configs(
        warehouses_form: &WarehousesFormData,
        repo_path: &Path,
        user_id: Uuid,
        secrets_manager: &SecretsManager,
    ) -> Result<Vec<Database>, StatusCode> {
        let mut config_databases = Vec::new();

        for warehouse in &warehouses_form.warehouses {
            let db_name = warehouse
                .name
                .clone()
                .unwrap_or_else(|| "default-db".to_string());

            let database = match warehouse.r#type.as_str() {
                "postgres" => {
                    Self::build_postgres_config(db_name, warehouse, user_id, secrets_manager)
                        .await?
                }
                "redshift" => {
                    Self::build_redshift_config(db_name, warehouse, user_id, secrets_manager)
                        .await?
                }
                "mysql" => {
                    Self::build_mysql_config(db_name, warehouse, user_id, secrets_manager).await?
                }
                "clickhouse" => {
                    Self::build_clickhouse_config(db_name, warehouse, user_id, secrets_manager)
                        .await?
                }
                "bigquery" => Self::build_bigquery_config(db_name, warehouse, repo_path).await?,
                "duckdb" => Self::build_duckdb_config(db_name, warehouse)?,
                // The workspace's own Airhouse: no fields, the credential is
                // minted per caller. What onboarding registers after an upload
                // lands there (`destination: airhouse`).
                "airhouse_managed" => Database {
                    name: db_name,
                    database_type: DatabaseType::AirhouseManaged(
                        oxy::config::model::AirhouseManaged::default(),
                    ),
                },
                "snowflake" => {
                    Self::build_snowflake_config(db_name, warehouse, user_id, secrets_manager)
                        .await?
                }
                _ => {
                    warn!("Unsupported database type: {}", warehouse.r#type);
                    continue;
                }
            };

            config_databases.push(database);
        }
        Ok(config_databases)
    }

    async fn build_postgres_config(
        db_name: String,
        warehouse: &WarehouseConfig,
        created_by: Uuid,
        secrets_manager: &SecretsManager,
    ) -> std::result::Result<Database, StatusCode> {
        let postgres_config = warehouse.get_postgres_config()?;

        let db_var_name = db_name.to_uppercase() + "_PASSWORD";

        if let Some(password) = &postgres_config.password {
            Self::create_secret(
                db_var_name.clone(),
                password.clone(),
                created_by,
                secrets_manager,
            )
            .await
            .map_err(|e| {
                error!("Failed to create PostgreSQL password secret: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
        }

        Ok(Database {
            name: db_name,
            database_type: DatabaseType::Postgres(oxy::config::model::Postgres {
                host: postgres_config.host,
                host_var: None,
                port: postgres_config.port,
                port_var: None,
                user: postgres_config.user,
                user_var: None,
                password: None,
                password_var: Some(db_var_name),
                database: postgres_config.database,
                database_var: None,
            }),
        })
    }

    async fn build_redshift_config(
        db_name: String,
        warehouse: &WarehouseConfig,
        created_by: Uuid,
        secrets_manager: &SecretsManager,
    ) -> std::result::Result<Database, StatusCode> {
        let redshift_config = warehouse.get_redshift_config()?;

        let db_var_name = db_name.to_uppercase() + "_PASSWORD";

        if let Some(password) = &redshift_config.password {
            Self::create_secret(
                db_var_name.clone(),
                password.clone(),
                created_by,
                secrets_manager,
            )
            .await
            .map_err(|e| {
                error!("Failed to create Redshift password secret: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
        }

        Ok(Database {
            name: db_name,
            database_type: DatabaseType::Redshift(oxy::config::model::Redshift {
                host: redshift_config.host,
                host_var: None,
                port: redshift_config.port,
                port_var: None,
                user: redshift_config.user,
                user_var: None,
                password: None,
                password_var: Some(db_var_name),
                database: redshift_config.database,
                database_var: None,
            }),
        })
    }

    async fn build_mysql_config(
        db_name: String,
        warehouse: &WarehouseConfig,
        created_by: Uuid,
        secrets_manager: &SecretsManager,
    ) -> std::result::Result<Database, StatusCode> {
        let mysql_config = warehouse.get_mysql_config()?;

        let db_var_name = db_name.to_uppercase() + "_PASSWORD";

        if let Some(password) = &mysql_config.password {
            Self::create_secret(
                db_var_name.clone(),
                password.clone(),
                created_by,
                secrets_manager,
            )
            .await
            .map_err(|e| {
                error!("Failed to create MySQL password secret: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
        }

        Ok(Database {
            name: db_name,
            database_type: DatabaseType::Mysql(oxy::config::model::Mysql {
                host: mysql_config.host,
                host_var: None,
                port: mysql_config.port,
                port_var: None,
                user: mysql_config.user,
                user_var: None,
                password: None,
                password_var: Some(db_var_name),
                database: mysql_config.database,
                database_var: None,
            }),
        })
    }

    async fn build_clickhouse_config(
        db_name: String,
        warehouse: &WarehouseConfig,
        created_by: Uuid,
        secrets_manager: &SecretsManager,
    ) -> std::result::Result<Database, StatusCode> {
        let clickhouse_config = warehouse.get_clickhouse_config()?;

        let db_var_name = db_name.to_uppercase() + "_PASSWORD";

        if let Some(password) = &clickhouse_config.password {
            Self::create_secret(
                db_var_name.clone(),
                password.clone(),
                created_by,
                secrets_manager,
            )
            .await
            .map_err(|e| {
                error!("Failed to create ClickHouse password secret: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
        }

        Ok(Database {
            name: db_name,
            database_type: DatabaseType::ClickHouse(oxy::config::model::ClickHouse {
                host: Some(
                    clickhouse_config
                        .host
                        .unwrap_or_else(|| "localhost".to_string()),
                ),
                host_var: None,
                user: Some(
                    clickhouse_config
                        .user
                        .unwrap_or_else(|| "default".to_string()),
                ),
                user_var: None,
                password: None,
                password_var: Some(db_var_name),
                database: Some(
                    clickhouse_config
                        .database
                        .unwrap_or_else(|| "default".to_string()),
                ),
                database_var: None,
                schemas: HashMap::new(),
                role: None,
                settings_prefix: None,
                filters: HashMap::new(),
            }),
        })
    }

    async fn build_bigquery_config(
        db_name: String,
        warehouse: &WarehouseConfig,
        repo_path: &Path,
    ) -> std::result::Result<Database, StatusCode> {
        let bigquery_config = warehouse.get_bigquery_config()?;

        // The key's contents live in the secret store: nothing is written to
        // the checkout.
        if let Some(key_json_var) = bigquery_config.key_json_var.filter(|v| !v.is_empty()) {
            let database: Database = serde_json::from_value(serde_json::json!({
                "name": db_name,
                "type": "bigquery",
                "key_json_var": key_json_var,
                "dataset": bigquery_config.dataset,
                "dry_run_limit": bigquery_config.dry_run_limit,
            }))
            .map_err(|e| {
                error!("BigQuery config for '{}' did not build: {}", db_name, e);
                StatusCode::BAD_REQUEST
            })?;
            require_kept(&database, "key_json_var")?;
            return Ok(database);
        }

        let key_content = match &bigquery_config.key {
            Some(key) => key,
            None => {
                error!("BigQuery key not provided for database '{}'", db_name);
                return Err(StatusCode::BAD_REQUEST);
            }
        };

        let key_filename = format!("{}.key", db_name);
        let key_path = PathBuf::from(&key_filename);

        if let Err(e) = std::fs::write(repo_path.join(&key_path), key_content) {
            error!(
                "Failed to write BigQuery key file '{}': {}",
                key_filename, e
            );
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }

        Ok(Database {
            name: db_name,
            database_type: DatabaseType::Bigquery(oxy::config::model::BigQuery {
                key_path: Some(key_path),
                key_path_var: None,
                key_json_var: None,
                dataset: bigquery_config.dataset,
                datasets: HashMap::new(),
                dry_run_limit: bigquery_config.dry_run_limit,
            }),
        })
    }

    fn build_duckdb_config(
        db_name: String,
        warehouse: &WarehouseConfig,
    ) -> std::result::Result<Database, StatusCode> {
        let duckdb_config = warehouse.get_duckdb_config()?;

        Ok(Database {
            name: db_name,
            database_type: DatabaseType::DuckDB(oxy::config::model::DuckDB {
                options: oxy::config::model::DuckDBOptions::Local {
                    file_search_path: duckdb_config
                        .file_search_path
                        .unwrap_or_else(|| "data".to_string()),
                },
                s3_mirror: None,
            }),
        })
    }

    async fn build_snowflake_config(
        db_name: String,
        warehouse: &WarehouseConfig,
        created_by: Uuid,
        secrets_manager: &SecretsManager,
    ) -> std::result::Result<Database, StatusCode> {
        let snowflake_config = warehouse.get_snowflake_config()?;

        // Determine auth type based on auth_mode field or fallback to legacy logic
        let auth_type = match snowflake_config.auth_mode.as_deref() {
            Some("browser") => {
                // Explicit browser auth mode
                oxy::config::model::SnowflakeAuthType::BrowserAuth {
                    browser_timeout_secs: default_snowflake_browser_timeout(),
                    cache_dir: None,
                }
            }
            Some("private_key") if snowflake_config.private_key_var.is_some() => {
                // The key's contents live in the secret store.
                let auth: oxy::config::model::SnowflakeAuthType =
                    serde_json::from_value(serde_json::json!({
                        "private_key_var": snowflake_config.private_key_var,
                    }))
                    .map_err(|e| {
                        error!("Snowflake private_key_var auth did not build: {}", e);
                        StatusCode::BAD_REQUEST
                    })?;
                require_kept(&auth, "private_key_var")?;
                auth
            }
            Some("private_key") => {
                // Explicit private key auth mode
                if let Some(private_key_path) = &snowflake_config
                    .private_key_path
                    .as_ref()
                    .map(PathBuf::from)
                {
                    oxy::config::model::SnowflakeAuthType::PrivateKey {
                        private_key_path: private_key_path.clone(),
                    }
                } else {
                    error!("Private key auth mode selected but no private_key_path provided");
                    return Err(StatusCode::BAD_REQUEST);
                }
            }
            Some("password") | None => {
                // Explicit password auth mode or legacy mode (no auth_mode specified)
                if let Some(password) = &snowflake_config.password {
                    let db_var_name = db_name.to_uppercase() + "_PASSWORD";
                    Self::create_secret(
                        db_var_name.clone(),
                        password.clone(),
                        created_by,
                        secrets_manager,
                    )
                    .await
                    .map_err(|e| {
                        error!("Failed to create Snowflake password secret: {}", e);
                        StatusCode::INTERNAL_SERVER_ERROR
                    })?;
                    oxy::config::model::SnowflakeAuthType::PasswordVar {
                        password_var: db_var_name.clone(),
                    }
                } else if snowflake_config.auth_mode.is_none() {
                    // Legacy fallback: no password and no auth_mode -> browser auth
                    oxy::config::model::SnowflakeAuthType::BrowserAuth {
                        browser_timeout_secs: default_snowflake_browser_timeout(),
                        cache_dir: None,
                    }
                } else {
                    error!("Password auth mode selected but no password provided");
                    return Err(StatusCode::BAD_REQUEST);
                }
            }
            Some(other) => {
                error!("Invalid auth_mode: {}", other);
                return Err(StatusCode::BAD_REQUEST);
            }
        };

        Ok(Database {
            name: db_name,
            database_type: DatabaseType::Snowflake(Snowflake {
                account: snowflake_config.account.unwrap_or_default(),
                username: snowflake_config.username.unwrap_or_default(),
                warehouse: snowflake_config.warehouse.unwrap_or_default(),
                database: snowflake_config.database.unwrap_or_default(),
                schema: snowflake_config.schema,
                role: snowflake_config.role,
                datasets: HashMap::new(),
                filters: HashMap::new(),
                auth_type,
            }),
        })
    }

    async fn create_secret(
        key: String,
        value: String,
        created_by: Uuid,
        secrets_manager: &SecretsManager,
    ) -> Result<(), OxyError> {
        secrets_manager
            .create_secret(&key, &value, created_by)
            .await?;
        Ok(())
    }
}

/// Refuse a config whose `field` did not survive into the typed model.
///
/// `key_json_var` (BigQuery) and `private_key_var` (Snowflake) are built
/// through serde rather than a struct literal, so they are written whenever
/// `oxy::config::model` declares them. A model that does not yet would drop
/// the BigQuery field silently, and would read the Snowflake one as browser
/// auth (`SnowflakeAuthType` is untagged); both are a 400 here instead.
fn require_kept(value: &impl serde::Serialize, field: &str) -> Result<(), StatusCode> {
    let rendered = serde_json::to_string(value).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if rendered.contains(&format!("\"{field}\"")) {
        Ok(())
    } else {
        error!("this build's config model has no `{field}`; refusing a config that would drop it");
        Err(StatusCode::BAD_REQUEST)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What onboarding posts after an upload lands in Airhouse. An unknown
    /// type is skipped with a warning, so without this arm the database the
    /// upload step registers would silently not be added.
    #[tokio::test]
    async fn airhouse_managed_registers_a_field_free_database() {
        let form: WarehousesFormData = serde_json::from_value(serde_json::json!({
            "warehouses": [{ "type": "airhouse_managed", "name": "uploads", "config": {} }]
        }))
        .unwrap();
        let secrets = SecretsManager::from_environment().unwrap();
        let databases = DatabaseConfigBuilder::build_configs(
            &form,
            Path::new("/nonexistent"),
            Uuid::nil(),
            &secrets,
        )
        .await
        .unwrap();
        assert_eq!(databases.len(), 1);
        assert_eq!(databases[0].name, "uploads");
        assert!(matches!(
            databases[0].database_type,
            DatabaseType::AirhouseManaged(_)
        ));
    }
}
