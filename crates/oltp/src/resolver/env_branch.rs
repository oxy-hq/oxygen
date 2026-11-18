//! The writer connection for one custom-app environment's own OLTP branch —
//! what a sandbox's `ctx.oltp` resolves instead of staging's or production's.
//!
//! Mirrors [`super::resolve_branch_writer_in_schema`] against
//! `oltp_env_branches`: a query and a decrypt, no provider call on a request
//! path. **`Ok(None)` means the environment has no branch row** — the caller's
//! cue to fall back to whatever it did before — while every other gap is a
//! typed error. Production must never be reachable from here: a row that names
//! it is refused (`EnvBranchIsProduction`), and a row not yet cut is not ready,
//! never production.

use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;

use super::branch::{BranchOwner, BranchWriter, guard};
use super::{ResolveError, WriterConnection, open, split_host_port};
use crate::branches::BranchCut;
use crate::entity::env_branches::{self, EnvBranchStatus};
use crate::entity::roles::{self as oltp_roles, Entity as OltpRoles};
use crate::entity::tenants::{self as oltp_tenants, TenantStatus};
use crate::env_branches::EnvBranchKey;
use crate::provisioner::BranchSite;
use crate::schema::{self, GrantLevel, WriterRef};

/// Which branch states a caller is served in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvBranchAccept {
    /// A function call: only a branch that is cut, minted and migrated.
    Ready,
    /// The migration apply, which runs while the branch is `migrating`.
    ReadyOrMigrating,
}

impl EnvBranchAccept {
    fn serves(self, status: EnvBranchStatus) -> bool {
        match status {
            EnvBranchStatus::Ready => true,
            EnvBranchStatus::Migrating => self == EnvBranchAccept::ReadyOrMigrating,
            EnvBranchStatus::Cutting | EnvBranchStatus::Resetting | EnvBranchStatus::Failed => {
                false
            }
        }
    }
}

/// A writable connection for `writer` on the environment's own branch, and the
/// cut it reaches.
///
/// Errors match production's where the cause is production's (no tenant,
/// tenant not active, writer never provisioned), plus the three an environment
/// branch adds: [`ResolveError::EnvBranchNotReady`],
/// [`ResolveError::EnvBranchIsProduction`] and
/// [`ResolveError::EnvBranchCredentialMissing`].
pub async fn resolve_env_branch_writer_for_org(
    db: &DatabaseConnection,
    org_id: Uuid,
    app_id: Uuid,
    env_name: &str,
    writer: &WriterRef,
    accept: EnvBranchAccept,
) -> Result<Option<BranchWriter>, ResolveError> {
    let key = EnvBranchKey {
        org_id,
        app_id,
        env_name: env_name.to_string(),
    };
    let (tenant, row) = crate::env_branches::find_with_tenant(db, &key).await?;
    let tenant = tenant.ok_or(ResolveError::NotProvisioned(org_id))?;
    if tenant.status != TenantStatus::Active {
        return Err(ResolveError::NotActive(org_id, tenant.status.as_str()));
    }
    let Some(row) = row else { return Ok(None) };
    let (site, cut) = servable(&tenant, &row, accept, &key)?;

    // The role NAME does not change on a branch, only where it logs in.
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
    let password = role_password(db, &tenant, &site, &key, &role, &production).await?;

    let (host, port) = split_host_port(&site.host);
    let base = format!(
        "postgres://{role}:{password}@{host}:{port}/{db}?sslmode={ssl}",
        password = crate::roles::encode_userinfo(&password),
        db = site.database_name,
        ssl = crate::provisioner::sslmode_for(&tenant.provider),
    );
    let schema = writer.schema_name();
    Ok(Some(BranchWriter {
        connection: WriterConnection {
            dsn: schema::with_search_path_to(&base, &schema),
            schema,
            role,
            verify_tls: crate::provisioner::verify_tls_for(&tenant.provider),
        },
        cut,
    }))
}

/// The owner's connection to `app_id`'s `env_name` branch, for the events
/// outbox there (a grant, the relay's claim, a body read or delete), and the
/// cut it reaches. Never handed to app code.
///
/// `Ok(None)`: the environment has no branch row, or one not serving now —
/// being cut, mid-reset or failed, whose database may be going away. A branch
/// that serves (`ready`, or `migrating` while a publish applies its
/// migrations) is reached; one that names production is refused, as every
/// resolver here refuses it.
pub(crate) async fn resolve_env_branch_owner(
    db: &DatabaseConnection,
    org_id: Uuid,
    app_id: Uuid,
    env_name: &str,
) -> Result<Option<BranchOwner>, ResolveError> {
    let key = EnvBranchKey {
        org_id,
        app_id,
        env_name: env_name.to_string(),
    };
    let (tenant, row) = crate::env_branches::find_with_tenant(db, &key).await?;
    let tenant = tenant.ok_or(ResolveError::NotProvisioned(org_id))?;
    if tenant.status != TenantStatus::Active {
        return Err(ResolveError::NotActive(org_id, tenant.status.as_str()));
    }
    let Some(row) = row else { return Ok(None) };
    let (site, cut) = match servable(&tenant, &row, EnvBranchAccept::ReadyOrMigrating, &key) {
        Ok(found) => found,
        Err(ResolveError::EnvBranchNotReady { .. }) => return Ok(None),
        Err(e) => return Err(e),
    };
    let missing = || ResolveError::EnvBranchCredentialMissing {
        app_id,
        env_name: env_name.to_string(),
        role: site.owner_role.clone(),
    };
    // The branch's own sealed owner password where it has one (Neon); the
    // tenant's where branches share the tenant's roles (a local cluster).
    let sealed = match &site.owner_password_ciphertext {
        Some(sealed) => sealed.clone(),
        None if schema::shares_role_namespace(&tenant.provider) => tenant
            .owner_password_ciphertext
            .clone()
            .ok_or_else(missing)?,
        None => return Err(missing()),
    };
    let password = open(&sealed)?;
    Ok(Some(BranchOwner {
        dsn: crate::provisioner::branch_dsn(&tenant.provider, &site, &site.owner_role, &password),
        owner_role: site.owner_role.clone(),
        platform_schema_version: version_on_cut(&row, &cut),
        cut,
        tenant,
    }))
}

/// The platform version `row` records, trusted only on the cut it was
/// recorded on, `cut` being the one the row describes now. On any other —
/// a reset since by a binary that does not know the version, which stamps
/// `last_reset_at` and leaves the version alone — the branch is a fresh copy
/// of production at a version nobody read: 0, so its first touch replays
/// every platform step, each idempotent.
fn version_on_cut(row: &env_branches::Model, cut: &BranchCut) -> i32 {
    if row.platform_schema_cut_at == Some(cut.cut_at) {
        row.platform_schema_version
    } else {
        0
    }
}

/// The row's site and cut, or the refusal. Production first: a row that names
/// it is refused whatever its status, as it is the last door before a caller
/// holds a connection.
fn servable(
    tenant: &oltp_tenants::Model,
    row: &env_branches::Model,
    accept: EnvBranchAccept,
    key: &EnvBranchKey,
) -> Result<(BranchSite, BranchCut), ResolveError> {
    let named = |s: &Option<String>| s.clone().unwrap_or_default();
    if guard::names_production(
        tenant,
        &named(&row.provider_branch_id),
        &named(&row.host),
        &named(&row.database_name),
    ) {
        return Err(ResolveError::EnvBranchIsProduction {
            app_id: key.app_id,
            env_name: key.env_name.clone(),
        });
    }
    let not_ready = || ResolveError::EnvBranchNotReady {
        app_id: key.app_id,
        env_name: key.env_name.clone(),
        status: row.status.as_str(),
    };
    if !accept.serves(row.status) {
        return Err(not_ready());
    }
    // A row with a serving status but no cut (the provider has not answered)
    // is not ready — never production.
    match (BranchSite::of_env(row), BranchCut::of_env(row)) {
        (Some(site), Some(cut)) => Ok((site, cut)),
        _ => Err(not_ready()),
    }
}

/// A role's password ON the environment's branch: its own sealed row where the
/// branch has its own copy of the role (Neon), production's where the provider
/// shares one role namespace (`LocalProvider`).
async fn role_password(
    db: &DatabaseConnection,
    tenant: &oltp_tenants::Model,
    site: &BranchSite,
    key: &EnvBranchKey,
    role: &str,
    production: &oltp_roles::Model,
) -> Result<String, ResolveError> {
    if schema::shares_role_namespace(&tenant.provider) {
        return open(&production.password_ciphertext);
    }
    let sealed = site.roles.password_of(db, role).await?.ok_or_else(|| {
        ResolveError::EnvBranchCredentialMissing {
            app_id: key.app_id,
            env_name: key.env_name.clone(),
            role: role.to_string(),
        }
    })?;
    open(&sealed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_ready_branch_serves_a_function_call() {
        use EnvBranchStatus::*;
        for status in [Cutting, Migrating, Resetting, Failed] {
            assert!(!EnvBranchAccept::Ready.serves(status), "{status:?}");
        }
        assert!(EnvBranchAccept::Ready.serves(Ready));
    }

    #[test]
    fn the_migration_apply_is_served_while_migrating_and_nothing_earlier() {
        use EnvBranchStatus::*;
        let accept = EnvBranchAccept::ReadyOrMigrating;
        assert!(accept.serves(Ready) && accept.serves(Migrating));
        for status in [Cutting, Resetting, Failed] {
            assert!(!accept.serves(status), "{status:?}");
        }
    }

    /// A row the outbox recorded current on cut A, in the shape a cut leaves.
    fn recorded_on(cut_at: chrono::DateTime<chrono::FixedOffset>) -> env_branches::Model {
        use crate::entity::env_branches::EnvBranchMode;
        env_branches::Model {
            id: Uuid::from_u128(1),
            tenant_row_id: Uuid::from_u128(2),
            app_id: Uuid::from_u128(3),
            env_name: "dev-a1".into(),
            mode: EnvBranchMode::Branch,
            status: EnvBranchStatus::Ready,
            error: None,
            provider_branch_id: Some("br-1".into()),
            parent_branch_id: "br-0".into(),
            host: Some("h".into()),
            database_name: Some("d".into()),
            owner_role: Some("o".into()),
            owner_password_ciphertext: None,
            fork_at: cut_at,
            expires_at: cut_at,
            created_at: cut_at,
            last_reset_at: None,
            updated_at: cut_at,
            platform_schema_version: crate::platform::PLATFORM_SCHEMA_VERSION,
            platform_schema_cut_at: Some(cut_at),
        }
    }

    /// F3: the version holds on the cut it was recorded on. A reset by an
    /// older binary moves the cut (`last_reset_at`) and leaves the version:
    /// that branch reads as 0, as does one whose version was recorded before
    /// the cut was (`None`), until the outbox records it on the new cut.
    #[test]
    fn a_recorded_version_holds_on_its_own_cut_alone() {
        use chrono::TimeZone as _;
        let a = chrono::Utc
            .with_ymd_and_hms(2026, 11, 18, 9, 0, 0)
            .unwrap()
            .fixed_offset();
        let row = recorded_on(a);
        let cut = |row: &env_branches::Model| BranchCut::of_env(row).expect("a cut");
        assert_eq!(
            version_on_cut(&row, &cut(&row)),
            crate::platform::PLATFORM_SCHEMA_VERSION
        );

        let b = a + chrono::Duration::hours(1);
        let reset = env_branches::Model {
            last_reset_at: Some(b),
            ..row.clone()
        };
        assert_eq!(
            version_on_cut(&reset, &cut(&reset)),
            0,
            "an older binary's reset"
        );
        let recorded_before = env_branches::Model {
            platform_schema_cut_at: None,
            ..row
        };
        assert_eq!(version_on_cut(&recorded_before, &cut(&recorded_before)), 0);
        let recorded_again = env_branches::Model {
            platform_schema_cut_at: Some(b),
            ..reset
        };
        assert_eq!(
            version_on_cut(&recorded_again, &cut(&recorded_again)),
            crate::platform::PLATFORM_SCHEMA_VERSION
        );
    }

    #[test]
    fn the_not_ready_error_names_the_status() {
        let err = ResolveError::EnvBranchNotReady {
            app_id: Uuid::nil(),
            env_name: "dev-a1".into(),
            status: "cutting",
        };
        assert!(err.to_string().contains("status: cutting"), "{err}");
    }
}
