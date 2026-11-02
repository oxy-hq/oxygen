use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use arrow::array::RecordBatch;
use arrow::datatypes::SchemaRef;
use df_interchange::Interchange;
use duckdb::Connection;
use slugify::slugify;

use super::duckdb_pool::{PoolKey, PoolTarget, pool};
use super::engine::Engine;
use crate::adapters::secrets::SecretsManager;
use crate::config::model::{DuckDBOptions, DuckDbS3Mirror};
use crate::connector::constants::{
    CREATE_CONN, CREATE_TEMP_TABLE, EXECUTE_QUERY, PREPARE_DUCKDB_STMT, SET_FILE_SEARCH_PATH,
    SET_TEMP_DIRECTORY,
};
use crate::connector::utils::connector_internal_error;
use oxy_shared::errors::OxyError;

#[derive(Debug)]
pub(super) struct DuckDB {
    options: DuckDBOptions,
    /// Compiled-config-only: present on a stateless replica when the local data
    /// was mirrored to S3 at compile time. Its presence means "read from S3";
    /// local/IDE reads never carry it.
    s3_mirror: Option<DuckDbS3Mirror>,
    secrets_manager: SecretsManager,
}

impl DuckDB {
    pub fn new(
        options: DuckDBOptions,
        s3_mirror: Option<DuckDbS3Mirror>,
        secrets_manager: SecretsManager,
    ) -> Self {
        DuckDB {
            options,
            s3_mirror,
            secrets_manager,
        }
    }

    /// Hand out a connection ready to run a query.
    ///
    /// For `Local` and `File` modes, the heavy work (opening the DB, loading
    /// CSVs into tables, installing extensions) happens exactly once per
    /// `(target, file mtimes)` key in [`super::duckdb_pool`]. Subsequent
    /// calls return a cheap `try_clone()` that shares the cached database.
    ///
    /// For `DuckLake` mode the per-call attach statements are derived from
    /// runtime secrets, so we keep the historical "fresh connection per
    /// query" behavior to avoid serving stale credentials.
    ///
    /// The non-DuckLake paths are wrapped in [`tokio::task::spawn_blocking`]
    /// because DuckDB's Rust binding is fully synchronous (statement
    /// preparation, CSV scans, file mtime stats). Running them on the async
    /// runtime would block worker threads — particularly painful on a
    /// busy `oxy serve` process where one slow CSV import would stall every
    /// other future on the same worker.
    pub async fn init_connection(&self) -> Result<Connection, OxyError> {
        // Stateless-fleet read: the compile worker mirrored this workspace's
        // local DuckDB data to S3, so the local path doesn't exist here. Read it
        // over httpfs instead. (config.yml never carries a mirror, so local/IDE
        // reads fall through to the normal Local/File paths below.)
        if let Some(mirror) = &self.s3_mirror {
            let stmts = build_s3_mirror_sql(mirror);
            return tokio::task::spawn_blocking(move || init_s3_mirror_blocking(stmts))
                .await
                .map_err(|e| OxyError::DBError(format!("DuckDB s3-mirror join error: {e}")))?;
        }
        match &self.options {
            DuckDBOptions::Local { file_search_path } => {
                let path = file_search_path.clone();
                tokio::task::spawn_blocking(move || checkout_local_blocking(&path))
                    .await
                    .map_err(|e| OxyError::DBError(format!("DuckDB checkout join error: {e}")))?
            }
            DuckDBOptions::File { path } => {
                let path = path.clone();
                let read_only = crate::file_duckdb_limits::limits_apply();
                tokio::task::spawn_blocking(move || checkout_file_blocking(&path, read_only))
                    .await
                    .map_err(|e| OxyError::DBError(format!("DuckDB checkout join error: {e}")))?
            }
            DuckDBOptions::DuckLake(_) => self.init_ducklake().await,
        }
    }

    async fn init_ducklake(&self) -> Result<Connection, OxyError> {
        let DuckDBOptions::DuckLake(config) = &self.options else {
            unreachable!("init_ducklake called with non-DuckLake options");
        };
        // Async: fetch secrets before entering spawn_blocking.
        let attach_stmts = config.to_duckdb_attach_stmt(&self.secrets_manager).await?;
        tracing::info!("Executing DuckDB attach statements: {:?}", attach_stmts);
        tokio::task::spawn_blocking(move || init_ducklake_blocking(attach_stmts))
            .await
            .map_err(|e| OxyError::DBError(format!("DuckDB ducklake join error: {e}")))?
    }
}

/// Synchronous body of [`DuckDB::init_ducklake`]. Runs inside
/// `spawn_blocking` to avoid blocking Tokio workers during `INSTALL`
/// (which may fetch extensions from the network on first run).
fn init_ducklake_blocking(attach_stmts: Vec<String>) -> Result<Connection, OxyError> {
    let conn =
        Connection::open_in_memory().map_err(|err| connector_internal_error(CREATE_CONN, &err))?;
    conn.execute("INSTALL ducklake", [])
        .map_err(|err| connector_internal_error(CREATE_CONN, &err))?;
    conn.execute("LOAD ducklake", [])
        .map_err(|err| connector_internal_error(CREATE_CONN, &err))?;
    conn.execute("INSTALL postgres", [])
        .map_err(|err| connector_internal_error(CREATE_CONN, &err))?;
    conn.execute("LOAD postgres", [])
        .map_err(|err| connector_internal_error(CREATE_CONN, &err))?;
    for stmt in &attach_stmts {
        tracing::debug!("Executing DuckDB statement: {}", stmt);
        conn.execute(stmt, [])
            .map_err(|err| connector_internal_error(CREATE_CONN, &err))?;
    }
    install_icu(&conn)?;
    load_icu(&conn)?;
    Ok(conn)
}

/// Build the connection-setup SQL for an S3-mirrored DuckDB: load `httpfs`,
/// create the S3 secret (pod credential chain — no stored keys), then register
/// each mirrored data file as a view (`Local` mode) or attach the mirrored
/// database read-only (`File` mode). The views read Parquet/CSV lazily from S3,
/// so DuckDB still pushes projections/filters down rather than downloading
/// whole objects.
pub fn build_s3_mirror_sql(mirror: &DuckDbS3Mirror) -> Vec<String> {
    // `httpfs` + the secret come from `oxy_shared::duckdb_s3`, shared with the
    // pre-aggregation read path. The endpoint handling in particular (DuckDB
    // prepends the scheme itself, so a stored `http://host:9000` must be
    // stripped to `host:9000` or every read fails with "Could not resolve
    // hostname") is the kind of thing that must exist once.
    let mut stmts = oxy_shared::duckdb_s3::s3_setup_sql(
        "duckdb_mirror_s3",
        mirror.region.as_deref(),
        mirror.endpoint_url.as_deref(),
        // No fallback on this path: a tripped bound is a failed query, not a
        // slower one. See `S3ReadBounds`.
        oxy_shared::duckdb_s3::S3ReadBounds::NO_FALLBACK,
    );

    if let Some(key) = &mirror.attach_key {
        stmts.push(format!(
            "ATTACH 's3://{}/{}' AS mirror (READ_ONLY)",
            escape_sql_string(&mirror.bucket),
            escape_sql_string(key)
        ));
        stmts.push("USE mirror".to_string());
    }
    for table in &mirror.tables {
        let reader = if table.format.eq_ignore_ascii_case("csv") {
            "read_csv_auto"
        } else {
            "read_parquet"
        };
        let source = format!(
            "{}('s3://{}/{}')",
            reader,
            escape_sql_string(&mirror.bucket),
            escape_sql_string(&table.key)
        );
        // 1) Slug-named view (`oxymart`) — matches the local connector's pooled
        //    table registration; used by direct references / list_tables.
        stmts.push(format!(
            "CREATE OR REPLACE VIEW {} AS SELECT * FROM {source}",
            quote_sql_identifier(&table.table),
        ));
        // 2) Filename-qualified view. The semantic model references the table by
        //    its source filename (`table: "oxymart.csv"`); in local mode that
        //    resolves through DuckDB's `file_search_path` replacement scan, which
        //    the stateless fleet has no equivalent for. DuckDB parses
        //    `FROM oxymart.csv` as schema.table, so register a matching
        //    `"<stem>"."<ext>"` view (stem = slug, ext = format), or `FROM
        //    oxymart.csv` finds nothing and fails "No files found".
        stmts.push(format!(
            "CREATE SCHEMA IF NOT EXISTS {}",
            quote_sql_identifier(&table.table)
        ));
        stmts.push(format!(
            "CREATE OR REPLACE VIEW {}.{} AS SELECT * FROM {source}",
            quote_sql_identifier(&table.table),
            quote_sql_identifier(&table.format),
        ));
    }
    stmts
}

/// Synchronous body of the S3-mirror path. Fresh in-memory connection per
/// query (like DuckLake) — `INSTALL` is disk-cached after first run, and the
/// remaining `LOAD` / secret / view statements are cheap.
fn init_s3_mirror_blocking(stmts: Vec<String>) -> Result<Connection, OxyError> {
    let conn =
        Connection::open_in_memory().map_err(|err| connector_internal_error(CREATE_CONN, &err))?;
    for stmt in &stmts {
        tracing::debug!("DuckDB s3-mirror stmt: {}", stmt);
        conn.execute(stmt, [])
            .map_err(|err| s3_mirror_setup_error(stmt, &err))?;
    }
    install_icu(&conn)?;
    load_icu(&conn)?;
    Ok(conn)
}

/// Turn a DuckDB error from an S3-mirror setup statement into a message that
/// names the step and what an operator should check. This path only runs on a
/// stateless replica serving a workspace whose local DuckDB data was mirrored
/// to S3 at compile time, so the failures are S3/extension-shaped, not the
/// usual "bad SQL".
fn s3_mirror_setup_error(stmt: &str, err: &duckdb::Error) -> OxyError {
    let hint = if stmt.contains("httpfs") {
        "could not load the DuckDB `httpfs` extension — the node needs outbound network to the \
         extension repository on first run"
    } else if stmt.starts_with("SET ") {
        // `SET http_timeout` / `http_retries` are registered BY `LOAD httpfs`,
        // so a rejection here is the extension or the setting name drifting
        // under a DuckDB bump — nothing to do with S3 objects or IAM, which is
        // what the catch-all below would have told an operator to go check.
        "the DuckDB build rejected an `httpfs` read-bound setting — the extension may not be \
         loaded, or the setting name changed in this DuckDB version"
    } else if stmt.starts_with("ATTACH") {
        "could not attach the S3-mirrored DuckDB file read-only — the bundled DuckDB may not \
         support remote ATTACH, the object may be missing, or the pod's IAM role may lack \
         s3:GetObject on the bucket"
    } else if stmt.contains("SECRET") {
        "could not create the S3 credential for the mirrored DuckDB warehouse (expected the pod's \
         instance role via the credential chain)"
    } else {
        "could not register an S3-mirrored DuckDB table — check the mirrored object exists and \
         the pod's IAM role can read it"
    };
    OxyError::DBError(format!("DuckDB S3 mirror: {hint}. Underlying error: {err}"))
}

/// Synchronous body of [`DuckDB::init_connection`] for `Local` mode. Lives
/// outside the async fn so it can run inside `spawn_blocking`.
fn checkout_local_blocking(file_search_path: &str) -> Result<Connection, OxyError> {
    let canonical_dir = canonicalize_local_dir(file_search_path)?;
    let files = collect_supported_files(&canonical_dir)?;
    if files.is_empty() {
        return Err(OxyError::DBError(format!(
            "DuckDB directory '{}' contains no .csv or .parquet files. Add at least one supported file or point to a different directory.",
            canonical_dir.display()
        )));
    }

    let key = PoolKey::local(canonical_dir.clone(), &files)?;
    let canonical_str = canonical_dir.display().to_string();
    let entry = pool().get_or_init(key, || {
        let conn = init_local_db(&canonical_dir, &files)?;
        // Re-run on every clone: cloned connections get a fresh session
        // and don't inherit `file_search_path`, `temp_directory`, or the
        // `LOAD icu` from the primary.
        let setup = vec![
            format!(
                "SET file_search_path = '{}'",
                escape_sql_string(&canonical_str)
            ),
            format!(
                "SET temp_directory = '{}'",
                escape_sql_string(&format!("{canonical_str}/tmp"))
            ),
            "LOAD icu".to_string(),
        ];
        Ok((conn, setup))
    })?;
    entry.checkout()
}

/// Check out a connection to a DuckDB file database from the process-wide
/// pool, initialising it if this is the first call for `path`.
///
/// The returned connection shares the same underlying `duckdb_database`
/// handle as every other connection checked out from the pool for this path.
/// Callers (e.g. the airform integration) must use this instead of opening a
/// fresh `Connection::open(path)` to avoid having two independent
/// `duckdb_database` handles on the same file in the same process — a
/// situation that bypasses OS advisory locking and causes SIGSEGV in
/// DuckDB's native code.
pub fn checkout_file_connection(path: &str) -> Result<Connection, OxyError> {
    checkout_file_blocking(path, crate::file_duckdb_limits::limits_apply())
}

/// Check out a pooled connection for a local directory database (CSV/Parquet
/// files). On the first call for `file_search_path` the files are loaded into
/// an in-memory DuckDB; subsequent calls return a cheap `try_clone()` of the
/// same primary connection. Mtime-based eviction ensures stale data is never
/// served after a file change.
pub fn checkout_local_connection(file_search_path: &str) -> Result<Connection, OxyError> {
    checkout_local_blocking(file_search_path)
}

/// Hand the pool back a dataset directory whose lifetime has ended, dropping
/// the in-memory database it pinned along with every table materialised into
/// it.
///
/// The pool's bound is one slot per target, which bounds nothing when the
/// caller mints a fresh target per unit of work — a per-run `TempDir` dataset
/// is checked out once and then never again, so no same-key replacement ever
/// evicts it. Callers that own such a directory's lifetime must call this when
/// it ends; a caller pointing at a *stable* workspace dataset should not, since
/// keeping the handle warm across queries is the whole point of the pool.
///
/// Call this while the directory still exists. The key is built the way
/// [`checkout_local_blocking`] builds it — canonicalized — and canonicalization
/// needs the path to resolve, so a release issued after the directory is gone
/// falls back to the raw path and matches the checked-out key only where the
/// two happen to coincide. On macOS they never do: every `TempDir` sits under
/// the `/var` → `/private/var` symlink, so a late release there silently leaks
/// the handle it meant to free.
pub fn release_local_connection(dataset_dir: impl AsRef<Path>) {
    let path = dataset_dir.as_ref();
    let dir = match path.canonicalize() {
        Ok(canonical) => canonical,
        Err(e) => {
            // Fall through with the raw path rather than returning: it is the
            // right key in the case where the caller already handed us a
            // canonical path, and a miss costs nothing beyond the leak we were
            // already going to take.
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "DuckDB pool: cannot canonicalize a dataset directory being released, so the \
                 release may not match the handle that was checked out; release before the \
                 directory is removed"
            );
            path.to_path_buf()
        }
    };
    pool().release(&PoolTarget::Local { dir });
}

/// Synchronous body of [`DuckDB::init_connection`] for `File` mode.
///
/// `read_only` is set on the cloud roles (`oxy::file_duckdb_limits`): there a
/// file DuckDB is deprecated and only revisions compiled before its refusal
/// still reach it, and opening it read-only makes a write fail at DuckDB rather
/// than land in a file nobody else reads. The single-process role opens it read-write, as it always has.
fn checkout_file_blocking(path: &str, read_only: bool) -> Result<Connection, OxyError> {
    // `Connection::open` on a missing path silently CREATES an empty database,
    // which then fails every query with a confusing "table not found". Catch it
    // here with an actionable message instead — this is the common shape on a
    // stateless replica whose `.duckdb` wasn't mirrored to S3.
    if !Path::new(path).exists() {
        return Err(OxyError::DBError(format!(
            "DuckDB database file '{path}' was not found. If this is a stateless / cloud \
             deployment, the file must be mirrored to S3 by the compile worker — set \
             OXY_COMPILE_BLOB_S3_BUCKET and recompile (files over 256 MiB are skipped and stay \
             local-only). Otherwise check the path."
        )));
    }
    let key = PoolKey::file(PathBuf::from(path))?;
    let path_owned = path.to_owned();
    let entry = pool().get_or_init(key, move || {
        let conn = open_file_database(&path_owned, read_only)?;
        install_icu(&conn)?;
        // `LOAD icu` is per-session; re-run on every clone.
        Ok((conn, vec!["LOAD icu".to_string()]))
    })?;
    entry.checkout()
}

/// Open a `.duckdb` file, read-only when `read_only` is set.
fn open_file_database(path: &str, read_only: bool) -> Result<Connection, OxyError> {
    if !read_only {
        return Connection::open(path).map_err(|err| connector_internal_error(CREATE_CONN, &err));
    }
    let config = duckdb::Config::default()
        .access_mode(duckdb::AccessMode::ReadOnly)
        .map_err(|err| connector_internal_error(CREATE_CONN, &err))?;
    Connection::open_with_flags(path, config)
        .map_err(|err| connector_internal_error(CREATE_CONN, &err))
}

/// First-time initialization for a `Local` mode database. Builds an
/// in-memory DuckDB pre-loaded with one regular (non-temporary) table per
/// file in `dir` so cloned connections from the pool see them. Tables are
/// `CREATE TABLE` rather than `CREATE TEMPORARY TABLE` because temp tables
/// are session-local and would be invisible to cloned connections.
fn init_local_db(
    canonical_dir: &Path,
    files: &[(String, PathBuf)],
) -> Result<Connection, OxyError> {
    let conn =
        Connection::open_in_memory().map_err(|err| connector_internal_error(CREATE_CONN, &err))?;

    let canonical_str = canonical_dir.display().to_string();
    let dir_set_stmt = format!(
        "SET file_search_path = '{}';",
        escape_sql_string(&canonical_str)
    );
    conn.execute(&dir_set_stmt, [])
        .map_err(|err| connector_internal_error(SET_FILE_SEARCH_PATH, &err))?;
    let temp_set_stmt = format!(
        "SET temp_directory = '{}';",
        escape_sql_string(&format!("{canonical_str}/tmp"))
    );
    conn.execute(&temp_set_stmt, [])
        .map_err(|err| connector_internal_error(SET_TEMP_DIRECTORY, &err))?;

    for (stem, path) in files {
        let table_name = slugify!(stem, separator = "_");
        let path_display = path.display().to_string();
        let create_stmt = format!(
            "CREATE TABLE {} AS FROM '{}'",
            quote_sql_identifier(&table_name),
            escape_sql_string(&path_display)
        );
        tracing::info!(
            "Creating pooled table '{}' from file '{}'",
            table_name,
            path_display
        );
        conn.execute(&create_stmt, [])
            .map_err(|err| connector_internal_error(CREATE_TEMP_TABLE, &err))?;

        // Also expose the table under the full filename (e.g. "climbing.csv")
        // so semantic-model views that declare `table: "climbing.csv"` resolve
        // against the in-memory table instead of triggering a per-query file
        // scan — which reads the CSV from disk every time.
        if let Some(full_name) = path.file_name().and_then(|n| n.to_str()) {
            let full_escaped = full_name.replace('"', "\"\"");
            if full_escaped != table_name {
                let alias_sql = format!(
                    "CREATE OR REPLACE VIEW {full_quoted} AS SELECT * FROM {table_quoted}",
                    full_quoted = quote_sql_identifier(full_name),
                    table_quoted = quote_sql_identifier(&table_name),
                );
                let _ = conn.execute(&alias_sql, []);
            }
        }

        // Also register a schema-qualified alias `"<stem>"."<ext>"`. The
        // semantic-model compiler renders `table: stores.parquet` as an
        // UNQUOTED `FROM stores.parquet`, which DuckDB parses as schema.table.
        // The single-identifier alias above ("stores.parquet") only matches the
        // quoted form, so the unquoted reference falls back to DuckDB's file-
        // replacement scan. That scan's resolution of a qualified name via
        // `file_search_path` is environment-sensitive — it silently finds
        // nothing in some setups (observed for Parquet under `oxy serve`),
        // leaving callers with zero rows. Registering the qualified view makes
        // `FROM stores.parquet` resolve to a real catalog object regardless of
        // the replacement scan. Mirrors the S3-mirror path (`build_s3_mirror_sql`).
        if let (Some(stem_str), Some(ext_str)) = (
            path.file_stem().and_then(|s| s.to_str()),
            path.extension().and_then(|e| e.to_str()),
        ) {
            let schema_quoted = quote_sql_identifier(stem_str);
            let _ = conn.execute(&format!("CREATE SCHEMA IF NOT EXISTS {schema_quoted}"), []);
            let qualified_alias_sql = format!(
                "CREATE OR REPLACE VIEW {schema_quoted}.{ext_quoted} AS SELECT * FROM {table_quoted}",
                ext_quoted = quote_sql_identifier(ext_str),
                table_quoted = quote_sql_identifier(&table_name),
            );
            let _ = conn.execute(&qualified_alias_sql, []);
        }
    }

    install_icu(&conn)?;
    tracing::debug!(
        "Initialized pooled DuckDB with file search path '{}'",
        canonical_str,
    );
    Ok(conn)
}

fn install_icu(conn: &Connection) -> Result<(), OxyError> {
    conn.execute("INSTALL icu", [])
        .map_err(|err| connector_internal_error(CREATE_CONN, &err))?;
    Ok(())
}

fn load_icu(conn: &Connection) -> Result<(), OxyError> {
    conn.execute("LOAD icu", [])
        .map_err(|err| connector_internal_error(CREATE_CONN, &err))?;
    Ok(())
}

/// Escape single quotes for inclusion inside a DuckDB single-quoted string literal.
fn escape_sql_string(s: &str) -> String {
    s.replace('\'', "''")
}

/// Wrap `name` in double quotes, escaping any embedded double quotes per SQL rules.
fn quote_sql_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Canonicalize `file_search_path` and verify it points to an existing directory.
fn canonicalize_local_dir(file_search_path: &str) -> Result<PathBuf, OxyError> {
    let path = Path::new(file_search_path);
    let canonical = path.canonicalize().map_err(|e| {
        OxyError::DBError(format!(
            "DuckDB dataset directory '{file_search_path}' was not found ({e}). If this is a \
             stateless / cloud deployment, the workspace's local DuckDB data must be mirrored to \
             S3 by the compile worker — set OXY_COMPILE_BLOB_S3_BUCKET and recompile (note files \
             over 256 MiB are skipped and stay local-only). Otherwise check that the path exists \
             and is readable."
        ))
    })?;
    if !canonical.is_dir() {
        return Err(OxyError::DBError(format!(
            "DuckDB path '{}' must be a directory containing .csv or .parquet files.",
            canonical.display()
        )));
    }
    Ok(canonical)
}

/// Scan `dir` (non-recursively) for `.csv` and `.parquet` files and return
/// `(file_stem, path)` pairs sorted by stem for deterministic output.
///
/// When two files share a stem (e.g. `orders.csv` and `orders.parquet`), only
/// the `.parquet` file is returned.
fn collect_supported_files(dir: &Path) -> Result<Vec<(String, PathBuf)>, OxyError> {
    let entries = std::fs::read_dir(dir).map_err(|e| {
        OxyError::DBError(format!(
            "Cannot read DuckDB directory '{}': {e}",
            dir.display()
        ))
    })?;

    // BTreeMap gives deterministic iteration order by stem name.
    let mut candidates: BTreeMap<String, (PathBuf, bool)> = BTreeMap::new();
    for entry_result in entries {
        let entry = match entry_result {
            Ok(e) => e,
            Err(err) => {
                tracing::warn!(
                    "Skipping unreadable entry in DuckDB directory '{}': {err}",
                    dir.display()
                );
                continue;
            }
        };
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_default();
        if ext != "csv" && ext != "parquet" {
            continue;
        }
        let stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => continue,
        };
        let is_parquet = ext == "parquet";
        candidates
            .entry(stem)
            .and_modify(|e| {
                if is_parquet {
                    *e = (path.clone(), true);
                }
            })
            .or_insert((path, is_parquet));
    }

    Ok(candidates
        .into_iter()
        .map(|(stem, (path, _))| (stem, path))
        .collect())
}

impl Engine for DuckDB {
    async fn run_query_with_limit(
        &self,
        query: &str,
        _dry_run_limit: Option<u64>,
    ) -> Result<(Vec<RecordBatch>, SchemaRef), OxyError> {
        let query = query.to_string();

        // S3-mirror views read Parquet/CSV from S3 lazily, so a missing object
        // or a credentials problem surfaces here at execution, not at setup.
        let s3_mirror = self.s3_mirror.is_some();
        let conn = self.init_connection().await?;
        let mut stmt = conn
            .prepare(&query)
            .map_err(|err| connector_internal_error(PREPARE_DUCKDB_STMT, &err))?;
        let arrow_stream = stmt.query_arrow([]).map_err(|err| {
            let base = connector_internal_error(EXECUTE_QUERY, &err);
            if s3_mirror {
                OxyError::DBError(format!(
                    "{base} — this DuckDB warehouse is served from S3 on this node; verify the \
                     mirrored objects exist and the pod's IAM role can read them (s3:GetObject)."
                ))
            } else {
                base
            }
        })?;
        let duckdb_chunks: Vec<_> = arrow_stream.collect();
        tracing::debug!("Query results: {:?}", duckdb_chunks);
        // `Interchange::from_arrow_58` indexes `df[0]` without an empty
        // guard (df-interchange-0.3.3/src/from_arrow.rs:19), so a
        // zero-chunk result — zero-row queries, some startup metadata
        // calls — panics. Return the empty-result shape directly.
        if duckdb_chunks.is_empty() {
            return Ok((
                Vec::new(),
                std::sync::Arc::new(arrow::datatypes::Schema::empty()),
            ));
        }
        let arrow_chunks = Interchange::from_arrow_58(duckdb_chunks)
            .map_err(|err| connector_internal_error(EXECUTE_QUERY, &err))?
            .to_arrow_58()
            .map_err(|err| connector_internal_error(EXECUTE_QUERY, &err))?;
        let schema: SchemaRef = arrow_chunks
            .first()
            .map(|b| b.schema())
            .unwrap_or_else(|| std::sync::Arc::new(arrow::datatypes::Schema::empty()));
        Ok((arrow_chunks, schema))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_file(dir: &Path, name: &str, contents: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn escape_sql_string_doubles_single_quotes() {
        assert_eq!(escape_sql_string("no quotes"), "no quotes");
        assert_eq!(escape_sql_string("O'Brien"), "O''Brien");
        assert_eq!(escape_sql_string("'a'b'"), "''a''b''");
    }

    #[test]
    fn quote_sql_identifier_wraps_and_escapes_double_quotes() {
        assert_eq!(quote_sql_identifier("orders"), "\"orders\"");
        assert_eq!(quote_sql_identifier("weird\"name"), "\"weird\"\"name\"");
        assert_eq!(quote_sql_identifier(""), "\"\"");
    }

    #[test]
    fn canonicalize_local_dir_accepts_valid_directory() {
        let tmp = TempDir::new().unwrap();
        let canonical = canonicalize_local_dir(tmp.path().to_str().unwrap()).unwrap();
        assert_eq!(canonical, tmp.path().canonicalize().unwrap());
    }

    #[test]
    fn canonicalize_local_dir_rejects_nonexistent_path() {
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("does_not_exist");
        let err = canonicalize_local_dir(missing.to_str().unwrap()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("was not found"), "unexpected error: {msg}");
    }

    #[test]
    fn canonicalize_local_dir_rejects_file() {
        let tmp = TempDir::new().unwrap();
        let file = write_file(tmp.path(), "orders.csv", "a,b\n1,2\n");
        let err = canonicalize_local_dir(file.to_str().unwrap()).unwrap_err();
        assert!(
            err.to_string().contains("must be a directory"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn collect_supported_files_returns_csv_and_parquet() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "orders.csv", "a,b\n1,2\n");
        write_file(tmp.path(), "customers.parquet", "");
        write_file(tmp.path(), "readme.md", "ignore me");
        write_file(tmp.path(), ".DS_Store", "");

        let files = collect_supported_files(tmp.path()).unwrap();
        let stems: Vec<&str> = files.iter().map(|(s, _)| s.as_str()).collect();
        assert_eq!(stems, vec!["customers", "orders"]);
    }

    #[test]
    fn collect_supported_files_prefers_parquet_on_collision() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "orders.csv", "a,b\n1,2\n");
        let parquet = write_file(tmp.path(), "orders.parquet", "");

        let files = collect_supported_files(tmp.path()).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].0, "orders");
        assert_eq!(files[0].1, parquet);
    }

    #[test]
    fn collect_supported_files_is_case_insensitive_on_extension() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "orders.CSV", "a,b\n1,2\n");
        write_file(tmp.path(), "customers.PARQUET", "");

        let files = collect_supported_files(tmp.path()).unwrap();
        let stems: Vec<&str> = files.iter().map(|(s, _)| s.as_str()).collect();
        assert_eq!(stems, vec!["customers", "orders"]);
    }

    #[test]
    fn collect_supported_files_ignores_subdirectories() {
        let tmp = TempDir::new().unwrap();
        let subdir = tmp.path().join("nested");
        fs::create_dir(&subdir).unwrap();
        write_file(&subdir, "deep.csv", "a,b\n1,2\n");
        write_file(tmp.path(), "top.csv", "a,b\n1,2\n");

        let files = collect_supported_files(tmp.path()).unwrap();
        let stems: Vec<&str> = files.iter().map(|(s, _)| s.as_str()).collect();
        assert_eq!(stems, vec!["top"]);
    }

    #[test]
    fn collect_supported_files_returns_empty_for_dir_without_matches() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "notes.txt", "hi");
        write_file(tmp.path(), "data.json", "{}");

        let files = collect_supported_files(tmp.path()).unwrap();
        assert!(files.is_empty());
    }

    #[test]
    fn collect_supported_files_sorted_deterministically() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "zeta.csv", "");
        write_file(tmp.path(), "alpha.csv", "");
        write_file(tmp.path(), "mike.csv", "");

        let files = collect_supported_files(tmp.path()).unwrap();
        let stems: Vec<&str> = files.iter().map(|(s, _)| s.as_str()).collect();
        assert_eq!(stems, vec!["alpha", "mike", "zeta"]);
    }

    #[test]
    fn init_local_db_registers_schema_qualified_alias_for_unquoted_reference() {
        // The semantic-model compiler renders `table: stores.parquet` /
        // `table: orders.csv` as an UNQUOTED `FROM stores.parquet` — which
        // DuckDB parses as schema.table. Without a matching catalog object this
        // falls back to the file-replacement scan, whose resolution of the
        // qualified form via `file_search_path` is environment-sensitive and
        // silently returns nothing on some deployments. init_local_db must
        // register a `"<stem>"."<ext>"` view so the reference always resolves.
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "orders.csv", "id,amount\n1,10\n2,20\n3,30\n");
        // Materialize a small real parquet fixture via a throwaway connection.
        {
            let gen_conn = Connection::open_in_memory().unwrap();
            let parquet_path = tmp.path().join("stores.parquet");
            gen_conn
                .execute(
                    &format!(
                        "COPY (SELECT 1 AS id UNION ALL SELECT 2 UNION ALL SELECT 3 UNION ALL \
                     SELECT 4) TO '{}' (FORMAT PARQUET)",
                        parquet_path.display()
                    ),
                    [],
                )
                .unwrap();
        }

        let canonical = tmp.path().canonicalize().unwrap();
        let files = collect_supported_files(&canonical).unwrap();
        let conn = init_local_db(&canonical, &files).unwrap();

        // A FULLY-QUOTED "<stem>"."<ext>" reference resolves ONLY through the
        // registered catalog view (the file-replacement scan never fires for a
        // quoted identifier), so these assertions prove the alias exists.
        let csv_n: i64 = conn
            .query_row(r#"SELECT count(*) FROM "orders"."csv""#, [], |r| r.get(0))
            .unwrap();
        assert_eq!(csv_n, 3, "schema-qualified csv alias missing");
        let parquet_n: i64 = conn
            .query_row(r#"SELECT count(*) FROM "stores"."parquet""#, [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(parquet_n, 4, "schema-qualified parquet alias missing");

        // And the exact unquoted form the compiler emits resolves too.
        let unquoted: i64 = conn
            .query_row("SELECT count(*) FROM stores.parquet", [], |r| r.get(0))
            .unwrap();
        assert_eq!(unquoted, 4, "unquoted schema.table parquet did not resolve");
    }

    #[test]
    fn local_pool_serves_rows_appended_after_the_first_checkout() {
        // The simulation runner appends a period's rows and then immediately
        // asks the semantic layer to fit on them. `init_local_db` copies each
        // CSV into an in-memory table, so a pool that handed back the cached
        // handle regardless would feed the fitter a world frozen at period 0 —
        // and it would look like a converging estimate rather than a bug,
        // because a stale series is still a perfectly well-formed series.
        //
        // `PoolKey::local` captures every file's mtime for exactly this reason.
        // This asserts the eviction actually fires end to end, through the
        // public checkout path rather than against `init_local_db` directly.
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().to_str().unwrap().to_string();
        write_file(tmp.path(), "store_days.csv", "day,sales\n1,10\n");

        let conn = checkout_local_connection(&dir).unwrap();
        let before: i64 = conn
            .query_row("SELECT count(*) FROM store_days", [], |r| r.get(0))
            .unwrap();
        assert_eq!(before, 1);

        // The rewrite has to land on a mtime the pool can tell apart from the
        // first one, or the key legitimately matches and the test reports
        // "eviction is broken" on a platform where nothing is broken. macOS
        // HFS+ stamps whole seconds; APFS and ext4 stamp finer.
        //
        // Probed rather than slept through: a fixed 1.1s wait pays the HFS+
        // worst case on every filesystem, and this was the slowest test in the
        // crate for it. Rewriting until the stamp actually moves costs one
        // attempt where the resolution is sub-second, and the same tick where
        // it isn't. Bounded so a filesystem coarser than the deadline fails
        // loudly instead of spinning.
        let csv = tmp.path().join("store_days.csv");
        let mtime = |p: &std::path::Path| fs::metadata(p).unwrap().modified().unwrap();
        let before_mtime = mtime(&csv);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            write_file(
                tmp.path(),
                "store_days.csv",
                "day,sales\n1,10\n2,20\n3,30\n",
            );
            if mtime(&csv) != before_mtime {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the filesystem did not advance store_days.csv's mtime within 5s, so the \
                 pool key cannot distinguish the two writes and this test cannot assert \
                 eviction at all"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        let conn2 = checkout_local_connection(&dir).unwrap();
        let after: i64 = conn2
            .query_row("SELECT count(*) FROM store_days", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            after, 3,
            "appended rows were not visible — the pool served a stale in-memory copy"
        );

        // The sum, not just the count: a rebuild that re-read only the header
        // or truncated would still satisfy a row count in some orderings.
        let total: i64 = conn2
            .query_row("SELECT sum(sales) FROM store_days", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 60);
    }

    /// A per-run dataset directory is a `PoolTarget` that *dies*, and the pool
    /// has no capacity bound, no TTL and no `Drop` hook — its only eviction is
    /// same-key replacement. So a caller that materialises a fresh `TempDir`
    /// per run (the simulation runner does) permanently pins one in-memory
    /// DuckDB, plus every table materialised into it, for a directory that no
    /// longer exists on disk. Nothing will ever check out that target again.
    ///
    /// Counted against a baseline rather than zero because the pool is a
    /// process-global singleton: under `cargo test` this test shares it with
    /// every other test in the binary.
    #[test]
    fn per_run_dataset_dirs_do_not_accumulate_pooled_handles() {
        let slots_before = pool().slot_count();
        let init_locks_before = pool().init_lock_count();

        let mut targets = Vec::new();
        for _ in 0..3 {
            let tmp = TempDir::new().unwrap();
            write_file(tmp.path(), "store_days.csv", "day,sales\n1,10\n");
            let canonical = tmp.path().canonicalize().unwrap();
            let dir = tmp.path().to_str().unwrap().to_string();

            let conn = checkout_local_connection(&dir).unwrap();
            let n: i64 = conn
                .query_row("SELECT count(*) FROM store_days", [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 1);
            drop(conn);

            // The run ends. Release before `TempDir`'s drop takes the
            // directory with it: after that the path no longer canonicalizes
            // and the release cannot find the handle it meant to free. This is
            // the ordering `WorldDir`'s `Drop` relies on.
            release_local_connection(&canonical);
            targets.push(PoolTarget::Local { dir: canonical });
            drop(tmp);
        }

        let leaked: Vec<_> = targets
            .iter()
            .filter(|target| pool().holds_slot(target))
            .collect();
        assert!(
            leaked.is_empty(),
            "{} of {} dead dataset directories still pin a live in-memory DuckDB: {leaked:#?}",
            leaked.len(),
            targets.len()
        );
        assert_eq!(
            pool().slot_count(),
            slots_before,
            "the slot map grew across runs — the per-target bound in this module's doc \
             only holds while targets are reused"
        );
        assert_eq!(
            pool().init_lock_count(),
            init_locks_before,
            "the init-lock map grew across runs — its 'bounded by the number of distinct \
             targets' justification does not survive per-run-unique targets"
        );
    }

    /// The two maps are released together. `invalidate` drops only the slot —
    /// correct for a MotherDuck session that may be reopened, wrong for a
    /// target that is gone, because the `init_locks` entry it leaves behind
    /// accumulates on exactly the same per-run schedule the slot did.
    #[test]
    fn releasing_a_local_target_drops_its_slot_and_its_init_lock() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "store_days.csv", "day,sales\n1,10\n");
        let canonical = tmp.path().canonicalize().unwrap();
        let target = PoolTarget::Local {
            dir: canonical.clone(),
        };

        let conn = checkout_local_connection(tmp.path().to_str().unwrap()).unwrap();
        assert!(
            pool().holds_slot(&target),
            "checkout should have populated the slot this test is about to release"
        );
        assert!(pool().holds_init_lock(&target));
        drop(conn);

        // Deliberately released via the *uncanonicalized* path the caller
        // actually holds: on macOS `TempDir` hands back a `/var/...` path while
        // the pool is keyed on `/private/var/...`, so a release that skipped
        // canonicalization would silently match nothing here.
        release_local_connection(tmp.path());

        assert!(
            !pool().holds_slot(&target),
            "release must drop the pooled in-memory database, not merely unlink it"
        );
        assert!(
            !pool().holds_init_lock(&target),
            "release must also drop the init-lock entry — it is per-target too, and a \
             per-run target makes 'bounded by the number of distinct targets' unbounded"
        );
    }

    /// `release` is not `invalidate`: a target that can be checked out again
    /// must keep its init lock, because a caller may be blocked on it right now
    /// and dropping the map's copy would let the next caller mint a second lock
    /// and init concurrently.
    #[test]
    fn releasing_a_contended_init_lock_leaves_the_lock_entry_in_place() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "store_days.csv", "day,sales\n1,10\n");
        let canonical = tmp.path().canonicalize().unwrap();
        let target = PoolTarget::Local {
            dir: canonical.clone(),
        };

        let conn = checkout_local_connection(tmp.path().to_str().unwrap()).unwrap();
        drop(conn);

        // Stand in for a caller sitting between "cloned the Arc out of the map"
        // and "acquired it" — the exact window the count check exists for.
        let waiter = pool()
            .clone_init_lock(&target)
            .expect("init lock must exist");
        release_local_connection(tmp.path());
        assert!(
            !pool().holds_slot(&target),
            "the slot is released regardless — a waiter is about to rebuild it"
        );
        assert!(
            pool().holds_init_lock(&target),
            "a waiter still holds the lock, so removing the map's copy would let the next \
             caller mint an unrelated one and init concurrently"
        );
        drop(waiter);

        // Once the waiter is gone the entry is collectable again.
        release_local_connection(tmp.path());
        assert!(!pool().holds_init_lock(&target));
    }

    use crate::config::model::{DuckDbS3Mirror, DuckDbS3Table};

    #[test]
    fn s3_mirror_local_mode_builds_httpfs_secret_and_views() {
        let mirror = DuckDbS3Mirror {
            bucket: "my-bucket".into(),
            region: Some("us-west-2".into()),
            endpoint_url: None,
            tables: vec![
                DuckDbS3Table {
                    table: "orders".into(),
                    key: "ws/duckdb/orders.parquet".into(),
                    format: "parquet".into(),
                },
                DuckDbS3Table {
                    table: "events".into(),
                    key: "ws/duckdb/events.csv".into(),
                    format: "csv".into(),
                },
            ],
            attach_key: None,
        };
        let sql = build_s3_mirror_sql(&mirror);
        assert!(sql.contains(&"INSTALL httpfs".to_string()));
        assert!(sql.contains(&"LOAD httpfs".to_string()));
        assert!(
            sql.iter()
                .any(|s| s.contains("CREATE OR REPLACE SECRET duckdb_mirror_s3")
                    && s.contains("PROVIDER credential_chain")
                    && s.contains("REGION 'us-west-2'"))
        );
        assert!(sql.iter().any(|s| s.contains("CREATE OR REPLACE VIEW")
            && s.contains("orders")
            && s.contains("read_parquet('s3://my-bucket/ws/duckdb/orders.parquet')")));
        assert!(sql.iter().any(|s| s.contains("CREATE OR REPLACE VIEW")
            && s.contains("events")
            && s.contains("read_csv_auto('s3://my-bucket/ws/duckdb/events.csv')")));
        // Filename-qualified views so the semantic model's `FROM orders.parquet`
        // / `FROM events.csv` (which DuckDB parses as schema.table) resolve to the
        // mirrored data — local mode gets this from file_search_path; the fleet
        // can't, so register `"<stem>"."<ext>"` explicitly.
        assert!(
            sql.iter()
                .any(|s| s == "CREATE SCHEMA IF NOT EXISTS \"orders\""),
            "missing schema for orders: {sql:?}"
        );
        assert!(
            sql.iter().any(
                |s| s.contains("CREATE OR REPLACE VIEW \"orders\".\"parquet\"")
                    && s.contains("read_parquet('s3://my-bucket/ws/duckdb/orders.parquet')")
            ),
            "missing filename-qualified view for orders.parquet: {sql:?}"
        );
        assert!(
            sql.iter()
                .any(|s| s == "CREATE SCHEMA IF NOT EXISTS \"events\""),
            "missing schema for events: {sql:?}"
        );
        assert!(
            sql.iter()
                .any(|s| s.contains("CREATE OR REPLACE VIEW \"events\".\"csv\"")
                    && s.contains("read_csv_auto('s3://my-bucket/ws/duckdb/events.csv')")),
            "missing filename-qualified view for events.csv: {sql:?}"
        );
        // Local mode never attaches a remote database.
        assert!(!sql.iter().any(|s| s.contains("ATTACH")));
    }

    #[test]
    fn s3_mirror_file_mode_attaches_read_only() {
        let mirror = DuckDbS3Mirror {
            bucket: "b".into(),
            region: None,
            endpoint_url: None,
            tables: vec![],
            attach_key: Some("ws/duckdb/data.duckdb".into()),
        };
        let sql = build_s3_mirror_sql(&mirror);
        assert!(
            sql.iter()
                .any(|s| s == "ATTACH 's3://b/ws/duckdb/data.duckdb' AS mirror (READ_ONLY)")
        );
        assert!(sql.contains(&"USE mirror".to_string()));
    }

    #[test]
    fn s3_mirror_custom_endpoint_uses_path_style() {
        let mirror = DuckDbS3Mirror {
            bucket: "b".into(),
            region: Some("us-east-1".into()),
            endpoint_url: Some("http://localhost:9000".into()),
            tables: vec![],
            attach_key: None,
        };
        let sql = build_s3_mirror_sql(&mirror);
        // The ENDPOINT must be host[:port] with NO scheme — DuckDB prepends
        // http(s):// from USE_SSL. Passing the scheme yields `http://http://…`,
        // which fails to resolve and silently drops the connector.
        assert!(
            sql.iter().any(|s| s.contains("ENDPOINT 'localhost:9000'")
                && s.contains("URL_STYLE 'path'")
                && s.contains("USE_SSL false")),
            "expected scheme-stripped endpoint, got: {sql:?}"
        );
        assert!(
            !sql.iter().any(|s| s.contains("ENDPOINT 'http://")),
            "endpoint must not carry a scheme: {sql:?}"
        );
    }

    #[test]
    fn s3_mirror_https_endpoint_strips_scheme_and_enables_ssl() {
        let mirror = DuckDbS3Mirror {
            bucket: "b".into(),
            region: Some("us-east-1".into()),
            endpoint_url: Some("https://minio.example.com/".into()),
            tables: vec![],
            attach_key: None,
        };
        let sql = build_s3_mirror_sql(&mirror);
        assert!(
            sql.iter()
                .any(|s| s.contains("ENDPOINT 'minio.example.com'") && s.contains("USE_SSL true")),
            "https endpoint should strip scheme + trailing slash and enable SSL: {sql:?}"
        );
    }

    /// A database file with one row, written through a connection that is
    /// closed before the test opens it again.
    fn ledger_file(dir: &Path) -> String {
        let path = dir.join("ledger.duckdb").display().to_string();
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE entries AS SELECT 1 AS id")
            .unwrap();
        drop(conn);
        path
    }

    /// The cloud roles' open: the file answers reads and refuses writes at
    /// DuckDB, so nothing lands in a file no other node reads.
    #[test]
    fn a_file_database_opened_read_only_reads_and_refuses_writes() {
        let tmp = TempDir::new().unwrap();
        let path = ledger_file(tmp.path());
        let conn = open_file_database(&path, true).unwrap();
        let n: i64 = conn
            .query_row("SELECT count(*) FROM entries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
        let err = conn
            .execute("INSERT INTO entries VALUES (2)", [])
            .expect_err("a read-only database refuses a write");
        assert!(
            err.to_string().to_lowercase().contains("read-only"),
            "{err}"
        );
    }

    /// The single-process role (`oxy start`) still writes, as it always has.
    #[test]
    fn a_file_database_opened_read_write_still_writes() {
        let tmp = TempDir::new().unwrap();
        let path = ledger_file(tmp.path());
        let conn = open_file_database(&path, false).unwrap();
        conn.execute("INSERT INTO entries VALUES (2)", []).unwrap();
        let n: i64 = conn
            .query_row("SELECT count(*) FROM entries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);
    }
}
