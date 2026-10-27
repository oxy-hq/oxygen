//! Vendor-neutral connector configuration.
//!
//! [`ConnectorConfig`] describes *which* database/warehouse to connect to and
//! any connection-level parameters.  It contains no connector-crate types so
//! that this crate can be used without pulling in driver dependencies directly.
//!
//! `build_connector` reads this config and constructs a `Box<dyn DatabaseConnector>`.

use std::path::PathBuf;
use std::sync::Arc;

// ── DuckDB ────────────────────────────────────────────────────────────────────

/// Controls whether DuckDB loads files lazily (view) or eagerly (materialized).
#[derive(Debug, Clone, Default)]
pub enum DuckDbLoadStrategy {
    /// `CREATE TEMP VIEW` — zero memory, re-reads the file on each query.
    #[default]
    View,
    /// `CREATE TEMP TABLE AS SELECT *` — materialized in DuckDB's in-process memory.
    Materialized,
}

/// Configuration for a local DuckDB connector backed by CSV/Parquet files in a
/// directory.
#[derive(Debug, Clone)]
pub struct DuckDbConfig {
    /// Directory that contains the CSV / Parquet files to register as tables.
    pub data_dir: PathBuf,
    /// Whether files are loaded as views (lazy) or materialized temp tables.
    pub load_strategy: DuckDbLoadStrategy,
    /// Whether the connector may be a clone of the process's cached database
    /// for this directory (`DuckDbConnector::from_directory_cached`). `false`
    /// builds its own, as every connector did before the cache: right for a
    /// caller that writes before it reads, which would only move onto a
    /// private copy at its first statement — pre-aggregation builds.
    pub reuse_cached_directory: bool,
}

// ── Postgres / Redshift ───────────────────────────────────────────────────────

/// Already-resolved connection parameters for a PostgreSQL (or Redshift) database.
#[derive(Debug, Clone)]
pub struct PostgresConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub database: String,
    /// libpq `sslmode`, when the caller resolved one. `None` leaves
    /// `tokio_postgres`'s default of `prefer`.
    ///
    /// **`prefer` is a downgrade the server chooses.** It offers TLS and
    /// continues in plaintext if refused, silently — which is fine for a local
    /// container and wrong for a managed tenant. `oxy-oltp`'s resolver already
    /// decides `require` per provider and carries it on `AnalystConnection`;
    /// this field is what stops that decision being thrown away one struct
    /// later. Adding rustls made these connections *able* to negotiate TLS;
    /// only this makes them insist.
    pub sslmode: Option<String>,
}

// ── MySQL ─────────────────────────────────────────────────────────────────────

/// Already-resolved connection parameters for a MySQL / MariaDB database.
#[derive(Debug, Clone)]
pub struct MysqlConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub database: String,
}

// ── ClickHouse ────────────────────────────────────────────────────────────────

/// Already-resolved connection parameters for a ClickHouse HTTP endpoint.
#[derive(Debug, Clone)]
pub struct ClickHouseConfig {
    /// Full HTTP URL, e.g. `http://localhost:8123`.
    pub url: String,
    pub user: String,
    pub password: String,
    pub database: String,
}

// ── Snowflake ─────────────────────────────────────────────────────────────────

/// Callback invoked with the browser SSO URL during Snowflake external-browser
/// authentication. Wrap in `Arc` so the closure is `Clone + Debug`.
#[derive(Clone)]
pub struct SsoUrlCallback(pub Arc<dyn Fn(String) + Send + Sync>);

impl std::fmt::Debug for SsoUrlCallback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SsoUrlCallback(<fn>)")
    }
}

/// Authentication mode for a Snowflake connection.
#[derive(Debug, Clone)]
pub enum SnowflakeAuth {
    /// Username + password.
    Password { password: String },
    /// External browser (SSO / SAML). Token is cached in `cache_dir` across
    /// calls so subsequent connections skip the browser step.
    Browser {
        timeout_secs: u64,
        cache_dir: Option<PathBuf>,
        /// Fired once with the redirect URL so callers can stream it to the UI.
        sso_url_callback: Option<SsoUrlCallback>,
    },
}

/// Already-resolved connection parameters for Snowflake.
#[derive(Debug, Clone)]
pub struct SnowflakeConfig {
    pub account: String,
    pub username: String,
    pub auth: SnowflakeAuth,
    pub role: Option<String>,
    pub warehouse: String,
    pub database: Option<String>,
    pub schema: Option<String>,
}

// ── BigQuery ──────────────────────────────────────────────────────────────────

/// Already-resolved connection parameters for Google BigQuery.
#[derive(Debug, Clone)]
pub struct BigQueryConfig {
    /// Path to a service-account JSON key file.
    pub key_path: String,
    pub project_id: String,
    /// Datasets to expose for schema browsing. Empty means no schema browsing.
    /// Replaces the old single `dataset` field — callers should merge both
    /// `dataset` (legacy) and `datasets` (multi) from the oxy config here.
    pub datasets: Vec<String>,
}

// ── DOMO ──────────────────────────────────────────────────────────────────────

/// Already-resolved connection parameters for DOMO's REST query API.
#[derive(Debug, Clone)]
pub struct DomoConfig {
    /// Base URL for the DOMO API, e.g. `https://my-instance.domo.com/api`.
    /// Callers may also pass the bare subdomain (`"my-instance"`) if they
    /// build the URL via `DomoConfig::from_instance`.
    pub base_url: String,
    pub developer_token: String,
    /// Dataset ID to run queries against.
    pub dataset_id: String,
}

impl DomoConfig {
    /// Build a `DomoConfig` from an instance subdomain plus credentials.
    pub fn from_instance(
        instance: impl AsRef<str>,
        developer_token: impl Into<String>,
        dataset_id: impl Into<String>,
    ) -> Self {
        Self {
            base_url: format!("https://{}.domo.com/api", instance.as_ref()),
            developer_token: developer_token.into(),
            dataset_id: dataset_id.into(),
        }
    }
}

// ── DuckDB (raw init statements) ─────────────────────────────────────────────

/// Configuration for a DuckDB connector opened in-memory and initialised with
/// arbitrary SQL statements.
///
/// Use this for DuckLake (ATTACH via extension), custom extensions, or any
/// scenario where the caller pre-resolves secrets into plain SQL before handing
/// off to the connector layer.
#[derive(Debug, Clone)]
pub struct DuckDbRawConfig {
    /// SQL statements executed sequentially against a fresh in-memory connection.
    pub init_statements: Vec<String>,
}

// ── DuckDB (connection URL) ──────────────────────────────────────────────────

/// Configuration for a DuckDB connector opened from a connection URL string.
///
/// Suitable for MotherDuck (`md:mydb?motherduck_token=...`) or any DuckDB URL
/// that `Connection::open` accepts.
#[derive(Debug, Clone)]
pub struct DuckDbUrlConfig {
    /// Full connection URL passed to `duckdb::Connection::open`.
    pub url: String,
}

// ── Top-level enum ────────────────────────────────────────────────────────────

/// Which database/warehouse to connect to and how.
///
/// Constructed by the caller (e.g. route handler) after secret resolution and
/// handed to `build_connector()`, which returns a `Box<dyn DatabaseConnector>`.
#[derive(Debug, Clone)]
pub enum ConnectorConfig {
    DuckDb(DuckDbConfig),
    /// DuckDB opened in-memory with init SQL (DuckLake, extensions).
    DuckDbRaw(DuckDbRawConfig),
    /// DuckDB opened from a connection URL (MotherDuck).
    DuckDbUrl(DuckDbUrlConfig),
    Postgres(PostgresConfig),
    /// Redshift (Postgres-compatible wire protocol).
    Redshift(PostgresConfig),
    /// MySQL / MariaDB.
    Mysql(MysqlConfig),
    /// DOMO via its REST query API.
    Domo(DomoConfig),
    ClickHouse(ClickHouseConfig),
    Snowflake(SnowflakeConfig),
    BigQuery(BigQueryConfig),
}

impl ConnectorConfig {
    /// Convenience constructor: DuckDB in `View` mode pointed at `data_dir`.
    pub fn duckdb_from_dir(data_dir: impl Into<PathBuf>) -> Self {
        Self::DuckDb(DuckDbConfig {
            data_dir: data_dir.into(),
            load_strategy: DuckDbLoadStrategy::View,
            reuse_cached_directory: true,
        })
    }

    /// This config, building a directory database of its own rather than
    /// cloning the cached one (see [`DuckDbConfig::reuse_cached_directory`]).
    /// Every other kind is returned unchanged.
    pub fn without_cached_directory(self) -> Self {
        match self {
            Self::DuckDb(c) => Self::DuckDb(DuckDbConfig {
                reuse_cached_directory: false,
                ..c
            }),
            other => other,
        }
    }
}
