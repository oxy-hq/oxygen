//! Where `OxyContext.getPolicy` reads from: a literal (no database), or the
//! `policy_overrides` table in the org's OLTP database (`ctx.oltp`, Neon in
//! production).
//!
//! The table is a record an app edits, so it lives where `ctx.oltp` puts every
//! such record: in one app writer's own schema (`app_<slug>`), reached as that
//! writer. The connection comes from `oxy_oltp`'s resolver, never from a URL
//! a caller assembles, and is opened by `oxy_oltp::connect` so TLS is decided
//! the same way as for every other tenant connection: verified against public
//! roots whenever the DSN asks for TLS, as Neon's always does.
//!
//! ```sql
//! -- in app_<slug>, created by the app (its migration), not by Oxy
//! CREATE TABLE policy_overrides (
//!     policy_id  text PRIMARY KEY,
//!     policy     jsonb NOT NULL,          -- a `Policy`, camelCase, as the script sees it
//!     updated_at timestamptz NOT NULL DEFAULT now()
//! );
//! ```

use std::sync::Arc;

use oxy_oltp::resolver::WriterConnection;

use crate::policy::{MOCK_POLICY_JSON, Policy};

/// The table [`PolicySource::Oltp`] reads and `sync` copies.
pub const POLICY_OVERRIDES_TABLE: &str = "policy_overrides";

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("unknown policy: {0}")]
    NotFound(String),
    /// The row exists but is not a [`Policy`], or names a different policy.
    #[error("policy {name} is malformed: {reason}")]
    Malformed { name: String, reason: String },
    /// The database could not be reached or refused the query.
    /// `tokio_postgres`'s text names the failure, not the connection string,
    /// so it is safe to hand to the script.
    #[error("policy store read failed: {0}")]
    Unavailable(#[from] tokio_postgres::Error),
    /// The DSN is not one `oxy_oltp` issues to a writer.
    #[error(
        "not an OLTP writer DSN: {0}. Get one with: oxy oltp dsn --org <org> --role app:<slug>"
    )]
    NotAWriter(String),
    /// Connected, but not confined to the writer's schema.
    #[error("connected with search_path {actual}, expected the writer's schema {expected}")]
    WrongSchema { expected: String, actual: String },
}

/// A writer's way into the org's OLTP database: the DSN `oxy_oltp` issues,
/// and the one schema it is pinned to.
///
/// Built from a resolved [`WriterConnection`], or from the string `oxy oltp
/// dsn --role app:<slug>` prints. Either way the schema is the writer's own,
/// so an unqualified `policy_overrides` cannot land anywhere else.
#[derive(Clone)]
pub struct OltpWriter {
    dsn: String,
    schema: String,
}

impl OltpWriter {
    /// Reads the schema from the DSN's `options=-csearch_path=…`, which the
    /// resolver always sets on a writer DSN. A DSN without exactly one
    /// pinned schema (an analyst's, an owner's, a hand-built one) is refused.
    pub fn from_dsn(dsn: &str) -> Result<Self, StoreError> {
        let not = |why: &str| StoreError::NotAWriter(why.to_string());
        let url = url::Url::parse(dsn).map_err(|e| not(&e.to_string()))?;
        let options = url
            .query_pairs()
            .find(|(k, _)| k == "options")
            .map(|(_, v)| v.into_owned())
            .ok_or_else(|| not("no search_path is pinned"))?;
        let schema = options
            .split_whitespace()
            .find_map(|opt| opt.strip_prefix("-csearch_path="))
            .ok_or_else(|| not("no search_path is pinned"))?;
        if schema.is_empty() || schema.contains(',') {
            return Err(not("its search_path is not a single schema"));
        }
        Ok(Self {
            dsn: dsn.to_string(),
            schema: schema.to_string(),
        })
    }

    pub fn dsn(&self) -> &str {
        &self.dsn
    }

    pub fn schema(&self) -> &str {
        &self.schema
    }
}

impl From<WriterConnection> for OltpWriter {
    fn from(conn: WriterConnection) -> Self {
        Self {
            dsn: conn.dsn,
            schema: conn.schema,
        }
    }
}

/// The DSN carries the writer's password.
impl std::fmt::Debug for OltpWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OltpWriter")
            .field("schema", &self.schema)
            .field("dsn", &"<redacted>")
            .finish()
    }
}

/// Lives in the isolate's op state; cloned out of it by the read op before it
/// awaits, so no `OpState` borrow is held across the query.
#[derive(Clone)]
pub enum PolicySource {
    /// Serves a built-in literal as `"mock"` and nothing else.
    Mock,
    /// One connection, as the writer, opened at startup.
    Oltp(Arc<tokio_postgres::Client>),
}

impl std::fmt::Debug for PolicySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Mock => "PolicySource::Mock",
            Self::Oltp(_) => "PolicySource::Oltp",
        })
    }
}

impl PolicySource {
    /// Connect as `writer`, and check the session really is confined to its
    /// schema before anything reads through it.
    pub async fn connect(writer: &OltpWriter) -> Result<Self, StoreError> {
        let client = oxy_oltp::connect::connect(writer.dsn(), "policy store").await?;
        let actual: String = client.query_one("SHOW search_path", &[]).await?.get(0);
        if actual.trim_matches('"') != writer.schema() {
            return Err(StoreError::WrongSchema {
                expected: writer.schema().to_string(),
                actual,
            });
        }
        Ok(Self::Oltp(Arc::new(client)))
    }

    /// The policy called `name`, parsed and checked to be that policy.
    pub async fn fetch(&self, name: &str) -> Result<Policy, StoreError> {
        let raw: String = match self {
            Self::Mock if name == "mock" => MOCK_POLICY_JSON.to_string(),
            Self::Mock => return Err(StoreError::NotFound(name.to_string())),
            Self::Oltp(client) => client
                .query_opt(
                    "SELECT policy::text FROM policy_overrides WHERE policy_id = $1",
                    &[&name],
                )
                .await?
                .ok_or_else(|| StoreError::NotFound(name.to_string()))?
                .get(0),
        };
        let malformed = |reason: String| StoreError::Malformed {
            name: name.to_string(),
            reason,
        };
        let policy: Policy = serde_json::from_str(&raw).map_err(|e| malformed(e.to_string()))?;
        if policy.id != name {
            return Err(malformed(format!("its body names policy {}", policy.id)));
        }
        Ok(policy)
    }
}

#[cfg(test)]
mod tests {
    use oxy_oltp::schema::{WriterRef, with_search_path};

    use super::*;

    #[test]
    fn the_schema_is_read_from_a_resolver_issued_dsn() {
        let writer = WriterRef::app("pricing").expect("valid slug");
        let dsn = with_search_path(
            "postgres://app_pricing_rw:p%40ss@ep-x.neon.tech:5432/neondb?sslmode=require",
            &writer,
        );
        let parsed = OltpWriter::from_dsn(&dsn).expect("a writer DSN");
        assert_eq!(parsed.schema(), "app_pricing");
        assert!(!format!("{parsed:?}").contains("p%40ss"));
    }

    #[test]
    fn a_dsn_without_a_pinned_schema_is_refused() {
        for dsn in [
            "postgres://oxy_analyst_ro:pw@h/db?sslmode=require",
            "postgres://u:pw@h/db?options=-csearch_path%3Da,b",
            "postgres://u:pw@h/db?options=-cstatement_timeout%3D5s",
        ] {
            assert!(
                matches!(OltpWriter::from_dsn(dsn), Err(StoreError::NotAWriter(_))),
                "{dsn}"
            );
        }
    }
}
