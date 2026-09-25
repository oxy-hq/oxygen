//! Dispatch a [`SourceConfig`] into a concrete [`airway::SourceConnector`].
//!
//! The YAML schema is intentionally open: any `kind` string is
//! permitted at parse time, and dispatch happens here. Adding support
//! for a new airway source means adding one arm to
//! [`build_source_connector`]; the YAML surface stays unchanged.
//!
//! v1 wires up the generic airway sources (`rest_api`,
//! `filesystem`, `sql_database`, `clickhouse`, `postgres_cdc`). The
//! `clickhouse` arm targets ClickHouse's HTTP interface (JSONEachRow)
//! via airway's `SqlDatabaseSource::clickhouse` convenience
//! constructor — it carries discrete connection fields rather than a
//! single connection string, so it gets its own `kind` instead of
//! riding the `sql_database` backend enum. The vendor-specific
//! helpers (`shopify`, `github`, `stripe`, …) all build on top of
//! `RestApiSource` upstream — most can be expressed directly as a
//! `rest_api` config, so we defer wiring per-vendor sugar until there's
//! a real consumer asking for it.
//!
//! [`SourceConfig`]: crate::config::SourceConfig

use std::collections::BTreeMap;
use std::sync::Arc;

use airway::connector::SourceConnector;
use airway::connector::sources::besttime::{BestTimeConfig, besttime_source};
use airway::connector::sources::filesystem::{FilesystemSource, SourceFileFormat};
use airway::connector::sources::google_sheets::GoogleSheetsSource;
use airway::connector::sources::http_file::{HttpFileConfig, http_file_source};
use airway::connector::sources::netsuite::{NetSuiteCredentials, NetSuiteSource, SigningAlgorithm};
use airway::connector::sources::overpass::{OverpassConfig, overpass_source};
use airway::connector::sources::overture::{OvertureConfig, overture_source};
use airway::connector::sources::postgres_cdc::PostgresCdcSource;
use airway::connector::sources::quickbooks::{QuickBooksSource, SANDBOX_BASE_URL};
use airway::connector::sources::rest_api::{RestApiConfig, RestApiSource};
use airway::connector::sources::sp_api::{LwaCredentials, PartnerType, SpApiSource};
use airway::connector::sources::sql_database::{
    ClickHouseConn, DatabaseBackend, SqlDatabaseSource, TableConfig,
};
// Re-exported so `agentic-pipeline` / `agentic-http` can shape the
// discovery response without taking a direct `airway` dependency.
pub use airway::connector::sources::sql_database::{DiscoveredColumn, DiscoveredTable};
use airway::connector::sources::toast::ToastSource;
use airway::connector::sources::ubereats::UberEatsSource;
use airway::connector::sources::weather::{WeatherConfig, weather_source};
use airway::types::WriteDisposition;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;

use crate::config::SourceConfig;
use crate::error::AirwayError;

/// Write-back port for a rotated OAuth refresh token (QuickBooks).
///
/// Implemented by the host (`agentic-pipeline`'s executor) so the rotated
/// token can be persisted to the host's secret store between runs. Uses a
/// `String` error so implementors don't take a dependency on the `airway`
/// crate's error type — [`build_quickbooks`] bridges this to airway's own
/// `RefreshTokenSink` ([`AirwayRefreshSink`]).
#[async_trait]
pub trait RefreshTokenSink: Send + Sync {
    async fn persist(&self, refresh_token: &str) -> Result<(), String>;
}

/// Bridges a host [`RefreshTokenSink`] (String error) onto airway's
/// `RefreshTokenSink` (AirwayError), so the engine can drive it.
struct AirwayRefreshSink(Arc<dyn RefreshTokenSink>);

#[async_trait]
impl airway::connector::sources::quickbooks::RefreshTokenSink for AirwayRefreshSink {
    async fn persist(&self, refresh_token: &str) -> Result<(), airway::AirwayError> {
        self.0
            .persist(refresh_token)
            .await
            .map_err(airway::AirwayError::Extract)
    }
}

/// Read port for a host-maintained OAuth **access** token (QuickBooks).
///
/// The counterpart to [`RefreshTokenSink`], and mutually exclusive with it.
/// Supplying one puts the connector in read-only mode: it never calls Intuit's
/// token endpoint, because Intuit expires the previous refresh token whenever
/// it issues a new one, so a grant tolerates exactly one rotation writer. When
/// the host already runs that writer (for Poke House, a scheduled
/// `refresh-qb-token` Oxy Function), this pipeline must be a reader.
///
/// Implemented by `agentic-pipeline`'s executor over the secret store. Uses a
/// `String` error so implementors need no `airway` dependency —
/// [`build_quickbooks`] bridges it via [`AirwayAccessTokenSource`].
///
/// **Called per request, not once per run.** The executor resolves ordinary
/// `*_var` secrets once, before dispatch; doing that here would pin a ~60-minute
/// access token for a backfill that outlives it. This port is invoked on demand
/// so each call re-reads whatever the refresher last wrote.
///
/// Concretely, in airway ≥ 0.1.25: `QuickBooksAuth::authorization_header` is
/// called by every data-API request, and in read-only mode it returns from this
/// port before touching the token cache — nothing is memoised engine-side
/// (`readonly_mode_reasks_the_source_on_every_call` pins that). The only cache
/// in the path is `SecretManagerService`'s 300s TTL, so a rotation is picked up
/// within 5 minutes. That window is safe rather than merely tolerable: Poke
/// House's refresher fires every ~50 minutes against a 60-minute token, so the
/// value being served during it still has ~10 minutes of validity left.
#[async_trait]
pub trait AccessTokenSource: Send + Sync {
    async fn access_token(&self) -> Result<String, String>;
}

/// Which side of a QuickBooks grant's token custody this run takes.
///
/// An enum rather than two `Option`s because the two are mutually exclusive and
/// "both supplied" has no correct meaning — it would have to resolve to a
/// silent precedence rule, and the wrong branch of that rule is precisely the
/// failure this type exists to prevent (a second refresher forking the grant's
/// rotation chain). `None` at the call site means the source needs no token
/// hook at all, which is every non-QuickBooks source.
#[derive(Clone)]
pub enum QuickBooksTokens {
    /// This connector owns rotation: it refreshes at Intuit and writes the
    /// rotated refresh token back through the sink.
    Rotating(Arc<dyn RefreshTokenSink>),
    /// The host owns rotation: this connector only ever reads access tokens
    /// and never contacts Intuit's token endpoint.
    ReadOnly(Arc<dyn AccessTokenSource>),
}

/// Bridges a host [`AccessTokenSource`] (String error) onto airway's
/// `AccessTokenSource` (AirwayError), so the engine can drive it.
struct AirwayAccessTokenSource(Arc<dyn AccessTokenSource>);

#[async_trait]
impl airway::connector::sources::quickbooks::AccessTokenSource for AirwayAccessTokenSource {
    async fn access_token(&self) -> Result<String, airway::AirwayError> {
        self.0
            .access_token()
            .await
            .map_err(airway::AirwayError::Extract)
    }
}

/// Build the concrete [`SourceConnector`] for a parsed source config.
///
/// Returns a boxed trait object so the worker can hand it straight to
/// `airway::connector::parallel::extract_*` without committing to a
/// specific connector type at the worker layer.
///
/// `refresh_sink` is an optional write-back hook for OAuth refresh tokens
/// that the host supplies (only the `quickbooks` source consumes it; all
/// other arms ignore it). It lets a rotated refresh token be persisted to
/// the host's secret store between runs.
///
/// `environment` is the deployment-wide vendor-environment intent (see
/// [`resolve_quickbooks_base_url`]); only the `quickbooks` arm consumes it
/// today — every other arm ignores it, since QuickBooks is the only
/// connector currently declaring a sandbox host.
pub fn build_source_connector(
    config: &SourceConfig,
    tokens: Option<QuickBooksTokens>,
    environment: airway::connector::Environment,
) -> Result<Box<dyn SourceConnector>, AirwayError> {
    let (connector, applied) = build_source_connector_inner(config, tokens, environment)?;
    admit_environment_is_applied(config, environment, connector.as_ref(), applied)?;
    Ok(connector)
}

/// Whether the factory arm that built this connector resolved its base URL
/// from the run's [`Environment`](airway::connector::Environment).
///
/// The marker exists so [`admit_environment_is_applied`] can ask *"did an arm
/// apply it?"* instead of naming a connector kind. A kind named over in the
/// guard is a claim stored apart from the code that makes it true: it keeps
/// reading "handled" after the arm stops handling it, and it has to be
/// remembered again for every arm added later.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum EnvironmentApplied {
    /// The arm read `environment` and resolved this connector's base URL from
    /// it (including honouring an explicit per-source override).
    Yes,
    /// The arm ignored `environment`. Correct exactly while the connector
    /// declares no sandbox host — which is what the guard verifies.
    No,
}

/// Refuse a `sandbox` run whose connector declares a sandbox host that this
/// factory has no arm to apply.
///
/// Airway's `admit_with` checks that a connector *supports* the environment;
/// applying it is oxy's job, because airway resolves each source's sandbox host
/// from `Environment::installed()` — a process-wide global oxy never installs.
/// Today only the `quickbooks` arm applies one, and that is complete, since
/// QuickBooks is the sole connector in the pinned airway declaring
/// `sandbox_base_url()`. But the next airway bump that adds a host to any other
/// connector would otherwise produce the exact silent failure this module exists
/// to prevent: admission passes, and oxy leaves the source pointed at production.
///
/// Both halves are derived rather than listed — the connector states whether a
/// host exists, the arm states whether it applied one — so the guard stays true
/// without anyone remembering to update it.
fn admit_environment_is_applied(
    config: &SourceConfig,
    environment: airway::connector::Environment,
    connector: &dyn SourceConnector,
    applied: EnvironmentApplied,
) -> Result<(), AirwayError> {
    if !matches!(environment, airway::connector::Environment::Sandbox) {
        return Ok(());
    }
    if applied == EnvironmentApplied::Yes || connector.sandbox_base_url().is_none() {
        return Ok(());
    }
    Err(AirwayError::Other(format!(
        "source kind `{}` declares a sandbox host but this factory has no arm applying it, \
         so the run would pass admission and then talk to production. Add the mapping in \
         agentic_airway::source_factory alongside the quickbooks arm.",
        config.kind
    )))
}

fn build_source_connector_inner(
    config: &SourceConfig,
    tokens: Option<QuickBooksTokens>,
    environment: airway::connector::Environment,
) -> Result<(Box<dyn SourceConnector>, EnvironmentApplied), AirwayError> {
    // Dispatched ahead of the table below so the `Yes` is produced by the same
    // expression that passes `environment` in. An arm that stops taking
    // `environment` stops compiling here, rather than leaving a stale claim
    // behind in the guard.
    if config.kind == "quickbooks" {
        let connector = build_quickbooks(&config.config, tokens, environment)?;
        return Ok((connector, EnvironmentApplied::Yes));
    }

    // Every arm below ignores `environment`; `admit_environment_is_applied`
    // checks that against what the built connector actually declares.
    let connector = match config.kind.as_str() {
        "rest_api" => build_rest_api(&config.config),
        "filesystem" => build_filesystem(&config.config),
        "sql_database" => build_sql_database(&config.config),
        "clickhouse" => build_clickhouse(&config.config),
        "postgres_cdc" => build_postgres_cdc(&config.config),
        "toast" => build_toast(&config.config),
        "weather" => build_weather(&config.config),
        "besttime" => build_besttime(&config.config),
        "overture" => build_overture(&config.config),
        "http_file" => build_http_file(&config.config),
        "google_sheets" => build_google_sheets(&config.config),
        "overpass" => build_overpass(&config.config),
        "netsuite" => build_netsuite(&config.config),
        "sp_api" => build_sp_api(&config.config),
        "ubereats" => build_ubereats(&config.config),
        other => Err(AirwayError::Other(format!(
            "unsupported source kind `{other}`. Wire it up in \
             agentic_airway::source_factory::build_source_connector \
             — every airway source is fair game, this dispatch table \
             just enumerates the ones with a concrete arm so far."
        ))),
    }?;
    Ok((connector, EnvironmentApplied::No))
}

// ── rest_api ─────────────────────────────────────────────────────────────────

fn build_rest_api(raw: &Value) -> Result<Box<dyn SourceConnector>, AirwayError> {
    let config: RestApiConfig = serde_json::from_value(raw.clone())
        .map_err(|e| AirwayError::Other(format!("invalid rest_api config: {e}")))?;
    Ok(Box::new(RestApiSource::new(config)))
}

// ── ubereats ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UberEatsParams {
    /// Landing zone, or one report: `/data/ubereats`, `s3://bucket/zone`,
    /// `s3://bucket/zone/2026.08 UberEats SF.csv`.
    base_path: String,
    /// Store names in scope, matched per ROW — an API report section can span
    /// stores this tenancy does not own.
    ///
    /// Keyed on `store_name`, not `store_id`: reports exist where the ID is
    /// blank for an entire store, which is why it is not a JE-critical column.
    ///
    /// Absent means every store in the file loads — right for the one-store
    /// manual export, wrong for an API section, so it is opt-in rather than
    /// defaulted.
    #[serde(default)]
    allowed_stores: Option<Vec<String>>,
    /// Report year, when filenames and paths do not carry one.
    ///
    /// Source-wide, so it cannot serve a zone spanning periods — the normal
    /// path is to let the layout supply it (`.../2026.08/…` or
    /// `2026.08 UberEats SF.csv`).
    #[serde(default)]
    report_year: Option<i64>,
    #[serde(default)]
    report_month: Option<u32>,
    /// Extra row-id discriminator. Rarely needed: the file's path within the
    /// zone already separates chunks of one period.
    #[serde(default)]
    uid_salt: Option<String>,
    /// Optional table name (defaults to `ubereats_transactions`).
    #[serde(default)]
    table_name: Option<String>,
}

fn build_ubereats(raw: &Value) -> Result<Box<dyn SourceConnector>, AirwayError> {
    let params: UberEatsParams = serde_json::from_value(raw.clone())
        .map_err(|e| AirwayError::Other(format!("invalid ubereats config: {e}")))?;

    // Both or neither. A year without a month is not a period, and silently
    // taking one half would stamp a month the operator never named — the
    // failure `period_from_filename` refuses to guess at, arriving through
    // config instead of through a path.
    let period = match (params.report_year, params.report_month) {
        // One definition, shared with the upload path: a period nobody named
        // stamps a partition that cannot exist here, and produces an object key
        // the source's period scan cannot read there. Two copies of the bounds
        // would be free to drift.
        (Some(year), Some(month)) => {
            crate::report_validation::check_period(year, month)
                .map_err(|e| AirwayError::Other(format!("ubereats: {e}")))?;
            Some((year, month))
        }
        (None, None) => None,
        _ => {
            return Err(AirwayError::Other(
                "ubereats: `report_year` and `report_month` must be given \
                 together — half a period is not one, and guessing the other \
                 half stamps a month nobody named"
                    .to_string(),
            ));
        }
    };

    let mut source = UberEatsSource::new(&params.base_path);
    if let Some(stores) = params.allowed_stores {
        // Absent means "every store"; EMPTY would mean "no store", so a
        // pipeline that reads `allowed_stores: []` succeeds and lands zero
        // rows. That is the silent-wrong-shape failure this whole source is
        // built to avoid, and every sibling scoping list here guards it —
        // `restaurant_guids`, `locations`, `venue_ids`, `bboxes`.
        if stores.is_empty() {
            return Err(AirwayError::Other(
                "ubereats: `allowed_stores` was given but empty — an empty \
                 allow-list matches no store and would land zero rows. Omit \
                 the key to load every store in the file."
                    .to_string(),
            ));
        }
        source = source.with_allowed_stores(stores.into_iter().collect());
    }
    if let Some((year, month)) = period {
        source = source.with_period(year, month);
    }
    if let Some(salt) = params.uid_salt.as_deref() {
        source = source.with_uid_salt(salt);
    }
    if let Some(name) = params.table_name.as_deref() {
        source = source.with_table_name(name);
    }
    Ok(Box::new(source))
}

// ── filesystem ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FilesystemParams {
    /// Base path or URL (`/data`, `s3://bucket/prefix`, `gs://...`, `az://...`).
    base_path: String,
    /// Glob pattern (e.g. `*.jsonl`, `**/*.csv`).
    pattern: String,
    /// `json` | `jsonl` | `csv`.
    format: FilesystemFormatLabel,
    /// Optional table name (defaults to airway's `file_data`).
    #[serde(default)]
    table_name: Option<String>,
    /// JSON path to records inside each JSON document (e.g. `data.items`).
    #[serde(default)]
    json_path: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FilesystemFormatLabel {
    Json,
    Jsonl,
    Csv,
}

impl From<FilesystemFormatLabel> for SourceFileFormat {
    fn from(label: FilesystemFormatLabel) -> Self {
        match label {
            FilesystemFormatLabel::Json => SourceFileFormat::Json,
            FilesystemFormatLabel::Jsonl => SourceFileFormat::Jsonl,
            FilesystemFormatLabel::Csv => SourceFileFormat::Csv,
        }
    }
}

fn build_filesystem(raw: &Value) -> Result<Box<dyn SourceConnector>, AirwayError> {
    let params: FilesystemParams = serde_json::from_value(raw.clone())
        .map_err(|e| AirwayError::Other(format!("invalid filesystem config: {e}")))?;

    let mut source =
        FilesystemSource::new(&params.base_path, &params.pattern, params.format.into());
    if let Some(name) = params.table_name.as_deref() {
        source = source.with_table_name(name);
    }
    if let Some(jp) = params.json_path.as_deref() {
        source = source.with_json_path(jp);
    }
    Ok(Box::new(source))
}

// ── sql_database ─────────────────────────────────────────────────────────────

/// `sql_database` source over a single DSN.
///
/// The `agentic-pipeline` executor substitutes `connection_string_var` ->
/// `connection_string` from the secret manager before dispatch, so the factory
/// only ever sees the resolved literal — `connection_string_var` is therefore
/// not an accepted field here. Same arrangement as [`ClickHouseParams`] and
/// its `password_var`; stated here too because a reader of this crate alone
/// cannot otherwise tell that the upstream key exists.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SqlDatabaseParams {
    connection_string: String,
    backend: SqlBackendLabel,
    #[serde(default)]
    tables: Vec<SqlTableParams>,
}

/// Hand-written so a future `tracing::debug!(?params)` cannot print the DSN.
///
/// `connection_string` used to be a committed literal; it is now a live
/// per-org credential resolved from the secret manager, password included.
/// Nothing formats this struct today, so the derive was latent rather than a
/// bug — but the sibling credential-bearing params redact for exactly this
/// reason ([`NetSuiteParams`], [`SpApiParams`]), and the cheap moment to join
/// them is the diff that makes the field a secret.
impl std::fmt::Debug for SqlDatabaseParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqlDatabaseParams")
            .field("connection_string", &"<redacted>")
            // Not secrets, and both worth showing: they decide which engine is
            // dialled and what it reads, so a run that pulled nothing is
            // diagnosed from this line.
            .field("backend", &self.backend)
            .field("tables", &self.tables)
            .finish()
    }
}

/// Snake-case YAML labels for airway's `DatabaseBackend` variants.
/// Mirror is exhaustive — adding a backend in airway surfaces here.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SqlBackendLabel {
    Postgres,
    Mysql,
    Sqlite,
    Mssql,
    Oracle,
    Custom,
}

impl From<SqlBackendLabel> for DatabaseBackend {
    fn from(label: SqlBackendLabel) -> Self {
        match label {
            SqlBackendLabel::Postgres => DatabaseBackend::Postgres,
            SqlBackendLabel::Mysql => DatabaseBackend::MySQL,
            SqlBackendLabel::Sqlite => DatabaseBackend::SQLite,
            SqlBackendLabel::Mssql => DatabaseBackend::MSSQL,
            SqlBackendLabel::Oracle => DatabaseBackend::Oracle,
            SqlBackendLabel::Custom => DatabaseBackend::Custom,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SqlTableParams {
    name: String,
    #[serde(default)]
    schema: Option<String>,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    primary_key: Option<Vec<String>>,
    #[serde(default)]
    cursor_field: Option<String>,
    #[serde(default)]
    write_disposition: WriteDispositionLabel,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum WriteDispositionLabel {
    #[default]
    Append,
    Replace,
    Merge,
    /// Append to a buffer table in a sibling schema, `<schema>_raw.<table>`,
    /// then fold it into the public table latest-wins. The fold runs inline
    /// at the end of every successful load, and airhouse's scheduled vacuum
    /// runs the same fold again as a safety net — the vacuum is not the only
    /// thing that rebuilds the table. Avoids the O(target) `MERGE INTO` that
    /// OOMs the data plane on large tables.
    Replacing,
}

impl From<WriteDispositionLabel> for WriteDisposition {
    fn from(label: WriteDispositionLabel) -> Self {
        match label {
            WriteDispositionLabel::Append => WriteDisposition::Append,
            WriteDispositionLabel::Replace => WriteDisposition::Replace,
            WriteDispositionLabel::Merge => WriteDisposition::Merge,
            WriteDispositionLabel::Replacing => WriteDisposition::Replacing,
        }
    }
}

fn build_sql_database(raw: &Value) -> Result<Box<dyn SourceConnector>, AirwayError> {
    let params: SqlDatabaseParams = serde_json::from_value(raw.clone())
        .map_err(|e| AirwayError::Other(format!("invalid sql_database config: {e}")))?;

    let mut source = SqlDatabaseSource::new(&params.connection_string, params.backend.into());
    for table in params.tables {
        source = source.with_table(TableConfig {
            name: table.name,
            schema: table.schema,
            query: table.query,
            primary_key: table.primary_key,
            cursor_field: table.cursor_field,
            write_disposition: table.write_disposition.into(),
        });
    }
    Ok(Box::new(source))
}

// ── clickhouse ───────────────────────────────────────────────────────────────

/// ClickHouse source over the HTTP interface (`FORMAT JSONEachRow`).
///
/// Unlike `sql_database` (which takes a single `connection_string`),
/// ClickHouse carries discrete connection fields. The
/// `agentic-pipeline` executor substitutes `password_var` ->
/// `password` from the secret manager before dispatch, so the factory
/// only ever sees the resolved literal — `password_var` is therefore
/// not an accepted field here.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClickHouseParams {
    /// Host without scheme/port (e.g. `my-host.clickhouse.cloud`).
    host: String,
    /// HTTP(S) interface port. Defaults to airway's `8123`.
    #[serde(default)]
    port: Option<u16>,
    /// Database/schema to read from.
    database: String,
    /// Username. Defaults to airway's `default`.
    #[serde(default)]
    username: Option<String>,
    /// Resolved password literal (executor maps `password_var` -> this).
    #[serde(default)]
    password: Option<String>,
    /// Use HTTPS instead of HTTP. Defaults to airway's `false`.
    ///
    /// KNOWN / LOW-PRIORITY footgun for hand-authored YAML: a config with
    /// `port: 8443` and no `secure:` silently uses plaintext on a TLS port.
    /// The wizard template always emits `secure` and `port` together, so this
    /// is only reachable by hand-editing; left as-is deliberately.
    #[serde(default)]
    secure: Option<bool>,
    /// Client-side connect timeout in seconds. Defaults to airway's `30`.
    #[serde(default)]
    connect_timeout_secs: Option<u64>,
    /// Client-side read (inactivity) timeout in seconds. Defaults to airway's
    /// `120`. Resets on each received body chunk, so a steadily-streaming
    /// table of any size completes; only a stall longer than this aborts.
    /// Raise this when a slow destination back-pressures the read for long
    /// stretches (see also `settings.http_send_timeout`, which governs the
    /// ClickHouse *server* side of the same stall).
    #[serde(default)]
    read_timeout_secs: Option<u64>,
    /// Extra ClickHouse settings forwarded as URL query parameters on every
    /// request (e.g. `http_send_timeout`, `send_timeout`, `receive_timeout`,
    /// `max_execution_time`, `max_block_size`). Values may be written as
    /// numbers or strings in YAML; both are forwarded as strings.
    ///
    /// Socket timeouts MUST be set here rather than via a trailing SQL
    /// `SETTINGS` clause: ClickHouse fixes the HTTP response socket's timeout
    /// at request setup, before the query body's `SETTINGS` are parsed, so a
    /// `SETTINGS http_send_timeout=…` inside the SQL is ignored.
    #[serde(default)]
    settings: BTreeMap<String, Value>,
    /// Rows per streamed batch (one destination write). Defaults to airway's
    /// `10000`. Lower it when a slow destination back-pressures the read long
    /// enough to trip ClickHouse's socket send timeout — smaller writes drain
    /// faster, so the source read pauses for shorter stretches. `max_block_size`
    /// (a `settings` entry) does NOT do this: it tunes ClickHouse's output, not
    /// the destination-write size.
    #[serde(default)]
    batch_size: Option<usize>,
    /// Tables to extract. Reuses the `sql_database` table shape.
    #[serde(default)]
    tables: Vec<SqlTableParams>,
}

/// Build a `ClickHouseConn` from parsed params. All `ClickHouseConn`
/// fields are public; set them directly so an absent `password` stays
/// `None` (sending an empty `X-ClickHouse-Key` would differ from
/// sending no key at all).
fn clickhouse_conn(params: &ClickHouseParams) -> ClickHouseConn {
    let mut conn = ClickHouseConn::new(&params.host, &params.database);
    if let Some(port) = params.port {
        conn.port = port;
    }
    if let Some(username) = &params.username {
        conn.username = username.clone();
    }
    conn.password = params.password.clone();
    if let Some(secure) = params.secure {
        conn.secure = secure;
    }
    if let Some(secs) = params.connect_timeout_secs {
        conn.connect_timeout_secs = secs;
    }
    if let Some(secs) = params.read_timeout_secs {
        conn.read_timeout_secs = secs;
    }
    if let Some(n) = params.batch_size {
        conn.batch_size = n;
    }
    conn.settings = params
        .settings
        .iter()
        .map(|(k, v)| (k.clone(), json_value_to_setting(v)))
        .collect();
    conn
}

/// Render a YAML/JSON setting value as the string ClickHouse expects in a URL
/// query parameter. Strings pass through unquoted (`"600"` -> `600`); numbers
/// and bools use their plain literal so `http_send_timeout: 600` works without
/// the author needing to quote it.
fn json_value_to_setting(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn build_clickhouse(raw: &Value) -> Result<Box<dyn SourceConnector>, AirwayError> {
    let params: ClickHouseParams = serde_json::from_value(raw.clone())
        .map_err(|e| AirwayError::Other(format!("invalid clickhouse config: {e}")))?;

    let mut source = SqlDatabaseSource::clickhouse(clickhouse_conn(&params));
    for table in params.tables {
        source = source.with_table(TableConfig {
            name: table.name,
            schema: table.schema,
            query: table.query,
            primary_key: table.primary_key,
            cursor_field: table.cursor_field,
            write_disposition: table.write_disposition.into(),
        });
    }
    Ok(Box::new(source))
}

// ── discovery ────────────────────────────────────────────────────────────────

/// Connect to a source and list its tables (with columns) so a
/// pipeline-create UI can offer them for selection instead of making
/// the user hand-type table names. The `config` carries live
/// credentials supplied at wizard time — nothing is persisted here.
///
/// Only sources that support live introspection are wired; today that's
/// `clickhouse`. Other kinds return an error rather than silently
/// yielding nothing.
pub async fn discover_source_tables(
    config: &SourceConfig,
) -> Result<Vec<DiscoveredTable>, AirwayError> {
    match config.kind.as_str() {
        "clickhouse" => discover_clickhouse_tables(&config.config).await,
        other => Err(AirwayError::Other(format!(
            "table discovery is not supported for source kind `{other}`"
        ))),
    }
}

async fn discover_clickhouse_tables(raw: &Value) -> Result<Vec<DiscoveredTable>, AirwayError> {
    let params: ClickHouseParams = serde_json::from_value(raw.clone())
        .map_err(|e| AirwayError::Other(format!("invalid clickhouse config: {e}")))?;
    SqlDatabaseSource::clickhouse(clickhouse_conn(&params))
        .discover_tables()
        .await
        .map_err(|e| AirwayError::Other(format!("clickhouse discovery failed: {e}")))
}

// ── postgres_cdc ─────────────────────────────────────────────────────────────

/// `postgres_cdc` source. Shares the `connection_string_var` substitution
/// described on [`SqlDatabaseParams`] — the executor resolves it before
/// dispatch, so the factory only ever sees the literal.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PostgresCdcParams {
    connection_string: String,
    slot_name: String,
    publication_name: String,
    #[serde(default)]
    tables: Vec<String>,
    #[serde(default)]
    batch_size: Option<usize>,
    #[serde(default)]
    initial_snapshot: Option<bool>,
}

/// Redacts the DSN for the same reason as [`SqlDatabaseParams`]'s impl.
impl std::fmt::Debug for PostgresCdcParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PostgresCdcParams")
            .field("connection_string", &"<redacted>")
            // Server-side object names rather than credentials, and they are
            // what decides which changes the pipeline sees — a slot or
            // publication naming the wrong thing is diagnosed from here.
            .field("slot_name", &self.slot_name)
            .field("publication_name", &self.publication_name)
            .field("tables", &self.tables)
            .field("batch_size", &self.batch_size)
            .field("initial_snapshot", &self.initial_snapshot)
            .finish()
    }
}

fn build_postgres_cdc(raw: &Value) -> Result<Box<dyn SourceConnector>, AirwayError> {
    let params: PostgresCdcParams = serde_json::from_value(raw.clone())
        .map_err(|e| AirwayError::Other(format!("invalid postgres_cdc config: {e}")))?;

    let mut source = PostgresCdcSource::new(
        &params.connection_string,
        &params.slot_name,
        &params.publication_name,
    );
    if !params.tables.is_empty() {
        source = source.with_tables(params.tables);
    }
    if let Some(size) = params.batch_size {
        source = source.with_batch_size(size);
    }
    if let Some(snap) = params.initial_snapshot {
        source = source.with_initial_snapshot(snap);
    }
    Ok(Box::new(source))
}

// ── netsuite ─────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NetSuiteParams {
    /// NetSuite account id, e.g. `4544316` or `1234567_SB1`. An identifier,
    /// not a secret — it is visible in every API hostname.
    account_id: String,
    /// Client ID (Consumer Key) from the integration record. An identifier.
    client_id: String,
    /// Certificate ID from the OAuth 2.0 Client Credentials (M2M) Setup page.
    /// Travels as the JWT `kid`; an identifier, not the key itself.
    certificate_id: String,
    /// PEM-encoded **private** key matching the uploaded certificate.
    ///
    /// The `agentic-pipeline` executor substitutes `private_key_var` -> this
    /// field from the secret manager before dispatch, so the factory only ever
    /// sees the resolved literal — the same shape as toast's `client_secret`.
    ///
    /// A multi-line PEM rather than a token, which is the one thing to know
    /// about it: whatever stores the secret must preserve newlines. A key that
    /// arrives re-wrapped fails at construction naming the credential, rather
    /// than later as an opaque signature error.
    ///
    /// **`default` is load-bearing.** The executor treats a resolved-but-empty
    /// secret as unset and skips the insert entirely, so the field arrives
    /// *absent* rather than empty. Without a default, serde refuses first with
    /// `missing field \`private_key_pem\``, which names the struct rather than
    /// the secret an operator has to go and fix. Defaulting collapses absent
    /// into `""` so both land on the explicit message in [`build_netsuite`].
    #[serde(default)]
    private_key_pem: String,
    /// Signature algorithm; must match the key type. `PS256` (RSA) when absent,
    /// which is the pairing Oracle's own setup guide walks through.
    #[serde(default)]
    algorithm: Option<String>,
    /// Cold-start lookback for the cursored resources, in days. Widen for a
    /// deliberate backfill; airway defaults to 90 when absent.
    #[serde(default)]
    lookback_days: Option<i64>,
    /// Restrict to a subset of resources, **narrowing the connector itself**.
    ///
    /// There is already a kind-agnostic `resources:` at the top level of
    /// `.airway.yml` ([`crate::config::AirwayPipelineSpec`]), and for merely
    /// skipping a large table that one is the right knob — it is the same list
    /// for every source kind. Row count is *not* what makes this one useful.
    ///
    /// The difference is **when** each applies. This list is handed to the
    /// connector inside `build_netsuite`, so `resources()` and `contracts()`
    /// are already narrowed by the time `Source::try_from_connector_with` runs
    /// admission (`worker.rs`). The top-level list is applied *after* that, so
    /// a tightened `ContractPolicy` still judges the full resource set. Reach
    /// for this one when the subset must be visible to admission; reach for the
    /// top-level one otherwise.
    ///
    /// **They compose by intersection, and a disjoint pair extracts nothing
    /// without erroring.** Setting both is rarely what anyone means.
    #[serde(default)]
    resources: Option<Vec<String>>,
}

/// Hand-written so a `{params:?}` in future maintenance cannot put a private
/// key in the logs.
///
/// `ToastParams` derives `Debug` while holding `client_secret`, so this is a
/// departure from the neighbouring shape rather than a house rule — but the
/// argument is the same one this file's own tests make about
/// `Box<dyn SourceConnector>` implementing no `Debug`, and a PEM is the
/// credential with the longest blast radius here.
impl std::fmt::Debug for NetSuiteParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NetSuiteParams")
            .field("account_id", &self.account_id)
            .field("client_id", &self.client_id)
            .field("certificate_id", &self.certificate_id)
            .field("private_key_pem", &"<redacted>")
            .field("algorithm", &self.algorithm)
            .field("lookback_days", &self.lookback_days)
            .field("resources", &self.resources)
            .finish()
    }
}

fn build_netsuite(raw: &Value) -> Result<Box<dyn SourceConnector>, AirwayError> {
    let params: NetSuiteParams = serde_json::from_value(raw.clone())
        .map_err(|e| AirwayError::Other(format!("invalid netsuite config: {e}")))?;

    // Covers both shapes an unset secret can take. The executor treats a
    // resolved-but-empty secret as *unset* and skips the field insert
    // (`resolve_airway_source_secrets`), so it arrives absent — which is why
    // the field carries `#[serde(default)]`; a hand-written empty or
    // whitespace value in YAML arrives as itself. Either way the default
    // failure would name the struct field rather than the secret an operator
    // has to fix, so it is named explicitly here.
    if params.private_key_pem.trim().is_empty() {
        return Err(AirwayError::Other(
            concat!(
                "netsuite config: `private_key_pem` is empty or unset — set ",
                "`private_key_var` to a secret that is set, and check the stored ",
                "value kept the PEM's newlines",
            )
            .into(),
        ));
    }

    let algorithm = match params.algorithm.as_deref() {
        Some(name) => SigningAlgorithm::parse(name).map_err(AirwayError::Other)?,
        None => SigningAlgorithm::default(),
    };

    // Rejected rather than passed through: a zero or negative lookback puts the
    // cold-start window at or after now, so the first run of a new pipeline
    // returns nothing and looks like an empty account.
    if let Some(days) = params.lookback_days
        && days <= 0
    {
        return Err(AirwayError::Other(format!(
            "netsuite config: `lookback_days` must be positive, got {days} \
             (a non-positive window starts at or after now and extracts nothing)"
        )));
    }

    // Mirrors the `restaurant_guids` empty-check in `build_toast`. An empty
    // list here means "all" at the top level of `.airway.yml` but reaches the
    // connector verbatim from this one — the same key spelling with two
    // meanings, so the ambiguous form is refused rather than silently
    // resolved either way.
    if let Some(resources) = params.resources.as_deref()
        && resources.is_empty()
    {
        return Err(AirwayError::Other(
            concat!(
                "netsuite config: `resources` is present but empty. Omit the ",
                "key to extract every resource, or list the ones you want.",
            )
            .into(),
        ));
    }

    let mut source = NetSuiteSource::new(NetSuiteCredentials {
        account_id: params.account_id,
        client_id: params.client_id,
        certificate_id: params.certificate_id,
        private_key_pem: params.private_key_pem,
        algorithm,
    })?;
    if let Some(days) = params.lookback_days {
        source = source.with_lookback_days(days);
    }
    if let Some(resources) = params.resources {
        source = source.with_resources(resources)?;
    }
    Ok(Box::new(source))
}

// ── toast ────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToastParams {
    /// Toast OAuth2 client id (an identifier, not a secret).
    client_id: String,
    /// Toast OAuth2 client secret. The `agentic-pipeline` executor
    /// substitutes `client_secret_var` -> this field from the secret
    /// manager before dispatch, so the factory only ever sees the
    /// resolved literal.
    client_secret: String,
    /// One entry per restaurant GUID to fan the pull across.
    restaurant_guids: Vec<String>,
    /// Sandbox / non-prod override. Defaults to airway's prod base URL.
    #[serde(default)]
    base_url: Option<String>,
    /// Bounded-backfill window `[start, end)` (RFC3339), injected by the
    /// executor for a backfill run. Both-or-neither; absent for normal
    /// incremental runs. See [`parse_backfill_window`].
    #[serde(default)]
    backfill_start: Option<String>,
    #[serde(default)]
    backfill_end: Option<String>,
}

fn build_toast(raw: &Value) -> Result<Box<dyn SourceConnector>, AirwayError> {
    let params: ToastParams = serde_json::from_value(raw.clone())
        .map_err(|e| AirwayError::Other(format!("invalid toast config: {e}")))?;
    if params.restaurant_guids.is_empty() {
        return Err(AirwayError::Other(
            "toast config: `restaurant_guids` must list at least one restaurant".into(),
        ));
    }
    let mut source = ToastSource::new(
        params.client_id,
        params.client_secret,
        params.restaurant_guids,
    );
    if let Some(base) = params.base_url.as_deref() {
        source = source.with_base_url(base);
    }
    if let Some((start, end)) = parse_backfill_window(&params.backfill_start, &params.backfill_end)?
    {
        // Toast backfills are RESUMABLE: the cursor advances into the run's
        // resume_state so a reset-in-place retry resumes mid-window. This MUST
        // be paired with the run-scoped state store (the worker selects it for
        // backfill runs) — otherwise the advanced cursor would corrupt the live
        // pipeline cursor, the exact hazard the non-resumable freeze guards.
        source = source.with_resumable_backfill_window(start, end);
    }
    Ok(Box::new(source))
}

/// Parse an optional RFC3339 `[start, end)` backfill window from source config.
///
/// Both-or-neither: `Ok(None)` when neither bound is set (normal run),
/// `Ok(Some(..))` when both parse, and an error if only one is present or
/// either fails to parse. Used by the date-windowed source builders.
fn parse_backfill_window(
    start: &Option<String>,
    end: &Option<String>,
) -> Result<Option<(DateTime<Utc>, DateTime<Utc>)>, AirwayError> {
    let parse = |s: &str| {
        DateTime::parse_from_rfc3339(s)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|e| AirwayError::Other(format!("invalid backfill timestamp `{s}`: {e}")))
    };
    match (start.as_deref(), end.as_deref()) {
        (None, None) => Ok(None),
        (Some(s), Some(e)) => Ok(Some((parse(s)?, parse(e)?))),
        _ => Err(AirwayError::Other(
            "backfill window requires both `backfill_start` and `backfill_end`".into(),
        )),
    }
}

// ── quickbooks ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct QuickBooksParams {
    /// Intuit OAuth2 client id (an identifier, not a secret).
    client_id: String,
    /// Intuit OAuth2 client secret. The `agentic-pipeline` executor
    /// substitutes `client_secret_var` -> this field from the secret
    /// manager before dispatch, so the factory only sees the resolved
    /// literal.
    ///
    /// Required in rotating mode, unused in read-only mode (the data API
    /// authenticates with the bearer token alone) — hence optional here and
    /// enforced per-mode in [`build_quickbooks`].
    #[serde(default)]
    client_secret: Option<String>,
    /// Bootstrap refresh token (rotates on first use). Resolved from
    /// `refresh_token_var` by the executor; the rotated value is written
    /// back via the supplied [`RefreshTokenSink`]. Same per-mode optionality
    /// as `client_secret`.
    #[serde(default)]
    refresh_token: Option<String>,
    /// Name of the secret holding a host-maintained **access** token.
    ///
    /// Its presence in the YAML is what selects read-only mode. Unlike every
    /// other `*_var`, the executor does **not** substitute it into this struct:
    /// it builds an [`AccessTokenSource`] that re-reads per request, because an
    /// access token lives ~60 minutes and a backfill can outlive it. The field
    /// is declared here only so `deny_unknown_fields` accepts the YAML; the
    /// value is read by the executor, not by this factory.
    #[serde(default)]
    access_token_var: Option<String>,
    /// QuickBooks company id. Accepts a YAML string *or* a bare integer —
    /// realm ids are all-digits (e.g. 9341456860808037) and an unquoted
    /// value would otherwise fail deserialization ("invalid type: integer").
    #[serde(deserialize_with = "de_string_or_number")]
    realm_id: String,
    /// Sandbox override (`https://sandbox-quickbooks.api.intuit.com`).
    #[serde(default)]
    base_url: Option<String>,
    /// Optional API minor version (`?minorversion=`).
    #[serde(default, deserialize_with = "de_opt_string_or_number")]
    minor_version: Option<String>,
    /// Bounded-backfill window `[start, end)` (RFC3339), injected by the
    /// executor for a backfill run. Both-or-neither; absent for normal runs.
    #[serde(default)]
    backfill_start: Option<String>,
    #[serde(default)]
    backfill_end: Option<String>,
}

/// Deserialize a field that may appear as a YAML string or a bare number,
/// always yielding a `String`. Guards all-digit identifiers (realm id,
/// minor version) that YAML would otherwise parse as integers.
fn de_string_or_number<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrNumber {
        String(String),
        I64(i64),
        U64(u64),
    }
    Ok(match StringOrNumber::deserialize(deserializer)? {
        StringOrNumber::String(s) => s,
        StringOrNumber::I64(n) => n.to_string(),
        StringOrNumber::U64(n) => n.to_string(),
    })
}

fn de_opt_string_or_number<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Some(de_string_or_number(deserializer)?))
}

/// The base URL a QuickBooks source should use, or `None` to leave the
/// connector's own default in place.
///
/// `environment` is the deployment-wide *intent*; the host is per-vendor.
/// `admit_with` checks that a connector supports the environment, but
/// **applying** it is a separate step: airway resolves each source's sandbox
/// host from `Environment::installed()`, a process-wide global oxy never
/// installs. Without this, a `sandbox` run passes admission and then talks to
/// production — the one direction that must not fail silently.
///
/// An explicit `base_url` from the pipeline YAML is the narrower setting and
/// wins in either environment.
fn resolve_quickbooks_base_url(
    explicit: Option<&str>,
    environment: airway::connector::Environment,
) -> Option<String> {
    if let Some(base) = explicit {
        // Narrower wins — but say so. This pairing passes admission (QuickBooks
        // declares a sandbox host) and then sends production traffic, which is
        // the one direction this module's doc says must not fail silently.
        if matches!(environment, airway::connector::Environment::Sandbox)
            && base != SANDBOX_BASE_URL
        {
            tracing::warn!(
                base_url = %base,
                "environment is sandbox but an explicit base_url overrides it; \
                 requests go to that host, not the vendor sandbox"
            );
        }
        return Some(base.to_string());
    }
    matches!(environment, airway::connector::Environment::Sandbox)
        .then(|| SANDBOX_BASE_URL.to_string())
}

fn build_quickbooks(
    raw: &Value,
    tokens: Option<QuickBooksTokens>,
    environment: airway::connector::Environment,
) -> Result<Box<dyn SourceConnector>, AirwayError> {
    let params: QuickBooksParams = serde_json::from_value(raw.clone())
        .map_err(|e| AirwayError::Other(format!("invalid quickbooks config: {e}")))?;

    // FAIL CLOSED. `access_token_var` in the YAML is a declaration that some
    // other writer owns this grant's rotation. If the executor did not hand us
    // a matching source — an unresolvable secret, or a dispatch path that never
    // builds one — the only other way to authenticate is to refresh, which is
    // exactly what the declaration forbids. Refusing is the whole point: a
    // silent fallback would fork the rotation chain and brick the grant, and it
    // would do so at 3am on a schedule rather than here.
    let read_only_declared = params.access_token_var.is_some();
    let tokens = match (read_only_declared, tokens) {
        (true, Some(QuickBooksTokens::ReadOnly(src))) => Some(QuickBooksTokens::ReadOnly(src)),
        (true, other) => {
            let got = match other {
                Some(QuickBooksTokens::Rotating(_)) => "a refresh-token sink",
                _ => "nothing",
            };
            return Err(AirwayError::Other(format!(
                "quickbooks config declares `access_token_var` (read-only token custody) \
                 but the host supplied {got}. Refusing to fall back to refreshing: \
                 Intuit expires the previous refresh token whenever it issues a new one, \
                 so a second refresher would fork this grant's rotation chain."
            )));
        }
        (false, other) => other,
    };

    let mut source = match &tokens {
        // Read-only: the data API authenticates with the bearer token alone, so
        // the refresh-side credentials are genuinely unused. Passing empty
        // strings is safe *because* `with_access_token_source` below makes the
        // refresh path unreachable.
        Some(QuickBooksTokens::ReadOnly(_)) => {
            QuickBooksSource::new(params.client_id, "", "", params.realm_id)
        }
        _ => {
            let client_secret = params.client_secret.ok_or_else(|| {
                AirwayError::Other(
                    "quickbooks config: `client_secret_var` is required unless \
                     `access_token_var` selects read-only token custody"
                        .into(),
                )
            })?;
            let refresh_token = params.refresh_token.ok_or_else(|| {
                AirwayError::Other(
                    "quickbooks config: `refresh_token_var` is required unless \
                     `access_token_var` selects read-only token custody"
                        .into(),
                )
            })?;
            QuickBooksSource::new(
                params.client_id,
                client_secret,
                refresh_token,
                params.realm_id,
            )
        }
    };
    if let Some(base) = resolve_quickbooks_base_url(params.base_url.as_deref(), environment) {
        source = source.with_base_url(&base);
    }
    if let Some(mv) = params.minor_version.as_deref() {
        source = source.with_minor_version(mv);
    }
    match tokens {
        Some(QuickBooksTokens::Rotating(sink)) => {
            source = source.with_refresh_token_sink(Arc::new(AirwayRefreshSink(sink)));
        }
        Some(QuickBooksTokens::ReadOnly(src)) => {
            source = source.with_access_token_source(Arc::new(AirwayAccessTokenSource(src)));
        }
        None => {}
    }
    if let Some((start, end)) = parse_backfill_window(&params.backfill_start, &params.backfill_end)?
    {
        source = source.with_backfill_window(start, end);
    }
    Ok(Box::new(source))
}

// ── weather (Open-Meteo) ───────────────────────────────────────────────────────

fn build_weather(raw: &Value) -> Result<Box<dyn SourceConnector>, AirwayError> {
    // airway's `WeatherConfig` derives `Deserialize` and owns its own
    // defaults/validation, so — unlike toast/sql_database — we reuse it
    // directly rather than mirroring a params struct here.
    let config: WeatherConfig = serde_json::from_value(raw.clone())
        .map_err(|e| AirwayError::Other(format!("invalid weather config: {e}")))?;
    if config.locations.is_empty() {
        return Err(AirwayError::Other(
            "weather config: `locations` must list at least one location".into(),
        ));
    }
    let source = weather_source(config)
        .map_err(|e| AirwayError::Other(format!("weather source init failed: {e}")))?;
    Ok(Box::new(source))
}

// ── sp_api (Amazon Selling Partner API, report-based) ─────────────────────────

/// Marketplaces reachable on the North America endpoint.
///
/// `SpApiSource::new` pins `SP_API_HOST_NA` and `with_host` is not reachable
/// through it, so an EU or FE marketplace id would be sent to the NA host and
/// come back 403 — an auth-shaped error for what is really a routing mistake,
/// on a connector whose other 403 (an expired presigned url) is expected and
/// retried. Refused here by name instead, until the connector takes a host.
pub struct NaMarketplace {
    /// Amazon's marketplace id, written to `.airway.yml` verbatim.
    pub id: &'static str,
    /// ISO country code, for the refusal message.
    pub country: &'static str,
    /// Human name, for a picker.
    pub label: &'static str,
}

/// **The** list. Exported so the wizard's marketplace picker is driven from
/// here rather than from a second copy in TypeScript — a duplicate would go
/// stale silently the moment this one widens, and the UI would then be the
/// thing refusing a marketplace the connector accepts.
pub const NA_MARKETPLACES: &[NaMarketplace] = &[
    NaMarketplace {
        id: "ATVPDKIKX0DER",
        country: "US",
        label: "United States",
    },
    NaMarketplace {
        id: "A2EUQ1WTGCTBG2",
        country: "CA",
        label: "Canada",
    },
    NaMarketplace {
        id: "A1AM78C64UM0Y8",
        country: "MX",
        label: "Mexico",
    },
    NaMarketplace {
        id: "A2Q3Y263D9QSEX",
        country: "BR",
        label: "Brazil",
    },
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpApiParams {
    /// LWA application client id. An identifier: it names the APP, not the
    /// seller, and travels in every token request body.
    client_id: String,
    /// LWA application client secret.
    ///
    /// `default` for the same reason as netsuite's `private_key_pem`: the
    /// executor treats a resolved-but-empty secret as unset and skips the
    /// insert, so the field arrives absent. Without a default, serde refuses
    /// first with `missing field`, naming the struct rather than the secret an
    /// operator has to go and set.
    #[serde(default)]
    client_secret: String,
    /// The seller's LWA refresh token — the credential that authorizes access
    /// to THIS seller's data, and the one to rotate if access must be revoked.
    #[serde(default)]
    refresh_token: String,
    /// Marketplace id, e.g. `ATVPDKIKX0DER` (US) or `A2EUQ1WTGCTBG2` (CA).
    marketplace_id: String,
    /// Where a resource with no cursor starts, as `YYYY-MM-DD` or RFC 3339.
    ///
    /// Required rather than defaulted, deliberately. This connector pulls
    /// FORWARD only, so this single value is the entire backfill policy, and
    /// the two ways of getting it wrong are not symmetric.
    ///
    /// Too recent loses history quietly — it is absent with nothing to signal
    /// it, and only a cursor reset brings it back.
    ///
    /// Too far back is worse. `plan_pull` emits ONE `Window { start, end }` for
    /// the whole span — there is no chunking — and the cursor advances only on
    /// success. A window too large for Amazon to build within the poll budget,
    /// or too large to download inside the document deadline, therefore fails
    /// identically on every subsequent run. It stalls permanently rather than
    /// catching up.
    ///
    /// Neither failure is visible in the output, so it is not a decision to
    /// make on the operator's behalf.
    #[serde(default)]
    default_start: Option<String>,
    /// Bounded-backfill window `[start, end)` (RFC3339), injected by the
    /// executor for a backfill run. Both-or-neither; absent for normal
    /// incremental runs. See [`parse_backfill_window`].
    #[serde(default)]
    backfill_start: Option<String>,
    #[serde(default)]
    backfill_end: Option<String>,
    /// Which Amazon account the credentials above belong to: `seller` or
    /// `vendor`. Defaults to seller.
    ///
    /// A property of the CREDENTIAL, not of the run. Seller Central and Vendor
    /// Central are separate accounts with separate authorizations, and a
    /// refresh token belongs to one of them for as long as it exists.
    ///
    /// It decides which resources the pipeline can see AT ALL. `resources()`
    /// publishes only the reports the configured account can reach, so a
    /// pipeline that names no subset pulls that half and no more — which is
    /// why this is not merely a filter. Getting it wrong does not pull the
    /// wrong data; it pulls nothing, and every named resource is refused by
    /// name with a message saying which account it belongs to.
    ///
    /// The two therefore need SEPARATE pipelines rather than one with both
    /// sets of resources: one source holds one set of credentials, and those
    /// belong to one account.
    #[serde(default)]
    partner_type: PartnerType,
}

/// Hand-written: `{params:?}` must not put the seller's refresh token or the
/// app secret in a log. Same argument as `NetSuiteParams`.
impl std::fmt::Debug for SpApiParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpApiParams")
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .field("refresh_token", &"<redacted>")
            .field("marketplace_id", &self.marketplace_id)
            .field("default_start", &self.default_start)
            // Not a secret, and worth showing: it decides which resources the
            // pipeline can reach, so a run that pulled nothing is diagnosed
            // from this line.
            .field("partner_type", &self.partner_type)
            .finish()
    }
}

/// Parse `default_start` as a date or a full timestamp.
///
/// A bare `YYYY-MM-DD` is what an operator writes, and it means midnight UTC.
/// Split out so both accepted shapes are pinned by a test rather than by the
/// first pipeline that gets it wrong.
fn parse_default_start(raw: &str) -> Result<chrono::DateTime<chrono::Utc>, AirwayError> {
    let raw = raw.trim();
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(raw) {
        return Ok(dt.with_timezone(&chrono::Utc));
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
        return Ok(chrono::DateTime::from_naive_utc_and_offset(
            d.and_hms_opt(0, 0, 0).expect("midnight is a valid time"),
            chrono::Utc,
        ));
    }
    Err(AirwayError::Other(format!(
        "sp_api config: `default_start` must be `YYYY-MM-DD` or an RFC 3339 timestamp, \
         got {raw:?}"
    )))
}

fn build_sp_api(raw: &Value) -> Result<Box<dyn SourceConnector>, AirwayError> {
    let params: SpApiParams = serde_json::from_value(raw.clone())
        .map_err(|e| AirwayError::Other(format!("invalid sp_api config: {e}")))?;

    // Covers absent and hand-written-empty alike; see `client_secret`'s doc.
    for (name, value) in [
        ("client_secret", &params.client_secret),
        ("refresh_token", &params.refresh_token),
    ] {
        if value.trim().is_empty() {
            return Err(AirwayError::Other(format!(
                "sp_api config: `{name}` is empty or unset — set `{name}_var` to the secret \
                 name so the executor can resolve it before the pipeline runs"
            )));
        }
    }

    if !NA_MARKETPLACES
        .iter()
        .any(|m| m.id == params.marketplace_id)
    {
        let known: Vec<String> = NA_MARKETPLACES
            .iter()
            .map(|m| format!("{}={}", m.country, m.id))
            .collect();
        return Err(AirwayError::Other(format!(
            "sp_api config: `marketplace_id` {:?} is not a North America marketplace, and this \
             connector only reaches the NA endpoint — an EU/FE id would be sent to the NA host \
             and refused with a 403 that looks like an auth failure. Known: {}",
            params.marketplace_id,
            known.join(", ")
        )));
    }

    let default_start = match params.default_start.as_deref() {
        Some(raw) => parse_default_start(raw)?,
        None => {
            return Err(AirwayError::Other(
                "sp_api config: `default_start` is required — it is where a first run begins \
                 for a forward-only connector, and too recent loses history silently. A long span \
                 is now split into reports Amazon will finish, so it no longer stalls the way it \
                 once did; it does still cost one report per chunk on that first run"
                    .to_string(),
            ));
        }
    };

    let mut source = SpApiSource::new(
        LwaCredentials {
            client_id: params.client_id,
            client_secret: params.client_secret,
            refresh_token: params.refresh_token,
        },
        params.marketplace_id,
        default_start,
    )?;
    // Unconditional, not `if vendor`. The builder's default is `Seller` and so
    // is the field's, so passing it always keeps ONE place deciding — a
    // conditional would leave the seller case relying on two defaults agreeing,
    // in two crates, with nothing that fails when they stop.
    source = source.with_partner_type(params.partner_type);
    if let Some((start, end)) = parse_backfill_window(&params.backfill_start, &params.backfill_end)?
    {
        // RESUMABLE, and the builder is named for it — `with_resumable_backfill_window`,
        // not `with_backfill_window`. Toast's similarly-named builder ~400 lines above
        // is the FROZEN one, and two identically-named builders with opposite cursor
        // semantics in one file is a trap airway declined to set.
        //
        // Resumable is the right half of that choice here rather than a default:
        // a report costs Amazon minutes to build and the connector persists the
        // reportId across runs, so a backfill interrupted mid-window resumes at the
        // sub-window it reached. A frozen cursor discards that state on every retry,
        // which for this source means paying the build cost again from the start.
        //
        // It MUST be paired with the run-scoped state store — the executor selects it
        // when `resumable_backfill` is set — or the advanced cursor corrupts the live
        // pipeline cursor.
        source = source.with_resumable_backfill_window(start, end);
    }
    Ok(Box::new(source))
}

// ── besttime (POST-then-extract foot-traffic forecasts) ────────────────────────

fn build_besttime(raw: &Value) -> Result<Box<dyn SourceConnector>, AirwayError> {
    // airway's `BestTimeConfig` derives `Deserialize` and owns its own
    // validation; we reuse it directly (same shape as `weather`). The
    // agentic-pipeline executor substitutes `api_key_var` → `api_key` from
    // the secret manager before dispatch, so the factory only ever sees the
    // resolved literal — `api_key_var` is therefore not an accepted field
    // here.
    let config: BestTimeConfig = serde_json::from_value(raw.clone())
        .map_err(|e| AirwayError::Other(format!("invalid besttime config: {e}")))?;
    if config.venue_ids.is_empty() {
        return Err(AirwayError::Other(
            "besttime config: `venue_ids` must list at least one BestTime venue_id".into(),
        ));
    }
    let source = besttime_source(config)
        .map_err(|e| AirwayError::Other(format!("besttime source init failed: {e}")))?;
    Ok(Box::new(source))
}

// ── overture (Overture Maps Places, S3 GeoParquet) ─────────────────────────────

fn build_overture(raw: &Value) -> Result<Box<dyn SourceConnector>, AirwayError> {
    let config: OvertureConfig = serde_json::from_value(raw.clone())
        .map_err(|e| AirwayError::Other(format!("invalid overture config: {e}")))?;
    // Local empty-bboxes guard with a clear message, sibling to
    // build_besttime above. The upstream `overture_source` does its own
    // check but tests assert against this contract's wording; relying on
    // the upstream string pins our tests to a dependency we don't own.
    if config.bboxes.is_empty() {
        return Err(AirwayError::Other(
            "overture config: `bboxes` must list at least one bounding box".into(),
        ));
    }
    let source = overture_source(config)
        .map_err(|e| AirwayError::Other(format!("overture source init failed: {e}")))?;
    Ok(Box::new(source))
}

// ── http_file (generic HTTP/HTTPS file download via DuckDB) ────────────────────

fn build_http_file(raw: &Value) -> Result<Box<dyn SourceConnector>, AirwayError> {
    // airway's `HttpFileConfig` derives `Deserialize` and owns its own
    // validation; we reuse it directly (same shape as `weather` / `besttime`).
    let config: HttpFileConfig = serde_json::from_value(raw.clone())
        .map_err(|e| AirwayError::Other(format!("invalid http_file config: {e}")))?;
    let source = http_file_source(config)
        .map_err(|e| AirwayError::Other(format!("http_file source init failed: {e}")))?;
    Ok(Box::new(source))
}

// ── overpass (OpenStreetMap via Overpass API) ─────────────────────────────────

fn build_overpass(raw: &Value) -> Result<Box<dyn SourceConnector>, AirwayError> {
    let config: OverpassConfig = serde_json::from_value(raw.clone())
        .map_err(|e| AirwayError::Other(format!("invalid overpass config: {e}")))?;
    // Local empty-bboxes guard with a clear message, sibling to
    // build_overture above and build_besttime's venue_ids guard.
    if config.bboxes.is_empty() {
        return Err(AirwayError::Other(
            "overpass config: `bboxes` must list at least one bounding box".into(),
        ));
    }
    let source = overpass_source(config)
        .map_err(|e| AirwayError::Other(format!("overpass source init failed: {e}")))?;
    Ok(Box::new(source))
}

// ── google_sheets ─────────────────────────────────────────────────────────────

/// Authored config for the `google_sheets` source.
///
/// `access_token` is deliberately not something anyone writes in YAML. A Google
/// access token lives one hour, so a scheduled pipeline holding a stored token
/// would succeed once and then 401 forever. The executor mints a fresh one per
/// run from the service-account key named by `service_account_json_var` and
/// injects it here — see `resolve_google_sheets_auth` in agentic-pipeline.
/// That is also why the field carries `#[serde(default)]`: it is absent from
/// every file on disk and present only in the resolved spec.
#[derive(Debug, Deserialize)]
struct GoogleSheetsParams {
    /// From the sheet URL: `/spreadsheets/d/<THIS>/edit`.
    spreadsheet_id: String,
    #[serde(default)]
    access_token: String,
    /// Sheet names or A1 ranges (`Main`, `Main!A:S`). Each becomes one
    /// resource named after the part before `!`, lowercased with spaces
    /// turned into underscores — so `Main!A:S` is the resource `main`, which
    /// is the name `resources:` has to list. Empty means every sheet, landing
    /// as one resource called `sheet_data`.
    #[serde(default)]
    ranges: Vec<String>,
}

fn build_google_sheets(raw: &Value) -> Result<Box<dyn SourceConnector>, AirwayError> {
    let params: GoogleSheetsParams = serde_json::from_value(raw.clone())
        .map_err(|e| AirwayError::Other(format!("invalid google_sheets config: {e}")))?;
    // An empty token here means secret resolution did not run or the key was
    // missing. Failing with the reason beats handing the connector an empty
    // Bearer header and reading Google's 401 back as "spreadsheet not found".
    if params.access_token.is_empty() {
        return Err(AirwayError::Other(
            "google_sheets config: no access token was resolved. Set \
             `service_account_json_var` to the name of a secret holding the \
             service-account JSON key — the executor mints a short-lived \
             access token from it on every run."
                .into(),
        ));
    }
    // `None` and `Some(vec![])` mean different things to the connector: none
    // extracts every sheet under the single resource `sheet_data`, an empty
    // vec would produce no resources at all.
    let ranges = (!params.ranges.is_empty()).then_some(params.ranges);
    let source = GoogleSheetsSource::new(&params.spreadsheet_id, &params.access_token, ranges)
        .map_err(|e| AirwayError::Other(format!("google_sheets source init failed: {e}")))?;
    Ok(Box::new(source))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SourceConfig;
    use serde_json::json;

    use airway::connector::auth::AuthConfig;

    fn cfg(kind: &str, config: Value) -> SourceConfig {
        SourceConfig {
            kind: kind.to_string(),
            config,
        }
    }

    /// Build with no refresh-token sink (the common test case), under the
    /// default `Environment::Production`.
    fn build(config: &SourceConfig) -> Result<Box<dyn SourceConnector>, AirwayError> {
        build_source_connector(config, None, airway::connector::Environment::Production)
    }

    /// The field the executor resolves the managed secret into.
    ///
    /// Deserialized directly rather than through `build`, deliberately: building
    /// constructs an HTTP client, which reads the *process-global* deployment
    /// config — so a sibling test installing a `tls_ca_cert` makes this fail for
    /// reasons unrelated to what it asserts. `besttime`, `overpass` and
    /// `weather` already fail that way on `main`. Mirrors
    /// `rest_api_bearer_reads_resolved_token` below, which round-trips the
    /// config struct for the same reason.
    ///
    /// `NetSuiteParams` carries `deny_unknown_fields`, so a rename on either
    /// side of `("private_key_pem", "private_key_var")` fails here at CI rather
    /// than landing the key in an ignored field and authenticating with nothing.
    #[test]
    fn netsuite_params_read_the_resolved_private_key_field() {
        let params: NetSuiteParams = serde_json::from_value(json!({
            "account_id": "4544316",
            "client_id": "cid",
            "certificate_id": "kid",
            // Exactly what the executor's managed-secret table substitutes.
            "private_key_pem": "-----BEGIN PRIVATE KEY-----\nresolved\n-----END PRIVATE KEY-----\n",
            "lookback_days": 30,
            "resources": ["items", "locations"],
        }))
        .expect("deserialize");

        assert!(params.private_key_pem.contains("resolved"));
        assert_eq!(params.lookback_days, Some(30));
        assert_eq!(
            params.resources.as_deref(),
            Some(&["items".to_string(), "locations".to_string()][..])
        );
        // Identifiers, not secrets — the account id is in every API hostname.
        assert_eq!(params.account_id, "4544316");
        assert_eq!(params.certificate_id, "kid");
    }

    /// A misspelled key must be refused, not silently ignored: without
    /// `deny_unknown_fields` a typo'd optional field is dropped and the default
    /// used, which is invisible until the pull behaves unexpectedly.
    #[test]
    fn netsuite_params_refuse_an_unknown_field() {
        let err = serde_json::from_value::<NetSuiteParams>(json!({
            "account_id": "4544316",
            "client_id": "cid",
            "certificate_id": "kid",
            "private_key_pem": "x",
            "lookbackdays": 30,
        }))
        .expect_err("unknown field must be refused");
        assert!(err.to_string().contains("lookbackdays"), "{err}");
    }

    /// ES512 is a legitimate NetSuite choice this client cannot sign, so the
    /// error has to say so — the fix is a new key, not a new spelling. Rejected
    /// before any HTTP client is built, so this is independent of global state.
    #[test]
    fn netsuite_rejects_an_unsupported_algorithm_with_the_reason() {
        let built = build(&cfg(
            "netsuite",
            json!({
                "account_id": "4544316",
                "client_id": "cid",
                "certificate_id": "kid",
                "private_key_pem": "x",
                "algorithm": "ES512",
            }),
        ));
        // `match`, not `expect_err`: the `Ok` side is `Box<dyn SourceConnector>`,
        // which implements no `Debug` — and should not, holding credentials.
        let msg = match built {
            Ok(_) => panic!("ES512 must be refused"),
            Err(e) => e.to_string(),
        };
        assert!(msg.contains("ES512"), "{msg}");
        assert!(msg.contains("PS256"), "{msg}");
    }

    /// An sp_api config with everything valid except the one field under test,
    /// so each case fails for exactly one reason.
    fn sp_api_cfg(extra: serde_json::Map<String, Value>) -> SourceConfig {
        let mut obj = json!({
            "client_id": "amzn1.application-oa2-client.x",
            "client_secret": "secret",
            "refresh_token": "Atzr|refresh",
            "marketplace_id": "ATVPDKIKX0DER",
            "default_start": "2026-01-01",
        })
        .as_object()
        .expect("object")
        .clone();
        obj.extend(extra);
        cfg("sp_api", Value::Object(obj))
    }

    /// An EU/FE marketplace must be refused by NAME, not by a 403 later.
    ///
    /// `SpApiSource::new` pins the NA host and `with_host` is not reachable
    /// through it, so a non-NA id is sent to the NA endpoint and comes back
    /// 403 — indistinguishable from bad credentials, on a connector whose
    /// other 403 (an expired presigned url) is routine and retried. An
    /// operator would go and re-check the refresh token they had just set.
    #[test]
    fn sp_api_rejects_a_marketplace_the_pinned_host_cannot_reach() {
        // A1F83G8C2ARO7P = UK, on the EU endpoint.
        let msg = refusal(build(&sp_api_cfg(
            json!({ "marketplace_id": "A1F83G8C2ARO7P" })
                .as_object()
                .expect("object")
                .clone(),
        )));
        assert!(msg.contains("North America"), "{msg}");
        assert!(
            msg.contains("ATVPDKIKX0DER") && msg.contains("A2EUQ1WTGCTBG2"),
            "the refusal must list what IS reachable: {msg}"
        );

        // The two BMG actually uses are accepted.
        for id in ["ATVPDKIKX0DER", "A2EUQ1WTGCTBG2"] {
            assert!(
                build(&sp_api_cfg(
                    json!({ "marketplace_id": id })
                        .as_object()
                        .expect("object")
                        .clone()
                ))
                .is_ok(),
                "{id} is a NA marketplace"
            );
        }
    }

    /// `default_start` is the whole backfill policy, so it is not defaulted.
    ///
    /// The connector pulls forward only. Too recent and the history is simply
    /// absent with nothing to signal it; too far back and the first run spends
    /// report jobs against a budget that restores about once a minute. Neither
    /// shows up in the output, so guessing on the operator's behalf is worse
    /// than refusing.
    #[test]
    fn sp_api_requires_an_explicit_default_start() {
        let mut obj = sp_api_cfg(serde_json::Map::new());
        obj.config
            .as_object_mut()
            .expect("object")
            .remove("default_start");
        let msg = refusal(build(&obj));
        assert!(msg.contains("default_start"), "{msg}");
        // Asserted on the two CONSEQUENCES, not on the word "backfill" as it
        // was. That word left the message when chunking landed — this value
        // stopped being "the entire backfill policy" and became where history
        // starts — and a test pinned to the vocabulary would have failed for a
        // message that got MORE accurate.
        assert!(
            msg.contains("loses history"),
            "the refusal must name the silent cost of choosing too recent: {msg}"
        );
        assert!(
            msg.contains("report per chunk"),
            "and the visible cost of choosing too far back: {msg}"
        );
    }

    /// Both shapes an operator actually writes.
    #[test]
    fn sp_api_accepts_a_bare_date_and_an_rfc3339_timestamp() {
        assert_eq!(
            parse_default_start("2026-01-01").expect("bare date"),
            parse_default_start("2026-01-01T00:00:00Z").expect("rfc3339"),
            "a bare date means midnight UTC"
        );
        // A timezone offset is honoured rather than silently read as UTC.
        assert_eq!(
            parse_default_start("2026-01-01T00:00:00-05:00").expect("offset"),
            parse_default_start("2026-01-01T05:00:00Z").expect("utc"),
        );
        let msg = parse_default_start("01/01/2026")
            .expect_err("a US-format date is not accepted")
            .to_string();
        assert!(msg.contains("default_start"), "{msg}");
    }

    /// An unset secret names the secret, not the struct.
    ///
    /// The executor treats a resolved-but-empty secret as unset and skips the
    /// insert, so the field arrives ABSENT — which is why it carries
    /// `#[serde(default)]`. Without that, serde refuses first with
    /// `missing field`, which sends an operator to the wrong place.
    #[test]
    fn sp_api_names_the_credential_an_operator_has_to_set() {
        for (field, var) in [
            ("client_secret", "client_secret_var"),
            ("refresh_token", "refresh_token_var"),
        ] {
            let mut c = sp_api_cfg(serde_json::Map::new());
            c.config.as_object_mut().expect("object").remove(field);
            let msg = refusal(build(&c));
            assert!(msg.contains(field) && msg.contains(var), "{msg}");
        }
    }

    /// A typo must be refused, not silently ignored — a misspelled
    /// `marketplace` would otherwise leave the real field at its default and
    /// pull the wrong seller's marketplace.
    #[test]
    fn sp_api_params_refuse_an_unknown_field() {
        let msg = refusal(build(&sp_api_cfg(
            json!({ "marketplace": "ATVPDKIKX0DER" })
                .as_object()
                .expect("object")
                .clone(),
        )));
        assert!(msg.contains("marketplace"), "{msg}");
    }

    // The two roster tests that belonged here — that an absent `partner_type`
    // pulls the seller reports and `vendor` pulls the vendor ones — live in
    // `tests/integration/sp_api_partner_type_test.rs` instead.
    //
    // They have to call `build_source_connector`, which constructs an HTTP
    // client, which reads airway's process-global deployment config. That is a
    // `OnceCell`: set once per process, never reset. `deployment_config_tests`
    // in this crate installs a `tls_ca_cert` of `/etc/pki/ca.pem`, which does
    // not exist, so every later `build` in THIS binary fails on a TLS error
    // having nothing to do with what it was testing — six tests in this module
    // are red on `main` for that reason, including two of sp_api's own.
    //
    // An integration test gets its own binary and its own `OnceCell`. Putting
    // them there is the difference between covering the wiring and adding two
    // more names to a list of failures nobody reads.

    /// A misspelled partner type is refused rather than silently defaulted.
    ///
    /// The dangerous direction, and the reason this is worth a test of its own:
    /// `#[serde(default)]` means a value serde cannot read would otherwise
    /// become `Seller`, so a vendor pipeline with `partner_type: Vendor`
    /// (capitalised, as an operator reasonably might) would check the seller
    /// roster, be refused on every report for want of the role, and look like a
    /// credentials problem. `deny_unknown_fields` does not help — the field
    /// name is right and only the value is wrong.
    #[test]
    fn sp_api_refuses_a_partner_type_it_cannot_read() {
        for wrong in ["Vendor", "VENDOR", "vendour", "vendor_central"] {
            let msg = refusal(build(&sp_api_cfg(
                json!({ "partner_type": wrong })
                    .as_object()
                    .expect("object")
                    .clone(),
            )));
            // Serde names the VARIANT and the accepted set rather than the
            // field — "unknown variant `Vendor`, expected `seller` or
            // `vendor`" — which is the actionable half. Asserted on the
            // accepted values, because that is what an operator needs and it
            // is what would go missing if the enum ever lost its
            // `rename_all`.
            assert!(
                msg.contains("seller") && msg.contains("vendor"),
                "{wrong} must be refused with the values that ARE accepted: {msg}"
            );
        }
    }

    /// `{params:?}` must not put the seller's refresh token in a log.
    /// The DSN now carries a live per-org credential. Nothing formats this
    /// struct today, so this test is the thing that keeps it that way when
    /// someone adds a `tracing::debug!(?params)`.
    #[test]
    fn sql_database_params_debug_redacts_the_dsn() {
        let params = SqlDatabaseParams {
            connection_string: "postgres://u:SUPER_SECRET_PW@host/db".to_string(),
            backend: SqlBackendLabel::Postgres,
            // Present so a field added to the struct but not to the hand-written
            // `Debug` cannot slip through unread.
            tables: Vec::new(),
        };
        let rendered = format!("{params:?}");
        assert!(!rendered.contains("SUPER_SECRET_PW"), "{rendered}");
        assert!(!rendered.contains("postgres://"), "{rendered}");
        assert!(rendered.contains("<redacted>"), "{rendered}");
        // The non-secret fields must survive, or the redaction has cost the
        // line its diagnostic value.
        assert!(rendered.contains("backend"), "{rendered}");
    }

    /// Same credential, same reason. The slot and publication names are server
    /// -side objects, not secrets, and stay visible.
    #[test]
    fn postgres_cdc_params_debug_redacts_the_dsn_but_keeps_slot_and_publication() {
        let params = PostgresCdcParams {
            connection_string: "postgres://u:SUPER_SECRET_PW@host/db".to_string(),
            slot_name: "oxy_slot".to_string(),
            publication_name: "oxy_pub".to_string(),
            tables: vec!["public.orders".to_string()],
            batch_size: None,
            initial_snapshot: None,
        };
        let rendered = format!("{params:?}");
        assert!(!rendered.contains("SUPER_SECRET_PW"), "{rendered}");
        assert!(rendered.contains("<redacted>"), "{rendered}");
        assert!(rendered.contains("oxy_slot"), "{rendered}");
        assert!(rendered.contains("oxy_pub"), "{rendered}");
    }

    #[test]
    fn sp_api_params_debug_redacts_both_credentials() {
        let params = SpApiParams {
            client_id: "amzn1.application-oa2-client.x".to_string(),
            client_secret: "SUPER_SECRET_VALUE".to_string(),
            refresh_token: "Atzr|SUPER_SECRET_TOKEN".to_string(),
            marketplace_id: "ATVPDKIKX0DER".to_string(),
            default_start: Some("2026-01-01".to_string()),
            // Present so a future field cannot be added without this test
            // being read: `Debug` is hand-written, and a field added to the
            // struct but not to the impl is a credential one deploy away from
            // a log line.
            backfill_start: None,
            backfill_end: None,
            partner_type: PartnerType::Seller,
        };
        let rendered = format!("{params:?}");
        assert!(!rendered.contains("SUPER_SECRET_VALUE"), "{rendered}");
        assert!(!rendered.contains("SUPER_SECRET_TOKEN"), "{rendered}");
        assert!(rendered.contains("<redacted>"), "{rendered}");
        // Not a secret, and the one field that explains a run which pulled
        // nothing — so it must be IN the rendering, not merely absent from the
        // redacted set.
        assert!(rendered.contains("partner_type"), "{rendered}");
        // The identifiers stay legible — that is the point of the hand-written
        // impl rather than redacting the whole struct.
        assert!(rendered.contains("ATVPDKIKX0DER"), "{rendered}");
    }

    /// A netsuite config with everything valid except the one field under test,
    /// so each case fails for exactly one reason.
    fn netsuite_cfg(extra: serde_json::Map<String, Value>) -> SourceConfig {
        let mut obj = json!({
            "account_id": "4544316",
            "client_id": "cid",
            "certificate_id": "kid",
            "private_key_pem": "-----BEGIN PRIVATE KEY-----\nx\n-----END PRIVATE KEY-----\n",
        })
        .as_object()
        .expect("object")
        .clone();
        obj.extend(extra);
        cfg("netsuite", Value::Object(obj))
    }

    fn refusal(built: Result<Box<dyn SourceConnector>, AirwayError>) -> String {
        match built {
            Ok(_) => panic!("expected this config to be refused"),
            Err(e) => e.to_string(),
        }
    }

    /// The signing algorithm is a *security* default, and it lives upstream.
    /// If airway ever changes `SigningAlgorithm::default`, every existing
    /// netsuite pipeline silently changes how it signs — with no CI signal and
    /// a now-false comment. Pinned here the way this file pins its other
    /// cross-crate contracts.
    #[test]
    fn netsuite_default_algorithm_is_ps256() {
        assert_eq!(
            SigningAlgorithm::default(),
            SigningAlgorithm::parse("PS256").expect("PS256 parses"),
            concat!(
                "the documented default changed upstream — update the field ",
                "doc on NetSuiteParams::algorithm before taking this",
            )
        );
    }

    /// A non-positive lookback puts the cold-start window at or after now, so
    /// the first run of a new pipeline returns nothing and reads as an empty
    /// account — the silent-failure class this wiring is careful about.
    #[test]
    fn netsuite_rejects_a_non_positive_lookback() {
        for days in [0, -30] {
            let msg = refusal(build(&netsuite_cfg(
                json!({ "lookback_days": days })
                    .as_object()
                    .unwrap()
                    .clone(),
            )));
            assert!(msg.contains("lookback_days"), "{msg}");
            assert!(!msg.contains("  "), "collapsed whitespace: {msg}");
        }
    }

    /// `resources: []` means "all" at the top level of `.airway.yml` but reaches
    /// the connector verbatim from here, so the ambiguous form is refused rather
    /// than silently resolved to one meaning or the other.
    #[test]
    fn netsuite_rejects_an_empty_resource_list() {
        let msg = refusal(build(&netsuite_cfg(
            json!({ "resources": [] }).as_object().unwrap().clone(),
        )));
        assert!(msg.contains("resources"), "{msg}");
        assert!(msg.contains("Omit the key"), "{msg}");
        assert!(!msg.contains("  "), "collapsed whitespace: {msg}");
    }

    /// An unset secret reaches `build_netsuite` in three shapes, and all three
    /// must name the secret rather than the struct field:
    ///
    /// * resolved to `""` — the executor *skips the insert*, so the field
    ///   arrives **absent**; `#[serde(default)]` is what turns that into `""`
    /// * resolved to whitespace — `is_empty()` is false, so it is inserted
    ///   verbatim
    /// * written blank in YAML with no `_var` at all — passes through
    ///
    /// Without the default, the first shape fails as `missing field
    /// 'private_key_pem'`, which names the struct rather than the credential
    /// an operator has to go and set.
    #[test]
    fn netsuite_names_the_secret_when_the_resolved_key_is_empty() {
        // The shape the executor actually produces: an empty resolved secret is
        // treated as unset, so the field is *absent*, not `""`. Without
        // `#[serde(default)]` this fails as `missing field \`private_key_pem\``
        // — naming the struct rather than the secret to go and fix.
        let absent = refusal(build(&cfg(
            "netsuite",
            json!({
                "account_id": "4544316",
                "client_id": "cid",
                "certificate_id": "kid",
            }),
        )));
        assert!(absent.contains("private_key_var"), "{absent}");
        assert!(!absent.contains("  "), "collapsed whitespace: {absent}");

        // And a hand-written blank in YAML, which arrives as itself.
        let msg = refusal(build(&cfg(
            "netsuite",
            json!({
                "account_id": "4544316",
                "client_id": "cid",
                "certificate_id": "kid",
                "private_key_pem": "   ",
            }),
        )));
        assert!(msg.contains("private_key_var"), "{msg}");
        assert!(msg.contains("newlines"), "{msg}");
        // Not just `contains`. These messages have twice shipped with a run of
        // literal spaces mid-sentence — "... check that              `private_
        // key_var` ..." — while every `contains` assertion still passed. The
        // cause was the patch tooling used to write them, not rustfmt: a `\`
        // continuation is a correct Rust idiom and the `lookback_days` message
        // above relies on it. Asserting no double space is what catches it.
        assert!(
            !msg.contains("  "),
            "message has collapsed whitespace: {msg}"
        );
    }

    /// The file's convention for the `_var` -> resolved-field contract: an
    /// unsubstituted var key must not sail through as an unknown field.
    #[test]
    fn netsuite_rejects_unresolved_var_key() {
        let msg = refusal(build(&cfg(
            "netsuite",
            json!({
                "account_id": "4544316",
                "client_id": "cid",
                "certificate_id": "kid",
                // Left un-substituted: the executor failed to resolve it.
                "private_key_var": "NETSUITE_PRIVATE_KEY",
            }),
        )));
        assert!(msg.contains("private_key_var"), "{msg}");
    }

    #[test]
    fn google_sheets_builds_and_names_resources_after_the_range() {
        let source = build(&cfg(
            "google_sheets",
            json!({
                "spreadsheet_id": "1AbC",
                // The executor injects this; no file on disk carries it.
                "access_token": "minted-per-run",
                "ranges": ["Main!A:S"],
            }),
        ))
        .expect("build");
        assert_eq!(source.name(), "google_sheets");
        // `resources:` in the YAML has to name what the connector emits, and
        // the connector derives that from the range rather than being told.
        // Pin it: `Main!A:S` is the resource `main`, so a change upstream
        // fails here rather than as an empty run against the customer's sheet.
        let names: Vec<_> = source.resources().into_iter().map(|r| r.name).collect();
        assert_eq!(names, vec!["main".to_string()]);
    }

    /// A google_sheets access token is minted per run by the executor, never
    /// authored. If secret resolution did not run, the token is empty — and an
    /// empty Bearer header comes back from Google as a 404 on the spreadsheet,
    /// which reads like a wrong id rather than a missing credential. Fail here
    /// instead, naming the field that is actually missing.
    #[test]
    fn google_sheets_without_a_resolved_token_is_rejected() {
        // `expect_err` would need the Ok type to be Debug, and a boxed
        // connector is not.
        let Err(err) = build(&cfg(
            "google_sheets",
            json!({
                "spreadsheet_id": "1AbC",
                "service_account_json_var": "GOOGLE_SA_JSON",
                "ranges": ["Main!A:S"],
            }),
        )) else {
            panic!("google_sheets must not build without a resolved token");
        };
        let msg = err.to_string();
        assert!(
            msg.contains("service_account_json_var"),
            "error should name the secret field to set, got: {msg}"
        );
    }

    #[test]
    fn rest_api_builds() {
        let source = build(&cfg(
            "rest_api",
            json!({
                "base_url": "https://api.example.com",
                "endpoints": [],
            }),
        ))
        .expect("build");
        assert_eq!(source.name(), "rest_api");
    }

    // The agentic-pipeline executor resolves a rest_api credential into
    // `auth.token` (bearer) / `auth.key` (`api_key` header, `api_key_query`).
    // RestApiConfig does NOT `deny_unknown_fields`, so if airway ever renamed
    // those fields the resolved secret would land in an ignored field and the
    // request would go out unauthenticated. These round-trips pin the contract
    // so a rename in airway-internal fails here at CI instead.
    #[test]
    fn rest_api_bearer_reads_resolved_token() {
        let config: RestApiConfig = serde_json::from_value(json!({
            "base_url": "https://api.yelp.com/v3",
            "auth": { "type": "bearer", "token": "resolved-secret" },
            "endpoints": [],
        }))
        .expect("deserialize");
        match config.auth {
            AuthConfig::Bearer { token } => assert_eq!(token, "resolved-secret"),
            other => panic!("expected bearer carrying the resolved token, got {other:?}"),
        }
    }

    #[test]
    fn rest_api_api_key_query_reads_resolved_key() {
        let config: RestApiConfig = serde_json::from_value(json!({
            "base_url": "https://api.census.gov/data/2023/acs/acs5",
            "auth": { "type": "api_key_query", "key": "resolved-secret", "param": "key" },
            "endpoints": [],
        }))
        .expect("deserialize");
        match config.auth {
            AuthConfig::ApiKeyQuery { key, param } => {
                assert_eq!(key, "resolved-secret");
                assert_eq!(param, "key");
            }
            other => panic!("expected api_key_query carrying the resolved key, got {other:?}"),
        }
    }

    #[test]
    fn rest_api_api_key_header_reads_resolved_key() {
        // The header variant's serde tag is `api_key` (not `api_key_header`); it
        // also carries the secret in `key`, so the executor's `key_var` -> `key`
        // mapping covers it.
        let config: RestApiConfig = serde_json::from_value(json!({
            "base_url": "https://example.com",
            "auth": { "type": "api_key", "key": "resolved-secret" },
            "endpoints": [],
        }))
        .expect("deserialize");
        match config.auth {
            AuthConfig::ApiKey { key, .. } => assert_eq!(key, "resolved-secret"),
            other => panic!("expected api_key carrying the resolved key, got {other:?}"),
        }
    }

    #[test]
    fn rest_api_rejects_unresolved_token_var() {
        // If an old binary fails to resolve `token_var` -> `token`, airway's
        // Bearer is missing its required `token`, so the build fails loudly (a
        // clear deserialize error) rather than silently sending an empty
        // credential.
        let err = build(&cfg(
            "rest_api",
            json!({
                "base_url": "https://x",
                "auth": { "type": "bearer", "token_var": "YELP_API_KEY" },
                "endpoints": [],
            }),
        ))
        .err()
        .expect("expected error");
        assert!(
            err.to_string().contains("invalid rest_api config"),
            "got: {err}"
        );
    }

    #[test]
    fn ubereats_builds_from_a_zone() {
        let source = build(&cfg(
            "ubereats",
            json!({
                "base_path": "s3://landing/ubereats",
                "allowed_stores": ["Poke House SF", "Poke House LA"],
            }),
        ))
        .expect("build");
        assert_eq!(source.name(), "ubereats");

        // The resource carries the merge key the deterministic row id exists
        // for. Any other disposition either duplicates on re-read or replaces a
        // table nothing else keys.
        let resources = source.resources();
        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0].name, "ubereats_transactions");
        assert_eq!(
            resources[0].primary_key.as_deref(),
            Some(["_row_uid".to_string()].as_slice())
        );
    }

    /// The whole reason for the 0.1.30 bump: the declared types must reach the
    /// factory-built connector, or an all-null column does not materialize and
    /// the landed table's shape follows load order.
    #[test]
    fn ubereats_declares_its_column_types_through_the_factory() {
        let source = build(&cfg("ubereats", json!({ "base_path": "/tmp/ue" }))).expect("build");
        let hints = source.column_hints();
        assert_eq!(
            hints["ubereats_transactions"]["total_payout"].data_type,
            Some(airway::types::DataType::Double),
            "money must land as double — decimal is a migration, not a port"
        );
        assert_eq!(
            hints["ubereats_transactions"]["order_date"].data_type,
            Some(airway::types::DataType::Date)
        );
    }

    /// Half a period is not one. Taking the given half and guessing the other
    /// stamps a month nobody named — the failure the source refuses to guess at
    /// when reading a path, arriving through config instead.
    #[test]
    fn ubereats_refuses_half_a_period() {
        for half in [json!({"report_year": 2026}), json!({"report_month": 8})] {
            let mut params = json!({ "base_path": "/tmp/ue" });
            let obj = params.as_object_mut().unwrap();
            for (k, v) in half.as_object().unwrap() {
                obj.insert(k.clone(), v.clone());
            }
            let err = build(&cfg("ubereats", params))
                .err()
                .expect("half a period must be refused");
            let msg = err.to_string();
            assert!(msg.contains("must be given together"), "got: {msg}");
            // The message IS the guard, so it has to read as one sentence — a
            // literal wrapped without `\` continuations renders ~18-space runs
            // mid-sentence, and asserting only on a substring hid exactly that.
            assert!(
                !msg.contains("  "),
                "the refusal must not carry a run of spaces: {msg:?}"
            );
        }

        // Both together is fine, and neither is fine.
        build(&cfg(
            "ubereats",
            json!({ "base_path": "/tmp/ue", "report_year": 2026, "report_month": 8 }),
        ))
        .expect("a whole period builds");
        build(&cfg("ubereats", json!({ "base_path": "/tmp/ue" }))).expect("no period builds");
    }

    /// Absent means "every store"; empty means "no store", so it would build a
    /// pipeline that succeeds and lands zero rows — inverting the field's
    /// purpose. Every sibling scoping list in this file guards this.
    #[test]
    fn ubereats_refuses_an_empty_allowed_stores() {
        let err = build(&cfg(
            "ubereats",
            json!({ "base_path": "/tmp/ue", "allowed_stores": [] }),
        ))
        .err()
        .expect("an empty allow-list must be refused");
        let msg = err.to_string();
        assert!(msg.contains("allowed_stores"), "names the field: {msg}");
        assert!(!msg.contains("  "), "reads as one sentence: {msg:?}");

        // Omitting the key still means "every store".
        build(&cfg("ubereats", json!({ "base_path": "/tmp/ue" })))
            .expect("absent allowed_stores loads every store");
    }

    /// Half a validated pair reads as though the pair were validated, so the
    /// month guard's argument — not a value anyone named — has to cover the
    /// year too.
    #[test]
    fn ubereats_refuses_a_year_that_does_not_exist() {
        for year in [-5, 0, 1900, 3000] {
            let err = build(&cfg(
                "ubereats",
                json!({ "base_path": "/tmp/ue", "report_year": year, "report_month": 8 }),
            ))
            .err()
            .unwrap_or_else(|| panic!("year {year} must be refused"));
            let msg = err.to_string();
            assert!(msg.contains("report_year"), "names the field: {msg}");
            assert!(!msg.contains("  "), "reads as one sentence: {msg:?}");
        }

        build(&cfg(
            "ubereats",
            json!({ "base_path": "/tmp/ue", "report_year": 2026, "report_month": 8 }),
        ))
        .expect("a real year builds");
    }

    /// A month outside 1–12 is not a month anyone named, which is the same
    /// argument that refuses half a period — forwarding it stamps a partition
    /// that cannot exist.
    #[test]
    fn ubereats_refuses_a_month_that_does_not_exist() {
        for month in [0, 13, 99] {
            let err = build(&cfg(
                "ubereats",
                json!({ "base_path": "/tmp/ue", "report_year": 2026, "report_month": month }),
            ))
            .err()
            .unwrap_or_else(|| panic!("month {month} must be refused"));
            let msg = err.to_string();
            assert!(msg.contains("report_month"), "names the field: {msg}");
            assert!(!msg.contains("  "), "reads as one sentence: {msg:?}");
        }

        for month in [1, 8, 12] {
            build(&cfg(
                "ubereats",
                json!({ "base_path": "/tmp/ue", "report_year": 2026, "report_month": month }),
            ))
            .unwrap_or_else(|_| panic!("month {month} is real and must build"));
        }
    }

    /// `deny_unknown_fields`, so a mistyped key is refused rather than silently
    /// ignored — a misspelled `allowed_stores` would load every store in an API
    /// section, which is the shape scoping exists to prevent.
    #[test]
    fn ubereats_refuses_an_unknown_field() {
        let err = build(&cfg(
            "ubereats",
            json!({ "base_path": "/tmp/ue", "allowed_store": ["Poke House SF"] }),
        ))
        .err()
        .expect("a mistyped key must be refused");
        assert!(
            err.to_string().contains("invalid ubereats config"),
            "got: {err}"
        );
    }

    #[test]
    fn filesystem_builds_with_json_format() {
        let source = build(&cfg(
            "filesystem",
            json!({
                "base_path": "/tmp/data",
                "pattern": "*.jsonl",
                "format": "jsonl",
                "table_name": "events",
            }),
        ))
        .expect("build");
        // Filesystem reports its connector name (`filesystem`).
        assert_eq!(source.name(), "filesystem");
    }

    #[test]
    fn sql_database_builds_with_no_tables() {
        let source = build(&cfg(
            "sql_database",
            json!({
                "connection_string": "postgres://u:p@h/d",
                "backend": "postgres",
            }),
        ))
        .expect("build");
        let _ = source.name(); // not asserting exact name; just exercise path
    }

    #[test]
    fn clickhouse_builds_minimal() {
        let source = build(&cfg(
            "clickhouse",
            json!({
                "host": "my-host.clickhouse.cloud",
                "database": "default",
            }),
        ))
        .expect("build");
        // SqlDatabaseSource reports its connector name.
        let _ = source.name();
    }

    #[test]
    fn clickhouse_builds_with_credentials_and_tables() {
        let source = build(&cfg(
            "clickhouse",
            json!({
                "host": "my-host.clickhouse.cloud",
                "port": 8443,
                "database": "analytics",
                "username": "reader",
                "password": "resolved-secret",
                "secure": true,
                "tables": [
                    { "name": "events", "cursor_field": "created_at", "write_disposition": "append" }
                ],
            }),
        ))
        .expect("build");
        let _ = source.name();
    }

    #[test]
    fn clickhouse_rejects_unresolved_password_var() {
        // `password_var` must be stripped by the executor; if it leaks
        // through, deny_unknown_fields catches it.
        let err = build(&cfg(
            "clickhouse",
            json!({
                "host": "h",
                "database": "d",
                "password_var": "CLICKHOUSE_PASSWORD",
            }),
        ))
        .err()
        .expect("expected error");
        assert!(err.to_string().contains("invalid clickhouse config"));
    }

    #[test]
    fn postgres_cdc_builds() {
        let source = build(&cfg(
            "postgres_cdc",
            json!({
                "connection_string": "postgres://u:p@h/d",
                "slot_name": "oxy_slot",
                "publication_name": "oxy_pub",
                "tables": ["users", "orders"],
                "batch_size": 5000,
                "initial_snapshot": false,
            }),
        ))
        .expect("build");
        let _ = source.name();
    }

    #[test]
    fn toast_builds_with_resolved_credentials() {
        // Mirrors what the executor hands the factory: literal
        // client_id / client_secret (no `*_var` keys survive).
        let source = build(&cfg(
            "toast",
            json!({
                "client_id": "abc123",
                "client_secret": "shhh-resolved",
                "restaurant_guids": ["11111111-2222-3333-4444-555555555555"],
            }),
        ))
        .expect("build");
        let _ = source.name();
    }

    #[test]
    fn toast_rejects_empty_restaurant_guids() {
        let err = build(&cfg(
            "toast",
            json!({
                "client_id": "abc",
                "client_secret": "s",
                "restaurant_guids": [],
            }),
        ))
        .err()
        .expect("expected error");
        assert!(err.to_string().contains("restaurant_guids"));
    }

    #[test]
    fn toast_rejects_unresolved_var_key() {
        // `client_secret_var` must be stripped by the executor; if it
        // leaks through, deny_unknown_fields catches it.
        let err = build(&cfg(
            "toast",
            json!({
                "client_id": "abc",
                "client_secret_var": "TOAST_SECRET",
                "restaurant_guids": ["g"],
            }),
        ))
        .err()
        .expect("expected error");
        assert!(err.to_string().contains("invalid toast config"));
    }

    #[test]
    fn quickbooks_builds_with_resolved_credentials() {
        // Mirrors what the executor hands the factory: literal
        // client_secret / refresh_token (no `*_var` keys survive).
        let source = build(&cfg(
            "quickbooks",
            json!({
                "client_id": "intuit-client",
                "client_secret": "shhh-resolved",
                "refresh_token": "refresh-resolved",
                "realm_id": "1234567890",
            }),
        ))
        .expect("build");
        assert_eq!(source.name(), "quickbooks");
        // All 9 resources are advertised. Naming the newest one as well as the
        // count: a bare count says a resource was added or removed but not
        // which, and this assertion's whole job is to notice an airway bump
        // changing the surface — it caught `journal_entries` arriving in 0.1.28.
        // Bind the Vec: `resources()` returns owned, so borrowing `&str` out of
        // a temporary would not outlive the statement.
        let resources = source.resources();
        let names: Vec<&str> = resources.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names.len(), 9, "resource surface changed: {names:?}");
        assert!(
            names.contains(&"journal_entries"),
            "journal_entries missing: {names:?}"
        );
    }

    #[test]
    fn quickbooks_builds_with_optional_fields() {
        let source = build(&cfg(
            "quickbooks",
            json!({
                "client_id": "c",
                "client_secret": "s",
                "refresh_token": "r",
                "realm_id": "realm",
                "base_url": "https://sandbox-quickbooks.api.intuit.com",
                "minor_version": "70",
            }),
        ))
        .expect("build");
        assert_eq!(source.name(), "quickbooks");
    }

    #[test]
    fn quickbooks_accepts_numeric_realm_id() {
        // A bare-integer realm_id (unquoted YAML) must coerce to a string
        // rather than fail with "invalid type: integer".
        let source = build(&cfg(
            "quickbooks",
            json!({
                "client_id": "c",
                "client_secret": "s",
                "refresh_token": "r",
                "realm_id": 9341456860808037i64,
            }),
        ))
        .expect("build");
        assert_eq!(source.name(), "quickbooks");
    }

    #[test]
    fn toast_builds_with_backfill_window() {
        let source = build(&cfg(
            "toast",
            json!({
                "client_id": "abc",
                "client_secret": "shh",
                "restaurant_guids": ["g"],
                "backfill_start": "2024-01-01T00:00:00Z",
                "backfill_end": "2024-02-01T00:00:00Z",
            }),
        ))
        .expect("build toast with backfill window");
        assert_eq!(source.name(), "toast");
    }

    #[test]
    fn quickbooks_builds_with_backfill_window() {
        let source = build(&cfg(
            "quickbooks",
            json!({
                "client_id": "c",
                "client_secret": "s",
                "refresh_token": "r",
                "realm_id": "1234567890",
                "backfill_start": "2024-01-01T00:00:00Z",
                "backfill_end": "2024-02-01T00:00:00Z",
            }),
        ))
        .expect("build quickbooks with backfill window");
        assert_eq!(source.name(), "quickbooks");
    }

    /// The key names the executor injects must be the ones `SpApiParams`
    /// deserializes.
    ///
    /// Sharper here than for its two siblings: `SpApiParams` is
    /// `deny_unknown_fields`, so a name that drifts from the executor's is not a
    /// window quietly ignored — it is a hard parse failure on every backfill
    /// run. Without this test the first thing to notice would be a failed run
    /// against a live seller account.
    #[test]
    fn sp_api_builds_with_backfill_window() {
        let source = build(&cfg(
            "sp_api",
            json!({
                "client_id": "amzn1.application-oa2-client.x",
                "client_secret": "shh",
                "refresh_token": "Atzr|x",
                "marketplace_id": "A2EUQ1WTGCTBG2",
                "default_start": "2026-01-01",
                "backfill_start": "2026-03-01T00:00:00Z",
                "backfill_end": "2026-09-01T00:00:00Z",
            }),
        ))
        .expect("build sp_api with backfill window");
        assert_eq!(source.name(), "sp_api");
    }

    #[test]
    fn backfill_window_requires_both_bounds() {
        let err = build(&cfg(
            "toast",
            json!({
                "client_id": "abc",
                "client_secret": "shh",
                "restaurant_guids": ["g"],
                "backfill_start": "2024-01-01T00:00:00Z",
            }),
        ))
        .err()
        .expect("expected error for one-sided backfill window");
        assert!(err.to_string().contains("requires both"));
    }

    #[test]
    fn backfill_window_rejects_malformed_timestamp() {
        let err = build(&cfg(
            "quickbooks",
            json!({
                "client_id": "c",
                "client_secret": "s",
                "refresh_token": "r",
                "realm_id": "1",
                "backfill_start": "not-a-date",
                "backfill_end": "2024-02-01T00:00:00Z",
            }),
        ))
        .err()
        .expect("expected error for malformed backfill timestamp");
        assert!(err.to_string().contains("invalid backfill timestamp"));
    }

    #[test]
    fn quickbooks_rejects_unresolved_var_key() {
        // `client_secret_var` / `refresh_token_var` must be stripped by
        // the executor; if one leaks through, deny_unknown_fields catches it.
        let err = build(&cfg(
            "quickbooks",
            json!({
                "client_id": "c",
                "client_secret_var": "QB_CLIENT_SECRET",
                "refresh_token_var": "QB_REFRESH_TOKEN",
                "realm_id": "realm",
            }),
        ))
        .err()
        .expect("expected error");
        assert!(err.to_string().contains("invalid quickbooks config"));
    }

    // ── read-only token custody ─────────────────────────────────────────

    struct StubAccessTokenSource;

    #[async_trait]
    impl AccessTokenSource for StubAccessTokenSource {
        async fn access_token(&self) -> Result<String, String> {
            Ok("access.stub".into())
        }
    }

    struct StubRefreshSink;

    #[async_trait]
    impl RefreshTokenSink for StubRefreshSink {
        async fn persist(&self, _refresh_token: &str) -> Result<(), String> {
            Ok(())
        }
    }

    fn build_with(
        config: &SourceConfig,
        tokens: Option<QuickBooksTokens>,
    ) -> Result<Box<dyn SourceConnector>, AirwayError> {
        build_source_connector(config, tokens, airway::connector::Environment::Production)
    }

    /// Read-only mode needs neither `client_secret` nor `refresh_token` — the
    /// data API authenticates with the bearer token alone.
    #[test]
    fn quickbooks_read_only_builds_without_refresh_credentials() {
        let source = build_with(
            &cfg(
                "quickbooks",
                json!({
                    "client_id": "c",
                    "realm_id": "realm",
                    "access_token_var": "apps/app-id/QB_ACCESS_TOKEN",
                }),
            ),
            Some(QuickBooksTokens::ReadOnly(Arc::new(StubAccessTokenSource))),
        )
        .expect("build read-only quickbooks");
        assert_eq!(source.name(), "quickbooks");
    }

    /// THE safety property. `access_token_var` declares that another writer
    /// owns this grant's rotation; if no source arrives, the only other way to
    /// authenticate is to refresh — which would fork the chain. Refuse instead.
    #[test]
    fn quickbooks_read_only_declared_without_a_source_is_refused() {
        let err = build_with(
            &cfg(
                "quickbooks",
                json!({
                    "client_id": "c",
                    "client_secret": "s",
                    "refresh_token": "r",
                    "realm_id": "realm",
                    "access_token_var": "apps/app-id/QB_ACCESS_TOKEN",
                }),
            ),
            None,
        )
        .err()
        .expect("expected refusal");
        assert!(
            err.to_string()
                .contains("Refusing to fall back to refreshing"),
            "unexpected error: {err}"
        );
    }

    /// Same refusal when the host supplies the *wrong* hook — a sink for a
    /// config that declared read-only custody.
    #[test]
    fn quickbooks_read_only_declared_but_handed_a_sink_is_refused() {
        let err = build_with(
            &cfg(
                "quickbooks",
                json!({
                    "client_id": "c",
                    "client_secret": "s",
                    "refresh_token": "r",
                    "realm_id": "realm",
                    "access_token_var": "apps/app-id/QB_ACCESS_TOKEN",
                }),
            ),
            Some(QuickBooksTokens::Rotating(Arc::new(StubRefreshSink))),
        )
        .err()
        .expect("expected refusal");
        assert!(
            err.to_string().contains("a refresh-token sink"),
            "unexpected error: {err}"
        );
    }

    /// Rotating mode still demands both credentials, so dropping them to
    /// `Option` for read-only mode can't silently weaken the refresh path.
    #[test]
    fn quickbooks_rotating_still_requires_refresh_credentials() {
        let err = build_with(
            &cfg(
                "quickbooks",
                json!({ "client_id": "c", "realm_id": "realm" }),
            ),
            None,
        )
        .err()
        .expect("expected error");
        assert!(
            err.to_string().contains("`client_secret_var` is required"),
            "unexpected error: {err}"
        );
    }

    // ── the sandbox-reaches-production guards ───────────────────────────

    /// Stands in for a future airway connector that declares a sandbox host
    /// this factory has no arm for. None exists today, which is exactly why
    /// the guard is checked against the connector rather than a list.
    ///
    /// Carries the host so the same fixture covers the other side too — a
    /// connector that declares none is the shape all ~11 environment-blind
    /// arms have in the pinned airway.
    struct Declares(Option<&'static str>);

    const DECLARES_SANDBOX: Declares = Declares(Some("https://sandbox.example.invalid"));
    const DECLARES_NO_SANDBOX: Declares = Declares(None);

    #[async_trait]
    impl SourceConnector for Declares {
        fn name(&self) -> &str {
            "declares"
        }
        fn resources(&self) -> Vec<airway::connector::ResourceInfo> {
            Vec::new()
        }
        fn sandbox_base_url(&self) -> Option<&str> {
            self.0
        }
        async fn extract(
            &self,
            _resource: &str,
            _state: Option<&Value>,
        ) -> Result<airway::connector::ExtractionResult, airway::AirwayError> {
            unimplemented!("not exercised by these tests")
        }
    }

    #[test]
    fn a_declared_sandbox_host_with_no_arm_is_refused() {
        let err = admit_environment_is_applied(
            &cfg("future_vendor", serde_json::json!({})),
            airway::connector::Environment::Sandbox,
            &DECLARES_SANDBOX,
            EnvironmentApplied::No,
        )
        .expect_err("admission would pass and oxy would leave it on production");
        assert!(err.to_string().contains("future_vendor"), "got: {err}");
    }

    #[test]
    fn an_arm_that_applied_the_environment_is_exempt() {
        admit_environment_is_applied(
            &cfg("quickbooks", serde_json::json!({})),
            airway::connector::Environment::Sandbox,
            &DECLARES_SANDBOX,
            EnvironmentApplied::Yes,
        )
        .expect("the arm resolved the host from `environment`");
    }

    /// The reason the marker replaced a hardcoded kind: the exemption follows
    /// the arm's behaviour, not its name. A `quickbooks` arm that stopped
    /// applying the environment must be refused exactly like any other, rather
    /// than keep passing on the strength of its kind string.
    #[test]
    fn the_kind_alone_does_not_exempt_an_arm_that_skipped_the_environment() {
        admit_environment_is_applied(
            &cfg("quickbooks", serde_json::json!({})),
            airway::connector::Environment::Sandbox,
            &DECLARES_SANDBOX,
            EnvironmentApplied::No,
        )
        .expect_err("a kind that no longer applies the environment must not stay exempt");
    }

    /// The guard must stay inert for the ~11 arms that legitimately ignore
    /// `environment`, or stage 1 would stop being behaviour-preserving.
    #[test]
    fn an_arm_that_ignored_the_environment_passes_when_no_host_is_declared() {
        admit_environment_is_applied(
            &cfg("toast", serde_json::json!({})),
            airway::connector::Environment::Sandbox,
            &DECLARES_NO_SANDBOX,
            EnvironmentApplied::No,
        )
        .expect("nothing to apply: the connector declares no sandbox host");
    }

    #[test]
    fn production_never_triggers_the_guard() {
        admit_environment_is_applied(
            &cfg("future_vendor", serde_json::json!({})),
            airway::connector::Environment::Production,
            &DECLARES_SANDBOX,
            EnvironmentApplied::No,
        )
        .expect("production needs no sandbox mapping");
    }

    #[test]
    fn sandbox_resolves_the_intuit_sandbox_host() {
        assert_eq!(
            resolve_quickbooks_base_url(None, airway::connector::Environment::Sandbox),
            Some(airway::connector::sources::quickbooks::SANDBOX_BASE_URL.to_string()),
        );
    }

    #[test]
    fn production_leaves_the_connector_default_alone() {
        // `None` means "don't call `with_base_url`", so the connector keeps
        // its own production default rather than oxy restating it.
        assert_eq!(
            resolve_quickbooks_base_url(None, airway::connector::Environment::Production),
            None,
        );
    }

    /// An explicit `base_url` in the pipeline YAML is narrower than the
    /// deployment-wide intent and must win over it — in both environments.
    #[test]
    fn an_explicit_base_url_outranks_the_environment() {
        for env in [
            airway::connector::Environment::Sandbox,
            airway::connector::Environment::Production,
        ] {
            assert_eq!(
                resolve_quickbooks_base_url(Some("https://qb.internal.invalid"), env),
                Some("https://qb.internal.invalid".to_string()),
                "explicit base_url must win under {env:?}",
            );
        }
    }

    #[test]
    fn quickbooks_still_builds_under_sandbox() {
        let config = SourceConfig {
            kind: "quickbooks".to_string(),
            config: serde_json::json!({
                "client_id": "id",
                "client_secret": "secret",
                "refresh_token": "token",
                "realm_id": 9341456860808037i64,
            }),
        };
        build_source_connector(&config, None, airway::connector::Environment::Sandbox)
            .expect("builds under sandbox");
    }

    #[test]
    fn weather_builds_with_locations() {
        let source = build(&cfg(
            "weather",
            json!({
                "locations": [
                    { "id": 1, "latitude": 37.7749, "longitude": -122.4194 }
                ],
            }),
        ))
        .expect("build");
        assert_eq!(source.name(), "weather");
    }

    #[test]
    fn weather_rejects_empty_locations() {
        let err = build(&cfg("weather", json!({ "locations": [] })))
            .err()
            .expect("expected error");
        assert!(err.to_string().contains("locations"));
    }

    #[test]
    fn besttime_builds_with_resolved_credentials() {
        // Mirrors what the executor hands the factory: literal `api_key`
        // (no `api_key_var` survives).
        let source = build(&cfg(
            "besttime",
            json!({
                "api_key": "resolved-secret",
                "venue_ids": ["ven_abc123", "ven_def456"],
            }),
        ))
        .expect("build");
        assert_eq!(source.name(), "besttime");
    }

    #[test]
    fn besttime_rejects_empty_venue_ids() {
        let err = build(&cfg(
            "besttime",
            json!({
                "api_key": "resolved-secret",
                "venue_ids": [],
            }),
        ))
        .err()
        .expect("expected error");
        assert!(err.to_string().contains("venue_ids"));
    }

    #[test]
    fn besttime_rejects_unresolved_var_key() {
        // `api_key_var` must be stripped by the executor; if it leaks
        // through, deny_unknown_fields catches it.
        let err = build(&cfg(
            "besttime",
            json!({
                "api_key_var": "BESTTIME_API_KEY",
                "venue_ids": ["ven_abc"],
            }),
        ))
        .err()
        .expect("expected error");
        // Either `api_key` is missing (required) or `api_key_var` is unknown —
        // both surface as "invalid besttime config".
        assert!(err.to_string().contains("invalid besttime config"));
    }

    #[test]
    fn overture_builds_with_bboxes() {
        let source = build(&cfg(
            "overture",
            json!({
                "release": "2026-05-21.0",
                "bboxes": [{
                    "name": "bay_area",
                    "min_lat": 36.85, "min_lng": -123.10,
                    "max_lat": 38.15, "max_lng": -121.55
                }]
            }),
        ))
        .expect("build");
        assert_eq!(source.name(), "overture");
    }

    #[test]
    fn overture_rejects_empty_bboxes() {
        let err = build(&cfg(
            "overture",
            json!({ "release": "2026-05-21.0", "bboxes": [] }),
        ))
        .err()
        .expect("expected error");
        assert!(err.to_string().contains("bbox"));
    }

    #[test]
    fn http_file_builds_csv_gz_with_filters_and_columns() {
        let source = build(&cfg(
            "http_file",
            json!({
                "url": "https://example.com/sample.csv.gz",
                "format": "csv_gz",
                "resource_name": "sample_table",
                "csv_options": {"header": true, "delimiter": ","},
                "filters": [{"column": "state", "op": "eq", "value": "06"}],
                "columns": ["w_geocode", "C000"]
            }),
        ))
        .expect("build");
        assert_eq!(source.name(), "http_file");
    }

    #[test]
    fn http_file_builds_zip_csv() {
        let source = build(&cfg(
            "http_file",
            json!({
                "url": "https://example.com/places.zip",
                "format": "zip_csv",
                "resource_name": "census_places",
                "zip_inner_glob": "*.csv"
            }),
        ))
        .expect("build");
        assert_eq!(source.name(), "http_file");
    }

    #[test]
    fn http_file_rejects_unknown_field() {
        let err = build(&cfg(
            "http_file",
            json!({
                "url": "https://example.com/x.csv",
                "format": "csv",
                "resource_name": "x",
                "bogus_field": true
            }),
        ))
        .err()
        .expect("expected error");
        assert!(err.to_string().contains("invalid http_file config"));
    }

    #[test]
    fn overpass_builds_with_bbox_and_template() {
        let source = build(&cfg(
            "overpass",
            json!({
                "bboxes": [{
                    "name": "bay_area",
                    "min_lat": 36.85, "min_lng": -123.10,
                    "max_lat": 38.15, "max_lng": -121.55
                }],
                "query_template":
                    "[out:json][timeout:60];(node[\"amenity\"=\"fast_food\"]({bbox}););out tags center;",
                "resource_name": "osm_pois"
            }),
        ))
        .expect("build");
        assert_eq!(source.name(), "overpass");
    }

    #[test]
    fn overpass_rejects_empty_bboxes() {
        let err = build(&cfg(
            "overpass",
            json!({
                "bboxes": [],
                "query_template": "[out:json];out;",
                "resource_name": "x"
            }),
        ))
        .err()
        .expect("expected error");
        assert!(err.to_string().to_lowercase().contains("bbox"));
    }

    #[test]
    fn overpass_rejects_unknown_field() {
        let err = build(&cfg(
            "overpass",
            json!({
                "bboxes": [{
                    "name": "x", "min_lat": 0.0, "min_lng": 0.0,
                    "max_lat": 1.0, "max_lng": 1.0
                }],
                "query_template": "[out:json];out;",
                "resource_name": "x",
                "bogus_field": "nope"
            }),
        ))
        .err()
        .expect("expected error");
        assert!(err.to_string().contains("invalid overpass config"));
    }

    #[test]
    fn unknown_kind_errors_with_extension_hint() {
        let err = build(&cfg("not_a_real_thing", json!({})))
            .err()
            .expect("expected error");
        let msg = err.to_string();
        assert!(msg.contains("not_a_real_thing"));
        assert!(msg.contains("source_factory"));
    }

    // Note: airway's `RestApiConfig` doesn't `deny_unknown_fields`, so
    // stowaway fields under `config:` are silently ignored. The
    // top-level `AirwayPipelineSpec`/`SourceConfig` structs ARE strict
    // — see `config::tests::rejects_unknown_top_level_field`. Strictness
    // at the connector-config level is airway's call, not ours.
}
