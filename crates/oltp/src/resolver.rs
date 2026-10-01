//! Resolving a `postgres_managed` database reference into a live connection.
//!
//! The counterpart to `airhouse`'s broker, and deliberately narrower. Airhouse
//! mints a fresh ephemeral credential per `(workspace_id, subject, role)` and
//! picks the role from the caller's `effective_role`. Here there is exactly one
//! answer for every human and agent: **the read-only analyst**.
//!
//! That is the invariant, not an omission — see [`crate::schema::ANALYST_ROLE`].
//! A writable connection is issued only to published code, through
//! [`crate::provisioner::OltpProvisioner::ensure_writer`].
//!
//! # Grain crossing
//!
//! Callers hold a `workspace_id`; tenants are keyed by `org_id`. The hop goes
//! through `workspaces.org_id`, which is a lookup rather than an inference.

use oxy_platform::secrets::envelope;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;

use crate::entity::tenants::{self as oltp_tenants, Entity as OltpTenants, TenantStatus};
use crate::schema::{GrantLevel, WriterRef};

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("workspace {0} not found")]
    WorkspaceNotFound(Uuid),
    #[error("workspace {0} has no organization; postgres_managed needs an org-scoped tenant")]
    WorkspaceHasNoOrg(Uuid),
    #[error(
        "org {0} has no OLTP database yet — provision one from Settings → OLTP Database, or run `just oltp-seed`"
    )]
    NotProvisioned(Uuid),
    #[error("org {0}'s OLTP database is not active (status: {1})")]
    NotActive(Uuid, &'static str),
    #[error(
        "org {0}'s OLTP database has no analyst credential yet; re-run provisioning to mint one"
    )]
    NoAnalystCredential(Uuid),
    // Names the command, not just the situation. `writer` already holds the
    // `app:<slug>` / `pipeline:<source>` spec that `--writer` takes, so the
    // message is a line you can paste — and the old "before the app writes"
    // wording was wrong for the pipeline half of the same error.
    #[error(
        "org {org_id} has no provisioned writer for {writer}. \
         Run: oxy oltp provision --org {org_id} --writer {writer}"
    )]
    WriterNotProvisioned { org_id: Uuid, writer: String },
    #[error("database error: {0}")]
    Db(#[from] sea_orm::DbErr),
    #[error("envelope crypto failed: {0}")]
    Crypto(String),
    /// The branch exists but is mid-provision or mid-reset. Refused rather than
    /// served: its credentials may be the ones the reset is replacing.
    #[error(
        "org {0}'s {1} OLTP branch is not active (status: {2}). {fix}",
        fix = branch_fix(.0, .1, .2)
    )]
    BranchNotActive(Uuid, crate::OltpBranch, &'static str),
    /// The branch row names production — its branch id, or its exact host and
    /// database. Nothing records that on purpose, and serving it would hand a
    /// staging caller a writable production connection, so it is refused here
    /// as well as wherever the row is written.
    #[error(
        "org {0}'s {1} OLTP branch is recorded as production's own database; refusing to \
         resolve it"
    )]
    BranchIsProduction(Uuid, crate::OltpBranch),
    /// The branch has no credential for a role production has — a writer
    /// provisioned after the branch was cut. Re-running the branch provision
    /// mints it and gives it its schema there, without touching anyone's data.
    #[error(
        "org {org_id}'s {branch} OLTP branch has no credential for {role}. \
         Run: oxyc oltp provision --org {org_id} --branch {branch}"
    )]
    BranchCredentialMissing {
        org_id: Uuid,
        branch: crate::OltpBranch,
        role: String,
    },
}

/// The command that gets a branch out of `status` — they differ, and naming
/// the wrong one sends an operator to a provision that refuses a reset in flight.
fn branch_fix(org_id: &Uuid, branch: &crate::OltpBranch, status: &str) -> String {
    match status {
        "resetting" => format!(
            "A reset is running or failed part way; finish it with: \
             oxyc oltp reset --org {org_id} --branch {branch}"
        ),
        _ => format!(
            "It was never finished; finish it with: \
             oxyc oltp provision --org {org_id} --branch {branch}"
        ),
    }
}

// Non-production branches: same connection shapes, pointed at the branch.
mod branch;
pub use branch::{
    BranchWriter, branch_is_active, resolve_branch_analyst_connection_for_org,
    resolve_branch_writer_connection_for_org, resolve_branch_writer_for_org,
};

/// Read-only connection coordinates for a workspace's per-org OLTP database.
///
/// Carries a password, so [`std::fmt::Debug`] redacts it by hand — a derived
/// impl would leak the credential into every `tracing` span it touched.
#[derive(Clone)]
pub struct AnalystConnection {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub user: String,
    pub password: String,
    /// `disable` locally, `require` on a managed provider.
    ///
    /// Carried rather than derived by the caller: this DSN is what `oxy oltp
    /// connect`, `oxy oltp dsn --role analyst` and the console's connection
    /// panel hand an operator, and it omitted `sslmode` entirely — libpq
    /// defaults to `prefer`, which downgrades to plaintext without saying so.
    /// Against Neon that is a credential crossing the public internet in the
    /// clear. The writer DSN two functions below always set it; this one did not.
    pub sslmode: String,
    /// Whether the CONNECTOR must verify the server certificate — a separate
    /// axis from `sslmode`. `sslmode` governs the DSN string an operator's psql
    /// gets (only `disable`/`prefer`/`require` parse there); this governs whether
    /// the in-process analytics connector authenticates the peer or merely
    /// encrypts. `require` does not check the chain, so a managed tenant needs
    /// this `true` or an active MITM captures the analyst password and its rows.
    /// See [`crate::provisioner::verify_tls_for`].
    pub verify_tls: bool,
}

impl std::fmt::Debug for AnalystConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnalystConnection")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("database", &self.database)
            .field("user", &self.user)
            .field("password", &"<redacted>")
            .field("verify_tls", &self.verify_tls)
            .finish()
    }
}

impl AnalystConnection {
    pub fn dsn(&self) -> String {
        format!(
            "postgres://{}:{}@{}:{}/{}?sslmode={}",
            self.user,
            // See `roles::encode_userinfo`: an unescaped `@` in a password
            // makes libpq parse the remainder as the hostname.
            crate::roles::encode_userinfo(&self.password),
            self.host,
            self.port,
            self.database,
            self.sslmode
        )
    }
}

/// Resolve the analyst connection for a workspace's org.
///
/// Note what this function does **not** take: no `effective_role`, no subject.
/// There is nothing a caller can pass that widens the access it returns.
pub async fn resolve_analyst_connection(
    db: &DatabaseConnection,
    workspace_id: Uuid,
) -> Result<AnalystConnection, ResolveError> {
    let org_id = org_for_workspace(db, workspace_id).await?;
    analyst_connection_for_org(db, org_id).await
}

/// The org-keyed core both public entry points share.
async fn analyst_connection_for_org(
    db: &DatabaseConnection,
    org_id: Uuid,
) -> Result<AnalystConnection, ResolveError> {
    let tenant = OltpTenants::find()
        .filter(oltp_tenants::Column::OrgId.eq(org_id))
        .one(db)
        .await?
        .ok_or(ResolveError::NotProvisioned(org_id))?;

    if tenant.status != TenantStatus::Active {
        return Err(ResolveError::NotActive(org_id, tenant.status.as_str()));
    }

    let sealed = tenant
        .analyst_password_ciphertext
        .as_ref()
        .ok_or(ResolveError::NoAnalystCredential(org_id))?;
    let password = open(sealed)?;

    let (host, port) = split_host_port(&tenant.host);
    Ok(AnalystConnection {
        host,
        port,
        database: tenant.database_name.clone(),
        // Derived, not the bare constant: on a shared-cluster provider the
        // analyst's real name is qualified per tenant. Same function the
        // provisioner minted it with, so no column is needed to carry it.
        user: crate::schema::analyst_role_for(&tenant.provider, &tenant.database_name),
        password,
        sslmode: crate::provisioner::sslmode_for(&tenant.provider).to_string(),
        verify_tls: crate::provisioner::verify_tls_for(&tenant.provider),
    })
}

/// Read-write connection for one writer. Same shape as
/// [`AnalystConnection`], with the `search_path` already pinned to the
/// writer's schema.
#[derive(Clone)]
pub struct WriterConnection {
    pub schema: String,
    pub role: String,
    pub dsn: String,
    /// Whether a connector built from `dsn` must verify the server certificate
    /// — a managed writer's peer (Neon, public cert) must be authenticated, not
    /// merely encrypted to. The DSN carries `sslmode=require` for libpq, which
    /// only encrypts; a caller building an in-process connector (`ctx.oltp`)
    /// passes this to `PostgresConnector::from_dsn`. See
    /// [`crate::provisioner::verify_tls_for`]. (An external consumer that only
    /// gets the string — e.g. Airway — cannot use this and stays encrypt-only.)
    pub verify_tls: bool,
}

impl std::fmt::Debug for WriterConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WriterConnection")
            .field("schema", &self.schema)
            .field("role", &self.role)
            .field("dsn", &"<redacted>")
            .finish()
    }
}

/// Same as [`resolve_analyst_connection`], for a caller that already holds the
/// org — the admin console does, and it is not a member of any workspace in it.
pub async fn resolve_analyst_connection_for_org(
    db: &DatabaseConnection,
    org_id: Uuid,
) -> Result<AnalystConnection, ResolveError> {
    analyst_connection_for_org(db, org_id).await
}

/// Resolve a **writable** connection for one writer.
///
/// Unlike [`resolve_analyst_connection`] this yields DML rights — which is why
/// it takes a concrete [`WriterRef`] rather than a user. There is no path from
/// a human subject to this function: a writer is an app or a pipeline, named by
/// published code.
///
/// Reads the sealed password from `oltp_roles` rather than minting one, so this
/// costs a query and a decrypt — no provider round-trip on a request path.
pub async fn resolve_writer_connection(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    writer: &WriterRef,
) -> Result<WriterConnection, ResolveError> {
    let org_id = org_for_workspace(db, workspace_id).await?;
    resolve_writer_connection_for_org(db, org_id, writer).await
}

/// Whether a writer role is physically provisioned for this org — a cheap
/// existence check (tenant + role row), no decrypt and no provider round-trip.
///
/// Deliberately ignores the tenant's status, unlike
/// [`resolve_writer_connection_for_org`]: the role and its schema exist on the
/// tenant whether or not it is `Active`, so a guard that protects them —
/// refusing an app rename that would orphan the schema, or free its slug for
/// another app to claim — must see them in every state.
pub async fn writer_is_provisioned(
    db: &DatabaseConnection,
    org_id: Uuid,
    writer: &WriterRef,
) -> Result<bool, ResolveError> {
    let Some(tenant) = OltpTenants::find()
        .filter(oltp_tenants::Column::OrgId.eq(org_id))
        .one(db)
        .await?
    else {
        return Ok(false);
    };
    // `ReadWrite` MUST match `resolve_writer_connection_for_org`'s derivation:
    // this guard exists to see the same role the resolver would hand out, and a
    // future `ReadOnly` variant here would silently let a rename/delete through
    // while the writer role still stood. The two must move together.
    let role = crate::schema::qualify_role(
        &tenant.provider,
        &tenant.database_name,
        &writer.role_name(GrantLevel::ReadWrite),
    );
    let exists = crate::entity::roles::Entity::find()
        .filter(crate::entity::roles::Column::TenantRowId.eq(tenant.id))
        .filter(crate::entity::roles::Column::RoleName.eq(role))
        .one(db)
        .await?
        .is_some();
    Ok(exists)
}

/// Same, for callers that already hold the org — the function host does, so
/// making it hop through a workspace would be a query for nothing.
pub async fn resolve_writer_connection_for_org(
    db: &DatabaseConnection,
    org_id: Uuid,
    writer: &WriterRef,
) -> Result<WriterConnection, ResolveError> {
    let tenant = OltpTenants::find()
        .filter(oltp_tenants::Column::OrgId.eq(org_id))
        .one(db)
        .await?
        .ok_or(ResolveError::NotProvisioned(org_id))?;
    if tenant.status != TenantStatus::Active {
        return Err(ResolveError::NotActive(org_id, tenant.status.as_str()));
    }

    let role = crate::schema::qualify_role(
        &tenant.provider,
        &tenant.database_name,
        &writer.role_name(GrantLevel::ReadWrite),
    );
    let row = crate::entity::roles::Entity::find()
        .filter(crate::entity::roles::Column::TenantRowId.eq(tenant.id))
        .filter(crate::entity::roles::Column::RoleName.eq(role.clone()))
        .one(db)
        .await?
        .ok_or_else(|| ResolveError::WriterNotProvisioned {
            org_id,
            writer: writer.to_string(),
        })?;

    let password = open(&row.password_ciphertext)?;
    let (host, port) = split_host_port(&tenant.host);
    // One definition of "which providers need TLS", shared with the analyst
    // DSN and the provisioner — three copies is how they drifted apart.
    let ssl = crate::provisioner::sslmode_for(&tenant.provider);
    let base = format!(
        "postgres://{role}:{password}@{host}:{port}/{db}?sslmode={ssl}",
        password = crate::roles::encode_userinfo(&password),
        db = tenant.database_name
    );

    Ok(WriterConnection {
        schema: writer.schema_name(),
        role,
        dsn: crate::schema::with_search_path(&base, writer),
        verify_tls: crate::provisioner::verify_tls_for(&tenant.provider),
    })
}

async fn org_for_workspace(
    db: &DatabaseConnection,
    workspace_id: Uuid,
) -> Result<Uuid, ResolveError> {
    let ws = entity::workspaces::Entity::find_by_id(workspace_id)
        .one(db)
        .await?
        .ok_or(ResolveError::WorkspaceNotFound(workspace_id))?;
    ws.org_id
        .ok_or(ResolveError::WorkspaceHasNoOrg(workspace_id))
}

/// Tenants persist `host` as the provider returns it, which may or may not
/// carry a port. Neon omits it (5432 implied); the local provider includes one.
///
/// IPv6 needs explicit handling rather than a plain `rsplit_once(':')`: a bare
/// `::1` would otherwise split into host `::` and port `1`. Per RFC 3986 an
/// IPv6 literal carrying a port is bracketed (`[::1]:5432`), so an unbracketed
/// string with more than one colon is an address, not an address plus port.
pub(crate) fn split_host_port(host: &str) -> (String, u16) {
    const DEFAULT_PORT: u16 = 5432;

    // Bracketed IPv6: `[::1]` or `[::1]:5432`.
    if let Some(rest) = host.strip_prefix('[') {
        return match rest.split_once(']') {
            Some((addr, after)) => {
                let port = after
                    .strip_prefix(':')
                    .and_then(|p| p.parse::<u16>().ok())
                    .unwrap_or(DEFAULT_PORT);
                (addr.to_string(), port)
            }
            // Unterminated bracket — malformed; keep it whole rather than guess.
            None => (host.to_string(), DEFAULT_PORT),
        };
    }

    // Unbracketed with 2+ colons: a bare IPv6 address, no port.
    if host.matches(':').count() > 1 {
        return (host.to_string(), DEFAULT_PORT);
    }

    match host.rsplit_once(':') {
        Some((h, p)) => match p.parse::<u16>() {
            Ok(port) => (h.to_string(), port),
            Err(_) => (host.to_string(), DEFAULT_PORT),
        },
        None => (host.to_string(), DEFAULT_PORT),
    }
}

fn open(ciphertext: &[u8]) -> Result<String, ResolveError> {
    let bytes = envelope::open(ciphertext).map_err(|e| ResolveError::Crypto(e.to_string()))?;
    String::from_utf8(bytes).map_err(|e| ResolveError::Crypto(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    // Only the tests name the constant; the module body reaches it through
    // `schema::` paths.
    use crate::schema::ANALYST_ROLE;

    #[test]
    fn host_with_port_is_split() {
        assert_eq!(
            split_host_port("localhost:55432"),
            ("localhost".to_string(), 55432)
        );
    }

    #[test]
    fn host_without_port_defaults_to_5432() {
        assert_eq!(
            split_host_port("ep-1.aws-us-east-2.neon.tech"),
            ("ep-1.aws-us-east-2.neon.tech".to_string(), 5432)
        );
    }

    #[test]
    fn a_bare_ipv6_address_is_not_split_into_host_and_port() {
        // A plain rsplit_once(':') yields ("::", 1) here — the whole address
        // must survive instead.
        assert_eq!(split_host_port("::1"), ("::1".to_string(), 5432));
        assert_eq!(
            split_host_port("2001:db8::1"),
            ("2001:db8::1".to_string(), 5432)
        );
    }

    #[test]
    fn a_bracketed_ipv6_address_keeps_its_port() {
        assert_eq!(split_host_port("[::1]:55432"), ("::1".to_string(), 55432));
        assert_eq!(split_host_port("[::1]"), ("::1".to_string(), 5432));
    }

    #[test]
    fn a_non_numeric_suffix_is_not_mistaken_for_a_port() {
        assert_eq!(
            split_host_port("host:notaport"),
            ("host:notaport".to_string(), 5432)
        );
    }

    #[test]
    fn writer_connection_debug_redacts_the_password() {
        let c = WriterConnection {
            schema: "app_bookings".into(),
            role: "app_bookings_rw".into(),
            dsn: "postgres://app_bookings_rw:sup3rs3cret@h/db".into(),
            verify_tls: true,
        };
        let rendered = format!("{c:?}");
        assert!(!rendered.contains("sup3rs3cret"), "got {rendered}");
    }

    #[test]
    fn an_unprovisioned_writer_says_what_to_do() {
        let e = ResolveError::WriterNotProvisioned {
            org_id: Uuid::nil(),
            writer: "app:bookings".into(),
        };
        assert!(e.to_string().contains("app:bookings"), "got {e}");
        assert!(
            e.to_string()
                .contains("oxy oltp provision --org 00000000-0000-0000-0000-000000000000 --writer app:bookings"),
            "got {e}"
        );
    }

    #[test]
    fn analyst_connection_debug_redacts_the_password() {
        let c = AnalystConnection {
            host: "h".into(),
            port: 5432,
            database: "db".into(),
            user: ANALYST_ROLE.into(),
            password: "sup3rs3cret".into(),
            sslmode: "require".to_string(),
            verify_tls: true,
        };
        let rendered = format!("{c:?}");
        assert!(!rendered.contains("sup3rs3cret"), "got {rendered}");
        assert!(rendered.contains(ANALYST_ROLE), "got {rendered}");
    }

    #[test]
    fn dsn_carries_the_analyst_role_not_a_writer() {
        let c = AnalystConnection {
            host: "h".into(),
            port: 55432,
            database: "db".into(),
            user: ANALYST_ROLE.into(),
            password: "pw".into(),
            sslmode: "require".to_string(),
            verify_tls: true,
        };
        assert_eq!(
            c.dsn(),
            "postgres://oxy_analyst_ro:pw@h:55432/db?sslmode=require"
        );
        assert!(
            !c.dsn().contains("_rw"),
            "a resolve must never yield a writer"
        );
    }
    /// The analyst DSN is what `oxy oltp connect`, `oxy oltp dsn` and the admin
    /// console hand an operator. It carried no `sslmode` at all, so libpq used
    /// `prefer` — which downgrades to plaintext without saying so, i.e. a
    /// credential in the clear against a managed provider.
    #[test]
    fn the_analyst_dsn_demands_tls_off_a_managed_provider() {
        let managed = AnalystConnection {
            host: "ep-x.neon.tech".into(),
            port: 5432,
            database: "neondb".into(),
            user: "oxy_analyst_ro".into(),
            password: "Aa7#pw".into(),
            sslmode: "require".into(),
            verify_tls: true,
        };
        let dsn = managed.dsn();
        assert!(dsn.contains("sslmode=require"), "{dsn}");
        // And it must still parse — `#` starts a URI fragment unencoded, so a
        // generated password would otherwise truncate here.
        let cfg: tokio_postgres::Config = dsn.parse().expect("must parse");
        assert_eq!(cfg.get_password(), Some("Aa7#pw".as_bytes()));

        let local = AnalystConnection {
            sslmode: "disable".into(),
            ..managed
        };
        assert!(local.dsn().contains("sslmode=disable"), "{}", local.dsn());
    }
}
