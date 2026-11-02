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
#[derive(Clone)]
pub enum SnowflakeAuth {
    /// Username + password.
    Password { password: String },
    /// Key-pair authentication with the private key's PEM contents, already
    /// read out of the secret store by the caller.
    PrivateKey { private_key_pem: String },
    /// External browser (SSO / SAML). Token is cached in `cache_dir` across
    /// calls so subsequent connections skip the browser step.
    Browser {
        timeout_secs: u64,
        cache_dir: Option<PathBuf>,
        /// Fired once with the redirect URL so callers can stream it to the UI.
        sso_url_callback: Option<SsoUrlCallback>,
    },
}

// By hand, so neither credential reaches a log through `{:?}`.
impl std::fmt::Debug for SnowflakeAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SnowflakeAuth::Password { .. } => f.write_str("Password { <redacted> }"),
            SnowflakeAuth::PrivateKey { .. } => f.write_str("PrivateKey { <redacted> }"),
            SnowflakeAuth::Browser {
                timeout_secs,
                cache_dir,
                sso_url_callback,
            } => f
                .debug_struct("Browser")
                .field("timeout_secs", timeout_secs)
                .field("cache_dir", cache_dir)
                .field("sso_url_callback", sso_url_callback)
                .finish(),
        }
    }
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
#[derive(Clone)]
pub struct BigQueryConfig {
    /// Path to a service-account JSON key file. Unused when `key_json` is set.
    pub key_path: String,
    /// The service-account key JSON itself, when the caller read it out of
    /// the secret store. Takes precedence over `key_path`; no file is read.
    pub key_json: Option<String>,
    pub project_id: String,
    /// Datasets to expose for schema browsing. Empty means no schema browsing.
    /// Replaces the old single `dataset` field — callers should merge both
    /// `dataset` (legacy) and `datasets` (multi) from the oxy config here.
    pub datasets: Vec<String>,
}

// By hand, so the key JSON never reaches a log through `{:?}`.
impl std::fmt::Debug for BigQueryConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BigQueryConfig")
            .field("key_path", &self.key_path)
            .field("key_json", &self.key_json.as_ref().map(|_| "<redacted>"))
            .field("project_id", &self.project_id)
            .field("datasets", &self.datasets)
            .finish()
    }
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
///
/// Statements that hold a secret must list it in `secret_values`: a failed
/// statement's error carries the statement and DuckDB's message, and both are
/// scrubbed of those values first, since either can quote the secret.
#[derive(Clone)]
pub struct DuckDbRawConfig {
    /// SQL statements executed sequentially against a fresh in-memory connection.
    pub init_statements: Vec<String>,
    /// The credentials `init_statements` carry, replaced by `***` in any error.
    pub secret_values: Vec<String>,
}

impl DuckDbRawConfig {
    /// `text` with each non-empty value in `secret_values` replaced by `***`,
    /// in each spelling it can take in DuckDB text: as written, inside a SQL
    /// literal (`'` doubled), and inside a quoted libpq value (`\` and `'`
    /// escaped). Longest spelling first, so a shorter one leaves no tail.
    pub fn redact(&self, text: &str) -> String {
        let mut out = text.to_string();
        for secret in self.secret_values.iter().filter(|s| !s.is_empty()) {
            let mut spellings = [
                secret.replace('\'', "''"),
                secret.replace('\\', "\\\\").replace('\'', "\\'"),
                secret.clone(),
            ];
            spellings.sort_by_key(|s| std::cmp::Reverse(s.len()));
            for spelling in spellings {
                out = out.replace(&spelling, "***");
            }
        }
        out
    }
}

// By hand, so neither the statements nor the secrets reach a log through `{:?}`.
impl std::fmt::Debug for DuckDbRawConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DuckDbRawConfig")
            .field("init_statements", &self.init_statements.len())
            .field("secret_values", &self.secret_values.len())
            .finish()
    }
}

// ── DuckDB (connection URL) ──────────────────────────────────────────────────

/// Configuration for a DuckDB connector opened from a connection URL string.
///
/// Suitable for MotherDuck (`md:mydb?motherduck_token=...`) or any DuckDB URL
/// that `Connection::open` accepts.
#[derive(Clone)]
pub struct DuckDbUrlConfig {
    /// Full connection URL passed to `duckdb::Connection::open`.
    pub url: String,
    /// Open the database read-only. Set for a workspace's file DuckDB on the
    /// cloud roles, where it is a fallback nothing writes into.
    pub read_only: bool,
}

impl DuckDbUrlConfig {
    /// `text` with the value of every credential parameter in `url`
    /// (`motherduck_token=…`, or any key naming a token, password or secret)
    /// replaced by `***`. DuckDB echoes the URL back in some open failures.
    pub fn redact(&self, text: &str) -> String {
        let Some((_, query)) = self.url.split_once('?') else {
            return text.to_string();
        };
        let mut out = text.to_string();
        for (key, value) in query.split('&').filter_map(|pair| pair.split_once('=')) {
            let key = key.to_ascii_lowercase();
            let is_credential = ["token", "password", "secret"]
                .iter()
                .any(|word| key.contains(word));
            if is_credential && !value.is_empty() {
                out = out.replace(value, "***");
            }
        }
        out
    }
}

// By hand, so a token in the URL never reaches a log through `{:?}`.
impl std::fmt::Debug for DuckDbUrlConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DuckDbUrlConfig")
            .field("url", &self.redact(&self.url))
            .field("read_only", &self.read_only)
            .finish()
    }
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
