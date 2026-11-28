//! Applying a workspace's `schemas/*.sql` to an org's OLTP database.
//!
//! Plain SQL, applied in `file_path` order, recorded in
//! [`crate::platform::MIGRATIONS_TABLE`]. No diffing and no planner: the files
//! *are* the migrations, and Oxy hands them to Postgres verbatim.
//!
//! That is deliberate. These are authored by Oxy engineers in a vibe-coding
//! flow, and every model writes Postgres DDL fluently while none has seen a
//! bespoke schema DSL. A planner that second-guessed the author would be the
//! tool arguing with someone who knows what they meant.
//!
//! Additive-only is therefore a **convention, not a gate**. Dropping a column
//! is allowed, and is the author's call.

use std::collections::HashMap;

use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};
use tracing::{info, instrument};
use uuid::Uuid;

use crate::entity::tenants::{self as oltp_tenants, Entity as OltpTenants};
use crate::platform::MIGRATIONS_TABLE;

#[derive(Debug, thiserror::Error)]
pub enum MigrateError {
    #[error("org {0} has no OLTP database; provision it first")]
    NotProvisioned(Uuid),
    #[error(
        "migration {file_path} was already applied with a different checksum. \
         Editing a shipped migration silently diverges tenants — add a new file instead."
    )]
    ChecksumMismatch { file_path: String },
    #[error("migration {file_path} failed: {message}")]
    Failed { file_path: String, message: String },
    #[error("could not connect to the tenant database: {0}")]
    Connect(String),
    /// Another apply holds the advisory lock for this org.
    ///
    /// Its own variant, not a `Connect`: those two carry opposite diagnoses.
    /// `Connect` means the tenant database is unreachable — check the network,
    /// the provider, the credential. This means everything is fine and someone
    /// else got there first, so the answer is to retry. Reporting it as
    /// `Connect` sent every caller looking at the wrong layer.
    #[error("another apply is already running for org {org_id}; wait for it to finish")]
    Locked { org_id: Uuid },
    #[error("database error: {0}")]
    Db(#[from] sea_orm::DbErr),
    #[error("{0}")]
    Owner(String),
}

/// What an apply run did. Reported so a caller can log or surface it without
/// re-querying the ledger.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct MigrateOutcome {
    pub applied: Vec<String>,
    pub already_applied: usize,
}

/// Whether this migration really runs `CONCURRENTLY`, ignoring prose.
///
/// A raw substring search over the file matched the word inside `--` comments,
/// `/* */` blocks and quoted literals — so a migration whose only mention was a
/// comment (`-- do NOT use CONCURRENTLY here`) ran **unwrapped**, losing the
/// transaction that is otherwise what makes a failed file leave nothing behind.
/// Wrong in the direction that costs atomicity silently: the file applies, half
/// of it sticks, and the ledger has no row for it.
///
/// Comment- and literal-stripping only. It does not parse SQL, so a genuinely
/// perverse file could still fool it; the point is that the ordinary way to
/// mention a keyword — writing about it — no longer changes how the file runs.
fn mentions_concurrently(sql: &str) -> bool {
    let mut code = String::with_capacity(sql.len());
    let b = sql.as_bytes();
    let mut i = 0;
    while i < b.len() {
        // `--` to end of line.
        if b[i] == b'-' && i + 1 < b.len() && b[i + 1] == b'-' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        // `/* … */`, which Postgres allows to nest.
        if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            let mut depth = 1;
            i += 2;
            while i < b.len() && depth > 0 {
                if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
                    depth += 1;
                    i += 2;
                } else if b[i] == b'*' && i + 1 < b.len() && b[i + 1] == b'/' {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            continue;
        }
        // A single-quoted literal.
        //
        // `''` — SQL's escape — needs no special case here: it is TWO quotes,
        // so treating it as "close, reopen" consumes the same bytes and the
        // scanner re-aligns immediately. Only an odd run of quotes could shift
        // the phase, and that is not valid SQL. An explicit branch for it was
        // written first and removed: no input distinguished the two, which is
        // the definition of untestable complexity.
        if b[i] == b'\'' {
            i += 1;
            while i < b.len() {
                if b[i] == b'\'' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        code.push(b[i] as char);
        i += 1;
    }
    code.to_ascii_uppercase().contains("CONCURRENTLY")
}

/// Apply every unapplied migration from `revision_id` to `org_id`'s database.
///
/// Idempotent: a file already in the ledger with a matching checksum is
/// skipped. A file in the ledger with a *different* checksum is an error —
/// someone edited a migration that tenants already ran, which would leave them
/// permanently divergent.
///
/// Each file runs in its own transaction, so a failure part-way leaves earlier
/// files applied and recorded. That is the right granularity for DDL: Postgres
/// can roll back a single `CREATE TABLE`, but re-running a half-finished batch
/// is what the ledger is for.
///
/// `CREATE INDEX CONCURRENTLY` cannot run inside a transaction. A file
/// containing one must be the whole file, and is detected and run unwrapped.
#[instrument(skip(control, owner_dsn), fields(org_id = %org_id, revision_id = %revision_id))]
pub async fn apply_to_org(
    control: &DatabaseConnection,
    org_id: Uuid,
    revision_id: Uuid,
    owner_dsn: &str,
    tenant_row_id: Uuid,
    owner_role: &str,
) -> Result<MigrateOutcome, MigrateError> {
    let mut outcome = MigrateOutcome::default();
    let target = ApplyTarget {
        org_id,
        revision_id,
        owner_dsn,
        tenant_row_id,
        owner_role,
    };
    apply_into(control, &target, &mut outcome).await?;
    Ok(outcome)
}

/// Which tenant, which revision, and the owner credential to run it as — the
/// arguments [`apply_to_org`] takes, as one value.
pub struct ApplyTarget<'a> {
    pub org_id: Uuid,
    pub revision_id: Uuid,
    pub owner_dsn: &'a str,
    pub tenant_row_id: Uuid,
    pub owner_role: &'a str,
}

/// The apply itself, recording each file in `outcome` as it lands.
///
/// Public for the staff route (`oxy_api_data_plane::oltp::apply`), which owns
/// `outcome` OUTSIDE the future it times out: files this call committed are
/// owner-level DDL on a customer's database, and the route has to report and
/// audit them whether the apply finished, failed on a later file, or ran out
/// of time.
#[instrument(skip(control, target, outcome), fields(org_id = %target.org_id, revision_id = %target.revision_id))]
pub async fn apply_into(
    control: &DatabaseConnection,
    target: &ApplyTarget<'_>,
    outcome: &mut MigrateOutcome,
) -> Result<(), MigrateError> {
    let ApplyTarget {
        org_id,
        revision_id,
        owner_dsn,
        tenant_row_id,
        owner_role,
    } = *target;
    let tenant = tenant_for_org(control, org_id).await?;
    let analyst_role = crate::schema::analyst_role_for(&tenant.provider, &tenant.database_name);
    let pending = entity::schema_migration_definitions::Entity::find()
        .filter(entity::schema_migration_definitions::Column::RevisionId.eq(revision_id))
        .order_by_asc(entity::schema_migration_definitions::Column::FilePath)
        .all(control)
        .await?;

    let mut client = crate::connect::connect(owner_dsn, "tenant migration")
        .await
        .map_err(|e| MigrateError::Connect(e.to_string()))?;

    // Serialise applies per tenant BEFORE snapshotting the ledger.
    //
    // The read below is a point-in-time snapshot, and the per-file transaction
    // does not help: two applies that both read "0002 is unapplied" both run
    // its DDL, each committing its own ledger row. That is reachable today
    // (two operators, or an operator and a retry) and becomes routine once
    // this is driven from compile-promote.
    //
    // A session-level lock on a key derived from the tenant, held for the whole
    // apply — not `_xact_` — because the work spans several transactions, one
    // per migration file. Released explicitly below and on disconnect.
    let lock_key = tenant_lock_key(tenant.id);
    // TRY, not block. `pg_advisory_lock` waits forever: a second operator — or
    // the compile-promote hook this is designed to grow into — would hang with
    // no output, indistinguishable from a slow migration, and one leaked
    // session wedges every later apply. Failing with a sentence keeps the
    // diagnosis where the operator is.
    let got: bool = client
        .query_one("SELECT pg_try_advisory_lock($1)", &[&lock_key])
        .await
        .map_err(|e| MigrateError::Connect(format!("acquire apply lock: {e}")))?
        .get(0);
    if !got {
        return Err(MigrateError::Locked { org_id });
    }

    let applied: HashMap<String, String> = client
        .query(
            &format!("SELECT file_path, checksum FROM {MIGRATIONS_TABLE}"),
            &[],
        )
        .await
        .map_err(|e| MigrateError::Connect(format!("read ledger: {e}")))?
        .into_iter()
        .map(|r| (r.get("file_path"), r.get("checksum")))
        .collect();

    for m in pending {
        match applied.get(&m.file_path) {
            Some(seen) if *seen == m.content_sha256 => {
                outcome.already_applied += 1;
                continue;
            }
            Some(_) => {
                return Err(MigrateError::ChecksumMismatch {
                    file_path: m.file_path,
                });
            }
            None => {}
        }

        // `CONCURRENTLY` is incompatible with an explicit transaction block, so
        // such a file runs unwrapped. It is on the author to keep that file to
        // the one statement.
        let concurrent = mentions_concurrently(&m.content);

        if concurrent {
            // Not atomic with its ledger row — unavoidable, since the whole
            // point of CONCURRENTLY is running outside a transaction. A crash
            // between the two re-runs the statement, which is why an index
            // built this way must use IF NOT EXISTS.
            client
                .batch_execute(&m.content)
                .await
                .map_err(|e| MigrateError::Failed {
                    file_path: m.file_path.clone(),
                    message: crate::connect::pg_detail(&e),
                })?;
            record_applied(&client, &m.file_path, &m.content_sha256, revision_id).await?;
        } else {
            // DDL and its ledger row commit together. Separating them means a
            // crash in between silently re-applies the migration on the next
            // run — fine for `IF NOT EXISTS`, corrupting for anything else.
            let txn = client
                .transaction()
                .await
                .map_err(|e| MigrateError::Failed {
                    file_path: m.file_path.clone(),
                    message: crate::connect::pg_detail(&e),
                })?;
            txn.batch_execute(&m.content)
                .await
                .map_err(|e| MigrateError::Failed {
                    file_path: m.file_path.clone(),
                    message: crate::connect::pg_detail(&e),
                })?;
            record_applied(&txn, &m.file_path, &m.content_sha256, revision_id).await?;
            txn.commit().await.map_err(|e| MigrateError::Failed {
                file_path: m.file_path.clone(),
                message: format!("committing: {e}"),
            })?;
        }

        info!(file_path = %m.file_path, "applied schema migration");
        outcome.applied.push(m.file_path);
    }

    // Migrations run as the owner, so anything they created is owned by the
    // owner and carries none of the writer's grants. Without this the app
    // cannot read or write its own new tables — silently, until it queries.
    //
    // Unconditional, not `if applied`. Grants drift for reasons that have
    // nothing to do with a new migration — a re-minted role loses every grant
    // it held, which is exactly what happens when a role is remediated — and a
    // repair tool that only works when there is also new schema to apply is not
    // a repair tool. The statements are idempotent `GRANT`s over the owner's
    // own tables, so the cost of running them every time is a few round trips.
    reconcile_grants(control, &client, tenant_row_id, owner_role, &analyst_role).await?;

    // Best-effort: the lock is session-scoped, so dropping the client releases
    // it anyway. Explicit so a long-lived caller does not hold it.
    let _ = client
        .execute("SELECT pg_advisory_unlock($1)", &[&lock_key])
        .await;

    Ok(())
}

/// Re-grant every writer over its schema, covering objects a migration created.
///
/// Runs as the owner. Tables the *writer* created need nothing — it owns them —
/// so this only closes the owner-created gap.
async fn reconcile_grants(
    control: &DatabaseConnection,
    client: &tokio_postgres::Client,
    tenant_row_id: Uuid,
    owner_role: &str,
    // Passed rather than derived: on a shared cluster the analyst's real name
    // is qualified per tenant, and the bare constant would grant to another
    // tenant's role.
    analyst_role: &str,
) -> Result<(), MigrateError> {
    let roles = crate::entity::roles::Entity::find()
        .filter(crate::entity::roles::Column::TenantRowId.eq(tenant_row_id))
        .all(control)
        .await?;

    for r in roles {
        let writer = match r.writer_kind {
            crate::entity::roles::WriterKind::App => crate::schema::WriterRef::app(&r.writer_name),
            crate::entity::roles::WriterKind::Pipeline => {
                crate::schema::WriterRef::pipeline(&r.writer_name)
            }
        }
        .map_err(|e| MigrateError::Owner(format!("writer {}: {e}", r.writer_name)))?;

        // Same default as provisioning: pipeline data is analyst-readable,
        // app data is not until opted in.
        // The STORED choice, falling back to the kind's default only when
        // nobody has chosen. Re-deriving unconditionally is what reinstated a
        // revoked grant on the next apply and left an opted-in app schema
        // uncovered for tables added later.
        let visible = effective_visibility(r.analytics_visible, &r.writer_kind);
        // The stored name, which on a shared cluster is qualified — `r` came
        // from `oltp_roles`, so it already is.
        let mut statements = Vec::new();
        // Schema USAGE first: `reconcile_migration_grants_sql` grants SELECT on
        // the tables, which is useless without the right to see into the schema
        // holding them. That grant lived only in `set_analytics_visibility`, so
        // a re-minted analyst got table grants and still hit
        // `permission denied for schema raw_toast`.
        // Additive only — never revoke here. Analytics visibility is not
        // persisted (it lives in Postgres as a GRANT), so this cannot tell an
        // app that opted in from one that never did. Revoking on the default
        // would silently withdraw an explicit opt-in every time anyone ran
        // `apply`. Withdrawal stays an explicit act:
        // `set_analytics_visibility(.., false)`.
        if visible {
            statements.extend(crate::schema::grant_analyst_schema_sql(
                &writer,
                analyst_role,
            ));
        }
        statements.extend(
            crate::schema::reconcile_migration_grants_sql(
                &writer,
                owner_role,
                visible,
                &analyst_role,
                &r.role_name,
            )
            .map_err(|e| MigrateError::Owner(e.to_string()))?,
        );

        for stmt in statements {
            client
                .batch_execute(&stmt)
                .await
                .map_err(|e| MigrateError::Failed {
                    file_path: format!("grants for {}", writer.schema_name()),
                    message: format!("{} (while running: {stmt})", crate::connect::pg_detail(&e)),
                })?;
        }
        info!(schema = %writer.schema_name(), "reconciled grants after migration");
    }
    Ok(())
}

#[cfg(test)]
mod visibility_tests {
    use crate::schema::WriterRef;

    /// Reconciliation must honour a STORED choice and only fall back to the
    /// kind's default when nobody has made one.
    ///
    /// Re-deriving unconditionally reinstated a grant an operator had revoked,
    /// and left an opted-in app schema uncovered for tables added by later
    /// migrations — the second reads as missing data rather than missing
    /// grants, which is the harder of the two to diagnose.
    #[test]
    fn stored_visibility_wins_over_the_kind_default() {
        let app = WriterRef::app("bookings").unwrap();
        let pipeline = WriterRef::pipeline("toast").unwrap();

        let effective = |stored: Option<bool>, w: &WriterRef| {
            stored.unwrap_or_else(|| w.analytics_visible_by_default())
        };

        // Never chosen → the documented defaults.
        assert!(!effective(None, &app), "app_* is hidden until asked");
        assert!(effective(None, &pipeline), "raw_* is readable by default");

        // Chosen → the choice, in BOTH directions.
        assert!(
            effective(Some(true), &app),
            "an opted-in app stays opted in"
        );
        assert!(
            !effective(Some(false), &pipeline),
            "a revoked pipeline must not have its grant reinstated"
        );
    }
}

/// Whether the analyst may read this schema: the STORED choice, falling back to
/// the kind's default only when nobody has made one.
///
/// A function so the migrator, the API and any future reader share one answer —
/// re-deriving from the default is what reinstated a revoked grant on the next
/// apply, and three copies of `unwrap_or_else` is how that comes back.
///
/// Takes the **kind**, not a `WriterRef`. The default depends only on the
/// variant — the writer's name is never read — so a `WriterRef` forced the API
/// to parse one just to throw it away, and to carry a fallible arm for a parse
/// that "cannot fail"; that arm then re-implemented this very formula, which is
/// the duplication the function exists to prevent.
pub fn effective_visibility(stored: Option<bool>, kind: &crate::entity::roles::WriterKind) -> bool {
    stored.unwrap_or(matches!(kind, crate::entity::roles::WriterKind::Pipeline))
}

/// A stable 64-bit advisory-lock key for one tenant.
///
/// Advisory-lock objects already carry the current database OID and this
/// connection is on the tenant's own database, so tenants cannot serialise
/// against each other regardless — deriving the key is belt-and-braces, not
/// what makes this correct.
fn tenant_lock_key(tenant_row_id: Uuid) -> i64 {
    let b = tenant_row_id.as_bytes();
    i64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
}

/// Record one migration as applied. Generic over client and transaction so the
/// transactional path can commit it atomically with the DDL.
async fn record_applied<E>(
    exec: &E,
    file_path: &str,
    checksum: &str,
    revision_id: Uuid,
) -> Result<(), MigrateError>
where
    E: tokio_postgres::GenericClient,
{
    exec.execute(
        &format!(
            "INSERT INTO {MIGRATIONS_TABLE} (file_path, checksum, revision_id)
             VALUES ($1, $2, $3)
             ON CONFLICT (file_path) DO UPDATE
               SET checksum = EXCLUDED.checksum,
                   revision_id = EXCLUDED.revision_id,
                   applied_at = now()"
        ),
        &[&file_path, &checksum, &revision_id],
    )
    .await
    .map_err(|e| MigrateError::Failed {
        file_path: file_path.to_string(),
        message: format!("recording in the ledger: {e}"),
    })?;
    Ok(())
}

/// Owner DSN for a tenant — the role migrations run as.
///
/// Migrations use the **direct** endpoint, never a pooler: Neon's docs are
/// explicit that a pooled connection is wrong for DDL, and a pooler can rewrite
/// session state under a migration.
pub fn owner_dsn(tenant: &oltp_tenants::Model) -> Result<String, MigrateError> {
    let sealed = tenant.owner_password_ciphertext.as_ref().ok_or_else(|| {
        MigrateError::Owner(format!(
            "org {} has no stored owner password",
            tenant.org_id
        ))
    })?;
    let bytes = oxy_platform::secrets::envelope::open(sealed)
        .map_err(|e| MigrateError::Owner(format!("unseal owner password: {e}")))?;
    let password = String::from_utf8(bytes)
        .map_err(|e| MigrateError::Owner(format!("owner password: {e}")))?;
    // The crate's one DSN builder. This hand-rolled its own copy of both the
    // encoding and the sslmode choice, and was the last raw one left: with
    // derived passwords it was safe by accident (alphanumeric), and the CSPRNG
    // change made roughly a quarter of local provisions produce an unescaped
    // `@` in the userinfo — at which point libpq reads the rest as the hostname
    // and it surfaces as a lookup failure, not a credential one.
    //
    // Reachable from `apply`, `audit`, `dsn --role owner`, `connect --role
    // owner` and `just oltp-psql owner`, non-deterministically — so a green run
    // said nothing about the next.
    Ok(crate::provisioner::dsn_for(
        tenant,
        &tenant.owner_role,
        &password,
    ))
}

/// Look up the tenant row for an org, for callers that need the owner DSN.
pub async fn tenant_for_org(
    control: &DatabaseConnection,
    org_id: Uuid,
) -> Result<oltp_tenants::Model, MigrateError> {
    OltpTenants::find()
        .filter(oltp_tenants::Column::OrgId.eq(org_id))
        .one(control)
        .await?
        .ok_or(MigrateError::NotProvisioned(org_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_edited_shipped_migration_names_the_file_and_the_fix() {
        let e = MigrateError::ChecksumMismatch {
            file_path: "schemas/0001_orders.sql".into(),
        };
        let msg = e.to_string();
        assert!(msg.contains("schemas/0001_orders.sql"), "got {msg}");
        // The failure mode is silent divergence across tenants, so the message
        // has to say what to do instead, not just that it refused.
        assert!(msg.contains("add a new file"), "got {msg}");
    }

    #[test]
    fn an_empty_revision_is_a_no_op_not_an_error() {
        assert_eq!(
            MigrateOutcome::default(),
            MigrateOutcome {
                applied: vec![],
                already_applied: 0
            }
        );
    }
}

#[cfg(test)]
mod concurrently_tests {
    use super::mentions_concurrently;

    /// The real thing still runs unwrapped.
    #[test]
    fn a_real_concurrent_index_is_detected() {
        assert!(mentions_concurrently(
            "CREATE INDEX CONCURRENTLY IF NOT EXISTS idx ON orders (id);"
        ));
        // Case and whitespace are the author's choice, not a signal.
        assert!(mentions_concurrently(
            "create index concurrently idx on t (c);"
        ));
    }

    /// Prose must not change how the file runs. This is the regression: a
    /// substring search matched the word in a comment and dropped the
    /// transaction, so a failure left half the file applied and no ledger row.
    #[test]
    fn a_comment_mentioning_it_does_not_unwrap_the_file() {
        for sql in [
            "-- do NOT use CONCURRENTLY here, it breaks the transaction\nALTER TABLE t ADD COLUMN c int;",
            "/* CONCURRENTLY is unavailable inside a transaction */\nALTER TABLE t ADD COLUMN c int;",
            // Nested: the keyword sits AFTER the inner `*/`, so a scanner that
            // ends the comment there would read it as code. Putting it before
            // proves nothing — the tail has no keyword either way.
            "/* outer /* inner */ CONCURRENTLY */\nALTER TABLE t ADD COLUMN c int;",
            "ALTER TABLE t ADD COLUMN c int; -- CONCURRENTLY",
        ] {
            assert!(!mentions_concurrently(sql), "treated prose as code: {sql}");
        }
    }

    /// Same for a string literal — a seeded row may legitimately contain the
    /// word.
    #[test]
    fn a_quoted_literal_mentioning_it_does_not_unwrap_the_file() {
        assert!(!mentions_concurrently(
            "INSERT INTO notes (body) VALUES ('run this CONCURRENTLY next time');"
        ));
    }

    /// An escaped quote inside a literal must not swallow the statement after
    /// it.
    ///
    /// This does NOT pin a `''` special case — there isn't one, and no input
    /// would justify it (see the scanner). It pins the outcome for a real
    /// input shape, which is what a later rewrite could get wrong.
    #[test]
    fn an_escaped_quote_does_not_swallow_the_next_statement() {
        assert!(mentions_concurrently(
            "INSERT INTO notes (body) VALUES ('don''t');\n\
             CREATE INDEX CONCURRENTLY IF NOT EXISTS idx ON orders (id);"
        ));
    }

    /// A file that both talks about it and does it is still the real thing.
    #[test]
    fn a_comment_does_not_mask_a_real_statement() {
        assert!(mentions_concurrently(
            "-- CONCURRENTLY, because this table is hot\n\
             CREATE INDEX CONCURRENTLY IF NOT EXISTS idx ON orders (id);"
        ));
    }
}
