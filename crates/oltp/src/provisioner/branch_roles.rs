//! Who may touch what inside a branch — `credentials.rs`'s job, one database
//! over.
//!
//! A Neon branch arrives holding every role production had, with production's
//! password hashes. Leaving them would make every production credential open
//! staging and every staging credential open production, so each role Oxy
//! hands out (the analyst and every writer) is re-minted on the branch and its
//! new password sealed in `oltp_branch_roles`. `LocalProvider` is the
//! exception: its roles are cluster-global, so there is no branch-only copy to
//! re-mint, and resetting one would rotate production's.
//!
//! Everything here runs as the branch's own owner against the branch database.

use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter};
use tracing::{info, warn};
use uuid::Uuid;

use crate::entity::branch_roles::{self as branch_roles, Entity as BranchRoles};
use crate::entity::branches as oltp_branches;
use crate::entity::roles::WriterKind;
use crate::entity::roles::{self as oltp_roles, Entity as OltpRoles, GrantLevel as DbGrantLevel};
use crate::entity::tenants as oltp_tenants;
use crate::schema::{self, GrantLevel, WriterRef};

use super::{OltpProvisioner, ProvisionerError, open, seal, sslmode_for};

impl OltpProvisioner {
    /// Mint every missing branch credential: the analyst's and each writer's.
    ///
    /// Only the MISSING ones — see `provision_branch` — and none at all where
    /// the branch shares the tenant's roles.
    pub(super) async fn mint_branch_credentials(
        &self,
        tenant: &oltp_tenants::Model,
        row: &oltp_branches::Model,
    ) -> Result<(), ProvisionerError> {
        if schema::shares_role_namespace(&tenant.provider) {
            return Ok(());
        }
        let owner_dsn = self.branch_owner_dsn(tenant, row)?;
        let minted: Vec<String> = BranchRoles::find()
            .filter(branch_roles::Column::BranchRowId.eq(row.id))
            .all(&self.db)
            .await?
            .into_iter()
            .map(|r| r.role_name)
            .collect();

        for role in self.roles_on_branch(tenant).await? {
            if minted.contains(&role) {
                continue;
            }
            let password = crate::roles::generate_password();
            // Create-or-reset, prove it confined, let it in — the three steps
            // `mint_role` takes on production, on the branch's connection.
            let mut batch = crate::roles::ensure_login_role_sql(&role, &password)?;
            batch.push(crate::roles::assert_confined_sql(&role)?);
            batch.push(crate::roles::grant_connect_sql(&role)?);
            self.sql.execute_batch(&owner_dsn, &batch).await?;

            branch_roles::ActiveModel {
                id: ActiveValue::Set(Uuid::new_v4()),
                branch_row_id: ActiveValue::Set(row.id),
                role_name: ActiveValue::Set(role.clone()),
                password_ciphertext: ActiveValue::Set(seal(&password)?),
                created_at: ActiveValue::Set(chrono::Utc::now().into()),
            }
            .insert(&self.db)
            .await?;
            info!(role = %role, "minted OLTP branch credential");
        }
        Ok(())
    }

    /// Give every writer production has its schema, grants and `CONNECT` on
    /// the branch, and point the branch's default `search_path` at them.
    ///
    /// A no-op on a fresh cut, which copied all of it. What it is for is the
    /// writer provisioned AFTER the cut: without it that app's staging has a
    /// login on the branch and nowhere to write, and the only other way in is a
    /// reset that throws away every other app's staging data. All idempotent.
    pub(super) async fn converge_branch_writers(
        &self,
        tenant: &oltp_tenants::Model,
        row: &oltp_branches::Model,
    ) -> Result<(), ProvisionerError> {
        let owner_dsn = self.branch_owner_dsn(tenant, row)?;
        let writers = OltpRoles::find()
            .filter(oltp_roles::Column::TenantRowId.eq(tenant.id))
            .all(&self.db)
            .await?;
        let mut schemas = Vec::with_capacity(writers.len());
        for w in &writers {
            let writer = match w.writer_kind {
                WriterKind::App => WriterRef::app(&w.writer_name),
                WriterKind::Pipeline => WriterRef::pipeline(&w.writer_name),
            }?;
            let grant = match w.grant_level {
                DbGrantLevel::ReadWrite => GrantLevel::ReadWrite,
                DbGrantLevel::ReadOnly => GrantLevel::ReadOnly,
            };
            let mut batch =
                schema::ensure_writer_sql(&writer, grant, &row.owner_role, &w.role_name)?;
            if matches!(writer, WriterRef::Pipeline(_)) {
                batch.push(crate::roles::grant_schema_creation_sql(&w.role_name)?);
            }
            batch.push(crate::roles::grant_connect_sql(&w.role_name)?);
            self.sql.execute_batch(&owner_dsn, &batch).await?;
            schemas.push(w.schema_name.clone());
        }
        schemas.sort();
        schemas.dedup();
        let search_path = schema::database_search_path_sql(&row.database_name, &schemas)?;
        self.sql.execute_batch(&owner_dsn, &search_path).await?;
        Ok(())
    }

    /// Drop one writer's schema — and, where the branch has its own copy, its
    /// role — on every branch of the tenant. `deprovision_writer`'s first step.
    ///
    /// Two reasons, one per provider. On Neon the branch holds its own copy of
    /// the app's staging data and role, which a release of the app must not
    /// leave behind until some later reset. On a shared cluster the role is
    /// production's, and the branch database's grants and objects are
    /// dependencies of it: production's `DROP ROLE` fails on them ("objects
    /// depend on it … in database <branch>") until they are gone. So there only
    /// the per-database half runs here, and production's drop finishes the role.
    pub(super) async fn drop_writer_on_branches(
        &self,
        tenant: &oltp_tenants::Model,
        role_name: &str,
        drop_schema: &[String],
        plan: &crate::roles::RoleDropPlan,
    ) -> Result<(), ProvisionerError> {
        for kind in self.branch_kinds(tenant).await? {
            // Under the branch's lock, on the row as it is once held: a reset
            // in flight finishes first — re-cut from a production that still
            // has this writer, since production's drop comes after this step —
            // and the drop lands on the new copy instead of racing its
            // credential step.
            let lock = self.lock_branch(tenant.id, kind).await?;
            let out = match self.find_branch(tenant, kind).await {
                Ok(Some(row)) => {
                    self.drop_writer_on_branch(tenant, &row, role_name, drop_schema, plan)
                        .await
                }
                Ok(None) => Ok(()),
                Err(e) => Err(e),
            };
            lock.release().await;
            out?;
        }
        Ok(())
    }

    async fn drop_writer_on_branch(
        &self,
        tenant: &oltp_tenants::Model,
        row: &oltp_branches::Model,
        role_name: &str,
        drop_schema: &[String],
        plan: &crate::roles::RoleDropPlan,
    ) -> Result<(), ProvisionerError> {
        let shared = schema::shares_role_namespace(&tenant.provider);
        // A branch a failed reset or provision left mid-way, on a provider with
        // its own roles: finishing it re-cuts (or completes) it from a
        // production that will no longer have this writer, so skip it rather
        // than let a broken branch block the release. Where roles are shared
        // the drop below is what lets production's DROP ROLE succeed at all,
        // so there it stays strict.
        if !shared && row.status != crate::entity::branches::BranchStatus::Active {
            warn!(
                branch = %row.kind,
                status = row.status.as_str(),
                role = %role_name,
                "skipping the writer drop on a branch that is not active"
            );
            return self.forget_branch_credential(row, role_name).await;
        }
        let branch_dsn = self.branch_owner_dsn(tenant, row)?;
        // Membership first, on whichever connection can grant it: the cluster
        // admin where roles are shared, the branch's own owner where the
        // branch is its own cluster.
        let admin_dsn = match self.provider.role_admin_dsn() {
            Some(admin) if shared => admin,
            _ => branch_dsn.clone(),
        };
        // Tolerant: a writer provisioned after the cut may have no role or
        // schema on a branch nobody re-provisioned, and "already absent" is
        // exactly what this step wants.
        let tolerant = |s: &[String]| super::credentials::tolerate_missing(s.to_vec());
        self.sql
            .execute_batch(&admin_dsn, &tolerant(&plan.admin_pre))
            .await?;
        self.sql
            .execute_batch(&branch_dsn, &tolerant(drop_schema))
            .await?;
        self.sql
            .execute_batch(&branch_dsn, &tolerant(&plan.tenant))
            .await?;
        if !shared {
            self.sql
                .execute_batch(&branch_dsn, &tolerant(&plan.admin_post))
                .await?;
        }
        self.forget_branch_credential(row, role_name).await?;
        info!(branch = %row.kind, role = %role_name, "dropped OLTP writer on branch");
        Ok(())
    }

    async fn forget_branch_credential(
        &self,
        row: &oltp_branches::Model,
        role_name: &str,
    ) -> Result<(), ProvisionerError> {
        BranchRoles::delete_many()
            .filter(branch_roles::Column::BranchRowId.eq(row.id))
            .filter(branch_roles::Column::RoleName.eq(role_name))
            .exec(&self.db)
            .await?;
        Ok(())
    }

    /// The analyst, then every writer, by their real (qualified) names — the
    /// names the resolver looks a branch credential up by.
    async fn roles_on_branch(
        &self,
        tenant: &oltp_tenants::Model,
    ) -> Result<Vec<String>, ProvisionerError> {
        let mut roles = vec![schema::analyst_role_for(
            &tenant.provider,
            &tenant.database_name,
        )];
        roles.extend(
            OltpRoles::find()
                .filter(oltp_roles::Column::TenantRowId.eq(tenant.id))
                .all(&self.db)
                .await?
                .into_iter()
                .map(|r| r.role_name),
        );
        Ok(roles)
    }

    /// The branch owner's DSN: the branch's sealed password where it has one,
    /// the tenant's where the branch shares the tenant's owner.
    fn branch_owner_dsn(
        &self,
        tenant: &oltp_tenants::Model,
        row: &oltp_branches::Model,
    ) -> Result<String, ProvisionerError> {
        let password = match &row.owner_password_ciphertext {
            Some(sealed) => open(sealed)?,
            None if schema::shares_role_namespace(&tenant.provider) => {
                self.owner_password(tenant, tenant.org_id)?
            }
            None => {
                return Err(ProvisionerError::BranchOwnerPasswordMissing(
                    tenant.org_id,
                    row.kind,
                ));
            }
        };
        Ok(branch_dsn(
            &tenant.provider,
            row,
            &row.owner_role,
            &password,
        ))
    }
}

/// [`super::dsn_for`]'s shape, aimed at the branch's endpoint and database.
pub(crate) fn branch_dsn(
    provider: &str,
    row: &oltp_branches::Model,
    role: &str,
    password: &str,
) -> String {
    format!(
        "postgres://{role}:{password}@{host}/{db}?sslmode={ssl}",
        password = crate::roles::encode_userinfo(password),
        host = row.host,
        db = row.database_name,
        ssl = sslmode_for(provider),
    )
}
