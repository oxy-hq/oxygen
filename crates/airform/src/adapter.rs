use std::collections::HashMap;

use airform_core::DbtTarget;
use airform_executor::{
    WarehouseAdapter,
    adapters::{
        BigQueryAdapter, ClickHouseAdapter, DuckDbAdapter, MySqlAdapter, PostgresAdapter,
        SnowflakeAdapter,
    },
};
use airform_loader::LoadState;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};

use oxy::adapters::secrets::SecretsManager;
use oxy::config::ConfigManager;
use oxy::config::model::{
    BigQuery, BigQueryKey, ClickHouse, Database, DatabaseType, DuckDB, DuckDBOptions, MotherDuck,
    Mysql, Postgres, Redshift, Snowflake,
};
use oxy::connector::checkout_file_connection;

use crate::config::OxyProjectConfig;
use crate::error::AirformIntegrationError;
use oxy::config::WorkingCopy;

/// Build an airform `WarehouseAdapter` from a resolved Oxy `Database`.
pub async fn build_adapter_from_db(
    db: &Database,
    config_manager: &ConfigManager<WorkingCopy>,
    secrets_manager: &SecretsManager,
    target_schema: &str,
) -> anyhow::Result<Box<dyn WarehouseAdapter>> {
    match &db.database_type {
        DatabaseType::Snowflake(sf) => {
            let target = snowflake_to_dbt_target(sf, secrets_manager, target_schema).await?;
            Ok(Box::new(SnowflakeAdapter::from_target(&target).await?))
        }
        DatabaseType::Bigquery(bq) => {
            let target = bigquery_to_dbt_target(bq, secrets_manager, target_schema).await?;
            Ok(Box::new(BigQueryAdapter::from_target(&target)?))
        }
        DatabaseType::DuckDB(duckdb) => {
            refuse_file_duckdb_target(&db.name, duckdb, oxy::file_duckdb_limits::limits_apply())?;
            let target = duckdb_to_dbt_target(duckdb, config_manager, target_schema).await?;
            // For file-backed DuckDB, reuse the process-wide pool connection
            // rather than opening a fresh `duckdb_database` handle. Two
            // independent handles on the same file in the same process bypass
            // OS advisory locking and cause SIGSEGV in DuckDB's native code.
            let path_str = target
                .extra
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or(":memory:");
            let adapter = if path_str != ":memory:" {
                let conn = tokio::task::spawn_blocking({
                    let path = path_str.to_owned();
                    move || checkout_file_connection(&path)
                })
                .await
                .map_err(|e| anyhow::anyhow!("spawn_blocking panicked: {e}"))??;
                DuckDbAdapter::from_connection(conn, path_str)
            } else {
                DuckDbAdapter::from_target(&target)?
            };
            Ok(Box::new(adapter))
        }
        DatabaseType::Postgres(pg) => {
            let target = postgres_to_dbt_target(pg, secrets_manager, target_schema).await?;
            Ok(Box::new(PostgresAdapter::from_target(&target).await?))
        }
        DatabaseType::Redshift(rs) => {
            let target = redshift_to_dbt_target(rs, secrets_manager, target_schema).await?;
            Ok(Box::new(PostgresAdapter::from_target(&target).await?))
        }
        DatabaseType::Mysql(my) => {
            let target = mysql_to_dbt_target(my, secrets_manager, target_schema).await?;
            Ok(Box::new(MySqlAdapter::from_target(&target)?))
        }
        DatabaseType::ClickHouse(ch) => {
            let target = clickhouse_to_dbt_target(ch, secrets_manager, target_schema).await?;
            Ok(Box::new(ClickHouseAdapter::from_target(&target)?))
        }
        DatabaseType::MotherDuck(md) => {
            let target = motherduck_to_dbt_target(md, secrets_manager, target_schema).await?;
            Ok(Box::new(DuckDbAdapter::from_target(&target)?))
        }
        _ => anyhow::bail!(
            "Database type '{}' is not yet supported for airform modeling. \
             Supported: snowflake, bigquery, duckdb, postgres, redshift, mysql, clickhouse, motherduck. \
             Unsupported: domo.",
            db.database_type_name()
        ),
    }
}

/// Refuse, on a cloud role (`limits_apply`), an Airform target that is a file
/// DuckDB (`dataset:` / `path:`): there it is a read-only fallback, and the
/// models would land in a file no other node reads. The compile refuses the
/// mapping too (`compile_config_gate.rs`); this covers a run that reaches the
/// adapter anyway. The single-process role writes into it, as it always has.
fn refuse_file_duckdb_target(
    database: &str,
    duckdb: &DuckDB,
    limits_apply: bool,
) -> anyhow::Result<()> {
    let is_file = matches!(
        duckdb.options,
        DuckDBOptions::Local { .. } | DuckDBOptions::File { .. }
    );
    if limits_apply && is_file {
        return Err(anyhow::anyhow!(
            oxy::file_duckdb_limits::write_refused_error(database, "an Airform run").to_string()
        ));
    }
    Ok(())
}

/// Full lookup chain: profile target → oxy.yml mapping → Oxy database → adapter.
///
/// The oxy.yml mapping provides the connection (which Oxy database to use).
/// Output schema comes from the `profiles.yml` target. For DuckDB, the file
/// path comes from the mapped Oxy database config.
pub async fn build_adapter(
    load_state: &LoadState,
    oxy_config: &OxyProjectConfig,
    config_manager: &ConfigManager<WorkingCopy>,
    secrets_manager: &SecretsManager,
) -> Result<Box<dyn WarehouseAdapter>, AirformIntegrationError> {
    let profile = load_state
        .profile
        .as_ref()
        .ok_or_else(|| AirformIntegrationError::MissingProfile(load_state.project.name.clone()))?;

    let target_name = &profile.target;
    let oxy_db_name = oxy_config
        .resolve_profile_database(target_name)
        .ok_or_else(|| AirformIntegrationError::UnmappedDbtDatabases {
            unmapped: target_name.clone(),
            config_path: "oxy.yml".to_string(),
        })?;

    let db = config_manager.resolve_database(oxy_db_name).map_err(|e| {
        AirformIntegrationError::Other(format!(
            "Could not resolve Oxy database '{}' (mapped from dbt target '{}'): {e}",
            oxy_db_name, target_name
        ))
    })?;

    let target_schema = load_state
        .target
        .as_ref()
        .and_then(|t| {
            t.schema
                .as_deref()
                .or_else(|| t.extra.get("dataset").and_then(|v| v.as_str()))
        })
        .unwrap_or("public");

    build_adapter_from_db(&db, config_manager, secrets_manager, target_schema)
        .await
        .map_err(|e| AirformIntegrationError::Other(e.to_string()))
}

// ── Credential helpers ────────────────────────────────────────────────────────

async fn snowflake_to_dbt_target(
    sf: &Snowflake,
    secrets_manager: &SecretsManager,
    target_schema: &str,
) -> anyhow::Result<DbtTarget> {
    let password = sf
        .get_password(secrets_manager)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to resolve Snowflake password: {e}"))?;

    let mut extra: HashMap<String, serde_yaml::Value> = HashMap::new();
    extra.insert(
        "account".into(),
        serde_yaml::Value::String(sf.account.clone()),
    );
    extra.insert(
        "user".into(),
        serde_yaml::Value::String(sf.username.clone()),
    );
    extra.insert("password".into(), serde_yaml::Value::String(password));
    extra.insert(
        "warehouse".into(),
        serde_yaml::Value::String(sf.warehouse.clone()),
    );
    if let Some(role) = &sf.role {
        extra.insert("role".into(), serde_yaml::Value::String(role.clone()));
    }

    Ok(DbtTarget {
        adapter_type: "snowflake".to_string(),
        database: Some(sf.database.clone()),
        schema: Some(target_schema.to_string()),
        threads: None,
        extra,
    })
}

async fn bigquery_to_dbt_target(
    bq: &BigQuery,
    secrets_manager: &SecretsManager,
    target_schema: &str,
) -> anyhow::Result<DbtTarget> {
    let key = bq
        .get_key(secrets_manager)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to resolve BigQuery key: {e}"))?;
    let (key_json, source) = match key {
        // The key's contents, from the secret store (`key_json_var`).
        BigQueryKey::Json(json) => (json, "the key_json_var secret".to_string()),
        BigQueryKey::Path(key_path) => (
            std::fs::read_to_string(&key_path).map_err(|e| {
                anyhow::anyhow!("Failed to read BigQuery key file '{key_path}': {e}")
            })?,
            format!("key file '{key_path}'"),
        ),
    };
    let key_value: serde_json::Value = serde_json::from_str(&key_json)
        .map_err(|e| anyhow::anyhow!("Failed to parse BigQuery key from {source}: {e}"))?;
    let project_id = key_value["project_id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("BigQuery key file missing 'project_id'"))?
        .to_string();

    let token = bigquery_service_account_token(&key_value).await?;

    let mut extra: HashMap<String, serde_yaml::Value> = HashMap::new();
    extra.insert("token".into(), serde_yaml::Value::String(token));

    Ok(DbtTarget {
        adapter_type: "bigquery".to_string(),
        database: Some(project_id),
        schema: Some(target_schema.to_string()),
        threads: None,
        extra,
    })
}

/// Exchange a Google service account JSON key for a short-lived OAuth2 bearer token.
async fn bigquery_service_account_token(key: &serde_json::Value) -> anyhow::Result<String> {
    let client_email = key["client_email"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("BigQuery key file missing 'client_email'"))?;
    let private_key_pem = key["private_key"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("BigQuery key file missing 'private_key'"))?;
    let token_uri = key["token_uri"]
        .as_str()
        .unwrap_or("https://oauth2.googleapis.com/token");

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    #[derive(serde::Serialize)]
    struct Claims<'a> {
        iss: &'a str,
        scope: &'static str,
        aud: &'a str,
        exp: u64,
        iat: u64,
    }

    let claims = Claims {
        iss: client_email,
        scope: "https://www.googleapis.com/auth/bigquery",
        aud: token_uri,
        exp: now + 3600,
        iat: now,
    };

    let encoding_key = EncodingKey::from_rsa_pem(private_key_pem.as_bytes())
        .map_err(|e| anyhow::anyhow!("Failed to parse BigQuery private key: {e}"))?;
    let assertion = encode(&Header::new(Algorithm::RS256), &claims, &encoding_key)
        .map_err(|e| anyhow::anyhow!("Failed to sign BigQuery JWT: {e}"))?;

    let client = oxy_shared::http::client();
    let resp = client
        .post(token_uri)
        .form(&[
            ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
            ("assertion", &assertion),
        ])
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to exchange BigQuery JWT: {e}"))?;

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to parse BigQuery token response: {e}"))?;

    body["access_token"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| {
            let err = body["error_description"]
                .as_str()
                .or_else(|| body["error"].as_str())
                .unwrap_or("unknown error");
            anyhow::anyhow!("BigQuery token exchange failed: {err}")
        })
}

async fn duckdb_to_dbt_target(
    duckdb: &DuckDB,
    config_manager: &ConfigManager<WorkingCopy>,
    target_schema: &str,
) -> anyhow::Result<DbtTarget> {
    let mut extra: HashMap<String, serde_yaml::Value> = HashMap::new();

    let db_path = match &duckdb.options {
        DuckDBOptions::Local { .. } => {
            anyhow::bail!(
                "DuckDB directory sources (file_search_path) are read-only and cannot be used \
                as a dbt output target. Use a DuckDB file database (type: duckdb, path: ...) instead."
            );
        }
        DuckDBOptions::File { path } => config_manager
            .resolve_file(path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to resolve DuckDB path '{path}': {e}"))?,
        DuckDBOptions::DuckLake(_) => {
            anyhow::bail!(
                "DuckLake is not yet supported for airform modeling. Use a DuckDB file database instead."
            );
        }
    };

    extra.insert("path".into(), serde_yaml::Value::String(db_path));

    Ok(DbtTarget {
        adapter_type: "duckdb".to_string(),
        database: None,
        schema: Some(target_schema.to_string()),
        threads: None,
        extra,
    })
}

pub(crate) async fn postgres_to_dbt_target(
    pg: &Postgres,
    secrets_manager: &SecretsManager,
    target_schema: &str,
) -> anyhow::Result<DbtTarget> {
    let host = pg
        .get_host(secrets_manager)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to resolve Postgres host: {e}"))?;
    let port = pg
        .get_port(secrets_manager)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to resolve Postgres port: {e}"))?;
    let user = pg
        .get_user(secrets_manager)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to resolve Postgres user: {e}"))?;
    let password = pg
        .get_password(secrets_manager)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to resolve Postgres password: {e}"))?;
    let database = pg
        .get_database(secrets_manager)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to resolve Postgres database: {e}"))?;

    let mut extra: HashMap<String, serde_yaml::Value> = HashMap::new();
    extra.insert("host".into(), serde_yaml::Value::String(host));
    extra.insert("port".into(), serde_yaml::Value::String(port));
    extra.insert("user".into(), serde_yaml::Value::String(user));
    extra.insert("password".into(), serde_yaml::Value::String(password));

    Ok(DbtTarget {
        adapter_type: "postgres".to_string(),
        database: Some(database),
        schema: Some(target_schema.to_string()),
        threads: None,
        extra,
    })
}

pub(crate) async fn redshift_to_dbt_target(
    rs: &Redshift,
    secrets_manager: &SecretsManager,
    target_schema: &str,
) -> anyhow::Result<DbtTarget> {
    let host = rs
        .get_host(secrets_manager)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to resolve Redshift host: {e}"))?;
    let port = rs
        .get_port(secrets_manager)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to resolve Redshift port: {e}"))?;
    let user = rs
        .get_user(secrets_manager)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to resolve Redshift user: {e}"))?;
    let password = rs
        .get_password(secrets_manager)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to resolve Redshift password: {e}"))?;
    let database = rs
        .get_database(secrets_manager)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to resolve Redshift database: {e}"))?;

    let mut extra: HashMap<String, serde_yaml::Value> = HashMap::new();
    extra.insert("host".into(), serde_yaml::Value::String(host));
    extra.insert("port".into(), serde_yaml::Value::String(port));
    extra.insert("user".into(), serde_yaml::Value::String(user));
    extra.insert("password".into(), serde_yaml::Value::String(password));

    Ok(DbtTarget {
        adapter_type: "redshift".to_string(),
        database: Some(database),
        schema: Some(target_schema.to_string()),
        threads: None,
        extra,
    })
}

pub(crate) async fn mysql_to_dbt_target(
    my: &Mysql,
    secrets_manager: &SecretsManager,
    target_schema: &str,
) -> anyhow::Result<DbtTarget> {
    let host = my
        .get_host(secrets_manager)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to resolve MySQL host: {e}"))?;
    let port = my
        .get_port(secrets_manager)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to resolve MySQL port: {e}"))?;
    let user = my
        .get_user(secrets_manager)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to resolve MySQL user: {e}"))?;
    let password = my
        .get_password(secrets_manager)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to resolve MySQL password: {e}"))?;
    let database = my
        .get_database(secrets_manager)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to resolve MySQL database: {e}"))?;

    let mut extra: HashMap<String, serde_yaml::Value> = HashMap::new();
    extra.insert("host".into(), serde_yaml::Value::String(host));
    extra.insert("port".into(), serde_yaml::Value::String(port));
    extra.insert("user".into(), serde_yaml::Value::String(user));
    extra.insert("password".into(), serde_yaml::Value::String(password));

    Ok(DbtTarget {
        adapter_type: "mysql".to_string(),
        database: Some(database),
        schema: Some(target_schema.to_string()),
        threads: None,
        extra,
    })
}

pub(crate) async fn clickhouse_to_dbt_target(
    ch: &ClickHouse,
    secrets_manager: &SecretsManager,
    target_schema: &str,
) -> anyhow::Result<DbtTarget> {
    let host = ch
        .get_host(secrets_manager)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to resolve ClickHouse host: {e}"))?;
    let user = ch
        .get_user(secrets_manager)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to resolve ClickHouse user: {e}"))?;
    let password = ch
        .get_password(secrets_manager)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to resolve ClickHouse password: {e}"))?;
    let database = ch
        .get_database(secrets_manager)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to resolve ClickHouse database: {e}"))?;

    let mut extra: HashMap<String, serde_yaml::Value> = HashMap::new();
    extra.insert("host".into(), serde_yaml::Value::String(host));
    extra.insert("user".into(), serde_yaml::Value::String(user));
    extra.insert("password".into(), serde_yaml::Value::String(password));

    Ok(DbtTarget {
        adapter_type: "clickhouse".to_string(),
        database: Some(database),
        schema: Some(target_schema.to_string()),
        threads: None,
        extra,
    })
}

pub(crate) async fn motherduck_to_dbt_target(
    md: &MotherDuck,
    secrets_manager: &SecretsManager,
    target_schema: &str,
) -> anyhow::Result<DbtTarget> {
    let token = md
        .get_token(secrets_manager)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to resolve MotherDuck token: {e}"))?;
    let database = md.database.as_deref().unwrap_or("my_db");
    let path = format!("md:{database}?motherduck_token={token}");

    let mut extra: HashMap<String, serde_yaml::Value> = HashMap::new();
    extra.insert("path".into(), serde_yaml::Value::String(path));

    Ok(DbtTarget {
        adapter_type: "duckdb".to_string(),
        database: None,
        schema: Some(target_schema.to_string()),
        threads: None,
        extra,
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use oxy::adapters::secrets::SecretsManager;
    use oxy::config::model::{
        BigQuery, ClickHouse, Database, DatabaseType, DuckDB, DuckDBOptions, MotherDuck, Mysql,
        Postgres, Redshift, Snowflake, SnowflakeAuthType,
    };

    #[tokio::test]
    async fn test_snowflake_to_dbt_target_populates_fields() {
        let sf = Snowflake {
            account: "myaccount.us-east-1".to_string(),
            username: "myuser".to_string(),
            warehouse: "compute_wh".to_string(),
            database: "analytics".to_string(),
            schema: None,
            role: Some("transformer".to_string()),
            auth_type: SnowflakeAuthType::Password {
                password: "s3cr3t".to_string(),
            },
            datasets: Default::default(),
            filters: Default::default(),
        };
        let secrets = SecretsManager::from_environment().unwrap();
        let target = snowflake_to_dbt_target(&sf, &secrets, "dbt_dev")
            .await
            .unwrap();
        assert_eq!(target.adapter_type, "snowflake");
        assert_eq!(target.database.as_deref(), Some("analytics"));
        assert_eq!(target.schema.as_deref(), Some("dbt_dev"));
        assert_eq!(
            target.extra["account"].as_str(),
            Some("myaccount.us-east-1")
        );
        assert_eq!(target.extra["user"].as_str(), Some("myuser"));
        assert_eq!(target.extra["password"].as_str(), Some("s3cr3t"));
        assert_eq!(target.extra["warehouse"].as_str(), Some("compute_wh"));
        assert_eq!(target.extra["role"].as_str(), Some("transformer"));
    }

    #[tokio::test]
    async fn test_bigquery_to_dbt_target_populates_fields() {
        use std::io::Write;
        use tempfile::NamedTempFile;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        // RSA-2048 PKCS#8 test private key (dev cert from repo root, not used in production).
        const TEST_RSA_KEY: &str = "-----BEGIN PRIVATE KEY-----\n\
            MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQDewzmIwZaU2/cl\n\
            XV+emOYjFfkobMbebQKsxZ7dE5VWMJjUhY0NZnfwUPBB3vbVXwEsP0jpxh+gWPJM\n\
            1nYIxsxVscPGMUGC91mkCcXQn+6GbvOd0wEtna8upruls9e4Yayr2bcJeLZiykYw\n\
            j8AQcKnm4mLijPMNnuPyDnL6Nf8I2fCYQACJpxTwYaBpsgu2EGaqAq4+11YExB/z\n\
            bZuguq9OWWer4GqZ/rRb/QOhysGtgm68wp16cOdbTLUaMPli8Eq3gCFmh1Xj/qfs\n\
            yle8DSvtOzSPeOpwZqrWU5vIGjr/jBBFuLUwOgEzI/aSQlTH7Uxuu5QHX31Us5I0\n\
            U006aUNfAgMBAAECggEBAIdlreC7meUc1dl2KZpiYO9OecTiaPXk2E1fSLIjJw/e\n\
            NeZmzlcowxnkeEPxW6JRPotAY/cDn1F8/rlJWTD4dFZZ2B7s7V7HLUsRTZUCwJ4h\n\
            bh2tlPe+8i2u1jtfVm0RoTxK9n/hSSo+u+7kUN8tO3fEfkopVcofm3kS4zvF+h/M\n\
            BUOQA5zy8SSqYf2rI/LVRAnwRhJjSF4mhItqQ5YR3guJ9uzM3a0ATjqqdEXU8Pos\n\
            DCZ64gSTVNCMvbJs+aSgIXpKQtRaO88W6VsVR43TPkSYx6Dk2YLTy79fgTl/rpNM\n\
            K4rEtuqUT3NBGrYAFr3+glTvAgG7sfJygfO1Mj8/LFECgYEA+/VIjYagk8EX9ox8\n\
            t6iu/E5bS21Ys0ewvRycRwMuRQpsB992CRAlaZhkYk9rOwKZTx9gXd6hlcWX6x03\n\
            yWb5S3Ub/vHfjpcdNz2svkdyjT1HgeC21QZOzqSENBV2TMNcjvmb8JrFIrVLrWbN\n\
            5y+ipv+UpodM7lu3ThM20OmEQNMCgYEA4lYLQvW3+Fm+0UjzWsZVEhYdghYVkVAm\n\
            nDeFYvUlxH/mn4vQ5W1uIDaXNdPS/smsya/SWiQzaV0+jK7fRlAkj8D/jMzftxi6\n\
            4JXUOnzm8UIZvg9ZEbpDkhZpYl69owYen4xM4UbqKyOL7KGmI9rP/BUHV03h0heC\n\
            WYDIvxLKe8UCgYA3+OqQPisoB8pqBBWkuz18YW/YlscQtMlniZaSE/vQbJtJOHRB\n\
            WSvmhGswh9Ibft1N/Xtr/wxIeGfiXFBLVqvk/nQks9jlFV7xKatZbgfdppJfIOuc\n\
            8VTKhTO1Wls4fGHwhTUGQ2ut5TaVo/Pz+toYXUjJod8OSKO1HYGc8XNm7wKBgA0A\n\
            rnLxVNlSppC1ZS2g2UBJvvY7OI/5j85HrkUKGlpYkrI1wRF9IOd+217/RU7X3TJV\n\
            BHujOsTh03cXkMIkVoVfrA61smB9bjb6xI97n3TavEnb7d0D21/oI7PAB5r2/gli\n\
            cQQ8I7XIvAAjJT1IE8zClIJiegeszBNCP8YiWTmVAoGADcsVrXTlPnuYTyta1u4q\n\
            QrfYEB6XK4LGORsbfoXESK6oSRbtX02ObY1wYHhSSUS7C4DFVjuvUpeenaQtsPje\n\
            LhHH+y5feqxlJ5CePlRdOn3nEdFNWKGaLygD6mIpPQYGLRXUSvxx15qU6JOCq4oZ\n\
            ABINuW1dGcdrUFvgOhbdKEU=\n\
            -----END PRIVATE KEY-----\n";

        // Spin up a minimal HTTP server to mock the OAuth2 token endpoint so this
        // test does not make real network calls.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf).await;
                let body =
                    r#"{"access_token":"test_token","token_type":"Bearer","expires_in":3600}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes()).await;
            }
        });

        let mut f = NamedTempFile::new().unwrap();
        let key_json = serde_json::json!({
            "type": "service_account",
            "project_id": "my-gcp-project",
            "private_key_id": "key1",
            "private_key": TEST_RSA_KEY,
            "client_email": "sa@p.iam.gserviceaccount.com",
            "token_uri": format!("http://127.0.0.1:{port}")
        });
        writeln!(f, "{key_json}").unwrap();

        let bq = BigQuery {
            key_path: Some(f.path().to_path_buf()),
            key_path_var: None,
            key_json_var: None,
            dataset: Some("my_dataset".to_string()),
            datasets: Default::default(),
            dry_run_limit: None,
        };
        let secrets = SecretsManager::from_environment().unwrap();
        let target = bigquery_to_dbt_target(&bq, &secrets, "dbt_dev")
            .await
            .unwrap();
        assert_eq!(target.adapter_type, "bigquery");
        assert_eq!(target.database.as_deref(), Some("my-gcp-project"));
        assert_eq!(target.schema.as_deref(), Some("dbt_dev"));
    }

    /// On a cloud role a file DuckDB target is refused by name; on the
    /// single-process role (`oxy start`) Airform still writes into it.
    #[test]
    fn a_file_duckdb_target_is_refused_only_on_a_cloud_role() {
        let file = DuckDB {
            options: DuckDBOptions::File {
                path: "models.duckdb".to_string(),
            },
            s3_mirror: None,
        };
        let local = DuckDB {
            options: DuckDBOptions::Local {
                file_search_path: ".db".to_string(),
            },
            s3_mirror: None,
        };
        for duckdb in [&file, &local] {
            let err = refuse_file_duckdb_target("warehouse", duckdb, true)
                .expect_err("refused on a cloud role")
                .to_string();
            assert!(
                err.contains("`warehouse`") && err.contains("Airhouse"),
                "{err}"
            );
            assert!(refuse_file_duckdb_target("warehouse", duckdb, false).is_ok());
        }
    }

    #[tokio::test]
    async fn test_duckdb_local_to_dbt_target_errors() {
        use oxy::config::ConfigBuilder;
        use tempfile::TempDir;
        let dir = TempDir::new().unwrap();
        let dir_path = dir.path().to_str().unwrap().to_string();
        let duckdb = DuckDB {
            options: DuckDBOptions::Local {
                file_search_path: dir_path.clone(),
            },
            s3_mirror: None,
        };
        let config = ConfigBuilder::new()
            .with_workspace_path(&dir_path)
            .unwrap()
            .build_with_working_copy(oxy::config::Origin::Disk, oxy::config::OnMissing::Empty)
            .await
            .unwrap();
        let result = duckdb_to_dbt_target(&duckdb, &config, "dbt_dev").await;
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("read-only"),
            "expected read-only error"
        );
    }

    #[tokio::test]
    async fn test_postgres_to_dbt_target_fields() {
        use oxy::config::model::Postgres;
        let pg = Postgres {
            host: Some("pg.example.com".to_string()),
            port: Some("5432".to_string()),
            user: Some("analyst".to_string()),
            password: Some("s3cr3t".to_string()),
            database: Some("warehouse".to_string()),
            ..Default::default()
        };
        let secrets = SecretsManager::from_environment().unwrap();
        let target = postgres_to_dbt_target(&pg, &secrets, "dbt_dev")
            .await
            .unwrap();
        assert_eq!(target.adapter_type, "postgres");
        assert_eq!(target.extra["host"].as_str(), Some("pg.example.com"));
        assert_eq!(target.extra["user"].as_str(), Some("analyst"));
        assert_eq!(target.extra["port"].as_str(), Some("5432"));
        assert_eq!(target.schema.as_deref(), Some("dbt_dev"));
    }

    #[tokio::test]
    async fn test_redshift_to_dbt_target_default_port() {
        use oxy::config::model::Redshift;
        let rs = Redshift {
            host: Some("rs.example.com".to_string()),
            user: Some("admin".to_string()),
            password: Some("secret".to_string()),
            ..Default::default()
        };
        let secrets = SecretsManager::from_environment().unwrap();
        let target = redshift_to_dbt_target(&rs, &secrets, "dbt_dev")
            .await
            .unwrap();
        assert_eq!(target.adapter_type, "redshift");
        assert_eq!(target.extra["port"].as_str(), Some("5439"));
    }

    #[tokio::test]
    async fn test_mysql_to_dbt_target_fields() {
        use oxy::config::model::Mysql;
        let my = Mysql {
            host: Some("mysql.example.com".to_string()),
            user: Some("root".to_string()),
            password: Some("secret".to_string()),
            database: Some("mydb".to_string()),
            ..Default::default()
        };
        let secrets = SecretsManager::from_environment().unwrap();
        let target = mysql_to_dbt_target(&my, &secrets, "dbt_dev").await.unwrap();
        assert_eq!(target.adapter_type, "mysql");
        assert_eq!(target.extra["host"].as_str(), Some("mysql.example.com"));
        assert_eq!(target.database.as_deref(), Some("mydb"));
    }

    #[tokio::test]
    async fn test_clickhouse_to_dbt_target_fields() {
        use oxy::config::model::ClickHouse;
        let ch = ClickHouse {
            host: Some("ch.example.com".to_string()),
            user: Some("default".to_string()),
            password: Some("secret".to_string()),
            database: Some("analytics".to_string()),
            ..Default::default()
        };
        let secrets = SecretsManager::from_environment().unwrap();
        let target = clickhouse_to_dbt_target(&ch, &secrets, "dbt_dev")
            .await
            .unwrap();
        assert_eq!(target.adapter_type, "clickhouse");
        assert_eq!(target.database.as_deref(), Some("analytics"));
    }

    #[tokio::test]
    async fn test_motherduck_to_dbt_target_path_format() {
        use oxy::config::model::MotherDuck;
        let md = MotherDuck {
            token_var: "MOTHERDUCK_TOKEN".to_string(),
            database: Some("mydb".to_string()),
            schemas: Default::default(),
        };
        unsafe { std::env::set_var("MOTHERDUCK_TOKEN", "test_token_123") };
        let secrets = SecretsManager::from_environment().unwrap();
        let target = motherduck_to_dbt_target(&md, &secrets, "dbt_dev")
            .await
            .unwrap();
        assert_eq!(target.adapter_type, "duckdb");
        let path = target.extra["path"].as_str().unwrap();
        assert!(path.starts_with("md:"), "Expected md: prefix, got: {path}");
        assert!(path.contains("test_token_123"));
    }

    #[tokio::test]
    async fn test_unsupported_db_type_returns_error() {
        use oxy::config::ConfigBuilder;
        use oxy::config::model::DOMO;
        let db = Database {
            name: "test".to_string(),
            database_type: DatabaseType::DOMO(DOMO {
                instance: "test".to_string(),
                developer_token_var: "VAR".to_string(),
                dataset_id: "id".to_string(),
            }),
        };
        let secrets = SecretsManager::from_environment().unwrap();
        let config = ConfigBuilder::new()
            .with_workspace_path("/tmp")
            .unwrap()
            .build_with_working_copy(oxy::config::Origin::Disk, oxy::config::OnMissing::Empty)
            .await
            .unwrap();
        let result = build_adapter_from_db(&db, &config, &secrets, "public").await;
        assert!(result.is_err());
        let msg = result.err().unwrap().to_string();
        assert!(msg.contains("not yet supported"), "got: {msg}");
    }

    // ── build_adapter_from_db routing tests ───────────────────────────────────

    #[tokio::test]
    async fn test_build_adapter_from_db_duckdb_local() {
        use oxy::config::ConfigBuilder;
        use tempfile::TempDir;
        let dir = TempDir::new().unwrap();
        let db = Database {
            name: "test_duckdb".to_string(),
            database_type: DatabaseType::DuckDB(DuckDB {
                options: DuckDBOptions::Local {
                    file_search_path: dir.path().to_str().unwrap().to_string(),
                },
                s3_mirror: None,
            }),
        };
        let secrets = SecretsManager::from_environment().unwrap();
        let config = ConfigBuilder::new()
            .with_workspace_path(dir.path())
            .unwrap()
            .build_with_working_copy(oxy::config::Origin::Disk, oxy::config::OnMissing::Empty)
            .await
            .unwrap();
        let result = build_adapter_from_db(&db, &config, &secrets, "main").await;
        assert!(result.is_err());
        assert!(
            result.err().unwrap().to_string().contains("read-only"),
            "expected read-only error for DuckDB Local"
        );
    }

    #[tokio::test]
    async fn test_build_adapter_from_db_snowflake_routes_correctly() {
        use oxy::config::ConfigBuilder;
        use tempfile::TempDir;
        let dir = TempDir::new().unwrap();
        let db = Database {
            name: "test_snowflake".to_string(),
            database_type: DatabaseType::Snowflake(Snowflake {
                account: "testaccount.us-east-1".to_string(),
                username: "user".to_string(),
                warehouse: "compute_wh".to_string(),
                database: "analytics".to_string(),
                schema: None,
                role: None,
                auth_type: SnowflakeAuthType::Password {
                    password: "secret".to_string(),
                },
                datasets: Default::default(),
                filters: Default::default(),
            }),
        };
        let secrets = SecretsManager::from_environment().unwrap();
        let config = ConfigBuilder::new()
            .with_workspace_path(dir.path())
            .unwrap()
            .build_with_working_copy(oxy::config::Origin::Disk, oxy::config::OnMissing::Empty)
            .await
            .unwrap();
        let result = build_adapter_from_db(&db, &config, &secrets, "dbt_dev").await;
        // May require snowflake-connector-python — verify routing is correct regardless
        if let Err(e) = result {
            assert!(
                !e.to_string().contains("not yet supported"),
                "expected connector/bridge error, got routing error: {e}"
            );
        }
    }

    #[tokio::test]
    async fn test_build_adapter_from_db_mysql() {
        use oxy::config::ConfigBuilder;
        use tempfile::TempDir;
        let dir = TempDir::new().unwrap();
        let db = Database {
            name: "test_mysql".to_string(),
            database_type: DatabaseType::Mysql(Mysql {
                host: Some("mysql.example.com".to_string()),
                port: Some("3306".to_string()),
                user: Some("root".to_string()),
                password: Some("secret".to_string()),
                database: Some("analytics".to_string()),
                ..Default::default()
            }),
        };
        let secrets = SecretsManager::from_environment().unwrap();
        let config = ConfigBuilder::new()
            .with_workspace_path(dir.path())
            .unwrap()
            .build_with_working_copy(oxy::config::Origin::Disk, oxy::config::OnMissing::Empty)
            .await
            .unwrap();
        let result = build_adapter_from_db(&db, &config, &secrets, "dbt_dev").await;
        assert!(result.is_ok(), "MySQL adapter failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_build_adapter_from_db_clickhouse() {
        use oxy::config::ConfigBuilder;
        use tempfile::TempDir;
        let dir = TempDir::new().unwrap();
        let db = Database {
            name: "test_ch".to_string(),
            database_type: DatabaseType::ClickHouse(ClickHouse {
                host: Some("ch.example.com".to_string()),
                user: Some("default".to_string()),
                password: Some("secret".to_string()),
                database: Some("analytics".to_string()),
                ..Default::default()
            }),
        };
        let secrets = SecretsManager::from_environment().unwrap();
        let config = ConfigBuilder::new()
            .with_workspace_path(dir.path())
            .unwrap()
            .build_with_working_copy(oxy::config::Origin::Disk, oxy::config::OnMissing::Empty)
            .await
            .unwrap();
        let result = build_adapter_from_db(&db, &config, &secrets, "dbt_dev").await;
        assert!(
            result.is_ok(),
            "ClickHouse adapter failed: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    async fn test_build_adapter_from_db_motherduck_routes_correctly() {
        use oxy::config::ConfigBuilder;
        use tempfile::TempDir;
        let dir = TempDir::new().unwrap();
        let db = Database {
            name: "test_md".to_string(),
            database_type: DatabaseType::MotherDuck(MotherDuck {
                token_var: "MOTHERDUCK_TEST_TOKEN".to_string(),
                database: Some("mydb".to_string()),
                schemas: Default::default(),
            }),
        };
        unsafe { std::env::set_var("MOTHERDUCK_TEST_TOKEN", "fake_token_for_test") };
        let secrets = SecretsManager::from_environment().unwrap();
        let config = ConfigBuilder::new()
            .with_workspace_path(dir.path())
            .unwrap()
            .build_with_working_copy(oxy::config::Origin::Disk, oxy::config::OnMissing::Empty)
            .await
            .unwrap();
        let result = build_adapter_from_db(&db, &config, &secrets, "dbt_dev").await;
        // MotherDuck authenticates at construction — with a fake token it fails at auth, not routing
        if let Err(e) = result {
            assert!(
                !e.to_string().contains("not yet supported"),
                "expected auth error, got routing error: {e}"
            );
        }
    }

    #[tokio::test]
    async fn test_build_adapter_from_db_postgres_routes_correctly() {
        use oxy::config::ConfigBuilder;
        use tempfile::TempDir;
        let dir = TempDir::new().unwrap();
        let db = Database {
            name: "test_pg".to_string(),
            database_type: DatabaseType::Postgres(Postgres {
                host: Some("127.0.0.1".to_string()),
                port: Some("15432".to_string()),
                user: Some("test_user".to_string()),
                password: Some("test_pass".to_string()),
                database: Some("test_db".to_string()),
                ..Default::default()
            }),
        };
        let secrets = SecretsManager::from_environment().unwrap();
        let config = ConfigBuilder::new()
            .with_workspace_path(dir.path())
            .unwrap()
            .build_with_working_copy(oxy::config::Origin::Disk, oxy::config::OnMissing::Empty)
            .await
            .unwrap();
        let result = build_adapter_from_db(&db, &config, &secrets, "public").await;
        // May succeed (lazy pool) or fail with a connection error — never "not yet supported"
        if let Err(e) = result {
            assert!(
                !e.to_string().contains("not yet supported"),
                "expected connection error, got routing error: {e}"
            );
        }
    }

    #[tokio::test]
    async fn test_build_adapter_from_db_redshift_routes_correctly() {
        use oxy::config::ConfigBuilder;
        use tempfile::TempDir;
        let dir = TempDir::new().unwrap();
        let db = Database {
            name: "test_rs".to_string(),
            database_type: DatabaseType::Redshift(Redshift {
                host: Some("127.0.0.1".to_string()),
                port: Some("15439".to_string()),
                user: Some("test_user".to_string()),
                password: Some("test_pass".to_string()),
                database: Some("test_db".to_string()),
                ..Default::default()
            }),
        };
        let secrets = SecretsManager::from_environment().unwrap();
        let config = ConfigBuilder::new()
            .with_workspace_path(dir.path())
            .unwrap()
            .build_with_working_copy(oxy::config::Origin::Disk, oxy::config::OnMissing::Empty)
            .await
            .unwrap();
        let result = build_adapter_from_db(&db, &config, &secrets, "public").await;
        // Redshift uses the Postgres adapter — may succeed (lazy pool) or fail at connection
        if let Err(e) = result {
            assert!(
                !e.to_string().contains("not yet supported"),
                "expected connection error, got routing error: {e}"
            );
        }
    }

    #[tokio::test]
    async fn test_build_adapter_from_db_bigquery_routes_correctly() {
        use oxy::config::ConfigBuilder;
        use std::io::Write;
        use tempfile::{NamedTempFile, TempDir};
        let dir = TempDir::new().unwrap();
        let mut key_file = NamedTempFile::new().unwrap();
        writeln!(
            key_file,
            r#"{{"type":"service_account","project_id":"my-gcp","private_key_id":"k1","private_key":"INVALID","client_email":"sa@p.iam.gserviceaccount.com","token_uri":"https://oauth2.googleapis.com/token"}}"#
        )
        .unwrap();
        let db = Database {
            name: "test_bq".to_string(),
            database_type: DatabaseType::Bigquery(BigQuery {
                key_path: Some(key_file.path().to_path_buf()),
                key_path_var: None,
                key_json_var: None,
                dataset: Some("dbt_dev".to_string()),
                datasets: Default::default(),
                dry_run_limit: None,
            }),
        };
        let secrets = SecretsManager::from_environment().unwrap();
        let config = ConfigBuilder::new()
            .with_workspace_path(dir.path())
            .unwrap()
            .build_with_working_copy(oxy::config::Origin::Disk, oxy::config::OnMissing::Empty)
            .await
            .unwrap();
        let result = build_adapter_from_db(&db, &config, &secrets, "dbt_dev").await;
        // BigQuery with an invalid private key fails at JWT signing — not at routing
        assert!(result.is_err(), "BigQuery with invalid key should fail");
        let err = result.err().unwrap();
        assert!(
            !err.to_string().contains("not yet supported"),
            "expected credential error, got routing error: {err}"
        );
    }
}
