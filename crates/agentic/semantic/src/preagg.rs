//! Thin wrappers over airlayer pre-aggregation functions and DuckDB execution.

use std::path::Path;
use std::sync::{Mutex, OnceLock, RwLock};

use agentic_core::result::{CellValue, QueryResult, QueryRow};
use serde_json::Value;

use crate::compile::PreaggSource;
use crate::error::SemanticError;
use crate::rollup_value;

/// Process-wide primary connection used to seed cheap per-call clones for
/// preagg reads. Opening a fresh `Connection::open_in_memory()` per request
/// dominates the cache-hit query path (~30–40ms cold); `try_clone()` on a
/// long-lived primary brings that down to sub-millisecond.
///
/// The primary holds no schema and no preloaded tables — every preagg SQL
/// has its own `read_parquet('...')` call, so clones are independent.
static PRIMARY_DUCKDB: OnceLock<Mutex<duckdb::Connection>> = OnceLock::new();

/// Return a fresh DuckDB connection cloned from the long-lived primary.
/// Use this for any per-request DuckDB work tied to preagg reads so we
/// don't pay the in-memory-DB open cost on every query.
pub fn pooled_duckdb_connection() -> Result<duckdb::Connection, SemanticError> {
    let primary = PRIMARY_DUCKDB.get_or_init(|| {
        Mutex::new(
            duckdb::Connection::open_in_memory()
                .expect("open primary in-memory DuckDB connection for preagg pool"),
        )
    });
    let guard = primary
        .lock()
        .expect("preagg DuckDB primary mutex poisoned");
    guard
        .try_clone()
        .map_err(|e| SemanticError::Runtime(format!("DuckDB try_clone failed: {e}")))
}

/// Scopes this feature's S3 secret.
///
/// Note the scope is the process, not the connection: every connection here is
/// a `try_clone` of one long-lived primary, and DuckDB stores secrets per
/// database instance — so each `CREATE OR REPLACE SECRET` mutates state shared
/// by every concurrent rollup read. Harmless while one process reads one
/// bucket with one credential chain, which is the only shape today; a distinct
/// name is what keeps a second S3-using feature on this pool from silently
/// replacing it.
const PREAGG_S3_SECRET: &str = "oxy_preagg_s3";

/// Ready a connection for the source it is about to read.
///
/// A local file needs nothing — the whole point of the fast path. A blob
/// source needs `httpfs` and an S3 secret, using the recipe
/// `connector::duckdb`'s S3 mirror already relies on
/// (`oxy_shared::duckdb_s3`) so the two cannot drift.
///
/// `INSTALL httpfs` reaches the network the first time a machine runs it. A
/// failure here is an error rather than a silent skip, and every caller of a
/// rollup read now catches that error and re-runs the warehouse SQL the
/// `CompiledQuery::Preaggregation` variant carries — see
/// `oxy_api_semantic::semantic::execute_semantic_query`, the analytics executing handler,
/// `metric_tree_runner`, and the builder's `semantic_query` tool. Returning
/// zero rows here instead would be indistinguishable from a rollup that
/// genuinely has none, which is why this reports rather than degrades.
fn prepare_connection(
    conn: &duckdb::Connection,
    source: &PreaggSource,
) -> Result<(), SemanticError> {
    let PreaggSource::Blob { config, .. } = source else {
        return Ok(());
    };
    let exec = |stmt: &str| {
        conn.execute_batch(stmt).map_err(|e| {
            SemanticError::Runtime(format!(
                "DuckDB could not be prepared to read the pre-aggregation blob store ({stmt}): {e}"
            ))
        })
    };

    // Three lifetimes, not one. `INSTALL`/`LOAD` and the secret live on the
    // DuckDB INSTANCE that every pooled connection clones, so they are
    // process-wide; the `SET http_timeout` / `http_retries` bounds are session
    // settings and have to be re-applied on each connection. Getting that last
    // part wrong is what keeps an unreachable endpoint from running at DuckDB's
    // 30s-times-4 default per range request.
    let (extensions, rest): (Vec<String>, Vec<String>) = oxy_shared::duckdb_s3::s3_setup_sql(
        PREAGG_S3_SECRET,
        config.region.as_deref(),
        config.endpoint_url.as_deref(),
        // Every caller of a rollup read re-runs the warehouse SQL when this
        // path errors, so a tripped bound costs a slower answer rather than a
        // failed one — which is what buys the tight ceiling. See
        // `S3ReadBounds`.
        oxy_shared::duckdb_s3::S3ReadBounds::WITH_FALLBACK,
    )
    .into_iter()
    .partition(|stmt| stmt.starts_with("INSTALL") || stmt.starts_with("LOAD"));

    if HTTPFS_READY.get().is_none() {
        // `INSTALL httpfs` reaches the network on a pod's first ever call.
        for stmt in &extensions {
            exec(stmt)?;
        }
        // Armed HERE, not after the whole setup. The statements below can fail
        // — a `CREATE SECRET` whose credential chain resolves to nothing does —
        // and the `?` on that failure used to return before this line, so the
        // network-touching `INSTALL` was re-run on every read for as long as
        // the condition lasted.
        let _ = HTTPFS_READY.set(());
    }

    // `CREATE OR REPLACE SECRET` is not the cheap local statement it looks
    // like: `PROVIDER credential_chain` is validated EAGERLY (the same fact
    // `da9075c6` established for the S3 mirror's probe), so re-issuing it per
    // read resolves the whole provider chain inside the read path on every
    // query. It is instance-scoped like the extensions, so it is issued once
    // per distinct config — and keyed by that config rather than a bare
    // `OnceLock`, so a workspace that changes region or endpoint re-issues on
    // its next read instead of being stuck with the old credential.
    //
    // Caching it is only safe because the statement carries `REFRESH true`:
    // eager resolution means the secret holds the session token the chain
    // produced at creation, and on IRSA / IMDS that expires in about an hour.
    // The per-read re-issue this replaced was hiding that. See `s3_setup_sql`.
    let fingerprint = format!(
        "{}|{}",
        config.region.as_deref().unwrap_or(""),
        config.endpoint_url.as_deref().unwrap_or("")
    );
    // Double-checked: the steady state — the secret is already this config's,
    // and only the two `SET`s run — takes the SHARED lock, so concurrent reads
    // do not serialize on a process-wide mutex through DuckDB execution.
    if S3_SECRET_APPLIED
        .read()
        .expect("preagg S3 secret fingerprint lock poisoned")
        .as_deref()
        == Some(fingerprint.as_str())
    {
        for stmt in &rest {
            if stmt.starts_with("CREATE OR REPLACE SECRET") {
                continue;
            }
            exec(stmt)?;
        }
        return Ok(());
    }

    // Issuing path only. The write guard is held ACROSS the statement rather
    // than taken after it: a read-then-write would let two threads with
    // different configs finish in the opposite order and leave the recorded
    // fingerprint naming a secret the instance does not have — unreachable with
    // one config per process, but the fingerprint now PERSISTS the loser, where
    // the old per-read `CREATE OR REPLACE` merely raced and recovered. Re-check
    // under the write lock, since another thread may have issued it while this
    // one waited.
    let mut applied = S3_SECRET_APPLIED
        .write()
        .expect("preagg S3 secret fingerprint lock poisoned");
    let secret_applied = applied.as_deref() == Some(fingerprint.as_str());

    for stmt in &rest {
        if secret_applied && stmt.starts_with("CREATE OR REPLACE SECRET") {
            continue;
        }
        exec(stmt)?;
    }
    if !secret_applied {
        *applied = Some(fingerprint);
    }
    Ok(())
}

/// Set once `httpfs` has been installed and loaded on the shared DuckDB
/// instance, so the network-touching half of the setup runs once per process
/// rather than once per query.
static HTTPFS_READY: OnceLock<()> = OnceLock::new();

/// `region|endpoint` the process-wide S3 secret was last created from, or
/// `None` before the first blob read — see `prepare_connection` for why the
/// secret is not re-issued per read, and why this is a fingerprint rather than
/// a `OnceLock<()>`.
static S3_SECRET_APPLIED: RwLock<Option<String>> = RwLock::new(None);

/// A pooled connection already prepared for `source` — the only shape callers
/// outside this module should use, so nobody can hold a connection that hasn't
/// been taught to read `s3://`.
pub fn prepared_duckdb_connection(
    source: &PreaggSource,
) -> Result<duckdb::Connection, SemanticError> {
    check_local_file(source)?;
    let conn = pooled_duckdb_connection()?;
    prepare_connection(&conn, source)?;
    Ok(conn)
}

/// Guard against a local file that vanished between compile and execute, so
/// the failure names the cache rather than surfacing as a cryptic DuckDB
/// error. A blob source has no local file by design.
fn check_local_file(source: &PreaggSource) -> Result<(), SemanticError> {
    if let Some(path) = source.local_path()
        && !path.is_file()
    {
        return Err(SemanticError::Runtime(format!(
            "Parquet cache file not found: {}",
            path.display()
        )));
    }
    Ok(())
}

/// Execute a re-aggregation SQL query against an in-memory DuckDB instance.
///
/// `preagg_sql` is produced by `oxy_airlayer_compat::preagg::generate_reagg_sql` and reads
/// its Parquet via `read_parquet('...')` — a local path or an `s3://` URI,
/// depending on `source`. The blob case reads the object in place over
/// `httpfs`; DuckDB pushes projections and filters down, so this is a scan of
/// what the query needs rather than a download of the whole rollup.
pub fn execute_preagg_sql(preagg_sql: &str, source: &PreaggSource) -> Result<Value, SemanticError> {
    let conn = prepared_duckdb_connection(source)?;

    let mut stmt = conn
        .prepare(preagg_sql)
        .map_err(|e| SemanticError::Runtime(format!("DuckDB prepare failed: {e}")))?;

    let mut duckdb_rows = stmt
        .query([])
        .map_err(|e| SemanticError::Runtime(format!("DuckDB query failed: {e}")))?;

    let columns: Vec<String> = duckdb_rows
        .as_ref()
        .map(|r| r.column_names().iter().map(|s| s.to_string()).collect())
        .unwrap_or_default();

    let mut json_rows: Vec<Value> = Vec::new();
    loop {
        match duckdb_rows.next() {
            Ok(Some(row)) => {
                let mut obj = serde_json::Map::new();
                for (i, col) in columns.iter().enumerate() {
                    obj.insert(col.clone(), rollup_value::to_json(row.get_ref_unwrap(i)));
                }
                json_rows.push(Value::Object(obj));
            }
            Ok(None) => break,
            Err(e) => return Err(SemanticError::Runtime(format!("DuckDB row error: {e}"))),
        }
    }

    let row_count = json_rows.len();
    Ok(serde_json::json!({
        "columns": columns,
        "rows": json_rows,
        "row_count": row_count,
        "truncated": false,
    }))
}

/// Typed variant of [`execute_preagg_sql`] that returns `QueryRow`s directly —
/// no intermediate `serde_json::Value` per cell — and applies a sample limit
/// at the SQL level so DuckDB stops scanning early.
///
/// Returns `(columns, sample_rows, total_row_count)`. `sample_rows.len()` is
/// `min(total_row_count, sample_limit)`. Callers derive `truncated` by
/// comparing the two.
///
/// **Sample first, count only if the sample filled up.** The caller wants
/// `sample_limit` rows, so that is the only query that always runs; DuckDB
/// stops scanning once the `LIMIT` is satisfied. A short result *is* its own
/// count, so the common case — a rollup narrower than the limit — costs one
/// pass and never materialises a row nobody asked for. Only a result that hits
/// the limit pays a `COUNT(*)`, which on a local source reads Parquet metadata
/// and on a blob source is a second trip.
///
/// The tempting shape here is a `CREATE TEMP TABLE … AS (preagg_sql)` so the
/// count and the sample share one remote scan. Don't: the primary connection
/// is `open_in_memory()` with no `temp_directory`, so a wide rollup has nowhere
/// to spill, and hitting `memory_limit` now falls back to the warehouse — which
/// means a big rollup would pay a full remote scan *and* a full warehouse query
/// and silently drop off the fast path for good.
pub fn execute_preagg_sql_typed(
    preagg_sql: &str,
    source: &PreaggSource,
    sample_limit: u64,
) -> Result<(Vec<String>, Vec<QueryRow>, u64), SemanticError> {
    let conn = prepared_duckdb_connection(source)?;

    let limited_sql = format!("SELECT * FROM ({preagg_sql}) LIMIT {sample_limit}");
    let mut stmt = conn
        .prepare(&limited_sql)
        .map_err(|e| SemanticError::Runtime(format!("DuckDB prepare failed: {e}")))?;

    let mut duckdb_rows = stmt
        .query([])
        .map_err(|e| SemanticError::Runtime(format!("DuckDB query failed: {e}")))?;

    let columns: Vec<String> = duckdb_rows
        .as_ref()
        .map(|r| r.column_names().iter().map(|s| s.to_string()).collect())
        .unwrap_or_default();

    let mut rows: Vec<QueryRow> = Vec::new();
    loop {
        match duckdb_rows.next() {
            Ok(Some(row)) => {
                let cells: Vec<CellValue> = (0..columns.len())
                    .map(|i| rollup_value::to_cell(row.get_ref_unwrap(i)))
                    .collect();
                rows.push(QueryRow(cells));
            }
            Ok(None) => break,
            Err(e) => return Err(SemanticError::Runtime(format!("DuckDB row error: {e}"))),
        }
    }

    let sampled = rows.len() as u64;
    let total_row_count = if sampled < sample_limit {
        // Below the limit, the sample is the population — nothing left to count.
        sampled
    } else {
        let count_sql = format!("SELECT COUNT(*) FROM ({preagg_sql})");
        conn.query_row(&count_sql, [], |row| row.get::<_, i64>(0))
            .map_err(|e| SemanticError::Runtime(format!("DuckDB count failed: {e}")))?
            as u64
    };

    Ok((columns, rows, total_row_count))
}

/// Atomically rename `temp_path` → `final_path`.
///
/// On POSIX systems `rename(2)` is atomic: readers of the old file are unaffected;
/// new readers get the new file.
pub fn hot_swap_parquet(temp_path: &Path, final_path: &Path) -> Result<(), SemanticError> {
    if let Some(parent) = final_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| SemanticError::Runtime(format!("create_dir_all failed: {e}")))?;
    }
    std::fs::rename(temp_path, final_path)
        .map_err(|e| SemanticError::Runtime(format!("hot-swap rename failed: {e}")))?;
    Ok(())
}

/// Write a warehouse result to a Parquet file at `path`: the local half of
/// pulling a rollup (`oxy_preagg::warehouse::pull_rollup` reads the rows).
///
/// Returns `true` if the file was written, `false` if the result had no rows.
pub fn write_result_to_parquet(result: &QueryResult, path: &Path) -> Result<bool, SemanticError> {
    if result.rows.is_empty() {
        tracing::warn!(
            "pull_rollup: no rows to write to {}, skipping",
            path.display()
        );
        return Ok(false);
    }

    let conn = duckdb::Connection::open_in_memory().map_err(|e| {
        SemanticError::Runtime(format!("DuckDB open failed for parquet write: {e}"))
    })?;

    let col_names: Vec<String> = result
        .columns
        .iter()
        .map(|c| format!("\"{}\"", c.replace('"', "\"\"")))
        .collect();

    let mut value_rows: Vec<String> = Vec::new();
    for row in &result.rows {
        let cells: Vec<String> = row
            .0
            .iter()
            .map(|cell| match cell {
                CellValue::Text(s) => format!("'{}'", s.replace('\'', "''")),
                // NaN / Inf are not valid SQL literals — map them to NULL.
                CellValue::Number(n) if n.is_finite() => n.to_string(),
                CellValue::Number(_) => "NULL".into(),
                CellValue::Null => "NULL".into(),
            })
            .collect();
        value_rows.push(format!("({})", cells.join(", ")));
    }

    let path_str = path
        .to_str()
        .ok_or_else(|| SemanticError::Runtime("non-UTF8 parquet path".into()))?;

    let sql = format!(
        "COPY (SELECT * FROM (VALUES {}) AS t({})) TO '{}' (FORMAT PARQUET)",
        value_rows.join(", "),
        col_names.join(", "),
        path_str.replace('\'', "''"),
    );

    conn.execute_batch(&sql)
        .map_err(|e| SemanticError::Runtime(format!("parquet write failed: {e}")))?;
    Ok(true)
}

/// Load the local manifest from `<cache_dir>/manifest.json`.
///
/// Returns `None` if the file doesn't exist or can't be parsed.
pub fn load_local_manifest(cache_dir: &Path) -> Option<oxy_airlayer_compat::preagg::LocalManifest> {
    let path = cache_dir.join("manifest.json");
    let content = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Persist a local manifest to `<cache_dir>/manifest.json` atomically.
///
/// Writes to a `.tmp` file first, then renames it into place. On POSIX systems
/// `rename(2)` is atomic: readers of the old file are unaffected; new readers
/// get the new file. This prevents partial writes if the process is killed mid-write.
pub fn save_local_manifest(
    cache_dir: &Path,
    manifest: &oxy_airlayer_compat::preagg::LocalManifest,
) -> Result<(), SemanticError> {
    std::fs::create_dir_all(cache_dir)
        .map_err(|e| SemanticError::Runtime(format!("create cache dir failed: {e}")))?;

    // Pid-scoped, not a fixed `manifest.json.tmp`. The publish lock that
    // serializes writers is process-local, so two processes over one state dir
    // — an `oxy serve` box also running `oxy worker`, or two dev instances —
    // would otherwise share a staging file and rename each other's half-written
    // bytes into place, under a lock-free status poll that reads the result.
    let tmp_path = cache_dir.join(format!("manifest.json.{}.tmp", std::process::id()));
    let final_path = cache_dir.join("manifest.json");

    let json = serde_json::to_string_pretty(manifest)
        .map_err(|e| SemanticError::Runtime(format!("manifest serialize failed: {e}")))?;

    std::fs::write(&tmp_path, &json)
        .map_err(|e| SemanticError::Runtime(format!("manifest tmp write failed: {e}")))?;

    std::fs::rename(&tmp_path, &final_path)
        .map_err(|e| SemanticError::Runtime(format!("manifest rename failed: {e}")))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn save_and_load_manifest_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = oxy_airlayer_compat::preagg::LocalManifest {
            pulled_at: "2026-01-01T00:00:00Z".into(),
            source_database: "my_db".into(),
            rollups: vec![],
        };
        save_local_manifest(dir.path(), &manifest).unwrap();

        let loaded = load_local_manifest(dir.path());
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().source_database, "my_db");
    }

    #[test]
    fn save_manifest_no_tmp_remains() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = oxy_airlayer_compat::preagg::LocalManifest {
            pulled_at: "2026-01-01T00:00:00Z".into(),
            source_database: "test".into(),
            rollups: vec![],
        };
        save_local_manifest(dir.path(), &manifest).unwrap();

        // No staging file should remain after a successful save — matched by
        // suffix, since the name is pid-scoped.
        let strays: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(strays.is_empty(), "staging file left behind: {strays:?}");
        assert!(dir.path().join("manifest.json").exists());
    }

    /// The `SET http_timeout` / `SET http_retries` bounds are only worth
    /// anything if the linked DuckDB accepts them. A rejected statement is not
    /// a soft failure here: `prepare_connection` turns it into an error, so
    /// EVERY blob read would fall back to the warehouse and the rollup tier
    /// would quietly stop existing. The setting names are httpfs's, not the
    /// core engine's, so a DuckDB bump is exactly when this could break.
    ///
    /// `INSTALL httpfs` needs the network on a machine that has never run it;
    /// skip rather than fail there, since the thing under test is whether the
    /// SETs are accepted, not whether CI has egress.
    ///
    /// `CREATE SECRET ... PROVIDER credential_chain` is validated eagerly:
    /// DuckDB walks the chain at create time and rejects the statement when it
    /// finds no credentials. That is a property of the machine, not of the SQL
    /// — CI runners have no AWS identity — so a validation failure is skipped
    /// the same way, while any other rejection (a parser or binder error, i.e.
    /// the syntax drifting under a DuckDB bump) still fails.
    #[test]
    fn the_linked_duckdb_accepts_the_http_bounds() {
        let conn = pooled_duckdb_connection().expect("pooled connection");
        if conn.execute_batch("INSTALL httpfs; LOAD httpfs").is_err() {
            eprintln!("skipping: httpfs unavailable (no network / no cached extension)");
            return;
        }
        for bounds in [
            oxy_shared::duckdb_s3::S3ReadBounds::WITH_FALLBACK,
            oxy_shared::duckdb_s3::S3ReadBounds::NO_FALLBACK,
        ] {
            for stmt in oxy_shared::duckdb_s3::s3_setup_sql("preagg_probe_s3", None, None, bounds) {
                // BOTH execution modes, because the recipe's two callers do not
                // agree on one: this module runs `execute_batch`, and the S3
                // mirror in `connector::duckdb` runs `conn.execute(stmt, [])`,
                // which is stricter — it takes a single statement and refuses
                // one that returns rows. Probing only the looser API would let
                // a statement that the mirror cannot run pass here.
                for result in [
                    conn.execute_batch(&stmt),
                    conn.execute(&stmt, []).map(|_| ()),
                ] {
                    if let Err(e) = result {
                        let msg = e.to_string();
                        assert!(
                            msg.contains("Secret Validation Failure"),
                            "DuckDB rejected {stmt:?}: {msg}"
                        );
                        eprintln!("skipping secret check: no credentials on this machine ({msg})");
                    }
                }
            }
        }
    }

    #[test]
    fn test_execute_preagg_sql_parquet_not_found() {
        let result = execute_preagg_sql(
            "SELECT * FROM read_parquet('/does/not/exist.parquet')",
            &PreaggSource::Local(PathBuf::from("/does/not/exist.parquet")),
        );
        assert!(result.is_err(), "missing parquet should error");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("not found"),
            "error should mention 'not found': {err}"
        );
    }
}
