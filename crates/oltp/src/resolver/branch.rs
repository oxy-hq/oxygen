//! Connections to an org's staging branch — what staging's `ctx.oltp` (Phase
//! 4b) resolves instead of production's.
//!
//! The same two shapes as production, [`WriterConnection`] and
//! [`AnalystConnection`], with the same role names; only the endpoint, the
//! database and the password differ. **`Ok(None)` means the org has no such
//! branch** — the caller's cue to keep production read-only rather than to
//! write anywhere — while every other gap is a typed error naming its fix.
//!
//! Like the production resolvers this is a query and a decrypt: no provider
//! call on a request path. Production resolution does not pass through here
//! and is unchanged.

use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;

use super::{AnalystConnection, ResolveError, WriterConnection, open, split_host_port};
use crate::OltpBranch;
use crate::entity::branch_roles::{self as branch_roles, Entity as BranchRoles};
use crate::entity::branches::{self as oltp_branches, BranchStatus};
use crate::entity::roles::{self as oltp_roles, Entity as OltpRoles};
use crate::entity::tenants::{self as oltp_tenants, TenantStatus};
use crate::schema::{self, GrantLevel, WriterRef};

// Whether a row names production, by id or by endpoint in any spelling.
mod guard;

/// A writable connection for `writer` on the org's `branch`.
///
/// `Ok(None)`: the org has an OLTP database but no such branch. Errors match
/// production's where the cause is production's (no tenant, tenant not active,
/// writer never provisioned), plus the two a branch adds —
/// [`ResolveError::BranchNotActive`] and
/// [`ResolveError::BranchCredentialMissing`].
pub async fn resolve_branch_writer_connection_for_org(
    db: &DatabaseConnection,
    org_id: Uuid,
    branch: OltpBranch,
    writer: &WriterRef,
) -> Result<Option<WriterConnection>, ResolveError> {
    Ok(resolve_branch_writer_for_org(db, org_id, branch, writer)
        .await?
        .map(|b| b.connection))
}

/// A writer's connection on a branch, and which branch it is — read from the
/// one row the connection was built from.
#[derive(Debug, Clone)]
pub struct BranchWriter {
    pub connection: WriterConnection,
    /// The cut the connection reaches. Its provider id is what the branch's
    /// migration ledger is keyed by (`crate::branches::ledger_target`); the
    /// whole cut is what a recorder re-checks (`crate::branches::still_current`).
    pub cut: crate::branches::BranchCut,
}

/// [`resolve_branch_writer_connection_for_org`], plus the branch's provider
/// id, so a caller recording what it applied names the branch it connected to
/// — not one a reset re-cut in between two reads (the staging migration
/// apply, previews P4b).
pub async fn resolve_branch_writer_for_org(
    db: &DatabaseConnection,
    org_id: Uuid,
    branch: OltpBranch,
    writer: &WriterRef,
) -> Result<Option<BranchWriter>, ResolveError> {
    resolve_branch_writer_in_schema(db, org_id, branch, writer, &writer.schema_name()).await
}

/// [`resolve_branch_writer_for_org`], with `schema` as the connection's one
/// `search_path` entry instead of the writer's own schema — a sandbox's
/// schema on the branch (`crate::sandbox_schema`), which is the only caller
/// that passes anything else. The role, the endpoint and the password are the
/// writer's on the branch either way.
pub(crate) async fn resolve_branch_writer_in_schema(
    db: &DatabaseConnection,
    org_id: Uuid,
    branch: OltpBranch,
    writer: &WriterRef,
    schema: &str,
) -> Result<Option<BranchWriter>, ResolveError> {
    let Some((tenant, row)) = active_branch(db, org_id, branch).await? else {
        return Ok(None);
    };
    // The same derivation production resolves with — the role NAME does not
    // change on a branch, only where it logs in and with what.
    let role = schema::qualify_role(
        &tenant.provider,
        &tenant.database_name,
        &writer.role_name(GrantLevel::ReadWrite),
    );
    // Production's row stays the authority on whether the writer exists.
    let production = OltpRoles::find()
        .filter(oltp_roles::Column::TenantRowId.eq(tenant.id))
        .filter(oltp_roles::Column::RoleName.eq(role.clone()))
        .one(db)
        .await?
        .ok_or_else(|| ResolveError::WriterNotProvisioned {
            org_id,
            writer: writer.to_string(),
        })?;
    let password =
        branch_password(db, &tenant, &row, &role, &production.password_ciphertext).await?;

    let (host, port) = split_host_port(&row.host);
    let base = format!(
        "postgres://{role}:{password}@{host}:{port}/{db}?sslmode={ssl}",
        password = crate::roles::encode_userinfo(&password),
        db = row.database_name,
        ssl = crate::provisioner::sslmode_for(&tenant.provider),
    );
    Ok(Some(BranchWriter {
        connection: WriterConnection {
            schema: schema.to_string(),
            role,
            dsn: schema::with_search_path_to(&base, schema),
            verify_tls: crate::provisioner::verify_tls_for(&tenant.provider),
        },
        cut: crate::branches::BranchCut::of(&row),
    }))
}

/// The read-only analyst on the org's `branch` — what a staging read of the
/// app's own OLTP database resolves to (env design §4.2: read-after-write in
/// staging must see staging's writes). `Ok(None)` as above.
pub async fn resolve_branch_analyst_connection_for_org(
    db: &DatabaseConnection,
    org_id: Uuid,
    branch: OltpBranch,
) -> Result<Option<AnalystConnection>, ResolveError> {
    let Some((tenant, row)) = active_branch(db, org_id, branch).await? else {
        return Ok(None);
    };
    let role = schema::analyst_role_for(&tenant.provider, &tenant.database_name);
    let production = tenant
        .analyst_password_ciphertext
        .as_ref()
        .ok_or(ResolveError::NoAnalystCredential(org_id))?;
    let password = branch_password(db, &tenant, &row, &role, production).await?;

    let (host, port) = split_host_port(&row.host);
    Ok(Some(AnalystConnection {
        host,
        port,
        database: row.database_name.clone(),
        user: role,
        password,
        sslmode: crate::provisioner::sslmode_for(&tenant.provider).to_string(),
        verify_tls: crate::provisioner::verify_tls_for(&tenant.provider),
    }))
}

/// The owner's connection to an org's `branch`, and what it reaches — for the
/// DDL only the database owner may run there (a sandbox's schema,
/// `crate::sandbox_schema`). Never handed to app code.
pub(crate) struct BranchOwner {
    pub dsn: String,
    pub owner_role: String,
    pub tenant: oltp_tenants::Model,
    pub cut: crate::branches::BranchCut,
}

/// The owner of the org's active `branch`. `Ok(None)`: the org has no such
/// branch. Refuses a branch that is not active or that names production, as
/// every resolver here does.
pub(crate) async fn resolve_branch_owner(
    db: &DatabaseConnection,
    org_id: Uuid,
    branch: OltpBranch,
) -> Result<Option<BranchOwner>, ResolveError> {
    let Some((tenant, row)) = active_branch(db, org_id, branch).await? else {
        return Ok(None);
    };
    // The branch's own sealed owner password where it has one (Neon); the
    // tenant's where the branch shares the tenant's roles (a local cluster).
    let sealed = match &row.owner_password_ciphertext {
        Some(sealed) => sealed,
        None if schema::shares_role_namespace(&tenant.provider) => tenant
            .owner_password_ciphertext
            .as_ref()
            .ok_or(ResolveError::BranchOwnerCredentialMissing(org_id, branch))?,
        None => return Err(ResolveError::BranchOwnerCredentialMissing(org_id, branch)),
    };
    let password = open(sealed)?;
    Ok(Some(BranchOwner {
        dsn: crate::provisioner::branch_dsn(&tenant.provider, &row, &row.owner_role, &password),
        owner_role: row.owner_role.clone(),
        cut: crate::branches::BranchCut::of(&row),
        tenant,
    }))
}

/// Whether the org's `branch` exists and is serving — the cheap check a caller
/// makes to choose between the branch and production-read-only. No decrypt.
pub async fn branch_is_active(
    db: &DatabaseConnection,
    org_id: Uuid,
    branch: OltpBranch,
) -> Result<bool, ResolveError> {
    let (_, row) = crate::branches::find(db, org_id, branch).await?;
    Ok(row.is_some_and(|r| r.status == BranchStatus::Active))
}

/// The active tenant and its active branch; `None` when there is no branch.
async fn active_branch(
    db: &DatabaseConnection,
    org_id: Uuid,
    branch: OltpBranch,
) -> Result<Option<(oltp_tenants::Model, oltp_branches::Model)>, ResolveError> {
    let (tenant, row) = crate::branches::find(db, org_id, branch).await?;
    let tenant = tenant.ok_or(ResolveError::NotProvisioned(org_id))?;
    if tenant.status != TenantStatus::Active {
        return Err(ResolveError::NotActive(org_id, tenant.status.as_str()));
    }
    let Some(row) = row else {
        return Ok(None);
    };
    // Checked on every resolve, not only where rows are written: this is the
    // last point before a staging caller holds a connection.
    if guard::names_production(&tenant, &row) {
        return Err(ResolveError::BranchIsProduction(org_id, branch));
    }
    if row.status != BranchStatus::Active {
        return Err(ResolveError::BranchNotActive(
            org_id,
            branch,
            row.status.as_str(),
        ));
    }
    Ok(Some((tenant, row)))
}

/// A role's password ON the branch.
///
/// Its own sealed row where the branch has its own copy of the role (Neon).
/// Production's where the provider shares one role namespace across databases
/// (`LocalProvider`): there the role IS production's, and so is its password.
async fn branch_password(
    db: &DatabaseConnection,
    tenant: &oltp_tenants::Model,
    row: &oltp_branches::Model,
    role: &str,
    production_sealed: &[u8],
) -> Result<String, ResolveError> {
    if schema::shares_role_namespace(&tenant.provider) {
        return open(production_sealed);
    }
    let found = BranchRoles::find()
        .filter(branch_roles::Column::BranchRowId.eq(row.id))
        .filter(branch_roles::Column::RoleName.eq(role))
        .one(db)
        .await?
        .ok_or_else(|| ResolveError::BranchCredentialMissing {
            org_id: tenant.org_id,
            branch: row.kind,
            role: role.to_string(),
        })?;
    open(&found.password_ciphertext)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both branch errors must name the command that fixes them — they reach
    /// a staging function's author through `ctx.oltp`'s error, far from here.
    #[test]
    fn branch_errors_name_their_fix() {
        let not_active =
            ResolveError::BranchNotActive(Uuid::nil(), OltpBranch::Staging, "resetting");
        assert!(not_active.to_string().contains("status: resetting"));
        assert!(
            not_active.to_string().contains(
                "oxyc oltp reset --org 00000000-0000-0000-0000-000000000000 --branch staging"
            ),
            "a reset in flight is finished by a reset, not a provision: {not_active}"
        );
        let half_made =
            ResolveError::BranchNotActive(Uuid::nil(), OltpBranch::Staging, "provisioning");
        assert!(
            half_made.to_string().contains("oxyc oltp provision"),
            "{half_made}"
        );
        let missing = ResolveError::BranchCredentialMissing {
            org_id: Uuid::nil(),
            branch: OltpBranch::Staging,
            role: "app_x_rw".into(),
        };
        assert!(missing.to_string().contains("app_x_rw"), "{missing}");
        assert!(
            missing.to_string().contains("oxyc oltp provision"),
            "{missing}"
        );
    }
}
