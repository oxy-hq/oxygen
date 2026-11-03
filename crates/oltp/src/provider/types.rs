//! Wire types, shaped after the Neon REST API.
//!
//! Field names follow Neon's JSON so the real client can deserialise straight
//! into these. Where Neon returns more than Oxy needs, the surplus is dropped
//! rather than modelled.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateProjectRequest {
    /// Provider-visible project name. Derived from the org, and unique —
    /// a collision is [`super::ProviderError::ProjectNameTaken`], never a
    /// silent adoption.
    pub name: String,
    /// Provider region id, e.g. `aws-us-east-2`.
    pub region_id: String,
    /// Major Postgres version.
    pub pg_version: u8,
}

/// A provisioned per-org Postgres.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub region_id: String,
    pub pg_version: u8,
    /// The default branch — production. Oxy's other branches (the org's
    /// staging branch) are [`ProjectBranch`]es cut from it and recorded in
    /// `oltp_branches`, never here.
    pub branch: Branch,
    pub database: DatabaseInfo,
    /// Role that owns the database. Its password is disclosed only at project
    /// creation.
    pub owner_role: Role,
    /// Hostname clients connect to.
    pub host: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Branch {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatabaseInfo {
    pub name: String,
    pub owner_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Role {
    pub name: String,
    /// Disclosed **once**, at create or reset. `None` on reads.
    ///
    /// Losing it means the role must be reset, not recovered — the provider
    /// stores only a hash. Seal it before this value goes out of scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

impl Role {
    /// Drop the password, for logging or for persisting a row that must not
    /// carry a plaintext secret.
    pub fn redacted(&self) -> Role {
        Role {
            name: self.name.clone(),
            password: None,
        }
    }
}

/// What a new branch is cut from: the parent's data, or only its tables.
///
/// A sandbox branch of an app that holds customer rows should be able to carry
/// the schema without the data, so a developer's migration can be tried against
/// the real table shapes while no customer record leaves production.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BranchInit {
    /// The parent's schema **and** rows — a copy-on-write fork, what staging is.
    #[default]
    ParentData,
    /// The parent's schema with every table empty. On Neon the branch has no
    /// parent at all (`init_source: schema-only`), which is why it cannot be
    /// reset from one.
    SchemaOnly,
}

/// What every provider answers (400) to a reset of a [`BranchInit::SchemaOnly`]
/// branch: there is no parent data to re-copy, and handing back production's
/// rows would be the opposite of what the branch was cut to avoid.
pub(crate) const SCHEMA_ONLY_RESET_REFUSAL: &str =
    "a schema-only branch cannot be reset from its parent; cut it again";

/// What to branch, and from where. Shared by `create_branch` and
/// `reset_branch`, which need the same facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchRequest {
    pub project_id: String,
    /// The branch the copy is cut from — the tenant's production branch.
    pub parent_branch_id: String,
    /// Provider-visible branch name. Derived from the branch kind
    /// (`oxy-staging`), never chosen, which is what makes adopting an existing
    /// branch of the same name an identity match rather than a coincidence.
    pub name: String,
    /// The tenant database the branch carries a copy of.
    pub database_name: String,
    /// The role that owns that database. On a provider that gives the branch
    /// its own copy of every role, this is the one whose password is reset on
    /// the branch, so production's owner credential stops opening it.
    pub owner_role: String,
    /// Cut the branch from the parent as it was at this instant rather than at
    /// its head. `None` is the head, which is all staging ever wants.
    ///
    /// A provider that cannot time-travel (`LocalProvider`) copies the head and
    /// says so; it never fails the create over it.
    pub parent_timestamp: Option<chrono::DateTime<chrono::Utc>>,
    /// When the provider should delete the branch by itself, so a sandbox that
    /// nobody cleaned up stops costing compute. `None` is never.
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Whether the branch carries the parent's rows or only its tables.
    pub init: BranchInit,
}

impl BranchRequest {
    /// A branch cut from the parent's head with its data and no expiry — what
    /// the org's staging branch is, and the base the other requests tweak.
    pub fn staging(
        project_id: impl Into<String>,
        parent_branch_id: impl Into<String>,
        name: impl Into<String>,
        database_name: impl Into<String>,
        owner_role: impl Into<String>,
    ) -> Self {
        Self {
            project_id: project_id.into(),
            parent_branch_id: parent_branch_id.into(),
            name: name.into(),
            database_name: database_name.into(),
            owner_role: owner_role.into(),
            parent_timestamp: None,
            expires_at: None,
            init: BranchInit::ParentData,
        }
    }
}

/// A copy-on-write branch of a tenant's database, cut from production's head.
///
/// Same data, same schemas and the same role *names* as production, on a
/// different endpoint — which is the whole contract a caller relies on: point
/// the connection at [`Self::host`] / [`Self::database`] and every `app_<slug>`
/// schema is where it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectBranch {
    /// Provider-side branch id. On `LocalProvider` it is the branch database's
    /// name.
    pub id: String,
    pub name: String,
    pub parent_id: String,
    /// Hostname clients connect to — the branch's own endpoint.
    pub host: String,
    pub database: DatabaseInfo,
    /// The owner on the branch.
    ///
    /// `password` is `Some` when this call minted a branch-only credential
    /// (Neon: reset on the branch, so the production owner password no longer
    /// opens it and this one never opens production), and `None` when the
    /// branch shares the project's roles — `LocalProvider`, where roles are
    /// cluster-global and resetting one would rotate production's too.
    pub owner_role: Role,
}

impl Project {
    /// Postgres DSN for a given role. Used to run schema/role DDL and, later,
    /// to hand a writer its connection.
    pub fn dsn_for(&self, role_name: &str, password: &str) -> String {
        format!(
            "postgres://{role_name}:{password}@{host}/{db}?sslmode=require",
            // Encoded: a provider-issued password may contain URI-special
            // characters, and an unescaped `@` silently becomes a host.
            password = crate::roles::encode_userinfo(password),
            host = self.host,
            db = self.database.name,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> Project {
        Project {
            id: "proj-1".into(),
            name: "oxy-org-acme".into(),
            region_id: "aws-us-east-2".into(),
            pg_version: 17,
            branch: Branch {
                id: "br-1".into(),
                name: "main".into(),
            },
            database: DatabaseInfo {
                name: "neondb".into(),
                owner_name: "oxy_owner".into(),
            },
            owner_role: Role {
                name: "oxy_owner".into(),
                password: Some("hunter2".into()),
            },
            host: "ep-1.aws-us-east-2.neon.tech".into(),
        }
    }

    #[test]
    fn dsn_requires_tls() {
        let dsn = project().dsn_for("app_bookings_rw", "pw");
        assert!(dsn.ends_with("?sslmode=require"), "got {dsn}");
        assert!(
            dsn.starts_with("postgres://app_bookings_rw:pw@"),
            "got {dsn}"
        );
    }

    #[test]
    fn redacted_role_drops_the_password() {
        let r = project().owner_role.redacted();
        assert!(r.password.is_none());
        assert_eq!(r.name, "oxy_owner");
    }

    #[test]
    fn absent_password_is_omitted_from_json_not_serialised_as_null() {
        let json = serde_json::to_string(&project().owner_role.redacted()).unwrap();
        assert!(!json.contains("password"), "got {json}");
    }
}
