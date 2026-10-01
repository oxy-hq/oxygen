//! Who may touch what inside a tenant database.
//!
//! Split out of `provisioner.rs`, which had grown past 1300 lines doing two
//! jobs at once. The seam is the question you are debugging: **does the tenant
//! exist** (`provisioner.rs` — create the project, reconcile it, record it,
//! destroy it) versus **who may touch what inside it** (here — mint a role,
//! confine it, grant it a schema, publish a schema to analytics).
//!
//! Everything here runs as the tenant's OWNER against the tenant database, and
//! the owner is not a superuser on any provider — several of the comments below
//! are about what that costs.

use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter};
use tracing::{info, instrument, warn};
use uuid::Uuid;

use crate::entity::roles::{
    self as oltp_roles, Entity as OltpRoles, GrantLevel as DbGrantLevel, WriterKind,
};
use crate::entity::tenants::{self as oltp_tenants};
use crate::schema::{self, GrantLevel, WriterRef};
use chrono::Utc;

use super::{
    OltpProvisioner, ProvisionerError, WriterCredentials, dsn_for, is_unconfined, open, seal,
};

/// Whether a teardown statement must find what it is removing.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Strictness {
    /// Surface `3F000` / `42704` — the caller asserted this object exists.
    Strict,
    /// Skip a statement whose schema, role or table is already gone.
    TolerateMissing,
}

/// Wrap statements so a missing object is a no-op rather than an error.
///
/// `REVOKE … ON SCHEMA x FROM y` does not no-op when `x` or `y` is absent:
/// Postgres raises `3F000 schema does not exist` or `42704 role does not
/// exist`. That is correct for an operator withdrawing a grant, and wrong for
/// a best-effort teardown that runs precisely where existence is not
/// guaranteed — a project recreated under the same name with its `oltp_roles`
/// rows intact, or a half-completed re-mint where the role is already gone.
/// Both would otherwise wedge: every retry dies on the same statement, and the
/// recovery path stops being able to recover.
///
/// `EXECUTE` inside the block because `ALTER DEFAULT PRIVILEGES` is a utility
/// statement that PL/pgSQL will not accept inline, and dollar-quoting because
/// the statement carries quoted identifiers of its own.
pub(super) fn tolerate_missing(statements: Vec<String>) -> Vec<String> {
    statements
        .into_iter()
        .map(|stmt| {
            format!(
                "DO $oxy_tol$ BEGIN EXECUTE $oxy_stmt${stmt}$oxy_stmt$; \
                 EXCEPTION WHEN undefined_object OR undefined_table \
                   OR undefined_function OR invalid_schema_name THEN NULL; \
                 END $oxy_tol$"
            )
        })
        .collect()
}

/// Which kind of role [`OltpProvisioner::mint_role`] is minting.
///
/// The two differ only when the role must be DROPPED and re-created. An analyst
/// holds grants spread across every writer schema in the tenant, and those must
/// be released first or the drop is refused; a writer holds none. Passing this
/// keeps the sweep on the one path that needs it instead of running it on every
/// password reset.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RoleClass {
    Analyst,
    Writer,
}

impl OltpProvisioner {
    /// Ensure `writer` has its schema and role, returning connection details.
    ///
    /// Idempotent, and self-healing: if the local row exists but the role was
    /// deleted provider-side, the role is recreated and the sealed password
    /// replaced.
    #[instrument(skip(self), fields(org_id = %org_id, writer = %writer, grant = grant.as_str()))]
    pub async fn ensure_writer(
        &self,
        org_id: Uuid,
        writer: &WriterRef,
        grant: GrantLevel,
        // `None` means "no workspace claims this yet" — an operator
        // provisioning ahead of an app. It is NOT `Uuid::nil()`: that is a
        // workspace id like any other as far as the comparison below is
        // concerned, so storing it locked the namespace to a workspace that
        // will never exist, and the real one then failed
        // `SchemaNamespaceClaimed` against it with no way out but manual SQL.
        claimant: Option<Uuid>,
    ) -> Result<WriterCredentials, ProvisionerError> {
        let tenant = self.active_tenant(org_id).await?;
        // Qualified, because on a shared-cluster provider the bare name is
        // another tenant's role — see `schema::qualify_role`.
        let role_name = schema::qualify_role(
            &tenant.provider,
            &tenant.database_name,
            &writer.role_name(grant),
        );

        // Claim first: refuse before any provider call or DDL, so a losing
        // workspace never half-creates a role in a namespace it does not own.
        self.claim_namespace(&tenant, writer, claimant).await?;

        let existing = OltpRoles::find()
            .filter(oltp_roles::Column::TenantRowId.eq(tenant.id))
            .filter(oltp_roles::Column::RoleName.eq(role_name.clone()))
            .one(&self.db)
            .await?;

        // Whether the analyst should see this writer's schema — computed from
        // `existing` before it is moved, so the two cases match on it once.
        //
        //   * first creation → the kind's default (`raw_*` visible, `app_*` not,
        //     until the app asks; without it a pipeline landed rows the analyst
        //     could not read and `postgres_managed` returned permission denied);
        //   * re-provision → the STORED choice, which restores what a re-mint's
        //     `DROP OWNED` dropped (the writer's `ALTER DEFAULT PRIVILEGES … TO
        //     analyst`). GRANT is idempotent, so a no-op when the grant survived.
        //
        // Only ever GRANTS here; withdrawing stays explicit
        // (`set_analytics_visibility(.., false)`), so a re-provision never
        // re-opens a schema an operator hid.
        let want_visible = match &existing {
            None => writer.analytics_visible_by_default(),
            Some(row) => {
                crate::migrator::effective_visibility(row.analytics_visible, &row.writer_kind)
            }
        };
        let password = match existing {
            Some(row) => self.reconcile_role(&tenant, &row).await?,
            None => self.create_role(&tenant, writer, grant, claimant).await?,
        };

        self.apply_writer_ddl(&tenant, writer, grant).await?;
        self.refresh_database_search_path(&tenant).await?;

        if want_visible {
            self.set_analytics_visibility(org_id, writer, true).await?;
        }

        Ok(WriterCredentials {
            schema_name: writer.schema_name(),
            role_name: role_name.clone(),
            dsn: schema::with_search_path(&dsn_for(&tenant, &role_name, &password), writer),
        })
    }

    /// Drop ONE writer — its `app_<slug>` schema (CASCADE, so its tables go with
    /// it) and its role — leaving the rest of the tenant intact.
    ///
    /// The per-app counterpart to [`OltpProvisioner::deprovision`], which
    /// destroys the org's whole database. This is what makes the app-delete and
    /// rename guards actionable: an operator releases one app's store without
    /// nuking every other app's and pipeline's schema. Idempotent — a no-op if
    /// the writer was never provisioned.
    ///
    /// Both statement sets are built (and their identifiers validated) BEFORE any
    /// DDL runs, so an invalid role name fails cleanly rather than half-dropping.
    #[instrument(skip(self), fields(org_id = %org_id, writer = %writer))]
    pub async fn deprovision_writer(
        &self,
        org_id: Uuid,
        writer: &WriterRef,
    ) -> Result<(), ProvisionerError> {
        // `find_tenant`, NOT `active_tenant`: the escape hatch must work while a
        // tenant is `failed`/`pending_delete` — those are the states an operator
        // untangling a stuck app is most likely in, and the schema exists in them
        // the same way the guard argues it does.
        let Some(tenant) = self.find_tenant(org_id).await? else {
            info!("no OLTP tenant; deprovision_writer is a no-op");
            return Ok(());
        };
        // Same guard as `deprovision`, and load-bearing here for a subtler
        // reason: the two DSNs below come from DIFFERENT sources — `tenant_dsn`
        // from the persisted row (its host/database/provider), `admin_dsn` from
        // the CONFIGURED provider's `role_admin_dsn()`. The connection split is
        // only correct if they name one cluster. A process configured `local`
        // against a `neon`-recorded row would drop the schema on the real Neon
        // tenant and then run the role DDL on the dev cluster (`42704`), leaving
        // the customer's row behind and wedging every retry on the same
        // misconfiguration. `oxy start` defaults `OXY_OLTP_PROVIDER=local`, so
        // this is one unset variable away on a box that also has a Neon org.
        if tenant.provider != self.provider.name() {
            return Err(ProvisionerError::ProviderMismatch {
                org_id,
                recorded: tenant.provider.clone(),
                configured: self.provider.name(),
            });
        }
        // Cluster-global role name, qualified on a shared-cluster provider — must
        // match what `ensure_writer` minted and `writer_is_provisioned` looks up.
        let role_name = schema::qualify_role(
            &tenant.provider,
            &tenant.database_name,
            &writer.role_name(GrantLevel::ReadWrite),
        );

        let Some(row) = OltpRoles::find()
            .filter(oltp_roles::Column::TenantRowId.eq(tenant.id))
            .filter(oltp_roles::Column::RoleName.eq(role_name.clone()))
            .one(&self.db)
            .await?
        else {
            info!("no such writer; deprovision_writer is a no-op");
            return Ok(());
        };

        // Build + validate every statement up front, so a bad role name errors
        // before any DDL — never a half-drop (schema gone, role stranded).
        let drop_schema = schema::drop_writer_sql(writer, &role_name)?;
        let plan = crate::roles::drop_role_plan(&role_name, &tenant.owner_role)?;

        let owner_password = self.owner_password(&tenant, org_id)?;
        let tenant_dsn = dsn_for(&tenant, &tenant.owner_role, &owner_password);
        // Role DDL rides the role-admin connection (a superuser on a shared
        // cluster, the owner on Neon where `role_admin_dsn` is None). Only
        // cluster-global statements go here — see `RoleDropPlan`.
        let admin_dsn = self
            .provider
            .role_admin_dsn()
            .unwrap_or_else(|| tenant_dsn.clone());

        // The org's branches first: their copy of the app's staging data goes
        // with the app, and on a shared cluster production's DROP ROLE below
        // cannot succeed while a branch database still depends on the role.
        self.drop_writer_on_branches(&tenant, &role_name, &drop_schema, &plan)
            .await?;

        // Run each statement group on the connection it MUST run on. Extracted
        // so that routing — the thing the original bug got wrong — is
        // unit-testable with a `RecordingSqlExecutor`, no cluster needed.
        run_writer_drop(
            self.sql.as_ref(),
            &tenant_dsn,
            &admin_dsn,
            &drop_schema,
            &plan,
        )
        .await?;

        OltpRoles::delete_by_id(row.id).exec(&self.db).await?;
        info!("deprovisioned OLTP writer");
        Ok(())
    }

    /// Rotate a writer's password provider-side and reseal it locally.
    ///
    /// Airhouse has no equivalent — its credentials expire on their own. Here
    /// roles are durable, so a suspected leak needs an explicit rotation.
    #[instrument(skip(self), fields(org_id = %org_id, writer = %writer))]
    pub async fn rotate_writer(
        &self,
        org_id: Uuid,
        writer: &WriterRef,
        grant: GrantLevel,
    ) -> Result<WriterCredentials, ProvisionerError> {
        let tenant = self.active_tenant(org_id).await?;
        let role_name = schema::qualify_role(
            &tenant.provider,
            &tenant.database_name,
            &writer.role_name(grant),
        );

        let row = OltpRoles::find()
            .filter(oltp_roles::Column::TenantRowId.eq(tenant.id))
            .filter(oltp_roles::Column::RoleName.eq(role_name.clone()))
            .one(&self.db)
            .await?
            .ok_or_else(|| ProvisionerError::NotProvisioned(org_id))?;

        // The stored visibility, before the row is consumed by the update below.
        let stored_visible =
            crate::migrator::effective_visibility(row.analytics_visible, &row.writer_kind);

        // SQL, for the same reason as minting: a provider-side reset would not
        // re-confine the role, and on Neon a role that round-trips through the
        // API keeps its `neon_superuser` membership.
        let password = self
            .mint_role(&tenant, &role_name, RoleClass::Writer)
            .await?;

        // Re-apply the grants, because `mint_role` does not always merely reset
        // a password. Its recovery path — a role minted by an older build,
        // carrying `neon_superuser` — DELETES the role through the provider and
        // re-creates it, and a fresh role keeps only what is granted to it
        // again. `mint_role` restores `CONNECT`, so such a writer came back
        // able to authenticate and unable to touch its own schema: a rotation
        // that reports success and leaves the credential useless, which is the
        // worst shape for an operation whose whole point is recovering from a
        // suspected leak.
        //
        // Idempotent, so the ordinary path (`ALTER ROLE … PASSWORD`, grants
        // untouched) pays a few redundant GRANTs and nothing else.
        self.apply_writer_ddl(&tenant, writer, grant).await?;

        let mut active: oltp_roles::ActiveModel = row.into();
        active.password_ciphertext = ActiveValue::Set(seal(&password)?);
        active.rotated_at = ActiveValue::Set(Some(Utc::now().into()));
        active.update(&self.db).await?;

        // Restore analyst visibility, the same repair `ensure_writer` makes: a
        // re-mint's `DROP OWNED` drops the writer's `ALTER DEFAULT PRIVILEGES …
        // TO analyst`, and `apply_writer_ddl` above does not put it back, so a
        // rotated pipeline writer's future tables would be invisible to the
        // analyst. Grant-only (never re-opens a hidden schema).
        //
        // AFTER the update, not before: `set_analytics_visibility` builds the
        // writer DSN from the sealed ciphertext, which must be the just-rotated
        // password, not the old one.
        // Only for a ReadWrite rotation: analyst visibility is a property of the
        // writer's own schema, and `set_analytics_visibility` resolves the
        // `_rw` role — so on a `ReadOnly` rotation it would look up a role this
        // call never touched and fail `NotProvisioned`. Latent (the CLI
        // hardcodes ReadWrite), guarded because `rotate_writer` takes the grant.
        if stored_visible && grant == GrantLevel::ReadWrite {
            self.set_analytics_visibility(org_id, writer, true).await?;
        }

        info!(role = %role_name, "rotated OLTP writer password");
        Ok(WriterCredentials {
            schema_name: writer.schema_name(),
            role_name: role_name.clone(),
            dsn: schema::with_search_path(&dsn_for(&tenant, &role_name, &password), writer),
        })
    }

    /// Mint (or re-mint) the tenant's `oxy_analyst_ro` login and seal it.
    ///
    /// The platform declaration creates that role `NOLOGIN` and leaves the
    /// provider to issue its credential, so this must run once before any
    /// `postgres_managed` query can resolve. Idempotent: re-running replaces
    /// the sealed password with a fresh one.
    #[instrument(skip(self), fields(org_id = %org_id))]
    pub async fn ensure_analyst(
        &self,
        org_id: Uuid,
    ) -> Result<oltp_tenants::Model, ProvisionerError> {
        let tenant = self.active_tenant(org_id).await?;
        self.ensure_analyst_for(tenant).await
    }

    /// The core of [`Self::ensure_analyst`], taking a tenant already loaded.
    ///
    /// Split out so `provision` can mint the credential at the right point in
    /// its sequence without a redundant round trip to reload the row.
    pub(super) async fn ensure_analyst_for(
        &self,
        tenant: oltp_tenants::Model,
    ) -> Result<oltp_tenants::Model, ProvisionerError> {
        let analyst_role = schema::analyst_role_for(&tenant.provider, &tenant.database_name);
        // The role may already exist: `platform.rs` step 1 creates it NOLOGIN in
        // SQL so the baseline grants have something to name, and this then gives
        // it a login password. `LocalProvider` hid that — its `create_role` is a
        // guarded DO block — but Neon's API answers a duplicate with 409, which
        // failed provisioning after the database and every schema were already
        // made. Reconcile the three states instead of assuming the first.
        let sealed_already = tenant.analyst_password_ciphertext.is_some();
        let exists_remotely = self
            .provider
            .get_role(&tenant.project_id, &tenant.branch_id, analyst_role.as_str())
            .await
            // A provider that cannot answer must not block provisioning: this
            // only decides whether to log "resetting", and `mint_role` handles
            // both cases regardless.
            .unwrap_or(None)
            .is_some();

        if sealed_already && exists_remotely {
            // A working credential is not sufficient — it also has to be a
            // confined one. This shortcut existed to avoid rotating a live
            // password on every re-provision, and it silently exempted the
            // analyst from the confinement check: on Neon it kept a role that
            // still held `neon_superuser` and could read every `app_*` schema.
            let owner_password = self.owner_password(&tenant, tenant.org_id)?;
            let owner_dsn = dsn_for(&tenant, &tenant.owner_role, &owner_password);
            match self
                .sql
                .execute_batch(
                    &owner_dsn,
                    &[
                        // Idempotent, and applied on the skip path too: this was
                        // only in the minting batch, so a tenant whose analyst
                        // was already sealed never received it and authenticated
                        // into `permission denied for database`. The database
                        // ACL carries no PUBLIC entry, so nothing grants it
                        // implicitly.
                        crate::roles::grant_connect_sql(analyst_role.as_str())?,
                        crate::roles::assert_confined_sql(analyst_role.as_str())?,
                    ],
                )
                .await
            {
                // Confined and working: leave the password alone.
                Ok(()) => return Ok(tenant),
                Err(e) if is_unconfined(&e) => {
                    warn!(
                        role = analyst_role.as_str(),
                        "analyst credential works but the role is over-privileged; re-minting"
                    );
                }
                Err(e) => return Err(e.into()),
            }
        }

        if !sealed_already && exists_remotely {
            warn!(
                role = analyst_role.as_str(),
                "analyst role exists but no sealed password; resetting"
            );
        }
        // `mint_role` covers both branches: it creates or resets, and confines
        // either way. A role that already existed may have been made by an
        // earlier build through the provider API, so it needs the confinement
        // pass just as much as a new one.
        // `RoleClass::Analyst` is what lets `mint_role` release the analyst's
        // grants at the point it actually drops the role — and only there. An
        // unconditional release here would revoke the org's entire analyst read
        // access on the ordinary reset path too, which is just `ALTER ROLE …
        // PASSWORD` and needs none of it: every `postgres_managed` query in the
        // org would return `permission denied` for the width of the call, and
        // stay that way if the re-apply then failed.
        let password = self
            .mint_role(&tenant, analyst_role.as_str(), RoleClass::Analyst)
            .await?;

        // Put back whatever the tenant is supposed to hold. `mint_role` may
        // have deleted and re-created the role, which drops every ACL entry
        // keyed on the old OID. Cheap and idempotent when it did not.
        self.reapply_analyst_grants(&tenant, analyst_role.as_str())
            .await?;

        let mut active: oltp_tenants::ActiveModel = tenant.into();
        active.analyst_password_ciphertext = ActiveValue::Set(Some(seal(&password)?));
        active.updated_at = ActiveValue::Set(Utc::now().into());
        let row = active.update(&self.db).await?;

        info!(
            role = analyst_role.as_str(),
            "minted OLTP analyst credential"
        );
        Ok(row)
    }

    /// Expose a writer's schema to the analyst, or withdraw it.
    ///
    /// Two batches on two connections, because two roles own the objects: the
    /// database owner owns the schema and grants `USAGE`; the writer owns the
    /// tables it created and grants `SELECT`. Postgres requires ownership to
    /// grant, so this cannot be one batch.
    #[instrument(skip(self), fields(org_id = %org_id, writer = %writer, visible))]
    pub async fn set_analytics_visibility(
        &self,
        org_id: Uuid,
        writer: &WriterRef,
        visible: bool,
    ) -> Result<(), ProvisionerError> {
        let tenant = self.active_tenant(org_id).await?;
        let analyst_role = schema::analyst_role_for(&tenant.provider, &tenant.database_name);
        let owner_password = self.owner_password(&tenant, org_id)?;
        let owner_dsn = dsn_for(&tenant, &tenant.owner_role, &owner_password);

        let role_name = schema::qualify_role(
            &tenant.provider,
            &tenant.database_name,
            &writer.role_name(GrantLevel::ReadWrite),
        );
        let row = OltpRoles::find()
            .filter(oltp_roles::Column::TenantRowId.eq(tenant.id))
            .filter(oltp_roles::Column::RoleName.eq(role_name.clone()))
            .one(&self.db)
            .await?
            .ok_or(ProvisionerError::NotProvisioned(org_id))?;
        let writer_dsn = schema::with_search_path(
            &dsn_for(&tenant, &role_name, &open(&row.password_ciphertext)?),
            writer,
        );

        // Both roles' tables have to be covered and neither connection can
        // cover the other's, because you must own an object to grant on it.
        // Migrations run as the owner, so that is where nearly every table
        // lives; the writer owns only what it created itself.
        if visible {
            self.grant_analyst_access(&tenant, writer, &owner_dsn, &writer_dsn, &analyst_role)
                .await?;
        } else {
            self.revoke_analyst_access(
                &tenant,
                writer,
                &owner_dsn,
                &writer_dsn,
                &analyst_role,
                // An operator withdrawing a grant asserted the schema exists.
                Strictness::Strict,
            )
            .await?;
        }

        // Record the choice. Without this the column stays NULL forever, every
        // reader falls back to the kind's default, and the next `apply`
        // reinstates a grant an operator revoked — the migration and both
        // readers were in place and nothing wrote it, so the fix did nothing.
        let mut active: oltp_roles::ActiveModel = row.into();
        active.analytics_visible = ActiveValue::Set(Some(visible));
        active.update(&self.db).await?;

        Ok(())
    }

    /// Reuse the stored role password, or heal if the role vanished
    /// provider-side.
    async fn reconcile_role(
        &self,
        tenant: &oltp_tenants::Model,
        row: &oltp_roles::Model,
    ) -> Result<String, ProvisionerError> {
        // Roles are SQL objects now, not provider resources, so the provider's
        // role API cannot answer this. `mint_role` is create-or-reset and
        // re-asserts confinement either way, which is what we want on every
        // reconcile: a role minted by an older build went through the provider
        // API and may still be carrying `neon_superuser`.
        //
        // The sealed password is kept when it still works, because re-minting
        // unconditionally would rotate a live credential on every provision.
        let existing = open(&row.password_ciphertext).ok();
        if let Some(password) = existing {
            let dsn = dsn_for(tenant, &row.role_name, &password);
            if self
                .sql
                .execute_batch(&dsn, &["SELECT 1".to_string()])
                .await
                .is_ok()
            {
                // Still valid — but confinement is not optional, so assert it
                // on the owner connection before handing the credential back.
                let owner_password = self.owner_password(tenant, tenant.org_id)?;
                let owner_dsn = dsn_for(tenant, &tenant.owner_role, &owner_password);
                let confined = self
                    .sql
                    .execute_batch(
                        &owner_dsn,
                        &[crate::roles::assert_confined_sql(&row.role_name)?],
                    )
                    .await;
                match confined {
                    Ok(()) => return Ok(password),
                    // Fall through to re-mint rather than returning: `mint_role`
                    // knows how to remediate this (delete through the provider,
                    // recreate in SQL), and a working-but-over-privileged
                    // credential is exactly what must not be handed back.
                    Err(e) if is_unconfined(&e) => {
                        warn!(role = %row.role_name, "sealed credential works but the role is over-privileged");
                    }
                    Err(e) => return Err(e.into()),
                }
            }
        }

        warn!(
            role = %row.role_name,
            "OLTP role missing or its sealed password no longer works; re-minting"
        );
        let password = self
            .mint_role(tenant, &row.role_name, RoleClass::Writer)
            .await?;

        let mut active: oltp_roles::ActiveModel = row.clone().into();
        active.password_ciphertext = ActiveValue::Set(seal(&password)?);
        active.rotated_at = ActiveValue::Set(Some(Utc::now().into()));
        active.update(&self.db).await?;
        Ok(password)
    }

    async fn create_role(
        &self,
        tenant: &oltp_tenants::Model,
        writer: &WriterRef,
        grant: GrantLevel,
        claimant: Option<Uuid>,
    ) -> Result<String, ProvisionerError> {
        let role_name = schema::qualify_role(
            &tenant.provider,
            &tenant.database_name,
            &writer.role_name(grant),
        );
        let password = self
            .mint_role(tenant, &role_name, RoleClass::Writer)
            .await?;

        oltp_roles::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            tenant_row_id: ActiveValue::Set(tenant.id),
            writer_kind: ActiveValue::Set(match writer {
                WriterRef::App(_) => WriterKind::App,
                WriterRef::Pipeline(_) => WriterKind::Pipeline,
            }),
            writer_name: ActiveValue::Set(match writer {
                WriterRef::App(n) | WriterRef::Pipeline(n) => n.clone(),
            }),
            schema_name: ActiveValue::Set(writer.schema_name()),
            role_name: ActiveValue::Set(role_name),
            grant_level: ActiveValue::Set(match grant {
                GrantLevel::ReadWrite => DbGrantLevel::ReadWrite,
                GrantLevel::ReadOnly => DbGrantLevel::ReadOnly,
            }),
            password_ciphertext: ActiveValue::Set(seal(&password)?),
            claimed_by_workspace_id: ActiveValue::Set(claimant),
            // Unset: "never chosen", so readers use the kind's default until
            // someone calls `set_analytics_visibility`.
            analytics_visible: ActiveValue::Set(None),
            created_at: ActiveValue::Set(Utc::now().into()),
            rotated_at: ActiveValue::Set(None),
        }
        .insert(&self.db)
        .await?;

        Ok(password)
    }

    /// Create or reset `role` in SQL, then prove it came out confined.
    ///
    /// **Three connections, because the objects sit at three levels.** The role
    /// itself is cluster-global and may need a superuser
    /// ([`OltpProvider::role_admin_dsn`]); the confinement check reads
    /// `pg_roles`, so it rides the same one; `CONNECT` is granted ON a database
    /// and therefore has to run while connected to the tenant's own.
    ///
    /// `role_admin_dsn` is `None` on Neon and the mock — one project per tenant,
    /// so no name is shared and role DDL rides the tenant's own owner connection.
    /// The local shared-cluster provider instead returns `OXY_OLTP_ADMIN_URL`, a
    /// superuser on that one cluster, because an owner there cannot alter a role
    /// a sibling tenant's owner created. That is why a real local deployment
    /// should point `OXY_OLTP_ADMIN_URL` at something narrower than the cluster
    /// superuser — and never at `OXY_DATABASE_URL`, which is Oxy's own control
    /// plane.
    ///
    /// Not `provider.create_role`: on Neon every API-created role is silently
    /// made a member of `neon_superuser` with `CREATEDB`/`CREATEROLE`/
    /// `BYPASSRLS`, which would let the read-only analyst read every `app_*`
    /// schema and `oxy_meta`, and let one app's writer read another's data. The
    /// owner cannot revoke that afterwards — it lacks ADMIN option on the role —
    /// so the only fix is not to acquire it. See [`crate::roles`].
    ///
    /// The check raises rather than returning a credential Oxy would then hand
    /// out.
    async fn mint_role(
        &self,
        tenant: &oltp_tenants::Model,
        role_name: &str,
        class: RoleClass,
    ) -> Result<String, ProvisionerError> {
        let owner_password = self.owner_password(tenant, tenant.org_id)?;
        // Role DDL goes to whichever connection can actually perform it. On a
        // shared cluster the tenant owner cannot alter a role another tenant's
        // owner created, because `oxy_analyst_ro` is one global name; the
        // provider hands back a superuser DSN there. Neon returns None and this
        // stays on the owner.
        let owner_dsn = self
            .provider
            .role_admin_dsn()
            .unwrap_or_else(|| dsn_for(tenant, &tenant.owner_role, &owner_password));
        let password = crate::roles::generate_password();

        // Check BEFORE touching it. A contaminated role cannot even be altered
        // by the owner ("permission denied to alter role … ADMIN option"), so
        // detecting after the fact would fail on the repair rather than on the
        // diagnosis. On a role that does not exist this passes trivially — the
        // query simply finds no rows.
        let precheck = self
            .sql
            .execute_batch(&owner_dsn, &[crate::roles::assert_confined_sql(role_name)?])
            .await;

        // Two connections, because the objects live at two levels. A ROLE is
        // cluster-global and may need the superuser; CONNECT is granted ON a
        // database and `grant_connect_sql` names it with `current_database()`,
        // so it has to run while connected to the tenant's own database — on
        // the admin connection it would grant CONNECT on the admin database
        // and the writer would authenticate into "permission denied for
        // database".
        let mut batch = crate::roles::ensure_login_role_sql(role_name, &password)?;
        batch.push(crate::roles::assert_confined_sql(role_name)?);

        let first = match precheck {
            Err(e) if is_unconfined(&e) => Err(e),
            _ => self.sql.execute_batch(&owner_dsn, &batch).await,
        };

        match first {
            Ok(()) => {}
            Err(e) if is_unconfined(&e) => {
                // A role minted by an older build went through the provider API
                // and carries `neon_superuser`. It cannot be repaired: `ALTER
                // ROLE` strips the attributes but not the membership, and the
                // owner may neither revoke that membership nor drop the role —
                // both need ADMIN option it does not hold. The API that created
                // it is the only thing that can remove it.
                warn!(
                    role = role_name,
                    "role holds provider-granted authority; deleting it through the \
                     provider and re-minting in SQL"
                );
                // Strip what depends on the role first, or the drop cannot
                // happen at all.
                //
                // `DROP ROLE` fails with `2BP01 — role … cannot be dropped
                // because some objects depend on it` for any role holding a
                // grant, and every role this manages holds grants by the time
                // anything wants to re-mint it. So this branch, the whole
                // recovery path for a provider-contaminated role, could only
                // ever have worked on a role nobody had granted anything —
                // which is to say a role not worth recovering. It surfaced as
                // a 500 from `ensure_analyst`, and the analyst is the case
                // that matters: a pipeline schema is analyst-visible by
                // default, so a tenant reaches "cannot repair" by existing.
                //
                // The two-step is Postgres's own recipe and the order is
                // load-bearing. `REASSIGN OWNED` moves objects the role owns
                // to the database owner; `DROP OWNED` then removes only the
                // privileges, because ownership has already moved out of the
                // way. Running `DROP OWNED` alone would drop the objects
                // themselves — for a writer, its tables.
                // Only the analyst has grants held across the tenant's writer
                // schemas, and only this branch drops the role, so this is the
                // one place the sweep is both needed and paid for.
                if matches!(class, RoleClass::Analyst) {
                    self.release_analyst_grants(tenant, role_name).await?;
                }
                self.strip_role_dependencies(tenant, role_name).await?;
                self.provider
                    .delete_role(&tenant.project_id, &tenant.branch_id, role_name)
                    .await?;
                self.sql.execute_batch(&owner_dsn, &batch).await?;
            }
            Err(e) => return Err(e.into()),
        }

        // Now on the tenant database, as its owner.
        let tenant_dsn = dsn_for(tenant, &tenant.owner_role, &owner_password);
        self.sql
            .execute_batch(&tenant_dsn, &[crate::roles::grant_connect_sql(role_name)?])
            .await?;

        info!(role = role_name, "minted confined OLTP role");
        Ok(password)
    }

    /// Test seam for [`Self::strip_role_dependencies`].
    ///
    /// The refusal it guards is only reachable through `mint_role`'s
    /// remediation branch, which needs a role contaminated the way a provider
    /// API contaminates one. Exposing the step directly lets the refusal be
    /// asserted on any cluster, including one with no such provider.
    #[doc(hidden)]
    pub async fn strip_role_dependencies_for_test(
        &self,
        tenant: &oltp_tenants::Model,
        role_name: &str,
    ) -> Result<(), ProvisionerError> {
        self.strip_role_dependencies(tenant, role_name).await
    }

    /// Make a role droppable by releasing its privileges — never by moving its
    /// data.
    ///
    /// Runs in the **tenant** database, because that is where privileges live:
    /// on the admin connection this would be a no-op and the drop would fail
    /// exactly as before.
    ///
    /// **This refuses rather than reassigns, and that is the whole design.**
    /// The obvious spelling is `REASSIGN OWNED BY role TO owner` followed by
    /// `DROP OWNED`, which is Postgres's own recipe and is right for a role
    /// that owns nothing. The analyst is such a role: read-only by
    /// construction, so `DROP OWNED` releases the `SELECT`/`USAGE` grants that
    /// were blocking the drop and nothing else moves.
    ///
    /// A writer is not. The objects a writer owns are its tables — the tenant's
    /// data — and `REASSIGN` would hand them to the database owner
    /// **permanently**, because nothing gives them back: `ensure_writer_sql`'s
    /// `ReadWrite` arm grants `USAGE, CREATE ON SCHEMA` and nothing more,
    /// resting on the invariant that a writer owns what it created. A rotated
    /// writer would come back able to create new tables and denied on every row
    /// it had before — the app up, writes working, existing data unreachable,
    /// and no error anywhere. A rotation is what an operator reaches for on a
    /// suspected leak, so that is the worst possible time to silently move
    /// their data.
    ///
    /// So an owning role stops here with a diagnosis instead. The guard runs
    /// before `DROP OWNED` for the same reason the confinement check runs
    /// before the repair: `DROP OWNED` alone would drop the objects outright,
    /// which is worse than reassigning them.
    async fn strip_role_dependencies(
        &self,
        tenant: &oltp_tenants::Model,
        role_name: &str,
    ) -> Result<(), ProvisionerError> {
        let owner_password = self.owner_password(tenant, tenant.org_id)?;
        let tenant_dsn = dsn_for(tenant, &tenant.owner_role, &owner_password);
        let role = crate::schema::quote_ident(role_name);
        let owner = crate::schema::quote_ident(&tenant.owner_role);
        let role_lit = role_name.replace('\'', "''");

        // The owner needs membership in the role before it may act on that
        // role's objects: `DROP OWNED` answers a non-member with `42501 — Only
        // roles with privileges of role … may drop objects owned by it`. On a
        // shared cluster the role was created by the superuser, not by this
        // owner, so the implicit ADMIN a creator gets does not apply.
        //
        // Granted on the same connection that mints roles — the superuser where
        // the provider gives one. If the drop or the re-create then fails, the
        // owner keeps this membership: harmless in privilege terms, since the
        // owner already dominates the role, but it does outlive the call.
        let admin_dsn = self
            .provider
            .role_admin_dsn()
            .unwrap_or_else(|| dsn_for(tenant, &tenant.owner_role, &owner_password));
        self.sql
            .execute_batch(&admin_dsn, &[format!("GRANT {role} TO {owner}")])
            .await?;

        self.sql
            .execute_batch(
                &tenant_dsn,
                &[
                    // Refuses on objects `DROP OWNED` would destroy with NO way
                    // back — tables (`pg_class`), functions (`pg_proc`), schemas
                    // (`pg_namespace`). Reassigning or dropping those is an
                    // operator's deliberate act, not something a re-mint does.
                    //
                    // `pg_default_acl` is deliberately NOT here, though `DROP
                    // OWNED` clears it too. `set_analytics_visibility` gives
                    // EVERY analytics-visible writer an `ALTER DEFAULT
                    // PRIVILEGES … TO analyst` row at provision — so counting it
                    // refused every pipeline writer and made the remediation
                    // path (the reason SQL-minting exists) unreachable for them.
                    // Unlike a table, that grant is restorable, and the caller
                    // does restore it — `ensure_writer` re-applies the stored
                    // visibility after this returns. `pg_type` is out for the
                    // same reason as before: a table's rowtype has the same
                    // owner and `pg_class` already refuses on it.
                    format!(
                        "DO $$ DECLARE n int; BEGIN \
                           SELECT \
                             (SELECT count(*) FROM pg_class c JOIN pg_roles r \
                                ON c.relowner = r.oid \
                               WHERE r.rolname = \'{role_lit}\' \
                                 AND c.relkind IN (\'r\',\'p\',\'S\',\'v\',\'m\')) \
                           + (SELECT count(*) FROM pg_proc p JOIN pg_roles r \
                                ON p.proowner = r.oid WHERE r.rolname = \'{role_lit}\') \
                           + (SELECT count(*) FROM pg_namespace ns JOIN pg_roles r \
                                ON ns.nspowner = r.oid WHERE r.rolname = \'{role_lit}\' \
                                 AND ns.nspname NOT LIKE \'pg\\_%\' \
                                 AND ns.nspname <> \'information_schema\') \
                           INTO n; \
                           IF n > 0 THEN \
                             RAISE EXCEPTION \'role % owns % object(s) and will not be \
                                               stripped\', \'{role_lit}\', n \
                               USING ERRCODE = \'OXY03\', \
                                     HINT = \'Re-minting would drop these (tables, \
                                             functions or schemas) and nothing would \
                                             restore them. Reassign or drop them \
                                             deliberately, then retry.\'; \
                           END IF; END $$"
                    ),
                    format!("DROP OWNED BY {role}"),
                ],
            )
            .await?;
        Ok(())
    }

    /// Assert `claimant` owns this schema namespace, adopting it if unowned.
    ///
    /// An OLTP database is per **org** while schema definitions compile per
    /// **workspace**, so an org holding two workspaces could have both declare
    /// `app_bookings`. Without a claim they would interleave DDL into one
    /// schema, each overwriting the other's tables — silently, since additive
    /// DDL rarely errors.
    ///
    /// Runs before any provider call: a losing workspace must not leave a
    /// half-created role behind.
    async fn claim_namespace(
        &self,
        tenant: &oltp_tenants::Model,
        writer: &WriterRef,
        claimant: Option<Uuid>,
    ) -> Result<(), ProvisionerError> {
        let schema_name = writer.schema_name();
        let Some(row) = OltpRoles::find()
            .filter(oltp_roles::Column::TenantRowId.eq(tenant.id))
            .filter(oltp_roles::Column::SchemaName.eq(schema_name.clone()))
            .one(&self.db)
            .await?
        else {
            // Unseen namespace — `create_role` stamps the claim on insert.
            return Ok(());
        };

        match (row.claimed_by_workspace_id, claimant) {
            // Two real workspaces want the same namespace. This is the case the
            // claim exists for: without it both would interleave DDL into one
            // schema, each overwriting the other's tables, and additive DDL
            // rarely errors so nothing would say so.
            (Some(owner), Some(c)) if c != owner => {
                Err(ProvisionerError::SchemaNamespaceClaimed {
                    org_id: tenant.org_id,
                    schema: schema_name,
                    owner,
                    // Bound from the tuple rather than unwrapped: the guard
                    // already proves it is `Some`, and a fallback to `owner`
                    // would read as "the owner claimed against itself" if
                    // anyone ever loosened it.
                    claimant: c,
                })
            }
            // Already claimed, and this call either agrees or is not asserting
            // ownership at all.
            (Some(_), _) => Ok(()),
            // Free, and a workspace is claiming it.
            //
            // Two paths reach here and the `warn` covers both. One is the
            // upgrade case, a row that predates the claim column. The other is
            // now ordinary: the console provisions with no claimant, so a
            // console-provisioned row is claimed-by-nobody until the workspace
            // that declares the writer adopts it. That is once per writer
            // rather than once per Provision click, which is quiet enough to
            // keep — an adoption is a real state change either way, and the
            // line an operator greps still marks one.
            (None, Some(_)) => {
                warn!(schema = %schema_name, "adopting unclaimed OLTP schema namespace");
                let mut active: oltp_roles::ActiveModel = row.into();
                active.claimed_by_workspace_id = ActiveValue::Set(claimant);
                active.update(&self.db).await?;
                Ok(())
            }
            // Free, and nobody is claiming it — the console provisioning ahead
            // of any workspace, re-run on every Provision click. Nothing to
            // write: the old code issued `SET claimed_by_workspace_id = NULL`
            // over a row that already held NULL, and logged the upgrade
            // adoption at `warn` while doing it. That is the one line an
            // operator would grep to find a real adoption, so firing it on the
            // ordinary path made it useless.
            (None, None) => Ok(()),
        }
    }

    /// Point the database's default `search_path` at every writer schema.
    ///
    /// Re-derived from `oltp_roles` on each call rather than appended to, so a
    /// removed writer drops out instead of leaving a dangling entry. Ordered by
    /// schema name for determinism — an unqualified name that exists in two
    /// schemas would otherwise resolve differently between runs.
    async fn refresh_database_search_path(
        &self,
        tenant: &oltp_tenants::Model,
    ) -> Result<(), ProvisionerError> {
        let mut schemas: Vec<String> = OltpRoles::find()
            .filter(oltp_roles::Column::TenantRowId.eq(tenant.id))
            .all(&self.db)
            .await?
            .into_iter()
            .map(|r| r.schema_name)
            .collect();
        schemas.sort();
        schemas.dedup();

        let owner_password = self.owner_password(tenant, tenant.org_id)?;
        let statements = schema::database_search_path_sql(&tenant.database_name, &schemas)?;
        self.run_batch(tenant, &owner_password, statements).await
    }

    /// Grant the analyst read access to one writer's schema.
    ///
    /// Both roles' tables have to be covered and neither connection can cover
    /// the other's, because you must own an object to grant on it. Migrations
    /// run as the owner, so that is where nearly every table lives; the writer
    /// owns only what it created itself.
    ///
    /// Idempotent, which is what lets [`Self::ensure_analyst_for`] replay it
    /// after a re-mint.
    async fn grant_analyst_access(
        &self,
        tenant: &oltp_tenants::Model,
        writer: &WriterRef,
        owner_dsn: &str,
        writer_dsn: &str,
        analyst_role: &str,
    ) -> Result<(), ProvisionerError> {
        self.sql
            .execute_batch(
                owner_dsn,
                &schema::grant_analyst_schema_sql(writer, analyst_role),
            )
            .await?;
        self.sql
            .execute_batch(
                owner_dsn,
                &schema::grant_analyst_owner_tables_sql(writer, &tenant.owner_role, analyst_role)?,
            )
            .await?;
        self.sql
            .execute_batch(
                writer_dsn,
                &schema::grant_analyst_tables_sql(writer, analyst_role),
            )
            .await?;
        Ok(())
    }

    /// Withdraw the analyst's read access to one writer's schema.
    ///
    /// The exact inverse of [`Self::grant_analyst_access`], and the order is
    /// load-bearing: tables before schema `USAGE`. Withdrawing `USAGE` first
    /// would leave the table grants stranded — invisible but present — and
    /// re-granting `USAGE` later would silently restore read access nobody
    /// re-authorised.
    ///
    /// The first batch runs as the WRITER because the entry it removes belongs
    /// to the writer's own default-privilege set. That is the one dependency
    /// `DROP OWNED BY`, run as the database owner, cannot reach — see
    /// [`Self::strip_role_dependencies`].
    async fn revoke_analyst_access(
        &self,
        tenant: &oltp_tenants::Model,
        writer: &WriterRef,
        owner_dsn: &str,
        writer_dsn: &str,
        analyst_role: &str,
        strictness: Strictness,
    ) -> Result<(), ProvisionerError> {
        let prepare = |stmts: Vec<String>| match strictness {
            Strictness::Strict => stmts,
            Strictness::TolerateMissing => tolerate_missing(stmts),
        };
        self.sql
            .execute_batch(
                writer_dsn,
                &prepare(schema::revoke_analyst_tables_sql(writer, analyst_role)),
            )
            .await?;
        self.sql
            .execute_batch(
                owner_dsn,
                &prepare(schema::revoke_analyst_owner_tables_sql(
                    writer,
                    &tenant.owner_role,
                    analyst_role,
                )?),
            )
            .await?;
        self.sql
            .execute_batch(
                owner_dsn,
                &prepare(schema::revoke_analyst_schema_sql(writer, analyst_role)),
            )
            .await?;
        Ok(())
    }

    /// Release every analyst grant across the tenant, so the role can be
    /// dropped.
    ///
    /// Runs before a re-mint. `DROP ROLE` refuses while anything depends on the
    /// role, and the dependency `DROP OWNED BY` cannot clear from the owner's
    /// connection is the `ALTER DEFAULT PRIVILEGES` entry each WRITER created
    /// when the schema was opted into analytics — it belongs to that writer's
    /// default-privilege set, so only that writer can remove it. This connects
    /// as each writer in turn and does exactly that.
    ///
    /// Paired with [`Self::reapply_analyst_grants`], which puts back whatever
    /// the tenant is supposed to hold once the new role exists.
    async fn release_analyst_grants(
        &self,
        tenant: &oltp_tenants::Model,
        analyst_role: &str,
    ) -> Result<(), ProvisionerError> {
        let owner_password = self.owner_password(tenant, tenant.org_id)?;
        let owner_dsn = dsn_for(tenant, &tenant.owner_role, &owner_password);

        let rows = OltpRoles::find()
            .filter(oltp_roles::Column::TenantRowId.eq(tenant.id))
            .all(&self.db)
            .await?;

        for row in rows {
            let writer = match row.writer_kind {
                WriterKind::App => WriterRef::app(&row.writer_name),
                WriterKind::Pipeline => WriterRef::pipeline(&row.writer_name),
            }?;
            let writer_dsn = schema::with_search_path(
                &dsn_for(tenant, &row.role_name, &open(&row.password_ciphertext)?),
                &writer,
            );
            // Unconditional, not gated on stored visibility: this is about what
            // Postgres still HOLDS, not what the tenant chose. A schema opted
            // out after being opted in can retain an entry, and one missed
            // entry is the whole drop refused.
            self.revoke_analyst_access(
                tenant,
                &writer,
                &owner_dsn,
                &writer_dsn,
                analyst_role,
                Strictness::TolerateMissing,
            )
            .await?;
        }
        Ok(())
    }

    /// Re-apply every analyst grant this tenant is supposed to hold.
    ///
    /// Called after the analyst role is re-minted. `mint_role`'s remediation
    /// path DELETES the role through the provider and re-creates it, and
    /// Postgres keys ACL entries on the role OID — so a fresh role with the
    /// same name inherits none of the old one's grants. Every `GRANT SELECT`
    /// `set_analytics_visibility` ever issued is gone, and the analyst comes
    /// back able to authenticate and unable to read anything: `permission
    /// denied` across the org's whole `postgres_managed` path, from a call
    /// whose logs say it succeeded.
    ///
    /// This is the same fix the writer path carries, on the role with more
    /// behind it: a writer re-mint costs one schema, an analyst re-mint costs
    /// every schema at once. `reconcile_grants` repairs it, but that is
    /// reachable only from `migrator::apply_to_org`, so neither `provision` nor
    /// the console ever ran it.
    async fn reapply_analyst_grants(
        &self,
        tenant: &oltp_tenants::Model,
        analyst_role: &str,
    ) -> Result<(), ProvisionerError> {
        let owner_password = self.owner_password(tenant, tenant.org_id)?;
        let owner_dsn = dsn_for(tenant, &tenant.owner_role, &owner_password);

        let rows = OltpRoles::find()
            .filter(oltp_roles::Column::TenantRowId.eq(tenant.id))
            .all(&self.db)
            .await?;

        for row in rows {
            let writer = match row.writer_kind {
                WriterKind::App => WriterRef::app(&row.writer_name),
                WriterKind::Pipeline => WriterRef::pipeline(&row.writer_name),
            }?;
            // The STORED choice, falling back to the kind's default only when
            // nobody has made one — the same rule `reconcile_grants` follows.
            // Re-deriving unconditionally would reinstate a grant an operator
            // revoked.
            if !crate::migrator::effective_visibility(row.analytics_visible, &row.writer_kind) {
                continue;
            }
            let writer_dsn = schema::with_search_path(
                &dsn_for(tenant, &row.role_name, &open(&row.password_ciphertext)?),
                &writer,
            );
            self.grant_analyst_access(tenant, &writer, &owner_dsn, &writer_dsn, analyst_role)
                .await?;
            info!(schema = %writer.schema_name(), "re-applied analyst grants after re-mint");
        }
        Ok(())
    }

    async fn apply_writer_ddl(
        &self,
        tenant: &oltp_tenants::Model,
        writer: &WriterRef,
        grant: GrantLevel,
    ) -> Result<(), ProvisionerError> {
        let owner_password = self.owner_password(tenant, tenant.org_id)?;
        let role_name = schema::qualify_role(
            &tenant.provider,
            &tenant.database_name,
            &writer.role_name(grant),
        );
        let mut statements =
            schema::ensure_writer_sql(writer, grant, &tenant.owner_role, &role_name)?;
        // Airway creates its dataset schema on every load and Postgres checks
        // CREATE on the database first, so a pipeline writer needs it. An app
        // writer never does — see `roles::grant_schema_creation_sql`.
        if matches!(writer, WriterRef::Pipeline(_)) {
            statements.push(crate::roles::grant_schema_creation_sql(&role_name)?);
        }
        self.run_batch(tenant, &owner_password, statements).await
    }
}

/// Execute a writer-drop with each statement group on the connection it MUST run
/// on: the schema drop and `REASSIGN OWNED`/`DROP OWNED` on the TENANT database
/// (they are per-database), the membership `GRANT` and `DROP ROLE` on the
/// role-admin connection (cluster-global). Separated from `deprovision_writer`
/// so this routing — the property the original bug got wrong — is unit-testable
/// with a `RecordingSqlExecutor`, no cluster needed.
async fn run_writer_drop(
    sql: &dyn crate::sql::TenantSqlExecutor,
    tenant_dsn: &str,
    admin_dsn: &str,
    drop_schema: &[String],
    plan: &crate::roles::RoleDropPlan,
) -> Result<(), crate::sql::SqlError> {
    sql.execute_batch(tenant_dsn, drop_schema).await?;
    sql.execute_batch(admin_dsn, &plan.admin_pre).await?;
    sql.execute_batch(tenant_dsn, &plan.tenant).await?;
    sql.execute_batch(admin_dsn, &plan.admin_post).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::run_writer_drop;
    use crate::sql::RecordingSqlExecutor;

    /// The routing the original bug got wrong: schema + REASSIGN/DROP OWNED on
    /// the TENANT connection (per-database, or they reach nothing), membership
    /// GRANT + DROP ROLE on the ADMIN one (cluster-global). Swap the two dsns in
    /// `run_writer_drop` and this fails — no cluster, no MockDatabase.
    #[tokio::test]
    async fn run_writer_drop_routes_per_database_ddl_to_the_tenant_connection() {
        let rec = RecordingSqlExecutor::new();
        let plan = crate::roles::drop_role_plan("app_x_rw", "app_owner").unwrap();
        let drop_schema = vec!["DROP SCHEMA IF EXISTS \"app_x\" CASCADE".to_string()];

        run_writer_drop(&rec, "TENANT", "ADMIN", &drop_schema, &plan)
            .await
            .expect("the recording executor never fails");

        let batches = rec.batches();
        // `.first()`, not `[0]`: an empty group would fail an assertion with the
        // routing printed rather than panicking on the index.
        let routed: Vec<(&str, &str)> = batches
            .iter()
            .map(|(dsn, stmts)| (dsn.as_str(), stmts.first().map_or("", |s| s.as_str())))
            .collect();
        assert_eq!(routed.len(), 4, "four batches, one per group");
        assert_eq!(routed[0].0, "TENANT");
        assert!(routed[0].1.contains("DROP SCHEMA"), "{routed:?}");
        assert_eq!(routed[1].0, "ADMIN");
        assert!(routed[1].1.starts_with("GRANT"), "{routed:?}");
        assert_eq!(
            routed[2].0, "TENANT",
            "REASSIGN/DROP OWNED are per-database"
        );
        assert!(routed[2].1.starts_with("REASSIGN OWNED"), "{routed:?}");
        assert_eq!(routed[3].0, "ADMIN", "DROP ROLE is cluster-global");
        assert!(routed[3].1.contains("DROP ROLE"), "{routed:?}");
    }
}
