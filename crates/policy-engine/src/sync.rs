//! Copy `policy_overrides` from the org's OLTP database to a local Parquet file
//! through DuckDB's `postgres` extension, connected as the app's writer:
//!
//! ```sql
//! COPY (SELECT * FROM neon."app_<slug>".policy_overrides ORDER BY policy_id)
//!   TO 'local.parquet' (FORMAT parquet)
//! ```
//!
//! The schema is named outright: DuckDB addresses an attached table as
//! `database.schema.table` from its own catalog, so the writer DSN's pinned
//! `search_path` does not pick it.
//!
//! TLS here is libpq's, inside the DuckDB extension, where Neon's
//! `sslmode=require` would encrypt without checking whose certificate it is:
//! an active MITM would get the writer's password. So every DSN that is not
//! `sslmode=disable` (a local cluster) is attached as `verify-full` against the
//! system CA bundle ([`ca_bundle`]), the same check the policy store's own
//! connection ([`crate::PolicySource::connect`]) makes. No bundle, no sync.
//!
//! The database is attached `READ_ONLY`, so the copy is a plain snapshot read
//! on the Postgres side (`AccessShareLock` only) and never waits on, or holds
//! up, a writer. `tests/integration/sync_parquet.rs` checks that against a
//! transaction held open across concurrent writes.
//!
//! Host-side only: the sync is a Rust function and a CLI subcommand, never an
//! op, so a policy script cannot trigger it.

use std::path::{Path, PathBuf};

use duckdb::Connection;
use url::Url;

use crate::store::{OltpWriter, POLICY_OVERRIDES_TABLE};

/// What the attached database is called inside DuckDB.
pub const ATTACHED_AS: &str = "neon";

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("not a postgres:// URL: {0}")]
    BadUrl(String),
    /// TLS was asked for but nothing to verify the server against was found.
    #[error(
        "no CA bundle to verify the database's certificate (set SSL_CERT_FILE, or add \
         sslrootcert= to the DSN); refusing to send the writer's password unverified"
    )]
    NoCaBundle,
    /// `step` names the statement that failed; DuckDB's own text often does
    /// not ("Unknown exception in ExecutorTask::Execute").
    #[error("duckdb, {step}: {source}")]
    DuckDb {
        step: &'static str,
        source: duckdb::Error,
    },
    #[error("{}: {source}", .path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// A Postgres URL turned into what DuckDB's `ATTACH … (TYPE postgres)` takes:
/// a libpq `key=value` string without the password, plus the password on its
/// own for an unnamed `TYPE postgres` secret (as `oxy`'s DuckLake attach does).
///
/// Not the URL itself: with a password secret present, attaching a
/// `postgres://` URI failed with DuckDB's "Unknown exception in
/// ExecutorTask::Execute" while the same target as `key=value` worked —
/// likely because the extension appends the secret's `password=` to the attach
/// string, which libpq cannot parse after a URI. And not the password inline:
/// DuckDB repeats the attach string in its errors.
pub struct PgSource {
    conninfo: String,
    password: Option<String>,
    schema: String,
}

impl PgSource {
    /// The writer's DSN, attached, and its schema as the one to copy from.
    pub fn from_writer(writer: &OltpWriter) -> Result<Self, SyncError> {
        Ok(Self {
            schema: writer.schema().to_string(),
            ..Self::from_url(writer.dsn())?
        })
    }

    /// The URL half: `schema` is left at `public` for [`Self::from_writer`]
    /// to replace. Private, so every sync goes through a writer.
    fn from_url(raw: &str) -> Result<Self, SyncError> {
        Self::from_url_with(raw, ca_bundle)
    }

    fn from_url_with(raw: &str, ca_bundle: fn() -> Option<PathBuf>) -> Result<Self, SyncError> {
        let url = Url::parse(raw).map_err(|e| SyncError::BadUrl(e.to_string()))?;
        if !matches!(url.scheme(), "postgres" | "postgresql") {
            return Err(SyncError::BadUrl(format!("scheme {}", url.scheme())));
        }
        let decode = |what: &str, s: &str| {
            urlencoding::decode(s)
                .map(|s| s.into_owned())
                .map_err(|e| SyncError::BadUrl(format!("{what} is not valid UTF-8: {e}")))
        };
        // libpq wants an IPv6 literal bare; the URL keeps it in brackets.
        let host = match url.host() {
            Some(url::Host::Ipv6(addr)) => addr.to_string(),
            Some(host) => host.to_string(),
            None => return Err(SyncError::BadUrl("no host".into())),
        };
        let mut parts = vec![format!("host={}", libpq_value(&host))];
        if let Some(port) = url.port() {
            parts.push(format!("port={port}"));
        }
        if !url.username().is_empty() {
            parts.push(format!(
                "user={}",
                libpq_value(&decode("user", url.username())?)
            ));
        }
        let dbname = url.path().trim_start_matches('/');
        if !dbname.is_empty() {
            parts.push(format!(
                "dbname={}",
                libpq_value(&decode("database", dbname)?)
            ));
        }
        // The writer's `options=-csearch_path=…` and friends, already decoded
        // by `query_pairs`; `sslmode` and `sslrootcert` are decided below.
        // A `?password=` is the password too, and goes where the userinfo one
        // does: into the secret, never inline in the attach string.
        let mut sslmode = None;
        let mut rootcert = None;
        let mut query_password = None;
        for (key, value) in url.query_pairs() {
            match &*key {
                "sslmode" => sslmode = Some(value.into_owned()),
                "sslrootcert" => rootcert = Some(value.into_owned()),
                k if k.eq_ignore_ascii_case("password") => {
                    query_password = Some(value.into_owned())
                }
                _ => parts.push(format!("{key}={}", libpq_value(&value))),
            }
        }
        parts.extend(tls_params(sslmode.as_deref(), rootcert, ca_bundle)?);
        let userinfo_password = url.password().map(|p| decode("password", p)).transpose()?;
        let password = match (userinfo_password, query_password) {
            (Some(_), Some(_)) => {
                return Err(SyncError::BadUrl(
                    "a password in both the userinfo and the query".into(),
                ));
            }
            (userinfo, query) => userinfo.or(query),
        };
        Ok(Self {
            conninfo: parts.join(" "),
            password,
            schema: "public".to_string(),
        })
    }
}

/// `sslmode`/`sslrootcert` for the attach: `disable` stays plaintext (a local
/// cluster has no certificate), and anything else becomes `verify-full`
/// against the DSN's own `sslrootcert` or else the system bundle.
fn tls_params(
    sslmode: Option<&str>,
    rootcert: Option<String>,
    ca_bundle: fn() -> Option<PathBuf>,
) -> Result<Vec<String>, SyncError> {
    if sslmode == Some("disable") {
        return Ok(vec!["sslmode=disable".into()]);
    }
    let rootcert = match rootcert {
        Some(path) => path,
        None => ca_bundle()
            .ok_or(SyncError::NoCaBundle)?
            .to_string_lossy()
            .into_owned(),
    };
    Ok(vec![
        "sslmode=verify-full".into(),
        format!("sslrootcert={}", libpq_value(&rootcert)),
    ])
}

/// The system's PEM CA bundle: the first that exists of `SSL_CERT_FILE` and
/// the usual locations (macOS, Debian/Ubuntu, RHEL/Fedora, Alpine). A stale
/// `SSL_CERT_FILE` is skipped like a missing default: the sync falls through
/// to a system bundle, or fails with [`SyncError::NoCaBundle`], never with
/// DuckDB's opaque attach error over a path libpq can't open.
/// A path, not libpq's `sslrootcert=system`, which needs libpq 16 and DuckDB
/// bundles its own.
pub fn ca_bundle() -> Option<PathBuf> {
    std::env::var_os("SSL_CERT_FILE")
        .map(PathBuf::from)
        .into_iter()
        .chain(
            [
                "/etc/ssl/cert.pem",
                "/etc/ssl/certs/ca-certificates.crt",
                "/etc/pki/tls/certs/ca-bundle.crt",
            ]
            .map(PathBuf::from),
        )
        .find(|p| p.is_file())
}

/// A libpq connection-string value, quoted when it has to be.
fn libpq_value(value: &str) -> String {
    let needs_quoting = value.is_empty()
        || value
            .chars()
            .any(|c| c.is_whitespace() || c == '\'' || c == '\\');
    if !needs_quoting {
        return value.to_string();
    }
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// An in-memory DuckDB with `source` attached read-only as [`ATTACHED_AS`].
///
/// Blocking — the first `INSTALL postgres` on a machine downloads the
/// extension — so call it from `spawn_blocking` in async code.
pub fn open_attached(source: &PgSource) -> Result<Connection, SyncError> {
    let conn = Connection::open_in_memory().map_err(at("open"))?;
    conn.execute_batch("INSTALL postgres")
        .map_err(at("install postgres"))?;
    conn.execute_batch("LOAD postgres")
        .map_err(at("load postgres"))?;
    if let Some(password) = &source.password {
        conn.execute_batch(&format!(
            "CREATE OR REPLACE SECRET (TYPE postgres, PASSWORD {})",
            quote(password)
        ))
        .map_err(at("create secret"))?;
    }
    conn.execute_batch(&format!(
        "ATTACH {} AS {ATTACHED_AS} (TYPE postgres, READ_ONLY)",
        quote(&source.conninfo)
    ))
    .map_err(at("attach"))?;
    Ok(conn)
}

fn at(step: &'static str) -> impl Fn(duckdb::Error) -> SyncError {
    move |source| SyncError::DuckDb { step, source }
}

/// `policy_overrides` in `source`'s schema, as DuckDB names it once attached.
pub fn overrides_table(source: &PgSource) -> String {
    format!(
        "{ATTACHED_AS}.{}.{POLICY_OVERRIDES_TABLE}",
        quote_ident(&source.schema)
    )
}

/// Copy `table` (see [`overrides_table`]) to `out` and return how many rows
/// the file holds.
///
/// Written to a sibling temp file and renamed into place, so a reader of
/// `out` sees the previous file or the new one, never half of one.
pub fn copy_overrides(conn: &Connection, table: &str, out: &Path) -> Result<u64, SyncError> {
    let tmp = temp_sibling(out);
    let tmp_sql = quote(&tmp.to_string_lossy());
    let result = (|| {
        conn.execute_batch(&format!(
            "COPY (SELECT * FROM {table} ORDER BY policy_id) TO {tmp_sql} (FORMAT parquet)"
        ))
        .map_err(at("copy"))?;
        let rows: i64 = conn
            .query_row(
                &format!("SELECT count(*) FROM read_parquet({tmp_sql})"),
                [],
                |row| row.get(0),
            )
            .map_err(at("count rows"))?;
        std::fs::rename(&tmp, out).map_err(|source| SyncError::Io {
            path: out.to_path_buf(),
            source,
        })?;
        Ok(rows.unsigned_abs())
    })();
    if result.is_err() {
        // Best effort: a failed copy may have left a partial temp file.
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// [`open_attached`] then [`copy_overrides`] as `writer`: the whole sync,
/// blocking.
pub fn sync_overrides(writer: &OltpWriter, out: &Path) -> Result<u64, SyncError> {
    let source = PgSource::from_writer(writer)?;
    let conn = open_attached(&source)?;
    copy_overrides(&conn, &overrides_table(&source), out)
}

fn temp_sibling(out: &Path) -> PathBuf {
    let name = out
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "policy_overrides.parquet".into());
    out.with_file_name(format!(".{name}.{}.tmp", std::process::id()))
}

/// A SQL string literal.
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// A SQL identifier.
fn quote_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_password_is_kept_out_of_the_attach_string() {
        let src = PgSource::from_url(
            "postgres://app:s3cr%27t@ep-x.neon.tech:5433/my%20db?sslmode=disable",
        )
        .expect("parses");
        assert_eq!(
            src.conninfo,
            "host=ep-x.neon.tech port=5433 user=app dbname='my db' sslmode=disable"
        );
        assert_eq!(src.password.as_deref(), Some("s3cr't"));
    }

    #[test]
    fn a_query_password_is_kept_out_of_the_attach_string_too() {
        for dsn in [
            "postgres://app@h/db?password=s3cr%27t&sslmode=disable",
            "postgres://app@h/db?PASSWORD=s3cr%27t&sslmode=disable",
        ] {
            let src = PgSource::from_url(dsn).expect("parses");
            assert_eq!(src.conninfo, "host=h user=app dbname=db sslmode=disable");
            assert_eq!(src.password.as_deref(), Some("s3cr't"), "{dsn}");
        }
        let both =
            PgSource::from_url("postgres://app:first-pw@h/db?password=second-pw&sslmode=disable");
        let Err(SyncError::BadUrl(why)) = both else {
            panic!("two passwords must be refused");
        };
        assert!(
            !why.contains("first-pw") && !why.contains("second-pw"),
            "{why}"
        );
    }

    #[test]
    fn an_ipv6_host_reaches_libpq_without_brackets() {
        let src =
            PgSource::from_url("postgres://app@[::1]:5432/db?sslmode=disable").expect("parses");
        assert_eq!(
            src.conninfo,
            "host=::1 port=5432 user=app dbname=db sslmode=disable"
        );
    }

    #[test]
    fn a_writer_copies_from_its_own_schema() {
        let dsn = "postgres://app_pricing_rw:pw@h:5432/db?sslmode=disable&options=-csearch_path%3Dapp_pricing";
        let src =
            PgSource::from_writer(&OltpWriter::from_dsn(dsn).expect("writer")).expect("parses");
        assert_eq!(
            overrides_table(&src),
            r#"neon."app_pricing".policy_overrides"#
        );
        assert!(
            src.conninfo.contains(" options=-csearch_path=app_pricing "),
            "{}",
            src.conninfo
        );
    }

    fn bundle() -> Option<PathBuf> {
        Some("/ca dir/roots.pem".into())
    }

    fn no_bundle() -> Option<PathBuf> {
        None
    }

    #[test]
    fn a_tls_dsn_is_attached_verifying_the_certificate() {
        for mode in ["require", "prefer", "verify-ca", "verify-full"] {
            let url = format!("postgres://u:pw@ep-x.neon.tech/db?sslmode={mode}");
            let src = PgSource::from_url_with(&url, bundle).expect("parses");
            assert!(
                src.conninfo
                    .ends_with("sslmode=verify-full sslrootcert='/ca dir/roots.pem'"),
                "{mode}: {}",
                src.conninfo
            );
        }
        let src = PgSource::from_url_with("postgres://u@h/db", bundle).expect("no sslmode");
        assert!(
            src.conninfo.contains("sslmode=verify-full"),
            "{}",
            src.conninfo
        );
        let src = PgSource::from_url_with(
            "postgres://u@h/db?sslmode=require&sslrootcert=/own.pem",
            no_bundle,
        )
        .expect("its own root");
        assert!(
            src.conninfo.ends_with("sslrootcert=/own.pem"),
            "{}",
            src.conninfo
        );
    }

    #[test]
    fn tls_without_a_ca_bundle_is_refused() {
        assert!(matches!(
            PgSource::from_url_with("postgres://u:pw@h/db?sslmode=require", no_bundle),
            Err(SyncError::NoCaBundle)
        ));
        assert!(PgSource::from_url_with("postgres://u:pw@h/db?sslmode=disable", no_bundle).is_ok());
    }

    #[test]
    fn a_non_postgres_url_is_refused() {
        assert!(matches!(
            PgSource::from_url("mysql://a@b/c"),
            Err(SyncError::BadUrl(_))
        ));
    }

    #[test]
    fn quotes_are_doubled() {
        assert_eq!(quote("it's"), "'it''s'");
    }
}
