//! Provider backed by a **local Postgres cluster**, for the POC.
//!
//! Mirrors `airhouse::config::autodetect_local_airhouse`, which wires local mode
//! to a running docker-compose stack without manual env setup. Here the "stack"
//! is the Postgres that `oxy start` already runs, so a demo needs no cloud
//! account and provisions nothing billable.
//!
//! Where Neon creates a *project* (its own isolated cluster), this creates a
//! *database* on the shared local cluster. Everything downstream — schemas,
//! roles, grants, the platform declaration — is identical, which is the point:
//! the demo exercises the real DDL, not a rehearsal of it.
//!
//! # One cluster, many tenants
//!
//! Every tenant lives in its own database on ONE cluster, so `CREATE DATABASE`
//! is enough to separate their data — but **roles are cluster-global**, and
//! `oxy_analyst_ro` / `app_<slug>_rw` are fixed names. Two tenants would
//! otherwise resolve to one role: one password, last mint wins, and the second
//! tenant's writer authenticating against the first tenant's database.
//!
//! [`crate::schema::qualify_role`] closes that by suffixing every role name
//! with a tag derived from the tenant's database, and
//! [`Self::role_admin_dsn`] gives role DDL the superuser it needs to manage
//! them. Neon needs neither — one project per tenant means no shared namespace.
//!

//!
//! **Postgres roles are cluster-global; schemas are not.** On Neon each project
//! is its own cluster, so `app_bookings_rw` in org A and org B never collide.
//! On one local cluster they would — so this provider **qualifies every role
//! name with a per-tenant tag** (`qualify_role` / `tenant_tag`), which is what
//! lets several tenants share one local cluster, each in its own database. The
//! tag is derived deterministically from the tenant's database name, so orphan
//! reconciliation still recomputes a role's name without a column to carry it.

use async_trait::async_trait;
use tokio_postgres::NoTls;

use super::types::{
    Branch, BranchRequest, CreateProjectRequest, DatabaseInfo, Project, ProjectBranch, Role,
};
use super::{OltpProvider, ProviderError};

// A branch here is a sibling DATABASE copied from the tenant's — see the module.
mod branches;
pub use branches::branch_database_name;

/// Fixed branch id. Local Postgres has no branching; the field exists to keep
/// the row shape identical to Neon's.
const LOCAL_BRANCH: &str = "local";

pub struct LocalProvider {
    /// Superuser DSN for the local cluster, e.g.
    /// `postgres://postgres:postgres@localhost:5432/postgres`.
    admin_dsn: String,
    /// Host clients should connect to, e.g. `localhost:5432`.
    host: String,
}

impl LocalProvider {
    pub fn new(admin_dsn: impl Into<String>, host: impl Into<String>) -> Self {
        Self {
            admin_dsn: admin_dsn.into(),
            host: host.into(),
        }
    }

    /// Read the local cluster's coordinates from `OXY_DATABASE_URL`, the same
    /// var the rest of Oxy uses for its control plane.
    pub fn from_env() -> Result<Self, ProviderError> {
        let dsn = std::env::var("OXY_DATABASE_URL").map_err(|_| {
            ProviderError::Transport(
                "OXY_DATABASE_URL is not set; the local OLTP provider needs a Postgres \
                 superuser DSN (try `oxy start` first)"
                    .into(),
            )
        })?;
        let host = host_from_dsn(&dsn);
        Ok(Self::new(dsn, host))
    }

    async fn exec(&self, sql: &str) -> Result<(), ProviderError> {
        let (client, connection) = tokio_postgres::connect(&self.admin_dsn, NoTls)
            .await
            .map_err(|e| ProviderError::Transport(format!("connect as admin: {e}")))?;
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                tracing::warn!("local provider admin connection closed: {e}");
            }
        });
        client
            .batch_execute(sql)
            .await
            .map_err(|e| ProviderError::Api {
                status: 500,
                // `pg_detail`, not `{e}`: `tokio_postgres::Error`'s Display is
                // the bare string "db error", so every failure this provider
                // reported arrived as `Api { status: 500, message: "db error
                // (while running: …)" }` — the statement, and nothing about
                // why it failed. The SQLSTATE and message live behind
                // `as_db_error()`.
                message: format!("{} (while running: {sql})", crate::connect::pg_detail(&e)),
            })?;
        Ok(())
    }

    /// One optional string from a query, or `None` when it returns no row.
    ///
    /// Distinguishes absent (`None`) from present-and-owned-by-X (`Some(x)`),
    /// which `scalar_exists` cannot — a fact about what the two return, rather
    /// than a caller count that is true at one commit and silently false at the
    /// next.
    ///
    /// Lets a single round trip distinguish absent / ours / someone else's,
    /// where `scalar_exists` can only say whether a row came back.
    async fn scalar_string(&self, sql: &str) -> Result<Option<String>, ProviderError> {
        let (client, connection) = tokio_postgres::connect(&self.admin_dsn, NoTls)
            .await
            .map_err(|e| ProviderError::Transport(format!("connect as admin: {e}")))?;
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                tracing::warn!("local provider admin connection closed: {e}");
            }
        });
        let rows = client
            .query(sql, &[])
            .await
            .map_err(|e| ProviderError::Api {
                status: 500,
                message: format!("{} (while running: {sql})", crate::connect::pg_detail(&e)),
            })?;
        Ok(rows.first().map(|r| r.get::<_, String>(0)))
    }

    async fn scalar_exists(&self, sql: &str) -> Result<bool, ProviderError> {
        let (client, connection) = tokio_postgres::connect(&self.admin_dsn, NoTls)
            .await
            .map_err(|e| ProviderError::Transport(format!("connect as admin: {e}")))?;
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                tracing::warn!("local provider admin connection closed: {e}");
            }
        });
        let rows = client
            .query(sql, &[])
            .await
            .map_err(|e| ProviderError::Api {
                status: 500,
                // `pg_detail`, not `{e}`: `tokio_postgres::Error`'s Display is
                // the bare string "db error", so every failure this provider
                // reported arrived as `Api { status: 500, message: "db error
                // (while running: …)" }` — the statement, and nothing about
                // why it failed. The SQLSTATE and message live behind
                // `as_db_error()`.
                message: format!("{} (while running: {sql})", crate::connect::pg_detail(&e)),
            })?;
        Ok(!rows.is_empty())
    }

    /// The cluster's own major version.
    ///
    /// `server_version_num` is `180006` for 18.6, so integer-dividing by 10000
    /// gives the major — the form `Project::pg_version` carries and a provider
    /// API takes.
    ///
    /// **Fails rather than falling back, because this value is persisted.**
    /// `create_project`'s result is the only thing that ever writes
    /// `oltp_tenants.pg_version` (`apply_remote`), and a fallback to
    /// `DEFAULT_PG_VERSION` would stamp the one number that reads as "no
    /// drift" into the row permanently — after which `oxy oltp status` renders
    /// it as fact. A `warn!` in the provisioning process is no help next to a
    /// wrong row a year later. The connection this needs is taken moments
    /// after `CREATE DATABASE` on a cluster that just answered three
    /// statements, so a failure here is a real failure, not a flake to
    /// paper over.
    async fn server_major_version(&self) -> Result<u8, ProviderError> {
        let (client, connection) = tokio_postgres::connect(&self.admin_dsn, NoTls)
            .await
            .map_err(|e| {
                ProviderError::Transport(format!(
                    "read server version: {}",
                    crate::connect::pg_detail(&e)
                ))
            })?;
        tokio::spawn(async move {
            let _ = connection.await;
        });
        let raw: String = client
            .query_one("SHOW server_version_num", &[])
            .await
            .map_err(|e| ProviderError::Api {
                status: 500,
                message: format!(
                    "{} (while running: SHOW server_version_num)",
                    crate::connect::pg_detail(&e)
                ),
            })?
            .get(0);
        raw.parse::<u32>()
            .map(|n| (n / 10_000) as u8)
            .map_err(|_| ProviderError::Api {
                status: 500,
                message: format!("server_version_num was not a number: {raw:?}"),
            })
    }
}

/// Local database name for a project. Neon's project names carry hyphens
/// (`oxy-org-<uuid>`); Postgres identifiers here must satisfy
/// [`crate::schema`]'s validator, so non-alphanumerics collapse to `_`.
pub fn database_name_for(project_name: &str) -> String {
    let mapped: String = project_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    // Must start with a letter to pass validation.
    if mapped.starts_with(|c: char| c.is_ascii_lowercase()) {
        mapped
    } else {
        format!("db_{mapped}")
    }
}

pub fn host_from_dsn(dsn: &str) -> String {
    let after_scheme = dsn.split("://").nth(1).unwrap_or(dsn);
    // `rsplit_once`, not `split('@').nth(1)`: an operator-supplied password may
    // contain '@' (this is the one credential Oxy does not generate), and
    // splitting on the FIRST one takes the tail of the password as the host —
    // which is then persisted as `tenant.host` and baked into every DSN after.
    let hostish = after_scheme
        .rsplit_once('@')
        .map(|(_, h)| h)
        .unwrap_or(after_scheme);
    hostish
        .split('/')
        .next()
        .unwrap_or(hostish)
        .split('?')
        .next()
        .unwrap_or(hostish)
        .to_string()
}

#[async_trait]
impl OltpProvider for LocalProvider {
    fn name(&self) -> &'static str {
        "local"
    }

    /// Every tenant shares this cluster, so role DDL runs as the superuser that
    /// created them all — see the trait method for why the owner is not enough.
    fn role_admin_dsn(&self) -> Option<String> {
        Some(self.admin_dsn.clone())
    }

    /// Four separate admin connections, not one session: `server_major_version`,
    /// the ownership probe, and two `exec` calls each open their own — three on
    /// the adoption path, where `CREATE DATABASE` is skipped. Irrelevant at
    /// provisioning rate, and `CREATE DATABASE` cannot share a transaction
    /// anyway — but worth knowing before reading this as one session.
    async fn create_project(&self, req: CreateProjectRequest) -> Result<Project, ProviderError> {
        let db = database_name_for(&req.name);
        let owner = format!("{db}_owner");
        // CSPRNG, not `pw_{db}`. The owner credential is the most powerful one
        // in the tenant, and a derived password means anyone who can reach the
        // cluster logs in as it by guessing the database name. `LocalProvider`
        // is reachable whenever OXY_OLTP_PROVIDER is not `neon`, and
        // OXY_OLTP_ADMIN_URL can point anywhere — "it is only loopback" is a
        // deployment assumption, not a property of this code.
        let password = crate::roles::generate_password();

        // Read the cluster's major FIRST, before anything is created.
        //
        // This is a property of the cluster, not of the database being made, so
        // nothing forces it to run late — and running late was a bug. Taken
        // after `CREATE ROLE` and `CREATE DATABASE`, a failure here returned
        // `Err` with both already created and no `oltp_tenants` row written, so
        // the next provision took `create_new` again, found the database, and
        // answered `ProjectNameTaken` — which is not retryable and which
        // `LocalProvider`, unlike Neon, has no adopt-by-name branch for. The org
        // wedged until someone ran `DROP DATABASE` by hand, behind an error
        // naming a collision rather than the version query that caused it.
        let pg_version = self.server_major_version().await?;

        // A derived name is ADOPTED; a chosen one still collides.
        //
        // The same half-provision window `NeonProvider::find_project_id_by_name`
        // exists for, and for the same reason: `create_new` has fallible steps
        // between the provider call and the `oltp_tenants` insert, so a crypto
        // failure or a control-plane blip leaves a database with no row. The
        // next provision takes the create path again, and without adoption it
        // gets `ProjectNameTaken` — which `is_retryable()` explicitly denies —
        // so the org wedges until someone runs `DROP DATABASE` by hand, behind
        // an error naming a collision rather than whatever really failed. Neon
        // recovered from this and local did not, which is the worse asymmetry
        // to leave in place.
        //
        // Adoption is safe for the same narrow reason it is safe on Neon:
        // `oxy_org_<uuid>` is derived from the org id, never chosen, so a match
        // is an identity match rather than a coincidence. A name that is NOT
        // ours still returns `ProjectNameTaken` — that guard exists for
        // user-chosen names, where two tenants could collide and adoption would
        // cross them.
        // ONE query answering all three states: absent, ours, or someone
        // else's. Two `scalar_exists` calls meant two connections and two
        // round trips to learn less — the second re-tested `datname` the first
        // had just tested, and neither could report WHO owns it.
        let owner_of = self
            .scalar_string(&format!(
                "SELECT pg_get_userbyid(datdba) FROM pg_database WHERE datname = '{db}'"
            ))
            .await?;

        // Owned by us, not merely named like us.
        //
        // "The name is derived, so a match is an identity match" holds for a
        // database OXY CREATED. It does not hold for one that merely carries
        // that name — a restored dump, a hand-created database, one left by an
        // older owner-naming scheme. Adopting those would reset `{owner}`'s
        // password, land the row, and then fail two steps later inside platform
        // step 1, where `REVOKE ALL ON DATABASE … FROM PUBLIC` requires
        // ownership and answers `must be owner of database` — strictly worse
        // than a clean refusal, because by then things have been mutated.
        //
        // This crate already drew that line for schemas: `assert_schema_owned_sql`
        // refuses with `OXY02` precisely because "exists" is not "is ours".
        let exists = owner_of.is_some();
        if let Some(actual) = &owner_of {
            if !crate::provisioner::is_derived_project_name(&req.name) {
                // A name the caller chose. Never adoptable, whoever owns it.
                // Carries the DATABASE name, matching `ProjectNotOwned` below —
                // two refusals from one function naming the same object two
                // ways (`oxy-org-<uuid>` vs `oxy_org_<uuid>`) reads as two
                // different objects.
                return Err(ProviderError::ProjectNameTaken(db.clone()));
            }
            if actual != &owner {
                // A derived name over a database that is not ours. Carries the
                // owner, because that is the fact an operator needs and the
                // query above already read it.
                return Err(ProviderError::ProjectNotOwned {
                    name: db.clone(),
                    owner: actual.clone(),
                });
            }
            tracing::warn!(
                database = %db,
                "adopting an existing OLTP database: derived name, owned by its \
                 expected owner role"
            );
        }

        // Roles are cluster-global, so this is guarded rather than bare — and
        // the guard must ALTER, not skip.
        //
        // A leftover owner role from a previous run kept its old password while
        // this call returned the new one, so the provisioner sealed a
        // credential that did not open anything: "could not connect to tenant
        // database". Invisible while passwords were derived from the database
        // name, because the stale one happened to match. Same rule as
        // `create_role` below — "create" here means "ensure usable".
        self.exec(&format!(
            "DO $$ BEGIN \
               IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '{owner}') THEN \
                 ALTER ROLE \"{owner}\" WITH LOGIN PASSWORD '{password}' CREATEROLE; \
               ELSE \
                 CREATE ROLE \"{owner}\" LOGIN PASSWORD '{password}' CREATEROLE; \
               END IF; \
             END $$"
        ))
        .await?;
        // CREATE DATABASE cannot run inside a transaction block, hence its own
        // call — and it is skipped when adopting, where the database is the
        // whole point of adopting. The owner's password was just reset above by
        // the guarded `ALTER`, so the credential this returns opens it.
        if !exists {
            self.exec(&format!("CREATE DATABASE \"{db}\" OWNER \"{owner}\""))
                .await?;
        }

        Ok(Project {
            id: db.clone(),
            name: req.name,
            region_id: req.region_id,
            // The CLUSTER's version, not the request's — same as `get_project`.
            // A local "project" is a database on a cluster that already exists
            // at whatever major it runs, so the requested version is not
            // honoured; echoing it made one project answer two different
            // numbers depending on which call you made. This is the result that
            // reaches `oltp_tenants.pg_version`, so it is what `oxy oltp
            // status` reports. Read above, before anything was created.
            pg_version,
            branch: Branch {
                id: LOCAL_BRANCH.to_string(),
                name: LOCAL_BRANCH.to_string(),
            },
            database: DatabaseInfo {
                name: db,
                owner_name: owner.clone(),
            },
            owner_role: Role {
                name: owner,
                password: Some(password),
            },
            host: self.host.clone(),
        })
    }

    async fn get_project(&self, project_id: &str) -> Result<Option<Project>, ProviderError> {
        let owner = format!("{project_id}_owner");
        // Ownership, not just existence — the same test `create_project` makes.
        //
        // This is the one function that reports a LIVE project, and answering
        // `Some` for a database that merely carries the name sends
        // `reconcile_existing` straight to `mark_active`: status flips to
        // Active, `pg_version` is stamped, and platform step 1 then answers
        // `must be owner of database` — the identical "mutated, then failed"
        // sequence adoption was just fixed to avoid, reached on the sibling
        // path whenever a tenant row already exists.
        //
        // `None` rather than an error: `get_*` returns `Ok(None)` for a missing
        // resource by the trait's contract, and a database that is not ours is
        // not our project. The caller then takes the recreate branch, where
        // `create_project` refuses with `ProjectNotOwned` and names the owner.
        let actual = self
            .scalar_string(&format!(
                "SELECT pg_get_userbyid(datdba) FROM pg_database WHERE datname = '{project_id}'"
            ))
            .await?;
        match actual {
            None => return Ok(None),
            Some(a) if a != owner => {
                tracing::warn!(
                    database = %project_id,
                    owner = %a,
                    expected = %owner,
                    "a database with this tenant's name exists but is not Oxy's"
                );
                return Ok(None);
            }
            Some(_) => {}
        }
        Ok(Some(Project {
            id: project_id.to_string(),
            name: project_id.to_string(),
            region_id: "local".to_string(),
            // Asked, not assumed. This is the one function that reports a live
            // local project's version, and a literal here reports a number
            // unrelated to the cluster actually running — and goes stale on
            // every bump. Latent today (`reconcile_existing` only checks
            // `.is_some()`), which is exactly how it would stay wrong.
            pg_version: self.server_major_version().await?,
            branch: Branch {
                id: LOCAL_BRANCH.to_string(),
                name: LOCAL_BRANCH.to_string(),
            },
            database: DatabaseInfo {
                name: project_id.to_string(),
                owner_name: owner.clone(),
            },
            // Password withheld on reads, matching Neon.
            owner_role: Role {
                name: owner,
                password: None,
            },
            host: self.host.clone(),
        }))
    }

    async fn delete_project(&self, project_id: &str) -> Result<(), ProviderError> {
        // WITH (FORCE) terminates open sessions; without it a single idle IDE
        // connection blocks teardown forever.
        self.exec(&format!(
            "DROP DATABASE IF EXISTS \"{project_id}\" WITH (FORCE)"
        ))
        .await?;
        self.exec(&format!("DROP ROLE IF EXISTS \"{project_id}_owner\""))
            .await?;
        Ok(())
    }

    async fn create_role(
        &self,
        project_id: &str,
        _branch_id: &str,
        role_name: &str,
    ) -> Result<Role, ProviderError> {
        let password = crate::roles::generate_password();
        // ALTER on the existing branch, not a no-op: the platform declaration
        // creates `oxy_analyst_ro` as NOLOGIN and leaves the provider to mint
        // its credential, so "create" here must mean "ensure usable".
        self.exec(&format!(
            "DO $$ BEGIN \
               IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '{role_name}') THEN \
                 CREATE ROLE \"{role_name}\" LOGIN PASSWORD '{password}'; \
               ELSE \
                 ALTER ROLE \"{role_name}\" LOGIN PASSWORD '{password}'; \
               END IF; \
             END $$"
        ))
        .await?;
        // The role needs to reach the database before any schema grant matters.
        self.exec(&format!(
            "GRANT CONNECT ON DATABASE \"{project_id}\" TO \"{role_name}\""
        ))
        .await?;
        Ok(Role {
            name: role_name.to_string(),
            password: Some(password),
        })
    }

    async fn get_role(
        &self,
        _project_id: &str,
        _branch_id: &str,
        role_name: &str,
    ) -> Result<Option<Role>, ProviderError> {
        let exists = self
            .scalar_exists(&format!(
                "SELECT 1 FROM pg_roles WHERE rolname = '{role_name}'"
            ))
            .await?;
        Ok(exists.then(|| Role {
            name: role_name.to_string(),
            password: None,
        }))
    }

    async fn reset_role_password(
        &self,
        _project_id: &str,
        branch_id: &str,
        role_name: &str,
    ) -> Result<Role, ProviderError> {
        if !self
            .scalar_exists(&format!(
                "SELECT 1 FROM pg_roles WHERE rolname = '{role_name}'"
            ))
            .await?
        {
            return Err(ProviderError::RoleNotFound(
                role_name.to_string(),
                branch_id.to_string(),
            ));
        }
        let password = crate::roles::generate_password();
        // `LOGIN` as well as the password: a rotate must leave behind a
        // credential that can actually connect. Setting only the password on a
        // NOLOGIN role hands back something that looks like a working
        // credential and fails at connect time, one layer away from the cause.
        self.exec(&format!(
            "ALTER ROLE \"{role_name}\" LOGIN PASSWORD '{password}'"
        ))
        .await?;
        Ok(Role {
            name: role_name.to_string(),
            password: Some(password),
        })
    }

    async fn delete_role(
        &self,
        _project_id: &str,
        _branch_id: &str,
        role_name: &str,
    ) -> Result<(), ProviderError> {
        self.exec(&format!("DROP ROLE IF EXISTS \"{role_name}\""))
            .await?;
        Ok(())
    }

    async fn create_branch(&self, req: &BranchRequest) -> Result<ProjectBranch, ProviderError> {
        self.create_branch_impl(req).await
    }

    async fn reset_branch(
        &self,
        req: &BranchRequest,
        branch_id: &str,
    ) -> Result<ProjectBranch, ProviderError> {
        self.reset_branch_impl(req, branch_id).await
    }

    async fn delete_branch(
        &self,
        req: &BranchRequest,
        branch_id: &str,
    ) -> Result<(), ProviderError> {
        self.delete_branch_impl(req, branch_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_names_become_valid_postgres_identifiers() {
        let name = database_name_for("oxy-org-11111111-2222-3333-4444-555555555555");
        assert_eq!(name, "oxy_org_11111111_2222_3333_4444_555555555555");
        assert!(crate::schema::validate_name(&name).is_ok());
    }

    #[test]
    fn a_name_starting_with_a_non_letter_is_prefixed() {
        let name = database_name_for("123-abc");
        assert!(name.starts_with("db_"));
        assert!(crate::schema::validate_name(&name).is_ok());
    }

    #[test]
    fn host_is_extracted_from_a_dsn_with_credentials() {
        assert_eq!(
            host_from_dsn("postgres://user:pass@localhost:5432/oxy?sslmode=disable"),
            "localhost:5432"
        );
    }

    #[test]
    fn host_is_extracted_from_a_dsn_without_credentials() {
        assert_eq!(
            host_from_dsn("postgres://localhost:5432/oxy"),
            "localhost:5432"
        );
    }
    /// The admin DSN is operator-supplied, so its password is the one credential
    /// in this system Oxy did not generate — and may contain '@'. Splitting on
    /// the first one takes the password's tail as the host, which is then
    /// persisted as `tenant.host` and baked into every DSN built afterwards.
    #[test]
    fn a_password_containing_an_at_sign_does_not_become_the_host() {
        assert_eq!(
            host_from_dsn("postgres://u:p@ss@localhost:15432/db"),
            "localhost:15432"
        );
        assert_eq!(
            host_from_dsn("postgres://u:a@b@c@example.com:5432/db?sslmode=require"),
            "example.com:5432"
        );
    }
}
