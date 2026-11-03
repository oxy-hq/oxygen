//! `LocalProvider` branches: a sibling **database**, copied from the tenant's.
//!
//! Local Postgres has no branching, so the nearest honest analogue of a Neon
//! branch is `CREATE DATABASE <branch> TEMPLATE <tenant>` — every schema and row,
//! the same object owners, reached by the same role names, under a different
//! database name. That keeps the one contract a caller relies on identical
//! across providers: point the connection somewhere else and every
//! `app_<slug>` schema is where it was. A sibling *schema* per writer (the
//! design doc's first sketch) would instead need every caller to rewrite its
//! `search_path` and would still let a schema-qualified name reach production.
//!
//! A schema-only request ([`BranchInit::SchemaOnly`]) is the same copy with
//! every table then truncated — the local stand-in for Neon's `init_source:
//! schema-only`. A requested fork point is ignored (with a warning) and so is an
//! expiry: a loopback cluster has no history to read and no scheduler to delete
//! a branch for it.
//!
//! Three differences from Neon, all consequences of one cluster:
//!
//! - **Roles are cluster-global**, so the branch shares production's roles and
//!   passwords. Resetting a password "on the branch" would rotate
//!   production's; `create_branch` therefore discloses no owner password, and
//!   callers use the tenant's credentials against the branch database.
//! - **`CREATE DATABASE` does not copy database-level ACLs or settings.** The
//!   copy would default to `CONNECT` for PUBLIC and lose the writers'
//!   `search_path`, so [`copy_database_state_sql`] replays both.
//! - **`TEMPLATE` needs the source to have no other sessions.** A copy that
//!   finds one terminates the tenant database's other sessions and retries —
//!   the same trade `DROP DATABASE … WITH (FORCE)` makes on deprovision, and
//!   acceptable only because this provider is a loopback dev cluster.

use tracing::warn;

use super::LocalProvider;
use crate::provider::ProviderError;
use crate::provider::types::{
    BranchInit, BranchRequest, DatabaseInfo, ProjectBranch, Role, SCHEMA_ONLY_RESET_REFUSAL,
};
use crate::schema::{MAX_NAME_LEN, validate_name};

/// Tries of the copy before a busy template is reported rather than retried.
const COPY_ATTEMPTS: usize = 3;

/// The branch database's name: the tenant's, plus a short hash suffix.
///
/// The suffix is FNV over the tenant database AND the branch name, so two
/// branches of one tenant differ and a truncated prefix cannot collide. The
/// whole stays within [`MAX_NAME_LEN`] — the validator every identifier in this
/// crate passes — by trimming the tenant half, never the hash: a 44-character
/// `oxy_org_<uuid>` comes out at 53.
pub fn branch_database_name(database: &str, branch_name: &str) -> String {
    let tag = crate::schema::tenant_tag(&format!("{database}/{branch_name}"));
    let room = MAX_NAME_LEN - tag.len() - 1;
    let head: String = database.chars().take(room).collect();
    format!("{}_{tag}", head.trim_end_matches('_'))
}

impl LocalProvider {
    pub(super) async fn create_branch_impl(
        &self,
        req: &BranchRequest,
    ) -> Result<ProjectBranch, ProviderError> {
        let branch = branch_database_name(&req.database_name, &req.name);
        checked(req, &branch)?;
        self.only_on_loopback()?;
        if req.parent_timestamp.is_some() {
            warn!(
                database = %branch,
                "a loopback cluster cannot time-travel; the branch copies the parent's head"
            );
        }
        // Absent, ours, or someone else's — the same three-way answer
        // `create_project` reads, for the same reason: a derived name is
        // adoptable only when Oxy's owner owns it.
        let owner = self.scalar_string(&owner_of_sql(&branch)).await?;
        match owner {
            None => {
                self.copy_database(&req.database_name, &branch, &req.owner_role)
                    .await?;
                if req.init == BranchInit::SchemaOnly
                    && let Err(e) = self.empty_tables(&branch).await
                {
                    // A populated copy left under the derived name would be
                    // adopted by the next create as if it were what was asked
                    // for. Best effort: the error worth reporting is the
                    // emptying one, and the adopt path empties it again anyway.
                    if let Err(drop_err) = self.drop_branch_database(&branch).await {
                        warn!(
                            database = %branch,
                            "could not drop a branch copy that failed to empty: {drop_err}"
                        );
                    }
                    return Err(e);
                }
            }
            Some(actual) if actual == req.owner_role => {
                warn!(
                    database = %branch,
                    "adopting an existing OLTP branch database: derived name, owned by the \
                     tenant's owner"
                );
                // Idempotent, and what makes a copy left populated by an
                // interrupted earlier attempt safe to adopt as schema-only.
                if req.init == BranchInit::SchemaOnly {
                    self.empty_tables(&branch).await?;
                }
            }
            Some(actual) => {
                return Err(ProviderError::ProjectNotOwned {
                    name: branch,
                    owner: actual,
                });
            }
        }
        Ok(describe(req, branch, &self.host))
    }

    /// The derived branch database, if the cluster has it and the tenant's
    /// owner owns it (the adopt rule); one someone else owns is an error.
    pub(super) async fn find_branch_id_impl(
        &self,
        req: &BranchRequest,
    ) -> Result<Option<String>, ProviderError> {
        let branch = branch_database_name(&req.database_name, &req.name);
        checked(req, &branch)?;
        self.only_on_loopback()?;
        match self.scalar_string(&owner_of_sql(&branch)).await? {
            None => Ok(None),
            Some(actual) if actual == req.owner_role => Ok(Some(branch)),
            Some(actual) => Err(ProviderError::ProjectNotOwned {
                name: branch,
                owner: actual,
            }),
        }
    }

    pub(super) async fn reset_branch_impl(
        &self,
        req: &BranchRequest,
        branch_id: &str,
    ) -> Result<ProjectBranch, ProviderError> {
        checked(req, branch_id)?;
        // Before any connection, and the same answer as Neon: the re-copy
        // below would hand a schema-only branch production's rows.
        if req.init == BranchInit::SchemaOnly {
            return Err(ProviderError::Api {
                status: 400,
                message: SCHEMA_ONLY_RESET_REFUSAL.into(),
            });
        }
        self.only_on_loopback()?;
        // No restore-in-place here: drop the copy and take a fresh one. WITH
        // (FORCE) because a staging function holding a connection would
        // otherwise block the reset forever.
        self.exec(&format!(
            "DROP DATABASE IF EXISTS \"{branch_id}\" WITH (FORCE)"
        ))
        .await?;
        self.copy_database(&req.database_name, branch_id, &req.owner_role)
            .await?;
        Ok(describe(req, branch_id.to_string(), &self.host))
    }

    pub(super) async fn delete_branch_impl(
        &self,
        req: &BranchRequest,
        branch_id: &str,
    ) -> Result<(), ProviderError> {
        // The tenant database is one identifier away from the branch here, and
        // every other org's database is on the same cluster: only the exact
        // derived name may be dropped.
        checked(req, branch_id)?;
        self.only_on_loopback()?;
        self.drop_branch_database(branch_id).await
    }

    /// `branch` must already have passed [`checked`].
    async fn drop_branch_database(&self, branch: &str) -> Result<(), ProviderError> {
        self.exec(&format!(
            "DROP DATABASE IF EXISTS \"{branch}\" WITH (FORCE)"
        ))
        .await
    }

    /// Branch copies and drops run only against a loopback cluster — the one
    /// the module header's trades are acceptable on. A copy terminates every
    /// other session on the tenant's PRODUCTION database to free the template,
    /// and both reset and delete `DROP DATABASE … WITH (FORCE)`. `from_env`
    /// only warns when `OXY_OLTP_PROVIDER=local` points at a shared box; for a
    /// branch it is a refusal, checked on both the cluster the statements run
    /// on and the host clients are handed.
    fn only_on_loopback(&self) -> Result<(), ProviderError> {
        for host in [super::host_from_dsn(&self.admin_dsn), self.host.clone()] {
            if !crate::host::is_loopback(&host) {
                // A unix-socket DSN (`host=/var/run/postgresql`, or no host at
                // all) has no TCP host to check, so `host_from_dsn` answers
                // `""` here — correctly not loopback, but a bare `""` in the
                // message reads like a bug rather than a configuration
                // answer, so name the reason.
                let no_tcp_host = if host.is_empty() {
                    " (the DSN names no TCP host)"
                } else {
                    ""
                };
                return Err(ProviderError::Api {
                    status: 400,
                    message: format!(
                        "refusing a branch operation on {host:?}{no_tcp_host}: the local \
                         provider copies and drops branch databases only on a loopback \
                         cluster (a copy terminates the production database's sessions)"
                    ),
                });
            }
        }
        Ok(())
    }

    /// Empty every table of `database`, keeping the tables.
    ///
    /// Runs on the branch's own database — a connection to the admin database
    /// cannot see another database's catalog — and only ever after
    /// [`Self::copy_database`] made it, so it can never reach the tenant's.
    async fn empty_tables(&self, database: &str) -> Result<(), ProviderError> {
        let mut config: tokio_postgres::Config =
            self.admin_dsn.parse().map_err(|e: tokio_postgres::Error| {
                ProviderError::Transport(format!("parse the admin DSN: {e}"))
            })?;
        config.dbname(database);
        let (client, connection) = config
            .connect(tokio_postgres::NoTls)
            .await
            .map_err(|e| ProviderError::Transport(format!("connect as admin: {e}")))?;
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                warn!("local provider admin connection closed: {e}");
            }
        });
        client
            .batch_execute(&truncate_all_tables_sql())
            .await
            .map_err(|e| ProviderError::Api {
                status: 500,
                message: format!(
                    "{} (while emptying {database})",
                    crate::connect::pg_detail(&e)
                ),
            })
    }

    /// `CREATE DATABASE … TEMPLATE`, then the state that command leaves behind.
    async fn copy_database(&self, src: &str, dst: &str, owner: &str) -> Result<(), ProviderError> {
        let create = format!("CREATE DATABASE \"{dst}\" TEMPLATE \"{src}\" OWNER \"{owner}\"");
        for attempt in 1..=COPY_ATTEMPTS {
            match self.exec(&create).await {
                Ok(()) => break,
                Err(e) if attempt < COPY_ATTEMPTS && is_template_busy(&e) => {
                    warn!(
                        database = %src,
                        attempt,
                        "tenant database has open sessions; terminating them to branch it"
                    );
                    self.exec(&terminate_sessions_sql(src)).await?;
                }
                Err(e) => return Err(e),
            }
        }
        self.exec(&copy_database_state_sql(src, dst)).await
    }
}

fn describe(req: &BranchRequest, database: String, host: &str) -> ProjectBranch {
    ProjectBranch {
        id: database.clone(),
        name: req.name.clone(),
        parent_id: req.parent_branch_id.clone(),
        host: host.to_string(),
        database: DatabaseInfo {
            name: database,
            owner_name: req.owner_role.clone(),
        },
        // Shared with production: roles are cluster-global. See the module.
        owner_role: Role {
            name: req.owner_role.clone(),
            password: None,
        },
    }
}

/// `branch` is exactly the database [`branch_database_name`] derives for this
/// tenant and branch — never the tenant's own, never another org's.
///
/// Every destructive statement here is a `DROP DATABASE` on a cluster that
/// holds every tenant, so "a plain identifier that is not the tenant" was not
/// enough: a corrupted row naming another org's database would pass it. The
/// name is derived, so it can be recomputed and required.
fn checked(req: &BranchRequest, branch: &str) -> Result<(), ProviderError> {
    let tenant = req.database_name.as_str();
    validate_name(tenant).map_err(|_| ProviderError::Api {
        status: 400,
        message: format!("{tenant:?} is not a database name this provider creates"),
    })?;
    if branch == tenant {
        return Err(ProviderError::BranchIsProduction(branch.to_string()));
    }
    let expected = branch_database_name(tenant, &req.name);
    if branch != expected {
        return Err(ProviderError::Api {
            status: 400,
            message: format!(
                "refusing a branch operation on {branch:?}: this tenant's {name} branch is \
                 {expected:?}",
                name = req.name
            ),
        });
    }
    Ok(())
}

/// `TRUNCATE … RESTART IDENTITY CASCADE` every table outside the system schemas
/// and [`crate::platform::META_SCHEMA`], in one statement so a foreign key
/// between two of them cannot order the work wrongly. `RESTART IDENTITY` so the
/// sequences start fresh, as on a Neon schema-only branch. A database with no
/// user tables is a no-op rather than an empty `TRUNCATE`.
///
/// `oxy_meta` is the platform migration ledger, not org data, so it is kept:
/// emptying it would make the branch look unmigrated. **On Neon the two differ**
/// — a schema-only branch there has an empty `oxy_meta` ledger, and the
/// environment-branch cut (a later step) copies the ledger back. Locally the
/// ledger just survives the truncate, which leaves the same end state.
fn truncate_all_tables_sql() -> String {
    format!(
        "DO $oxy_empty$ \
         DECLARE tables text; \
         BEGIN \
           SELECT string_agg(format('%I.%I', schemaname, tablename), ', ') INTO tables \
             FROM pg_tables \
            WHERE schemaname NOT IN ('pg_catalog', 'information_schema', '{meta}') \
              AND schemaname NOT LIKE 'pg\\_toast%' AND schemaname NOT LIKE 'pg\\_temp%'; \
           IF tables IS NOT NULL THEN \
             EXECUTE 'TRUNCATE ' || tables || ' RESTART IDENTITY CASCADE'; \
           END IF; \
         END $oxy_empty$",
        meta = crate::platform::META_SCHEMA,
    )
}

fn owner_of_sql(database: &str) -> String {
    format!("SELECT pg_get_userbyid(datdba) FROM pg_database WHERE datname = '{database}'")
}

fn terminate_sessions_sql(database: &str) -> String {
    format!(
        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
         WHERE datname = '{database}' AND pid <> pg_backend_pid()"
    )
}

/// `55006 object_in_use`: "source database … is being accessed by other users".
fn is_template_busy(e: &ProviderError) -> bool {
    matches!(e, ProviderError::Api { message, .. } if message.starts_with("[55006]"))
}

/// Replay what `CREATE DATABASE … TEMPLATE` does not copy: the database ACL and
/// the per-database settings.
///
/// Without the first, the copy grants `CONNECT` and `TEMPORARY` to PUBLIC —
/// every role on the cluster, including other tenants' writers — where the
/// tenant revoked them. Without the second, every writer loses the database
/// default `search_path` that `refresh_database_search_path` maintains.
///
/// Settings go through `set_config` + `SET … FROM CURRENT` so a stored value
/// (`app_a, raw_b`) is reapplied exactly as Postgres parsed it, rather than
/// re-quoted here into a single schema named `"app_a, raw_b"`.
pub(super) fn copy_database_state_sql(src: &str, dst: &str) -> String {
    format!(
        "DO $oxy_copy$ \
         DECLARE src_oid oid; a record; s record; item text; \
         BEGIN \
           SELECT oid INTO src_oid FROM pg_database WHERE datname = '{src}'; \
           IF (SELECT datacl FROM pg_database WHERE oid = src_oid) IS NOT NULL THEN \
             EXECUTE format('REVOKE ALL ON DATABASE %I FROM PUBLIC', '{dst}'); \
             FOR a IN SELECT e.grantee, e.privilege_type \
                        FROM pg_database d, aclexplode(d.datacl) e WHERE d.oid = src_oid LOOP \
               IF a.grantee = 0 THEN \
                 EXECUTE format('GRANT %s ON DATABASE %I TO PUBLIC', a.privilege_type, '{dst}'); \
               ELSE \
                 EXECUTE format('GRANT %s ON DATABASE %I TO %I', a.privilege_type, '{dst}', \
                                pg_get_userbyid(a.grantee)); \
               END IF; \
             END LOOP; \
           END IF; \
           FOR s IN SELECT setrole, setconfig FROM pg_db_role_setting \
                     WHERE setdatabase = src_oid LOOP \
             FOREACH item IN ARRAY s.setconfig LOOP \
               PERFORM set_config(split_part(item, '=', 1), \
                                  substr(item, strpos(item, '=') + 1), true); \
               IF s.setrole = 0 THEN \
                 EXECUTE format('ALTER DATABASE %I SET %s FROM CURRENT', '{dst}', \
                                split_part(item, '=', 1)); \
               ELSE \
                 EXECUTE format('ALTER ROLE %I IN DATABASE %I SET %s FROM CURRENT', \
                                pg_get_userbyid(s.setrole), '{dst}', split_part(item, '=', 1)); \
               END IF; \
             END LOOP; \
           END LOOP; \
         END $oxy_copy$"
    )
}

#[cfg(test)]
mod tests;
