//! A custom-app sandbox's own schema inside the org's **staging branch**
//! database (`internal-docs/per-org-oltp-postgres.md` → Sandbox schemas on
//! the staging branch).
//!
//! Every sandbox of every app in an org used to share staging's `app_<writer>`
//! schema on the branch. A sandbox now gets `app_<writer>__<label>` there — a
//! copy of staging's, seeded once and then migrated by the sandbox's own
//! builds — reached as the app's own writer with that schema as the only
//! `search_path` entry.
//!
//! **Nothing is provisioned.** No database, no branch, no role: the schema is
//! created by the branch's owner and granted to the writer the branch already
//! has. An org with no staging branch has no sandbox schema.
//!
//! **Never production.** Every function here reaches the database through the
//! branch resolvers (`crate::resolver::branch`), which refuse a branch row
//! that names production and one that is not `active`.
//!
//! **The name is derived, never taken.** [`SandboxSchema`] can only be built
//! from an app writer and an environment label, so nothing here creates,
//! connects to or drops a schema a caller spelled.
//!
//! * here — the name, and the owner's DDL: create, drop, exists;
//! * [`seed`] — the copy of staging's schema, run as the writer.

pub mod seed;

use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;

use crate::OltpBranch;
use crate::branches::BranchCut;
use crate::entity::roles::{self as oltp_roles, Entity as OltpRoles};
use crate::resolver::{
    BranchOwner, BranchWriter, ResolveError, resolve_branch_owner, resolve_branch_writer_in_schema,
};
use crate::schema::{GrantLevel, WriterRef, quote_ident};

/// Between an app's schema and the environment its copy belongs to. A writer
/// name maps each hyphen of a slug to one underscore, and no slug holds `--`,
/// so nothing else produces a double underscore — the separator the Airhouse
/// sibling uses, for the same reason (`airhouse::app_schema`).
pub const SEPARATOR: &str = "__";

/// Postgres's identifier limit. A longer name is refused, never truncated: a
/// truncated name could meet another schema.
pub const MAX_SCHEMA_LEN: usize = 63;

#[derive(Debug, thiserror::Error)]
pub enum SandboxSchemaError {
    #[error("{0} is not a custom app's writer; only an app has sandbox schemas")]
    NotAnApp(String),
    /// A legacy slug holding `--` derives a schema that already holds the
    /// separator, so a copy of it could not be told from another app's.
    #[error(
        "the app's own schema {0} holds `__` (a legacy slug with `--`), so it has no sandbox \
         schemas"
    )]
    LegacyAppSchema(String),
    #[error("{0:?} is not an environment's schema label")]
    BadLabel(String),
    #[error(
        "the sandbox's OLTP schema {name} would be {len} bytes, over Postgres's {MAX_SCHEMA_LEN}; \
         use a shorter sandbox handle (it is never truncated)"
    )]
    TooLong { name: String, len: usize },
    /// The name is a provisioned writer's own schema: another app's, from a
    /// legacy slug holding `--`. Never created over, connected to as a
    /// sandbox's, or dropped.
    #[error(
        "{schema} is another writer's own schema in this org's OLTP database, not a sandbox's; \
         use another sandbox handle"
    )]
    IsAWriters { schema: String },
    #[error(transparent)]
    Resolve(#[from] ResolveError),
    #[error("database error: {0}")]
    Db(#[from] sea_orm::DbErr),
    #[error("could not connect to the staging branch: {0}")]
    Connect(String),
    #[error("{step} failed on the staging branch: {message}")]
    Statement { step: &'static str, message: String },
    #[error("{0} is still there after it was dropped; the drop is not confirmed")]
    NotDropped(String),
}

/// One sandbox's schema on the staging branch: `app_<writer>__<label>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxSchema {
    /// The app's own schema — staging's copy on the branch, which a new
    /// sandbox schema is seeded from.
    app_schema: String,
    name: String,
}

impl SandboxSchema {
    /// The schema of `writer`'s sandbox whose label is `label`
    /// (`AppEnvironment::schema_label`: `dev_<handle>`, each `-` as `_`).
    ///
    /// Refused — and the caller refuses the sandbox's OLTP rather than guess a
    /// home — for a pipeline's writer, an app schema that already holds the
    /// separator, a label that is not one, and a name over [`MAX_SCHEMA_LEN`].
    pub fn for_writer(writer: &WriterRef, label: &str) -> Result<Self, SandboxSchemaError> {
        let WriterRef::App(_) = writer else {
            return Err(SandboxSchemaError::NotAnApp(writer.to_string()));
        };
        let app_schema = writer.schema_name();
        if app_schema.contains(SEPARATOR) {
            return Err(SandboxSchemaError::LegacyAppSchema(app_schema));
        }
        if !is_schema_label(label) {
            return Err(SandboxSchemaError::BadLabel(label.to_string()));
        }
        let name = format!("{app_schema}{SEPARATOR}{label}");
        if name.len() > MAX_SCHEMA_LEN {
            return Err(SandboxSchemaError::TooLong {
                len: name.len(),
                name,
            });
        }
        Ok(Self { app_schema, name })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Staging's schema on the branch, which this one is a copy of.
    pub fn app_schema(&self) -> &str {
        &self.app_schema
    }
}

impl std::fmt::Display for SandboxSchema {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)
    }
}

/// `[a-z][a-z0-9]*(_[a-z0-9]+)*`: lowercase letters, digits and single inner
/// underscores, starting with a letter — so a label never holds the separator.
fn is_schema_label(label: &str) -> bool {
    label.starts_with(|c: char| c.is_ascii_lowercase())
        && !label.ends_with('_')
        && !label.contains(SEPARATOR)
        && label
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// The app writer's connection on the org's `branch`, with `schema` as its one
/// `search_path` entry — what a sandbox's `ctx.oltp`, its seed and its
/// migrations connect with. `Ok(None)`: the org has no such branch.
pub async fn resolve_branch_sandbox_writer_for_org(
    db: &DatabaseConnection,
    org_id: Uuid,
    branch: OltpBranch,
    writer: &WriterRef,
    schema: &SandboxSchema,
) -> Result<Option<BranchWriter>, SandboxSchemaError> {
    let Some(resolved) =
        resolve_branch_writer_in_schema(db, org_id, branch, writer, schema.name()).await?
    else {
        return Ok(None);
    };
    refuse_a_writers_schema(db, org_id, schema).await?;
    Ok(Some(resolved))
}

/// Create `schema` empty on the org's `branch`, owned by the branch owner and
/// granted to `writer`'s role — replacing whatever was under the name.
/// Answers the cut it was created on; `Ok(None)` when the org has no branch.
///
/// Dropped first, not `IF NOT EXISTS`: this runs only when the sandbox's row
/// records no usable schema, so anything under the name is a seed that died
/// part way, and a copy must start from nothing.
pub async fn create_on_branch(
    db: &DatabaseConnection,
    org_id: Uuid,
    branch: OltpBranch,
    writer: &WriterRef,
    schema: &SandboxSchema,
) -> Result<Option<BranchCut>, SandboxSchemaError> {
    let Some(owner) = resolve_branch_owner(db, org_id, branch).await? else {
        return Ok(None);
    };
    refuse_a_writers_schema(db, org_id, schema).await?;
    let role = writer_role(db, &owner, org_id, writer).await?;
    let statements = create_sql(schema, &owner.owner_role, &role)?;
    run_as_owner(&owner, "create the sandbox schema", &statements).await?;
    Ok(Some(owner.cut))
}

/// What a [`drop_on_branch`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dropped {
    /// The org has no OLTP database or no such branch: the schema went with it.
    NoBranch,
    /// Dropped (or already absent), and confirmed absent.
    Confirmed,
}

/// Drop `schema` on the org's `branch` and confirm it is gone. A branch that
/// is not active, or cannot be reached, is an error — the caller keeps what
/// records the schema rather than forget one it could not drop.
pub async fn drop_on_branch(
    db: &DatabaseConnection,
    org_id: Uuid,
    branch: OltpBranch,
    schema: &SandboxSchema,
) -> Result<Dropped, SandboxSchemaError> {
    let owner = match resolve_branch_owner(db, org_id, branch).await {
        Ok(Some(owner)) => owner,
        Ok(None) | Err(ResolveError::NotProvisioned(_)) => return Ok(Dropped::NoBranch),
        Err(e) => return Err(e.into()),
    };
    refuse_a_writers_schema(db, org_id, schema).await?;
    let drop = format!(
        "DROP SCHEMA IF EXISTS {} CASCADE",
        quote_ident(schema.name())
    );
    run_as_owner(&owner, "drop the sandbox schema", &[drop]).await?;
    if exists(&owner, schema).await? {
        return Err(SandboxSchemaError::NotDropped(schema.name().to_string()));
    }
    Ok(Dropped::Confirmed)
}

/// Whether `schema` is on the org's `branch` now. `Ok(None)`: no branch.
pub async fn exists_on_branch(
    db: &DatabaseConnection,
    org_id: Uuid,
    branch: OltpBranch,
    schema: &SandboxSchema,
) -> Result<Option<bool>, SandboxSchemaError> {
    let Some(owner) = resolve_branch_owner(db, org_id, branch).await? else {
        return Ok(None);
    };
    exists(&owner, schema).await.map(Some)
}

/// The owner's statements that make `schema` empty and `role`'s to fill.
/// `ensure_writer`'s shape (`crate::schema::ensure_writer_sql`): created as
/// the owner, checked to be the owner's, then `USAGE, CREATE` to the writer —
/// who never owns it, so cannot drop it.
fn create_sql(
    schema: &SandboxSchema,
    owner_role: &str,
    writer_role: &str,
) -> Result<Vec<String>, SandboxSchemaError> {
    crate::schema::validate_name(owner_role).map_err(statement("name the owner"))?;
    crate::schema::validate_name(writer_role).map_err(statement("name the writer"))?;
    let name = quote_ident(schema.name());
    Ok(vec![
        format!("DROP SCHEMA IF EXISTS {name} CASCADE"),
        format!(
            "CREATE SCHEMA {name} AUTHORIZATION {}",
            quote_ident(owner_role)
        ),
        crate::roles::assert_schema_owned_sql(schema.name(), owner_role)
            .map_err(statement("check the schema's owner"))?,
        format!(
            "GRANT USAGE, CREATE ON SCHEMA {name} TO {}",
            quote_ident(writer_role)
        ),
    ])
}

fn statement<E: std::fmt::Display>(step: &'static str) -> impl Fn(E) -> SandboxSchemaError {
    move |e| SandboxSchemaError::Statement {
        step,
        message: e.to_string(),
    }
}

/// `writer`'s role on this tenant — production's row is the authority on
/// whether the writer exists, as it is for the branch resolver.
async fn writer_role(
    db: &DatabaseConnection,
    owner: &BranchOwner,
    org_id: Uuid,
    writer: &WriterRef,
) -> Result<String, SandboxSchemaError> {
    let role = crate::schema::qualify_role(
        &owner.tenant.provider,
        &owner.tenant.database_name,
        &writer.role_name(GrantLevel::ReadWrite),
    );
    let provisioned = OltpRoles::find()
        .filter(oltp_roles::Column::TenantRowId.eq(owner.tenant.id))
        .filter(oltp_roles::Column::RoleName.eq(role.clone()))
        .one(db)
        .await?;
    match provisioned {
        Some(_) => Ok(role),
        None => Err(ResolveError::WriterNotProvisioned {
            org_id,
            writer: writer.to_string(),
        }
        .into()),
    }
}

/// Refuse `schema` when it is a provisioned writer's own (`oltp_roles`, the
/// org's record of which schemas are writers'). Only a legacy slug holding
/// `--` derives one; a sandbox of that name has no OLTP schema.
async fn refuse_a_writers_schema(
    db: &DatabaseConnection,
    org_id: Uuid,
    schema: &SandboxSchema,
) -> Result<(), SandboxSchemaError> {
    let (Some(tenant), _) = crate::branches::find(db, org_id, OltpBranch::Staging).await? else {
        return Ok(());
    };
    let taken = OltpRoles::find()
        .filter(oltp_roles::Column::TenantRowId.eq(tenant.id))
        .filter(oltp_roles::Column::SchemaName.eq(schema.name()))
        .one(db)
        .await?;
    match taken {
        Some(_) => Err(SandboxSchemaError::IsAWriters {
            schema: schema.name().to_string(),
        }),
        None => Ok(()),
    }
}

async fn run_as_owner(
    owner: &BranchOwner,
    step: &'static str,
    statements: &[String],
) -> Result<(), SandboxSchemaError> {
    let client = crate::connect::connect(&owner.dsn, "sandbox schema DDL")
        .await
        .map_err(|e| SandboxSchemaError::Connect(crate::connect::pg_detail(&e)))?;
    for sql in statements {
        client
            .batch_execute(sql)
            .await
            .map_err(|e| SandboxSchemaError::Statement {
                step,
                message: crate::connect::pg_detail(&e),
            })?;
    }
    Ok(())
}

async fn exists(owner: &BranchOwner, schema: &SandboxSchema) -> Result<bool, SandboxSchemaError> {
    let client = crate::connect::connect(&owner.dsn, "sandbox schema check")
        .await
        .map_err(|e| SandboxSchemaError::Connect(crate::connect::pg_detail(&e)))?;
    let found = client
        .query_opt(
            "SELECT 1 FROM pg_namespace WHERE nspname = $1",
            &[&schema.name()],
        )
        .await
        .map_err(|e| SandboxSchemaError::Statement {
            step: "look the sandbox schema up",
            message: crate::connect::pg_detail(&e),
        })?;
    Ok(found.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str) -> WriterRef {
        WriterRef::app(name).expect("a writer")
    }

    #[test]
    fn the_name_is_the_app_schema_and_the_sandbox_label() {
        let schema = SandboxSchema::for_writer(&app("store_ops"), "dev_a1").expect("a name");
        assert_eq!(schema.name(), "app_store_ops__dev_a1");
        assert_eq!(schema.app_schema(), "app_store_ops");
        assert_eq!(schema.to_string(), "app_store_ops__dev_a1");
    }

    /// Two sandboxes of one app, and one handle under two apps, never share a
    /// name: neither a writer nor a label holds the separator, so the first
    /// `__` splits a name one way only.
    #[test]
    fn distinct_apps_and_sandboxes_get_distinct_names() {
        let names: std::collections::HashSet<String> = [
            ("store", "dev_a1"),
            ("store", "dev_a_1"),
            ("store_dev", "dev_a1"),
            ("store", "dev_a1_b"),
            ("store_a1", "dev_b"),
        ]
        .iter()
        .map(|(writer, label)| {
            SandboxSchema::for_writer(&app(writer), label)
                .expect("a name")
                .name()
                .to_string()
        })
        .collect();
        assert_eq!(names.len(), 5);
    }

    /// A legacy slug holding `--` derives a writer that holds the separator:
    /// its own schema reads as another app's sandbox, so it gets none.
    #[test]
    fn an_app_schema_holding_the_separator_has_no_sandbox_schema() {
        let err = SandboxSchema::for_writer(&app("store__dev_a1"), "dev_b").unwrap_err();
        assert!(
            matches!(&err, SandboxSchemaError::LegacyAppSchema(s) if s == "app_store__dev_a1"),
            "{err}"
        );
    }

    #[test]
    fn only_an_app_has_sandbox_schemas() {
        let pipeline = WriterRef::pipeline("toast").expect("a writer");
        let err = SandboxSchema::for_writer(&pipeline, "dev_a1").unwrap_err();
        assert!(matches!(err, SandboxSchemaError::NotAnApp(_)), "{err}");
    }

    #[test]
    fn a_label_that_could_hold_a_second_separator_or_needs_quoting_is_refused() {
        for label in [
            "", "dev-a1", "Dev_a1", "_dev", "dev_", "dev__a1", "1dev", "a b",
        ] {
            let err = SandboxSchema::for_writer(&app("store"), label).unwrap_err();
            assert!(
                matches!(err, SandboxSchemaError::BadLabel(_)),
                "{label:?}: {err}"
            );
        }
    }

    /// 63 bytes is the longest name; one more is refused, never cut — the
    /// same bound the Airhouse sibling has (a 41-character writer with a
    /// 12-character handle).
    #[test]
    fn a_name_over_the_identifier_limit_is_refused_not_truncated() {
        let label = "dev_abcdefghijkl";
        assert_eq!(label.len(), 16);
        let fits = "a".repeat(MAX_SCHEMA_LEN - "app_".len() - SEPARATOR.len() - label.len());
        assert_eq!(fits.len(), 41);
        let schema = SandboxSchema::for_writer(&app(&fits), label).expect("63 bytes fits");
        assert_eq!(schema.name().len(), MAX_SCHEMA_LEN);

        let over = "a".repeat(fits.len() + 1);
        let err = SandboxSchema::for_writer(&app(&over), label).unwrap_err();
        assert!(
            matches!(&err, SandboxSchemaError::TooLong { len: 64, .. }),
            "{err}"
        );
        assert!(err.to_string().contains("never truncated"), "{err}");
    }

    /// The owner's statements, in the order that matters: nothing under the
    /// name survives, the schema is the owner's, and the writer gets `USAGE,
    /// CREATE` on it and nothing else.
    #[test]
    fn the_create_statements_drop_first_and_grant_only_the_writer() {
        let schema = SandboxSchema::for_writer(&app("store"), "dev_a1").expect("a name");
        let sql = create_sql(&schema, "oxy_owner", "app_store_rw").expect("statements");
        assert_eq!(
            sql[0],
            r#"DROP SCHEMA IF EXISTS "app_store__dev_a1" CASCADE"#
        );
        assert_eq!(
            sql[1],
            r#"CREATE SCHEMA "app_store__dev_a1" AUTHORIZATION "oxy_owner""#
        );
        assert!(sql[2].contains("OXY02"), "the ownership check: {}", sql[2]);
        assert_eq!(
            sql[3],
            r#"GRANT USAGE, CREATE ON SCHEMA "app_store__dev_a1" TO "app_store_rw""#
        );
        assert_eq!(sql.len(), 4);
    }

    #[test]
    fn a_role_name_that_needs_escaping_is_refused_before_any_sql() {
        let schema = SandboxSchema::for_writer(&app("store"), "dev_a1").expect("a name");
        assert!(create_sql(&schema, "oxy\"; DROP", "app_store_rw").is_err());
        assert!(create_sql(&schema, "oxy_owner", "app store").is_err());
    }
}
