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
use crate::provider::types::{BranchRequest, DatabaseInfo, ProjectBranch, Role};
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
        // Absent, ours, or someone else's — the same three-way answer
        // `create_project` reads, for the same reason: a derived name is
        // adoptable only when Oxy's owner owns it.
        let owner = self.scalar_string(&owner_of_sql(&branch)).await?;
        match owner {
            None => {
                self.copy_database(&req.database_name, &branch, &req.owner_role)
                    .await?
            }
            Some(actual) if actual == req.owner_role => warn!(
                database = %branch,
                "adopting an existing OLTP branch database: derived name, owned by the \
                 tenant's owner"
            ),
            Some(actual) => {
                return Err(ProviderError::ProjectNotOwned {
                    name: branch,
                    owner: actual,
                });
            }
        }
        Ok(describe(req, branch, &self.host))
    }

    pub(super) async fn reset_branch_impl(
        &self,
        req: &BranchRequest,
        branch_id: &str,
    ) -> Result<ProjectBranch, ProviderError> {
        checked(req, branch_id)?;
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
        self.exec(&format!(
            "DROP DATABASE IF EXISTS \"{branch_id}\" WITH (FORCE)"
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
mod tests {
    use super::*;

    const TENANT: &str = "oxy_org_11111111_2222_3333_4444_555555555555";

    #[test]
    fn a_branch_database_is_a_valid_identifier_within_the_name_cap() {
        let name = branch_database_name(TENANT, "oxy-staging");
        assert!(name.len() <= MAX_NAME_LEN, "{name} is {} chars", name.len());
        assert!(validate_name(&name).is_ok(), "{name}");
        assert!(
            name.starts_with(TENANT),
            "readable as this tenant's: {name}"
        );
        assert_ne!(name, TENANT);
        assert_eq!(
            name,
            branch_database_name(TENANT, "oxy-staging"),
            "derived, so an orphan can be found again"
        );
    }

    #[test]
    fn the_suffix_separates_branches_and_survives_truncation() {
        assert_ne!(
            branch_database_name(TENANT, "oxy-staging"),
            branch_database_name(TENANT, "oxy-dev")
        );
        // Two long tenants sharing everything the cap keeps still differ,
        // because the hash covers the whole name and is never the part trimmed.
        let a = format!("{}_aaaaaaaaaaaaaaaaaaaa", "x".repeat(40));
        let b = format!("{}_bbbbbbbbbbbbbbbbbbbb", "x".repeat(40));
        let (na, nb) = (
            branch_database_name(&a, "oxy-staging"),
            branch_database_name(&b, "oxy-staging"),
        );
        assert_ne!(na, nb);
        assert!(na.len() <= MAX_NAME_LEN && nb.len() <= MAX_NAME_LEN);
        assert!(validate_name(&na).is_ok() && validate_name(&nb).is_ok());
    }

    fn staging(database: &str) -> BranchRequest {
        BranchRequest {
            project_id: database.into(),
            parent_branch_id: "local".into(),
            name: "oxy-staging".into(),
            database_name: database.into(),
            owner_role: format!("{database}_owner"),
        }
    }

    #[test]
    fn only_the_derived_branch_database_may_be_touched() {
        let req = staging(TENANT);
        let err = checked(&req, TENANT).unwrap_err();
        assert!(matches!(err, ProviderError::BranchIsProduction(_)), "{err}");

        // Another org's database — a valid identifier, not the tenant's, and
        // exactly what a corrupted row would name. Must not be dropped.
        let other = "oxy_org_99999999_2222_3333_4444_555555555555";
        assert!(checked(&req, other).is_err(), "another org's database");
        let other_branch = branch_database_name(other, "oxy-staging");
        assert!(
            checked(&req, &other_branch).is_err(),
            "another org's branch"
        );
        assert!(checked(&req, "bad\"; DROP DATABASE x; --").is_err());

        assert!(checked(&req, &branch_database_name(TENANT, "oxy-staging")).is_ok());
    }

    /// A shared box behind `OXY_OLTP_PROVIDER=local`: no branch copy, reset or
    /// drop — refused before a connection is even attempted (the hosts below
    /// do not resolve, so reaching one would fail differently).
    #[tokio::test]
    async fn branch_operations_refuse_a_cluster_that_is_not_loopback() {
        let req = staging(TENANT);
        let branch = branch_database_name(TENANT, "oxy-staging");
        for (admin, host) in [
            (
                "postgres://postgres:pw@db.internal.invalid:5432/postgres",
                "db.internal.invalid:5432",
            ),
            // Statements on loopback, clients sent elsewhere — still not a dev cluster.
            (
                "postgres://postgres:pw@localhost:5432/postgres",
                "10.0.0.5:5432",
            ),
            (
                "postgres://postgres:pw@10.0.0.5:5432/postgres",
                "localhost:5432",
            ),
        ] {
            let remote = LocalProvider::new(admin, host);
            let refusals = [
                remote.create_branch_impl(&req).await.map(|_| ()),
                remote.reset_branch_impl(&req, &branch).await.map(|_| ()),
                remote.delete_branch_impl(&req, &branch).await,
            ];
            for out in refusals {
                let err = out.expect_err("a non-loopback cluster must refuse");
                assert!(
                    err.to_string().contains("loopback cluster"),
                    "{admin}: {err}"
                );
            }
        }
        for (admin, host) in [
            (
                "postgres://postgres:pw@localhost:15432/postgres",
                "localhost:15432",
            ),
            (
                "postgres://postgres:p@ss@127.0.0.1:32768/postgres",
                "127.0.0.1:32768",
            ),
            ("postgres://postgres:pw@[::1]:5432/postgres", "[::1]:5432"),
        ] {
            assert!(
                LocalProvider::new(admin, host).only_on_loopback().is_ok(),
                "{admin}"
            );
        }
    }

    /// A unix-socket admin DSN names no TCP host at all, so `host_from_dsn`
    /// answers `""`: correctly refused (not loopback), but the message must
    /// say why the host is empty rather than reading like a bug.
    #[test]
    fn a_dsn_naming_no_tcp_host_gets_a_clarifying_refusal() {
        let admin = "postgres:///oxy?host=/var/run/postgresql";
        assert_eq!(super::super::host_from_dsn(admin), "");
        let err = LocalProvider::new(admin, "localhost:15432")
            .only_on_loopback()
            .expect_err("an empty host is not loopback");
        let msg = err.to_string();
        assert!(
            msg.contains("on \"\" (the DSN names no TCP host):"),
            "{msg}"
        );
        assert!(msg.contains("loopback cluster"), "{msg}");

        // A named, merely non-loopback host gets no such suffix.
        let named = LocalProvider::new(
            "postgres://postgres:pw@db.internal.invalid:5432/postgres",
            "localhost:15432",
        )
        .only_on_loopback()
        .expect_err("a non-loopback host is refused");
        assert!(!named.to_string().contains("names no TCP host"), "{named}");
    }

    #[test]
    fn only_object_in_use_counts_as_a_busy_template() {
        let busy = ProviderError::Api {
            status: 500,
            message: "[55006] source database \"x\" is being accessed by other users".into(),
        };
        let other = ProviderError::Api {
            status: 500,
            message: "[42P04] database \"y\" already exists".into(),
        };
        assert!(is_template_busy(&busy));
        assert!(!is_template_busy(&other));
    }

    #[test]
    fn the_state_replay_names_both_databases_and_never_grants_public_blindly() {
        let sql = copy_database_state_sql("oxy_org_a", "oxy_org_a_1234abcd");
        assert!(sql.contains("WHERE datname = 'oxy_org_a'"));
        assert!(sql.contains("REVOKE ALL ON DATABASE %I FROM PUBLIC', 'oxy_org_a_1234abcd'"));
        // PUBLIC is re-granted only what the SOURCE granted it (grantee 0).
        assert!(sql.contains("IF a.grantee = 0 THEN"));
        assert!(sql.contains("SET %s FROM CURRENT"));
    }
}
