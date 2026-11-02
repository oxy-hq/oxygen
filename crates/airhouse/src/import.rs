//! Loading a CSV or Parquet file into a table in a workspace's Airhouse.
//!
//! Airhouse has no upload route, and its pgwire surface takes `COPY … FROM
//! STDIN` only over the simple query protocol, which `tokio_postgres` cannot
//! send (its `copy_in` prepares the statement). So the file is read here, by
//! DuckDB running in this process, and sent as SQL over the same wire every
//! other Oxy writer uses (`oxy_cameras::airhouse`, the custom-app usage drain):
//!
//! 1. DuckDB's own reader (`read_csv_auto` / `read_parquet`) reads the file,
//!    so a column gets the type a file DuckDB would give it, and `DESCRIBE`
//!    names those types.
//! 2. One transaction on the tenant: `CREATE TABLE` with those types, then
//!    multi-row `INSERT … SELECT CAST(… AS <type>) FROM (VALUES …)` batches,
//!    every value sent as its `VARCHAR` rendering and cast back on the far
//!    side, then any alias views. A failed load rolls back and leaves nothing.
//!
//! The file is a local scratch path the caller filled from the request (an
//! upload) or from the binary (the demo dataset), never a workspace checkout,
//! so this runs on any pod. It is an Admin credential, minted for the
//! workspace as [`SystemPurpose::FileImport`]: DuckLake refuses `CREATE TABLE`
//! to a Writer.
//!
//! [`TenantImporter`] gives a workspace without a tenant one first
//! ([`crate::ensure_tenant`], idempotent); a workspace whose provisioning fails
//! gets [`ImportError::Tenant`] naming why.
//!
//! The transaction holds the tenant's write lock (Airhouse serializes a
//! tenant's commits) for the whole load, so a large file delays that tenant's
//! other writers until it commits. Onboarding uploads are capped at 200 MiB
//! per file by the caller.

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use sea_orm::DatabaseConnection;
use thiserror::Error;
use tokio::sync::mpsc;
use tokio_postgres::{Client, NoTls, SimpleQueryMessage};
use uuid::Uuid;

use crate::admin::UserRole;
use crate::broker::{BrokerError, DEFAULT_INTERNAL_TTL, SystemPurpose};
use crate::ensure_tenant::{EnsureTenantError, ensure_tenant};

/// The two file formats a file DuckDB reads, and so the two this loads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileFormat {
    Csv,
    Parquet,
}

impl FileFormat {
    /// The format a file name's extension names, case-insensitively.
    pub fn from_file_name(name: &str) -> Option<Self> {
        let ext = Path::new(name).extension()?.to_str()?;
        if ext.eq_ignore_ascii_case("csv") {
            Some(Self::Csv)
        } else if ext.eq_ignore_ascii_case("parquet") {
            Some(Self::Parquet)
        } else {
            None
        }
    }

    fn reader(self) -> &'static str {
        match self {
            Self::Csv => "read_csv_auto",
            Self::Parquet => "read_parquet",
        }
    }
}

/// A `schema.table` (or `schema.view`) in the tenant's catalog.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TableRef {
    pub schema: String,
    pub table: String,
}

impl TableRef {
    pub fn new(schema: impl Into<String>, table: impl Into<String>) -> Self {
        Self {
            schema: schema.into(),
            table: table.into(),
        }
    }

    /// The name in DuckDB's default schema, where an unqualified name lands.
    pub fn main(table: impl Into<String>) -> Self {
        Self::new("main", table)
    }

    fn sql(&self) -> String {
        format!("{}.{}", quote_ident(&self.schema), quote_ident(&self.table))
    }

    /// Refuse a name this load must not create: one in a schema another
    /// owner holds (`app_*` for custom apps, `preview_*` for previews, the
    /// catalogs), or one Oxy's own schema listing hides.
    fn check(&self) -> Result<(), ImportError> {
        if self.schema.is_empty() || self.table.is_empty() {
            return Err(ImportError::InvalidName(format!(
                "'{self}' is not a table name"
            )));
        }
        if self.schema.contains('\0') || self.table.contains('\0') {
            return Err(ImportError::InvalidName(format!(
                "'{self}' contains a NUL byte"
            )));
        }
        let schema = self.schema.to_ascii_lowercase();
        if RESERVED_SCHEMAS.contains(&schema.as_str())
            || RESERVED_SCHEMA_PREFIXES
                .iter()
                .any(|prefix| schema.starts_with(prefix))
        {
            return Err(ImportError::InvalidName(format!(
                "schema '{}' is reserved (custom apps own app_*, previews own preview_*); rename \
                 the file",
                self.schema
            )));
        }
        let table = self.table.to_ascii_lowercase();
        if HIDDEN_TABLE_PREFIXES
            .iter()
            .any(|prefix| table.starts_with(prefix))
        {
            return Err(ImportError::InvalidName(format!(
                "a table named '{}' would be hidden from the workspace's schema listing; rename \
                 the file",
                self.table
            )));
        }
        Ok(())
    }
}

impl fmt::Display for TableRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.schema, self.table)
    }
}

const RESERVED_SCHEMAS: [&str; 4] = ["information_schema", "pg_catalog", "pg_toast", "ducklake"];
const RESERVED_SCHEMA_PREFIXES: [&str; 2] = ["app_", "preview_"];
/// Filtered out by `AirhouseConnector`'s schema listing.
const HIDDEN_TABLE_PREFIXES: [&str; 3] = ["ducklake_", "airhouse_", "_agentic_"];

/// What to do when the target table already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IfExists {
    /// Refuse with [`ImportError::AlreadyExists`]; nothing is written.
    Fail,
    /// Drop the table and the alias views and load afresh, in the same
    /// transaction, so a reader sees the old table or the new one.
    Replace,
}

/// One file to load.
#[derive(Debug, Clone)]
pub struct ImportFile {
    /// A local scratch copy of the bytes: from the request or from the binary.
    pub path: PathBuf,
    pub format: FileFormat,
    pub target: TableRef,
    /// Views over `target` under further names (`SELECT * FROM target`).
    pub aliases: Vec<TableRef>,
    pub if_exists: IfExists,
}

impl ImportFile {
    /// Load `path`, uploaded as `file_name`, under the names a file DuckDB
    /// gives a file in its dataset directory, so SQL written against that
    /// database resolves unchanged:
    ///
    /// - the table `main.<slug of the stem>` (`Orders-2024.csv` → `orders_2024`),
    /// - a view `main."<file name>"`, for `FROM "orders.csv"`,
    /// - a view `"<stem>"."<extension>"`, for `FROM orders.csv`, which DuckDB
    ///   parses as schema and table — how the semantic layer renders a view's
    ///   `table: "orders.csv"`.
    pub fn named_like_a_file_database(
        path: impl Into<PathBuf>,
        file_name: &str,
        if_exists: IfExists,
    ) -> Result<Self, ImportError> {
        let format = FileFormat::from_file_name(file_name).ok_or_else(|| {
            ImportError::InvalidName(format!("'{file_name}' is not a .csv or .parquet file"))
        })?;
        let name = Path::new(file_name);
        let stem = name
            .file_stem()
            .and_then(|s| s.to_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ImportError::InvalidName(format!("'{file_name}' has no name")))?;
        let ext = name
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let table = table_name_for_stem(stem);
        let mut aliases = Vec::new();
        if file_name != table {
            aliases.push(TableRef::main(file_name));
        }
        aliases.push(TableRef::new(stem, ext));
        let target = TableRef::main(table);
        // Checked here as well as at import, so a caller can skip a file whose
        // names (an alias included: `app_x.csv` names the schema `app_x`) are
        // refused before it streams any bytes.
        target.check()?;
        for alias in &aliases {
            alias.check()?;
        }
        Ok(Self {
            path: path.into(),
            format,
            target,
            aliases,
            if_exists,
        })
    }
}

/// The table name a file DuckDB registers a file under: its stem slugged with
/// `_`, prefixed with `_` when it starts with a digit, `unnamed` when nothing
/// is left. The rule `agentic_connector`'s directory loader applies.
pub fn table_name_for_stem(stem: &str) -> String {
    let slug = slugify::slugify(stem, "", "_", None);
    if slug.is_empty() {
        "unnamed".to_string()
    } else if slug.starts_with(|c: char| c.is_ascii_digit()) {
        format!("_{slug}")
    } else {
        slug
    }
}

/// One column of a loaded table, with the DuckDB type it was created with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedColumn {
    pub name: String,
    pub data_type: String,
}

/// What a load created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportOutcome {
    pub table: TableRef,
    pub aliases: Vec<TableRef>,
    pub columns: Vec<ImportedColumn>,
    pub rows: u64,
}

#[derive(Debug, Error)]
pub enum ImportError {
    #[error("Airhouse is not configured on this deployment")]
    NotConfigured,
    /// The workspace has no usable tenant: none could be provisioned, or it
    /// is being deleted.
    #[error(transparent)]
    Tenant(#[from] EnsureTenantError),
    /// The broker found no tenant row after [`ensure_tenant`] said there was
    /// one: a delete raced the load.
    #[error("this workspace has no Airhouse tenant")]
    NoTenant(Uuid),
    #[error("a table named {0} already exists in this workspace's Airhouse")]
    AlreadyExists(TableRef),
    #[error("{0}")]
    InvalidName(String),
    #[error("could not read the file: {0}")]
    Read(String),
    #[error("Airhouse did not take the load: {0}")]
    Load(String),
    #[error("could not obtain an Airhouse credential: {0}")]
    Credential(String),
}

impl From<BrokerError> for ImportError {
    fn from(e: BrokerError) -> Self {
        match e {
            BrokerError::TenantNotFound(workspace_id) => Self::NoTenant(workspace_id),
            other => Self::Credential(other.to_string()),
        }
    }
}

/// Where the load's SQL goes: the tenant over pgwire in production, an
/// in-process DuckDB standing in for it in tests.
#[async_trait]
pub trait ImportSink: Send + Sync {
    /// Run `sql` to completion, discarding any rows.
    async fn execute(&self, sql: &str) -> Result<(), String>;
    /// Run `sql` and read the first column of its first row as a count.
    async fn count(&self, sql: &str) -> Result<u64, String>;
}

/// How large each `INSERT` is allowed to grow, in bytes of SQL and in rows.
#[derive(Debug, Clone, Copy)]
pub struct Batching {
    pub max_bytes: usize,
    pub max_rows: usize,
}

impl Default for Batching {
    fn default() -> Self {
        Self {
            max_bytes: 1024 * 1024,
            max_rows: 10_000,
        }
    }
}

/// Load `file` through `sink` in one transaction. See the module doc.
pub async fn load(sink: &dyn ImportSink, file: &ImportFile) -> Result<ImportOutcome, ImportError> {
    load_with(sink, file, Batching::default()).await
}

/// [`load`] with explicit batch bounds.
pub async fn load_with(
    sink: &dyn ImportSink,
    file: &ImportFile,
    batching: Batching,
) -> Result<ImportOutcome, ImportError> {
    file.target.check()?;
    for alias in &file.aliases {
        alias.check()?;
    }

    if file.if_exists == IfExists::Fail {
        let existing = sink
            .count(&exists_sql(&file.target))
            .await
            .map_err(ImportError::Load)?;
        if existing > 0 {
            return Err(ImportError::AlreadyExists(file.target.clone()));
        }
    }

    // The file is read on a blocking thread and handed over one INSERT at a
    // time, so a large file is never held in memory as SQL.
    let (tx, mut rx) = mpsc::channel::<ReadEvent>(2);
    let reader = {
        let path = file.path.clone();
        let format = file.format;
        let target = file.target.clone();
        tokio::task::spawn_blocking(move || {
            if let Err(e) = read_file(&path, format, &target, batching, &tx) {
                let _ = tx.blocking_send(ReadEvent::Failed(e));
            }
        })
    };

    let columns = match rx.recv().await {
        Some(ReadEvent::Columns(columns)) => columns,
        Some(ReadEvent::Failed(e)) => return Err(ImportError::Read(e)),
        Some(ReadEvent::Insert { .. }) | None => {
            return Err(ImportError::Read(
                "the reader stopped before describing the file".into(),
            ));
        }
    };
    if columns.is_empty() {
        return Err(ImportError::Read("the file has no columns".into()));
    }

    sink.execute("BEGIN").await.map_err(ImportError::Load)?;
    let loaded = async {
        for statement in setup_sql(file, &columns) {
            sink.execute(&statement).await.map_err(ImportError::Load)?;
        }
        let mut rows = 0u64;
        while let Some(event) = rx.recv().await {
            match event {
                ReadEvent::Insert { sql, rows: batch } => {
                    sink.execute(&sql).await.map_err(ImportError::Load)?;
                    rows += batch;
                }
                ReadEvent::Failed(e) => return Err(ImportError::Read(e)),
                ReadEvent::Columns(_) => {}
            }
        }
        for alias in &file.aliases {
            sink.execute(&format!(
                "CREATE VIEW {} AS SELECT * FROM {}",
                alias.sql(),
                file.target.sql()
            ))
            .await
            .map_err(ImportError::Load)?;
        }
        sink.execute("COMMIT").await.map_err(ImportError::Load)?;
        Ok(rows)
    }
    .await;

    // Dropping the receiver stops a reader still producing batches.
    drop(rx);
    let _ = reader.await;

    match loaded {
        Ok(rows) => Ok(ImportOutcome {
            table: file.target.clone(),
            aliases: file.aliases.clone(),
            columns,
            rows,
        }),
        Err(e) => {
            if let Err(rollback) = sink.execute("ROLLBACK").await {
                tracing::warn!(
                    table = %file.target,
                    "Airhouse import: ROLLBACK after a failed load also failed: {rollback}"
                );
            }
            Err(e)
        }
    }
}

enum ReadEvent {
    Columns(Vec<ImportedColumn>),
    Insert { sql: String, rows: u64 },
    Failed(String),
}

/// The statements that run inside the transaction before the first row.
fn setup_sql(file: &ImportFile, columns: &[ImportedColumn]) -> Vec<String> {
    let mut statements = Vec::new();
    let schemas: BTreeSet<&str> = std::iter::once(&file.target)
        .chain(&file.aliases)
        .map(|t| t.schema.as_str())
        .collect();
    for schema in schemas {
        statements.push(format!(
            "CREATE SCHEMA IF NOT EXISTS {}",
            quote_ident(schema)
        ));
    }
    if file.if_exists == IfExists::Replace {
        for alias in &file.aliases {
            statements.push(format!("DROP VIEW IF EXISTS {}", alias.sql()));
        }
        statements.push(format!("DROP TABLE IF EXISTS {}", file.target.sql()));
    }
    let definitions = columns
        .iter()
        .map(|c| format!("{} {}", quote_ident(&c.name), c.data_type))
        .collect::<Vec<_>>()
        .join(", ");
    statements.push(format!(
        "CREATE TABLE {} ({definitions})",
        file.target.sql()
    ));
    statements
}

fn exists_sql(table: &TableRef) -> String {
    format!(
        "SELECT count(*) FROM information_schema.tables WHERE table_schema = {} AND table_name = {}",
        quote_literal(&table.schema),
        quote_literal(&table.table)
    )
}

/// Read `path` with DuckDB, send its columns, then one `INSERT` per batch.
fn read_file(
    path: &Path,
    format: FileFormat,
    target: &TableRef,
    batching: Batching,
    tx: &mpsc::Sender<ReadEvent>,
) -> Result<(), String> {
    let conn = duckdb::Connection::open_in_memory().map_err(|e| e.to_string())?;
    let source = format!(
        "{}({})",
        format.reader(),
        quote_literal(&path.to_string_lossy())
    );
    conn.execute_batch(&format!(
        "CREATE TEMP VIEW import_source AS SELECT * FROM {source}"
    ))
    .map_err(|e| e.to_string())?;

    let columns: Vec<ImportedColumn> = {
        let mut stmt = conn
            .prepare("SELECT column_name, column_type FROM (DESCRIBE import_source)")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok(ImportedColumn {
                    name: row.get(0)?,
                    data_type: row.get(1)?,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())?
    };
    if tx
        .blocking_send(ReadEvent::Columns(columns.clone()))
        .is_err()
    {
        return Ok(());
    }
    if columns.is_empty() {
        return Ok(());
    }

    let select = format!(
        "SELECT {} FROM import_source",
        columns
            .iter()
            .map(|c| format!("CAST({} AS VARCHAR)", quote_ident(&c.name)))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let mut stmt = conn.prepare(&select).map_err(|e| e.to_string())?;
    let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
    let mut batch = InsertBatch::new(target, &columns);
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let mut values = Vec::with_capacity(columns.len());
        for i in 0..columns.len() {
            let value: Option<String> = row.get(i).map_err(|e| e.to_string())?;
            values.push(value);
        }
        batch.push(&values);
        if batch.rows as usize >= batching.max_rows || batch.sql.len() >= batching.max_bytes {
            let full = std::mem::replace(&mut batch, InsertBatch::new(target, &columns));
            if tx.blocking_send(full.finish()).is_err() {
                return Ok(());
            }
        }
    }
    if batch.rows > 0 {
        let _ = tx.blocking_send(batch.finish());
    }
    Ok(())
}

/// `INSERT INTO t SELECT CAST(c0 AS T0), … FROM (VALUES (…), …) AS v(c0, …)`.
///
/// Every value travels as a string literal (or `NULL`) and is cast to its
/// column's type on the far side, once per column rather than once per value.
struct InsertBatch {
    sql: String,
    suffix: String,
    rows: u64,
}

impl InsertBatch {
    fn new(target: &TableRef, columns: &[ImportedColumn]) -> Self {
        let casts = columns
            .iter()
            .enumerate()
            .map(|(i, c)| format!("CAST(c{i} AS {})", c.data_type))
            .collect::<Vec<_>>()
            .join(", ");
        let names = (0..columns.len())
            .map(|i| format!("c{i}"))
            .collect::<Vec<_>>()
            .join(", ");
        Self {
            sql: format!("INSERT INTO {} SELECT {casts} FROM (VALUES ", target.sql()),
            suffix: format!(") AS v({names})"),
            rows: 0,
        }
    }

    fn push(&mut self, values: &[Option<String>]) {
        if self.rows > 0 {
            self.sql.push_str(", ");
        }
        self.sql.push('(');
        for (i, value) in values.iter().enumerate() {
            if i > 0 {
                self.sql.push_str(", ");
            }
            match value {
                Some(v) => self.sql.push_str(&quote_literal(v)),
                None => self.sql.push_str("NULL"),
            }
        }
        self.sql.push(')');
        self.rows += 1;
    }

    fn finish(mut self) -> ReadEvent {
        self.sql.push_str(&self.suffix);
        ReadEvent::Insert {
            sql: self.sql,
            rows: self.rows,
        }
    }
}

fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn quote_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

// ── Production: the workspace's tenant over pgwire ──────────────────────────

/// Loads files into a workspace's Airhouse. Production is [`TenantImporter`];
/// a test hands its own over an in-process DuckDB.
#[async_trait]
pub trait WorkspaceImporter: Send + Sync {
    /// `Ok` when a load into `workspace_id` can be attempted: the deployment
    /// has Airhouse and the workspace has a tenant, provisioned now if it had
    /// none. Checked before an upload's body is read, so a failure is reported
    /// before the bytes are sent.
    async fn ready(&self, workspace_id: Uuid) -> Result<(), ImportError>;
    async fn import(
        &self,
        workspace_id: Uuid,
        file: &ImportFile,
    ) -> Result<ImportOutcome, ImportError>;
}

/// The workspace's own tenant (provisioned first when missing), reached with
/// an Admin credential minted by the service-account broker for
/// [`SystemPurpose::FileImport`].
pub struct TenantImporter {
    db: DatabaseConnection,
}

impl TenantImporter {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

#[async_trait]
impl WorkspaceImporter for TenantImporter {
    async fn ready(&self, workspace_id: Uuid) -> Result<(), ImportError> {
        if crate::token_broker().is_none() || crate::wire_endpoint().is_none() {
            return Err(ImportError::NotConfigured);
        }
        ensure_tenant(&self.db, workspace_id).await?;
        Ok(())
    }

    async fn import(
        &self,
        workspace_id: Uuid,
        file: &ImportFile,
    ) -> Result<ImportOutcome, ImportError> {
        self.ready(workspace_id).await?;
        let broker = crate::token_broker().ok_or(ImportError::NotConfigured)?;
        let endpoint = crate::wire_endpoint().ok_or(ImportError::NotConfigured)?;
        let cred = broker
            .mint_for_system(
                workspace_id,
                SystemPurpose::FileImport,
                UserRole::Admin,
                DEFAULT_INTERNAL_TTL,
            )
            .await?;

        // NoTls, as `AirhouseConnector`: the wire endpoint is in-cluster only.
        let mut config = tokio_postgres::Config::new();
        config
            .host(&endpoint.host)
            .port(endpoint.port)
            .user(&cred.username)
            .password(&cred.password)
            .dbname(&cred.tenant);
        let (client, connection) = config.connect(NoTls).await.map_err(|e| {
            ImportError::Load(format!(
                "could not connect to {}:{}: {}",
                endpoint.host,
                endpoint.port,
                pg_text(&e)
            ))
        })?;
        let driver = tokio::spawn(async move {
            if let Err(e) = connection.await {
                tracing::warn!("Airhouse import connection ended: {e}");
            }
        });

        let outcome = load(&WireSink { client }, file).await;
        // The sink owned the client; it is dropped, so the driver ends.
        let _ = driver.await;
        if let Ok(outcome) = &outcome {
            tracing::info!(
                %workspace_id,
                table = %outcome.table,
                rows = outcome.rows,
                "Airhouse import: loaded a file"
            );
        }
        outcome
    }
}

struct WireSink {
    client: Client,
}

#[async_trait]
impl ImportSink for WireSink {
    async fn execute(&self, sql: &str) -> Result<(), String> {
        self.client
            .simple_query(sql)
            .await
            .map(|_| ())
            .map_err(|e| pg_text(&e))
    }

    async fn count(&self, sql: &str) -> Result<u64, String> {
        let messages = self
            .client
            .simple_query(sql)
            .await
            .map_err(|e| pg_text(&e))?;
        for message in messages {
            if let SimpleQueryMessage::Row(row) = message {
                return row
                    .get(0)
                    .unwrap_or("0")
                    .parse::<u64>()
                    .map_err(|e| format!("count did not parse: {e}"));
            }
        }
        Ok(0)
    }
}

/// The server's message with its SQLSTATE rather than `tokio_postgres`'s
/// "db error". `DETAIL` is left out: it can carry a row's values.
fn pg_text(e: &tokio_postgres::Error) -> String {
    match e.as_db_error() {
        Some(db) => format!("[{}] {}", db.code().code(), db.message()),
        None => e.to_string(),
    }
}

#[cfg(test)]
#[path = "import_tests.rs"]
mod tests;
