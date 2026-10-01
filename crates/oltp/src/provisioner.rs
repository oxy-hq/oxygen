//! Idempotent provisioning of a per-org Postgres and its per-writer schemas.
//!
//! Deliberately parallel to `airhouse::TenantProvisioner`, including the two
//! lessons that crate paid for:
//!
//! 1. **Reconcile, don't duplicate.** Re-running `provision` for an org that
//!    already has a row contacts the provider to confirm the remote still
//!    exists, and recreates it if not — it never creates a second project.
//! 2. **Never silently adopt a name collision.** A remote project that already
//!    holds our deterministic name is an error an operator must look at, not
//!    something to quietly start writing into. Silent adoption is how you grant
//!    one tenant access to another's data.
//!
//! **Does the tenant exist** is this file; **who may touch what inside it** is
//! `provisioner/credentials.rs`. That seam is the question you are
//! debugging: creating, reconciling, recording and destroying the database here;
//! minting a role, confining it, granting it a schema and publishing a schema to
//! analytics there. One 1300-line file answered both and made either harder to
//! follow.

use std::sync::Arc;

use chrono::Utc;
use oxy_platform::secrets::envelope;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
};
use thiserror::Error;
use tracing::{info, instrument, warn};
use uuid::Uuid;

use crate::entity::tenants::{self as oltp_tenants, Entity as OltpTenants, TenantStatus};
use crate::platform;
use crate::provider::{CreateProjectRequest, OltpProvider, Project, ProviderError};
use crate::schema::SchemaError;
use crate::sql::{SqlError, TenantSqlExecutor};

#[derive(Debug, Error)]
pub enum ProvisionerError {
    #[error("org {0} not found")]
    OrgNotFound(Uuid),
    #[error("org {0} has no OLTP database; provision it first")]
    NotProvisioned(Uuid),
    #[error("org {0}'s OLTP database is not active (status: {1})")]
    NotActive(Uuid, &'static str),
    #[error(
        "org {0}'s OLTP tenant has no stored owner password; reset it before running schema DDL"
    )]
    OwnerPasswordMissing(Uuid),
    #[error("provider error: {0}")]
    Provider(#[from] ProviderError),
    #[error("tenant SQL error: {0}")]
    Sql(#[from] SqlError),
    #[error("database error: {0}")]
    Db(#[from] sea_orm::DbErr),
    #[error("schema error: {0}")]
    Schema(#[from] SchemaError),
    #[error(
        "schema {schema} in org {org_id} is owned by workspace {owner}; workspace {claimant} \
         cannot also define it. Rename the writer, or move the definition into the owning workspace."
    )]
    SchemaNamespaceClaimed {
        org_id: Uuid,
        schema: String,
        owner: Uuid,
        claimant: Uuid,
    },
    #[error(
        "org {org_id} is provisioned on provider {recorded:?}, but this process is configured \
         for {configured:?}. Refusing to reconcile across providers — deprovision it first, or \
         point OXY_OLTP_PROVIDER back at {recorded:?}."
    )]
    ProviderMismatch {
        org_id: Uuid,
        recorded: String,
        configured: &'static str,
    },
    #[error("envelope crypto failed: {0}")]
    Crypto(String),
    #[error(
        "org {0} has no {1} OLTP branch. Create it with: oxyc oltp provision --org {0} --branch {1}"
    )]
    BranchNotProvisioned(Uuid, crate::OltpBranch),
    /// A reset that did not finish. Nothing hands out a connection to the
    /// branch in this state; running the reset again completes it.
    #[error(
        "org {org_id}'s {branch} OLTP branch is {status}; finish it with: \
         oxyc oltp reset --org {org_id} --branch {branch}"
    )]
    BranchNotReady {
        org_id: Uuid,
        branch: crate::OltpBranch,
        status: &'static str,
    },
    /// A recorded branch id that names production. Never acted on: a reset or
    /// delete of it would be production's.
    #[error(
        "org {org_id}'s {branch} branch is recorded as {branch_id:?}, which is its production \
         database — refusing to touch it"
    )]
    BranchIsProduction {
        org_id: Uuid,
        branch: crate::OltpBranch,
        branch_id: String,
    },
    #[error("org {0}'s {1} OLTP branch has no stored owner password; reset the branch")]
    BranchOwnerPasswordMissing(Uuid, crate::OltpBranch),
    /// The row was not in the status this operation read it in — something
    /// else moved it. Refused rather than overwritten, so a stale write can
    /// never flip a `resetting` branch back to `active`.
    #[error(
        "the {branch} OLTP branch changed under this operation (expected {expected}, found \
         {found}); another provision or reset ran — re-run once it finishes"
    )]
    BranchStateChanged {
        branch: crate::OltpBranch,
        expected: &'static str,
        found: &'static str,
    },
    /// OLTP is not configured on this deployment.
    ///
    /// Its own variant rather than a `ProviderError::Transport`, which is what
    /// it used to be: a deployment that simply never turned OLTP on is a normal
    /// state and must answer **503**, not 500. As a transport error it was
    /// indistinguishable from "Neon is down", so it paged, and the console said
    /// "Could not provision" where it should say "OLTP isn't configured here".
    #[error("{0}")]
    NotConfigured(String),
}

impl ProvisionerError {
    /// Whether this is "the deployment has no OLTP", the one error on this
    /// surface that is a configuration state rather than a fault.
    pub fn is_not_configured(&self) -> bool {
        matches!(self, ProvisionerError::NotConfigured(_))
    }
}

/// Connection details for one writer. Carries a password, so [`std::fmt::Debug`]
/// is implemented by hand to redact it — a derived `Debug` would put the
/// credential in every `tracing` span that touched this value.
#[derive(Clone)]
pub struct WriterCredentials {
    pub schema_name: String,
    pub role_name: String,
    pub dsn: String,
}

impl std::fmt::Debug for WriterCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WriterCredentials")
            .field("schema_name", &self.schema_name)
            .field("role_name", &self.role_name)
            .field("dsn", &"<redacted>")
            .finish()
    }
}

/// Provider-visible project name for an org. Deterministic so an orphaned
/// remote project can still be found after a local-DB wipe.
pub fn project_name_for(org_id: Uuid) -> String {
    format!("oxy-org-{org_id}")
}

/// Whether a project name was derived by [`project_name_for`] rather than
/// chosen by a person.
///
/// **The whole safety argument for adopting an existing project.** A derived
/// name identifies exactly one org, so a match is an identity match rather than
/// a coincidence — which is what makes it safe to reset an existing project's
/// owner password and take it over. A chosen name has no such property: the
/// `airhouse` incident `ProviderError::ProjectNameTaken` exists for was
/// user-chosen names, where two tenants could collide and adoption would cross
/// them.
///
/// Lives beside `project_name_for` because it encodes that function's shape,
/// and both providers gate on it — the invariant was prose in `NeonProvider`
/// and code in `LocalProvider`, which is the arrangement that decays.
pub fn is_derived_project_name(name: &str) -> bool {
    // Re-render and compare, rather than parse-and-accept.
    //
    // `Uuid::parse_str` takes the simple, braced and `urn:uuid:` forms, so a
    // parse-only test passes `oxy-org-{11111111-…}` and the 32-char form while
    // `project_name_for` renders only hyphenated. That is looser than the
    // constructor this claims to encode, and on Neon the predicate is the ONLY
    // gate — Local backstops it with an ownership test. Comparing against the
    // constructor's own output makes this exactly its inverse, so the two
    // cannot drift.
    name.strip_prefix("oxy-org-")
        .and_then(|rest| Uuid::parse_str(rest).ok())
        .is_some_and(|id| project_name_for(id) == name)
}

// `provisioner.rs` beside a `provisioner/` directory — edition 2018 allows it,
// so this needs no `#[path]` attribute and no invented `_parts` suffix.
mod branch_ledger;
mod branch_roles;
mod branch_state;
mod branches;
mod credentials;

pub struct OltpProvisioner {
    db: DatabaseConnection,
    provider: Arc<dyn OltpProvider>,
    sql: Arc<dyn TenantSqlExecutor>,
    region: String,
    pg_version: u8,
}

impl OltpProvisioner {
    pub fn new(
        db: DatabaseConnection,
        provider: Arc<dyn OltpProvider>,
        sql: Arc<dyn TenantSqlExecutor>,
        region: impl Into<String>,
        pg_version: u8,
    ) -> Self {
        Self {
            db,
            provider,
            sql,
            region: region.into(),
            pg_version,
        }
    }

    /// Idempotent. Re-running for an org that already has a tenant reconciles
    /// it rather than creating a second one.
    #[instrument(skip(self), fields(org_id = %org_id))]
    pub async fn provision(&self, org_id: Uuid) -> Result<oltp_tenants::Model, ProvisionerError> {
        self.assert_org_exists(org_id).await?;

        let existing = self.find_tenant(org_id).await?;
        let row = match existing {
            Some(local) => self.reconcile_existing(local).await?,
            None => self.create_new(org_id).await?,
        };

        // ORDER MATTERS. The analyst must exist as a *login* role before the
        // platform DDL grants on it. `platform.rs` step 1 has a guarded
        // `CREATE ROLE ... NOLOGIN` so the grants have something to name; if
        // that runs first on a managed provider, the provider then owns a
        // passwordless role it cannot issue a password for — Neon answers
        // `reset_password` with "cannot update password for role without
        // password", after the database and every schema already exist.
        // Minting first makes the SQL guard a no-op instead of a trap.
        let row = self.ensure_analyst_for(row).await?;

        // Single convergence point for both paths. A new tenant sits at version
        // 0 and gets every step, so it is correct by construction rather than
        // by remembering to run something extra at create time.
        let row = self.reconcile_platform_schema(&row).await?;

        info!(
            project_id = %row.project_id,
            provider = %row.provider,
            "provisioned per-org OLTP database"
        );
        Ok(row)
    }

    /// Idempotent: succeeds whether or not the local row or remote project
    /// exist. Call from the org-delete path.
    ///
    /// Provider failures are **fatal** here, unlike airhouse's best-effort SA
    /// revoke: an orphaned project keeps costing money and keeps holding
    /// customer data, so a caller must know it didn't go away.
    #[instrument(skip(self), fields(org_id = %org_id))]
    pub async fn deprovision(&self, org_id: Uuid) -> Result<(), ProvisionerError> {
        if let Some(row) = self.find_tenant(org_id).await? {
            // Same guard as `reconcile_existing`, and it matters more here.
            // Deleting under the wrong provider drops the local row while the
            // provider call is a no-op against an id it does not own — so the
            // real database survives with nobody able to find it, and on a
            // managed provider it keeps billing.
            if row.provider != self.provider.name() {
                return Err(ProvisionerError::ProviderMismatch {
                    org_id,
                    recorded: row.provider.clone(),
                    configured: self.provider.name(),
                });
            }
        }

        let Some(local) = self.find_tenant(org_id).await? else {
            info!("no local OLTP tenant row; deprovision is a no-op");
            return Ok(());
        };

        // Branches first, and each one explicitly. On Neon the project delete
        // would take them anyway; on a local cluster a branch is a separate
        // database that nothing else would ever drop — and the staging branch
        // is a copy of the org's data, which the org-deletion path must not
        // leave behind (the 2026-09-29 ruling rests the MSA's 30-day deletion
        // commitment on exactly this).
        let strict = if self.provider.project_delete_takes_branches() {
            branches::Strict::UnlessProjectDeleteTakesBranches
        } else {
            branches::Strict::Always
        };
        self.delete_tenant_branches(&local, strict).await?;
        self.provider.delete_project(&local.project_id).await?;
        // `oltp_roles` and `oltp_branches` rows go with it via ON DELETE CASCADE.
        OltpTenants::delete_by_id(local.id).exec(&self.db).await?;

        info!(project_id = %local.project_id, "deprovisioned per-org OLTP database");
        Ok(())
    }

    // ── internals ────────────────────────────────────────────────────────────

    async fn find_tenant(
        &self,
        org_id: Uuid,
    ) -> Result<Option<oltp_tenants::Model>, ProvisionerError> {
        Ok(OltpTenants::find()
            .filter(oltp_tenants::Column::OrgId.eq(org_id))
            .one(&self.db)
            .await?)
    }

    async fn active_tenant(&self, org_id: Uuid) -> Result<oltp_tenants::Model, ProvisionerError> {
        let tenant = self
            .find_tenant(org_id)
            .await?
            .ok_or(ProvisionerError::NotProvisioned(org_id))?;
        if tenant.status != TenantStatus::Active {
            return Err(ProvisionerError::NotActive(org_id, tenant.status.as_str()));
        }
        Ok(tenant)
    }

    async fn assert_org_exists(&self, org_id: Uuid) -> Result<(), ProvisionerError> {
        let found = entity::organizations::Entity::find_by_id(org_id)
            .one(&self.db)
            .await?;
        found
            .map(|_| ())
            .ok_or(ProvisionerError::OrgNotFound(org_id))
    }

    /// Local row exists. Confirm the remote does too, recreating it if the
    /// project was wiped provider-side, and clear a stale `failed` status.
    async fn reconcile_existing(
        &self,
        local: oltp_tenants::Model,
    ) -> Result<oltp_tenants::Model, ProvisionerError> {
        // A row provisioned on one provider must never be reconciled against
        // another. The ids are not portable — a LocalProvider `project_id` is a
        // database name with underscores, which Neon rejects outright — but the
        // format error is luck, not safety. Had it parsed, Neon would have
        // answered 404, `get_project` would return `Ok(None)`, and the branch
        // below would "recreate" a second database while this row still pointed
        // at the first. Refuse instead: switching providers is a migration, and
        // an operator has to decide what happens to the existing data.
        if local.provider != self.provider.name() {
            return Err(ProvisionerError::ProviderMismatch {
                org_id: local.org_id,
                recorded: local.provider.clone(),
                configured: self.provider.name(),
            });
        }

        if let Some(remote) = self.provider.get_project(&local.project_id).await? {
            // The fetched project is USED, not merely counted. Its
            // `pg_version` is the provider's live answer, and until now it was
            // discarded — the row was stamped once at creation and never
            // re-read, so a cluster upgraded under an existing tenant could
            // never show up. That is the one drift `oxy oltp status`'s pg line
            // exists for, and on the local provider it was the only kind
            // possible.
            return self.mark_active(local, &remote).await;
        }

        warn!(
            project_id = %local.project_id,
            // Not "missing": `get_project` also answers `None` for a database
            // that exists and is not ours, where nothing will be recreated —
            // `create_project` refuses two lines down and names the owner. It
            // logs that reason itself, so this stays neutral rather than
            // contradicting it in the same run.
            "no OLTP project of ours under this name; attempting to create it"
        );
        // No preflight here, and that is not an oversight: the row already
        // exists, so a failure in `apply_remote` leaves it in place rather than
        // stranding anything, and both providers now adopt the derived name, so
        // a retry converges on the same project. That convergence is a
        // load-bearing consequence of adoption, not a coincidence.
        let created = self.create_remote(&local.project_name).await?;
        let mut active: oltp_tenants::ActiveModel = local.into();
        apply_remote(&mut active, &created, self.provider.name())?;
        active.status = ActiveValue::Set(TenantStatus::Active);
        // The recreated project is an empty database. Leaving the recorded
        // version in place would claim Oxy's objects exist when they don't, and
        // reconcile would then skip it forever.
        active.platform_schema_version = ActiveValue::Set(0);
        active.updated_at = ActiveValue::Set(Utc::now().into());
        Ok(active.update(&self.db).await?)
    }

    async fn create_new(&self, org_id: Uuid) -> Result<oltp_tenants::Model, ProvisionerError> {
        let project_name = project_name_for(org_id);

        // Exercise the key material BEFORE anything is created.
        //
        // `apply_remote` seals the owner password, so whatever the crypto layer
        // does on a bad key it does between the provider call and the row
        // insert — which strands a created database with no `oltp_tenants` row.
        // Doing it here costs one seal and one open of a constant and moves it
        // to a point where nothing exists to strand.
        //
        // **What actually fires is a panic, not this `?`.** I wrote this
        // believing `seal` returns `Err` on missing or unreadable key material;
        // neither is true of `platform::secrets::encryption`. A malformed
        // `OXY_ENCRYPTION_KEY` panics inside `decode_key_from_string` (`expect`
        // on the base64, `panic!` on a wrong length), and a MISSING one does
        // not fail at all — `get_encryption_key` generates a fresh key and
        // writes it to the state dir. So the round trip's real value is moving
        // that panic earlier, and the `?` covers only a genuine AEAD failure.
        //
        // The case this CANNOT catch is the one worth naming: with
        // `OXY_ENCRYPTION_KEY` unset on a multi-instance fleet, each instance
        // fabricates its own key, so this preflight passes everywhere and the
        // owner password sealed by one instance is undecryptable on every
        // other. That surfaces later as a `Crypto` error on a credential that
        // was written successfully, and no probe here can distinguish a
        // fabricated key from a configured one.
        let probe = seal("oltp-provision-preflight")?;
        open(&probe)?;

        let created = self.create_remote(&project_name).await?;

        // Fail here rather than after the row lands: without the owner password
        // no schema DDL can ever run, and the password is disclosed only once.
        // This one genuinely cannot be hoisted — it is a property of the
        // provider's answer — but it is recoverable now that a derived name is
        // adopted on retry rather than colliding.
        if created.owner_role.password.is_none() {
            return Err(ProvisionerError::Crypto(
                "provider disclosed no owner password".into(),
            ));
        }

        let mut active = oltp_tenants::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            org_id: ActiveValue::Set(org_id),
            status: ActiveValue::Set(TenantStatus::Active),
            created_at: ActiveValue::Set(Utc::now().into()),
            updated_at: ActiveValue::Set(Utc::now().into()),
            ..Default::default()
        };
        apply_remote(&mut active, &created, self.provider.name())?;
        let row = active.insert(&self.db).await?;

        Ok(row)
    }

    async fn create_remote(&self, project_name: &str) -> Result<Project, ProvisionerError> {
        Ok(self
            .provider
            .create_project(CreateProjectRequest {
                name: project_name.to_string(),
                region_id: self.region.clone(),
                pg_version: self.pg_version,
            })
            .await?)
    }

    async fn mark_active(
        &self,
        local: oltp_tenants::Model,
        remote: &crate::provider::Project,
    ) -> Result<oltp_tenants::Model, ProvisionerError> {
        // The early return covers status only, so the version has to be checked
        // first: an Active tenant is exactly the case where a cluster upgrade
        // would otherwise never be recorded.
        let observed = i16::from(remote.pg_version);
        if local.status == TenantStatus::Active && local.pg_version == observed {
            return Ok(local);
        }
        if local.pg_version != observed {
            info!(
                project_id = %local.project_id,
                from = local.pg_version,
                to = observed,
                "recorded Postgres major version changed"
            );
        }
        let mut active: oltp_tenants::ActiveModel = local.into();
        active.status = ActiveValue::Set(TenantStatus::Active);
        active.pg_version = ActiveValue::Set(observed);
        active.updated_at = ActiveValue::Set(Utc::now().into());
        Ok(active.update(&self.db).await?)
    }

    /// Bring a tenant's Oxy-owned objects up to [`platform::PLATFORM_SCHEMA_VERSION`].
    ///
    /// Idempotent, and free when already current: it opens no connection, which
    /// matters because waking a scale-to-zero database costs money. Every step
    /// is itself idempotent, so this repairs drift rather than only filling
    /// gaps.
    ///
    /// Call it wherever a tenant is touched — provision, publish, function
    /// invocation, pipeline run — and idle tenants converge lazily. An operator
    /// sweep can force it when a change is security-relevant.
    #[instrument(skip(self), fields(project_id = %row.project_id))]
    pub async fn reconcile_platform_schema(
        &self,
        row: &oltp_tenants::Model,
    ) -> Result<oltp_tenants::Model, ProvisionerError> {
        let from = row.platform_schema_version;
        if from >= platform::PLATFORM_SCHEMA_VERSION {
            return Ok(row.clone());
        }

        let statements = platform::statements_since(from, &row.provider, &row.database_name)?;
        let owner_password = self.owner_password(row, row.org_id)?;
        self.run_batch(row, &owner_password, statements).await?;

        // Recorded only after the batch succeeds. A tenant left behind is
        // recoverable on its next touch; one recorded as current but missing
        // the change never gets it.
        let mut active: oltp_tenants::ActiveModel = row.clone().into();
        active.platform_schema_version = ActiveValue::Set(platform::PLATFORM_SCHEMA_VERSION);
        active.updated_at = ActiveValue::Set(Utc::now().into());
        let updated = active.update(&self.db).await?;

        info!(
            from,
            to = platform::PLATFORM_SCHEMA_VERSION,
            "reconciled tenant platform schema"
        );
        Ok(updated)
    }

    fn owner_password(
        &self,
        tenant: &oltp_tenants::Model,
        org_id: Uuid,
    ) -> Result<String, ProvisionerError> {
        let sealed = tenant
            .owner_password_ciphertext
            .as_ref()
            .ok_or(ProvisionerError::OwnerPasswordMissing(org_id))?;
        open(sealed)
    }

    /// All DDL runs as the database owner, never as the writer being granted.
    async fn run_batch(
        &self,
        tenant: &oltp_tenants::Model,
        owner_password: &str,
        statements: Vec<String>,
    ) -> Result<(), ProvisionerError> {
        let dsn = dsn_for(tenant, &tenant.owner_role, owner_password);
        self.sql.execute_batch(&dsn, &statements).await?;
        Ok(())
    }
}

pub(crate) fn dsn_for(tenant: &oltp_tenants::Model, role: &str, password: &str) -> String {
    format!(
        "postgres://{role}:{password}@{host}/{db}?sslmode={ssl}",
        password = crate::roles::encode_userinfo(password),
        host = tenant.host,
        db = tenant.database_name,
        ssl = sslmode_for(&tenant.provider),
    )
}

/// TLS mode for a provider's connections.
///
/// Every managed provider terminates TLS and must be `require` — sending a
/// tenant's credentials in the clear over the internet is not a thing to get
/// wrong, so this defaults to `require` for anything unrecognised. The local
/// POC provider is the sole exception: a throwaway container on loopback has no
/// certificate, and `require` fails the handshake outright.
///
/// The `host:port` if it is NOT a loopback address, for the plaintext warning.
///
/// Splits on the LAST colon so a bracketed IPv6 authority survives —
/// `[2001:db8::1]:5432` → host `[2001:db8::1]` — and recognises every spelling
/// of loopback (`localhost`, IPv4, and IPv6 bracketed or bare).
fn non_loopback_host(hostport: &str) -> Option<&str> {
    let host = hostport.rsplit_once(':').map_or(hostport, |(h, _)| h);
    let loopback = matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]");
    (!loopback).then_some(host)
}

/// Read from the tenant's **persisted** provider name rather than current
/// config, so changing config can't retroactively alter how an existing tenant
/// connects.
pub(crate) fn sslmode_for(provider: &str) -> &'static str {
    match provider {
        "local" => "disable",
        _ => "require",
    }
}

/// Whether the analyst/writer connector must **verify** the server certificate
/// for `provider`, not merely encrypt to it.
///
/// This is a different axis from [`sslmode_for`]. `require` in a DSN — and our
/// connector's own weaker mode — means "encrypt, do not fall back to plaintext";
/// neither authenticates the peer. So `require` alone leaves the analyst
/// password and every row it reads exposed to an active MITM that can present
/// any certificate. A managed tenant's credentials cross the public internet, so
/// that path must check the chain.
///
/// True for every managed provider: Neon presents a publicly-trusted
/// certificate that `webpki_roots` (the connector's `verify-full` trust anchor)
/// covers. False for `local`, where [`sslmode_for`] returns `disable` and no TLS
/// is negotiated at all — a loopback container has no certificate to verify.
///
/// **Deliberately keyed on the provider, not on a stored CA.** A future managed
/// provider on a PRIVATE CA (RDS's Amazon root, in-cluster CloudNativePG) would
/// need `require` WITHOUT verify, and at that point this stops being derivable
/// from the name and must carry the anchor. Neon is the only managed provider
/// today, and its cert is public, so the name is enough.
pub(crate) fn verify_tls_for(provider: &str) -> bool {
    !matches!(provider, "local")
}

/// Copy provider-returned coordinates onto a tenant row, sealing the owner
/// password if this response disclosed one.
fn apply_remote(
    active: &mut oltp_tenants::ActiveModel,
    created: &Project,
    provider_name: &str,
) -> Result<(), ProvisionerError> {
    active.provider = ActiveValue::Set(provider_name.to_string());
    active.project_id = ActiveValue::Set(created.id.clone());
    active.branch_id = ActiveValue::Set(created.branch.id.clone());
    active.project_name = ActiveValue::Set(created.name.clone());
    active.region = ActiveValue::Set(created.region_id.clone());
    active.pg_version = ActiveValue::Set(created.pg_version as i16);
    active.host = ActiveValue::Set(created.host.clone());
    active.database_name = ActiveValue::Set(created.database.name.clone());
    active.owner_role = ActiveValue::Set(created.owner_role.name.clone());
    if let Some(pw) = &created.owner_role.password {
        active.owner_password_ciphertext = ActiveValue::Set(Some(seal(pw)?));
    }
    Ok(())
}

fn seal(plaintext: &str) -> Result<Vec<u8>, ProvisionerError> {
    envelope::seal(plaintext.as_bytes()).map_err(|e| ProvisionerError::Crypto(e.to_string()))
}

/// Whether a batch failed on [`crate::roles::assert_confined_sql`] rather than
/// anything else.
///
/// Matched on the SQLSTATE, which `pg_detail` puts first in `source_message`.
///
/// This doc used to say the check "reaches us as a plain SQL error with no
/// distinguishing SQLSTATE", three lines above a body that reads exactly that
/// SQLSTATE. The prose was true of the FIRST spelling, which matched the prose
/// "is not confined" and so fired on any error echoing the statement text; the
/// fix moved to `USING ERRCODE = 'OXY01'` and the doc did not follow.
fn is_unconfined(e: &SqlError) -> bool {
    // The SQLSTATE out of `source_message`, NOT out of `to_string()`.
    //
    // `SqlError::Statement`'s Display is `"statement failed ({statement}):
    // {source_message}"`, and the statement here is `assert_confined_sql`,
    // whose own text contains `USING ERRCODE = 'OXY01'`. So matching the
    // rendered error meant a statement timeout, a dropped connection or a Neon
    // compute waking up ALL classified as "this role is over-privileged" and
    // routed to the destructive branch — `provider.delete_role` — on a
    // credential that was very likely fine. The first spelling of this bug
    // matched the prose "is not confined" for the same reason; moving to a
    // SQLSTATE fixed the wrong half.
    //
    // `pg_detail` puts the real SQLSTATE first in `source_message`, so an
    // anchored prefix cannot be satisfied by anything the server echoed back.
    match e {
        SqlError::Statement { source_message, .. } => source_message.starts_with("[OXY01]"),
        _ => false,
    }
}

fn open(ciphertext: &[u8]) -> Result<String, ProvisionerError> {
    let bytes = envelope::open(ciphertext).map_err(|e| ProvisionerError::Crypto(e.to_string()))?;
    String::from_utf8(bytes).map_err(|e| ProvisionerError::Crypto(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_hosts_do_not_warn_and_others_do() {
        // Loopback in every spelling — no warning.
        for hp in [
            "localhost:15432",
            "127.0.0.1:5432",
            "[::1]:5432",
            "localhost",
        ] {
            assert_eq!(non_loopback_host(hp), None, "{hp} is loopback");
        }
        // Non-loopback, including a bracketed IPv6 authority whose own colons
        // must not be mistaken for the port separator.
        assert_eq!(non_loopback_host("db.internal:5432"), Some("db.internal"));
        assert_eq!(non_loopback_host("10.0.0.5:5432"), Some("10.0.0.5"));
        assert_eq!(
            non_loopback_host("[2001:db8::1]:5432"),
            Some("[2001:db8::1]"),
            "the bracket-preserving split is the point"
        );
    }

    /// Adoption is scoped to names Oxy derives, and nothing else.
    ///
    /// The whole safety argument is that `oxy-org-<uuid>` identifies one org,
    /// so a match cannot be a coincidence. A chosen name must keep the
    /// collision guard — that is what the `airhouse` incident was about, where
    /// adopting would have crossed two tenants.
    #[test]
    fn only_a_derived_project_name_is_adoptable() {
        let org = Uuid::new_v4();
        assert!(is_derived_project_name(&project_name_for(org)));

        for chosen in [
            "oxy-org-not-a-uuid",
            "oxy-org-",
            "bookings",
            "oxy-org-00000000-0000-0000-0000-00000000000",
            "prefix-oxy-org-11111111-2222-3333-4444-555555555555",
            // FORMAT, not merely shape. `Uuid::parse_str` accepts all three of
            // these; `project_name_for` renders none of them, and on Neon this
            // predicate is the only thing between a chosen name and an adoption
            // that resets someone's owner password.
            "oxy-org-{11111111-2222-3333-4444-555555555555}",
            "oxy-org-11111111222233334444555555555555",
            "oxy-org-urn:uuid:11111111-2222-3333-4444-555555555555",
            "oxy-org-11111111-2222-3333-4444-555555555555-suffix",
        ] {
            assert!(
                !is_derived_project_name(chosen),
                "{chosen} must keep the collision guard"
            );
        }
    }

    /// The destructive branch must fire on the CHECK failing and on nothing
    /// else. Every case below is a real failure of the very statement that
    /// carries `ERRCODE = 'OXY01'` in its text, which is why matching the
    /// rendered error could not tell them apart.
    #[test]
    fn only_the_confinement_check_itself_counts_as_unconfined() {
        let confined_check = crate::roles::assert_confined_sql("app_x_rw").unwrap();
        let raised = SqlError::Statement {
            statement: confined_check.clone(),
            source_message: "[OXY01] role \"app_x_rw\" is not confined: CREATEDB".to_string(),
        };
        assert!(is_unconfined(&raised));

        for source_message in [
            // A statement timeout on that statement.
            "[57014] canceling statement due to statement timeout",
            // The connection dying mid-batch.
            "connection closed",
            // A Neon compute waking up.
            "[57P03] the database system is starting up",
        ] {
            let e = SqlError::Statement {
                statement: confined_check.clone(),
                source_message: source_message.to_string(),
            };
            assert!(
                !is_unconfined(&e),
                "`{source_message}` classified as unconfined — it would delete a working role"
            );
        }

        assert!(!is_unconfined(&SqlError::Connect(
            "[OXY01] whatever".into()
        )));
    }

    #[test]
    fn project_name_is_deterministic_and_derivable_from_the_org() {
        let org = Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap();
        assert_eq!(
            project_name_for(org),
            "oxy-org-11111111-2222-3333-4444-555555555555"
        );
        assert_eq!(project_name_for(org), project_name_for(org));
    }

    #[test]
    fn only_the_local_provider_disables_tls() {
        assert_eq!(sslmode_for("local"), "disable");
        // Anything managed, and anything unrecognised, must require TLS —
        // failing closed matters more here than a working connection.
        for p in ["neon", "mock", "", "LOCAL", "somethingnew"] {
            assert_eq!(sslmode_for(p), "require", "provider {p:?} must require TLS");
        }
    }

    #[test]
    fn only_the_local_provider_skips_cert_verification() {
        // `require` encrypts but does not authenticate the peer, so the analyst
        // connector upgrades a verifying provider to `verify-full`. Exactly the
        // providers that require TLS must also verify it — a managed tenant on a
        // public cert cannot be handed to an unauthenticated server.
        assert!(!verify_tls_for("local"), "loopback has no cert to verify");
        for p in ["neon", "mock", "", "LOCAL", "somethingnew"] {
            assert!(
                verify_tls_for(p),
                "provider {p:?} requires TLS, so it must also verify the cert"
            );
        }
    }

    /// A provider switch must be refused, not reconciled.
    ///
    /// Found by pointing a control plane that already held a `local` tenant at
    /// real Neon: the recorded `project_id` was a database name with
    /// underscores, which Neon rejected on format. That error was luck. Had the
    /// id parsed, Neon would have answered 404, `get_project` would return
    /// `Ok(None)`, and reconcile would have created a SECOND database while the
    /// row still pointed at the first.
    /// `is_unconfined` gates a DESTRUCTIVE branch — it deletes the role through
    /// the provider — so it must fire only on the confinement check's own
    /// signal.
    ///
    /// It used to match the substring "is not confined", which appears in
    /// `assert_confined_sql`'s SQL **text**; `SqlError::Statement` embeds the
    /// failing statement, so a dropped connection or a timeout on that
    /// statement classified as unconfined and dropped the role.
    #[test]
    fn only_the_confinement_check_triggers_the_destructive_branch() {
        let real = SqlError::Statement {
            statement: "DO $$ ... $$".to_string(),
            source_message: "[OXY01] role app_x_rw is not confined: createdb".to_string(),
        };
        assert!(is_unconfined(&real));

        // The statement text alone must not be enough — this is the shape that
        // used to match: the SQL echoed back after an unrelated failure.
        let timeout = SqlError::Statement {
            statement: "DO $$ ... RAISE EXCEPTION 'role % is not confined: %' ... $$".to_string(),
            source_message: "[57014] canceling statement due to statement timeout".to_string(),
        };
        assert!(
            !is_unconfined(&timeout),
            "a timeout must not delete the role"
        );

        let dropped = SqlError::Connect("connection closed".to_string());
        assert!(
            !is_unconfined(&dropped),
            "a dropped connection must not either"
        );
    }

    #[test]
    fn a_provider_mismatch_names_both_sides_and_refuses() {
        let err = ProvisionerError::ProviderMismatch {
            org_id: Uuid::from_u128(7),
            recorded: "local".to_string(),
            configured: "neon",
        };
        let msg = err.to_string();
        // Both providers must appear: an operator's next move depends on which
        // direction they were switching.
        assert!(
            msg.contains("local"),
            "must name the recorded provider: {msg}"
        );
        assert!(
            msg.contains("neon"),
            "must name the configured provider: {msg}"
        );
        assert!(
            msg.contains("deprovision"),
            "must say how to proceed, not just that it refused: {msg}"
        );
    }

    #[test]
    fn a_namespace_conflict_names_both_workspaces_and_the_fix() {
        let owner = Uuid::from_u128(1);
        let claimant = Uuid::from_u128(2);
        let err = ProvisionerError::SchemaNamespaceClaimed {
            org_id: Uuid::from_u128(9),
            schema: "app_bookings".into(),
            owner,
            claimant,
        };
        let msg = err.to_string();
        // An operator hitting this needs to know who holds it and what to do —
        // "conflict" alone sends them digging through two repos.
        assert!(msg.contains("app_bookings"), "got {msg}");
        assert!(msg.contains(&owner.to_string()), "got {msg}");
        assert!(msg.contains(&claimant.to_string()), "got {msg}");
        assert!(msg.contains("Rename the writer"), "got {msg}");
    }

    #[test]
    fn writer_credentials_debug_redacts_the_password() {
        let creds = WriterCredentials {
            schema_name: "app_bookings".into(),
            role_name: "app_bookings_rw".into(),
            dsn: "postgres://app_bookings_rw:sup3rs3cret@host/db".into(),
        };
        let rendered = format!("{creds:?}");
        assert!(!rendered.contains("sup3rs3cret"), "got {rendered}");
        assert!(rendered.contains("app_bookings_rw"), "got {rendered}");
    }

    #[test]
    fn provider_errors_that_are_not_retryable_stay_that_way_through_conversion() {
        let err: ProvisionerError = ProviderError::ProjectNameTaken("oxy-org-x".into()).into();
        assert!(matches!(
            err,
            ProvisionerError::Provider(ProviderError::ProjectNameTaken(_))
        ));
    }
}

/// Build a provisioner from the `OXY_OLTP_*` environment.
///
/// Shared by `oxy oltp` and the admin console so the two can never disagree
/// about which provider an org is being provisioned on — an operator clicking
/// Provision must get the same database an engineer would get from the CLI.
///
/// `OXY_OLTP_PROVIDER=neon` provisions for real; otherwise this falls back to a
/// local cluster, which demands an explicit admin DSN so nobody reaches it by
/// accident. A **misconfigured** Neon setup is an error rather than a silent
/// downgrade: asking for Neon and getting a local database that looks correct
/// until production is the worst of the three outcomes.
pub async fn from_env(db: DatabaseConnection) -> Result<OltpProvisioner, ProvisionerError> {
    let cfg_err = ProvisionerError::NotConfigured;

    match crate::config::OltpConfig::from_env() {
        crate::config::OltpConfig::Enabled(cfg) => match &cfg.provider {
            crate::config::ProviderKind::Neon { api_key, org_id } => {
                return Ok(OltpProvisioner::new(
                    db,
                    Arc::new(crate::provider::NeonProvider::new(
                        api_key.clone(),
                        org_id.clone(),
                    )),
                    Arc::new(crate::sql::PgSqlExecutor),
                    cfg.region.clone(),
                    cfg.pg_version,
                ));
            }
            // Named explicitly rather than inferred from an absent provider:
            // `oxy` loads `.env` itself, so a shell that unset the variable to
            // mean "local" had it restored inside the binary — and a demo asked
            // for local provisioned against Neon.
            crate::config::ProviderKind::Local { admin_url } => {
                // Local speaks plaintext (`sslmode=disable`, `NoTls`), which is
                // fine for the throwaway loopback cluster it is meant for and a
                // credential leak for anything else. `.env.example` says never
                // point it at a shared or staging box; this makes ignoring that
                // loud instead of silent, since the DSN carries a password and
                // this provider never negotiates TLS.
                let hostport = crate::provider::host_from_dsn(admin_url);
                if let Some(host) = non_loopback_host(&hostport) {
                    // Once per process: `from_env` runs per CLI invocation and
                    // per admin request, and a misconfigured deployment should
                    // stay loud without warning on every provision click.
                    static WARNED: std::sync::Once = std::sync::Once::new();
                    WARNED.call_once(|| {
                        tracing::warn!(
                            host = %host,
                            "OXY_OLTP_PROVIDER=local points at a NON-loopback host and sends \
                             the tenant password in plaintext — use the `neon` provider (TLS \
                             required) for any cluster that is not a local throwaway"
                        );
                    });
                }
                return Ok(OltpProvisioner::new(
                    db,
                    Arc::new(crate::provider::LocalProvider::new(
                        admin_url.clone(),
                        hostport,
                    )),
                    Arc::new(crate::sql::PgSqlExecutor),
                    "local",
                    cfg.pg_version,
                ));
            }
            // Mock documents itself as "provisions nothing real", and used to
            // fall through to the LocalProvider branch below — which creates an
            // actual database. Silently doing the opposite of what the config
            // says is the single thing a tri-state config exists to prevent.
            crate::config::ProviderKind::Mock => {
                return Ok(OltpProvisioner::new(
                    db,
                    Arc::new(crate::provider::MockProvider::new()),
                    Arc::new(crate::sql::PgSqlExecutor),
                    cfg.region.clone(),
                    cfg.pg_version,
                ));
            }
        },
        crate::config::OltpConfig::Misconfigured(reason) => {
            return Err(cfg_err(reason.to_string()));
        }
        // Disabled is an ERROR, not a fallback.
        //
        // The tail below used to read OXY_DATABASE_URL when OXY_OLTP_ADMIN_URL
        // was absent — and OXY_DATABASE_URL is always set. So a deployment that
        // never configured OLTP got a LocalProvider pointed at Oxy's OWN
        // control plane, and two later changes made that dangerous rather than
        // merely odd:
        //
        //   `role_admin_dsn` hands that DSN back for role DDL, so `CREATE ROLE`
        //   would run on the control-plane connection as Oxy's own user.
        //
        //   Org deletion now calls this on every delete. One fleet instance
        //   missing OXY_OLTP_PROVIDER=neon would drop the `oltp_tenants` row
        //   while `LocalProvider::delete_project` ran
        //   `DROP DATABASE IF EXISTS "<neon-project-id>"` as a local no-op —
        //   losing Oxy's only record of a project that keeps billing and keeps
        //   holding customer data.
        //
        // `local` is a named provider now, so nothing needs the implicit route.
        crate::config::OltpConfig::Disabled => {
            return Err(cfg_err(
                "OLTP is not configured. Set OXY_OLTP_PROVIDER to `local` (with \
                 OXY_OLTP_ADMIN_URL), `neon` (with OXY_OLTP_NEON_API_KEY \
                 and OXY_OLTP_NEON_ORG_ID), \
                 or `mock`."
                    .into(),
            ));
        }
    }
    // Nothing follows the match. The implicit-local fallback that used to live
    // here is the one the `Disabled` arm above describes; it was left behind
    // unreachable, which is how a deleted safety property comes back.
}
