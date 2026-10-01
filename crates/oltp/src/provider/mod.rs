//! Provider abstraction for per-org Postgres.
//!
//! The trait is shaped by the **Neon REST API** — projects contain branches,
//! branches contain roles and databases, and a role's password is returned
//! exactly once at create/reset time. Modelling that faithfully now means the
//! real client is a transport swap rather than a redesign.
//!
//! Three implementations: [`NeonProvider`] (the real control plane),
//! [`LocalProvider`] (one Postgres cluster, for tests and the demo), and
//! [`MockProvider`] (in-memory, with fault injection).

mod local;
mod mock;
mod neon;
mod types;

pub use local::branch_database_name;
pub use local::{LocalProvider, database_name_for, host_from_dsn};
pub use mock::MockProvider;
pub use neon::NeonProvider;
pub use types::{
    Branch, BranchRequest, CreateProjectRequest, DatabaseInfo, Project, ProjectBranch, Role,
};

/// Role that owns a tenant database.
///
/// Fixed rather than derived: a Neon project holds exactly one org's database,
/// so there is nothing to disambiguate, and a deterministic name is what lets a
/// half-finished provision be reconciled instead of duplicated.
/// [`LocalProvider`] deliberately differs — it puts every tenant on one cluster,
/// where roles are global and one fixed name would collide across tenants.
pub const OWNER_ROLE: &str = "oxy_owner";

/// Database created alongside the project. Neon's own default name.
pub const DEFAULT_DATABASE: &str = "neondb";

/// A clickable console link for a provisioned project, when the provider has a
/// console.
///
/// Neon projects live at a stable console URL keyed on the project id, and an
/// operator's most common next step after "it is provisioned" is to open it —
/// so the admin UI shows the link rather than making someone paste the id into
/// a URL by hand. `LocalProvider` and `MockProvider` have no console, so this
/// is `None` and the UI shows the plain name.
pub fn console_url(provider: &str, project_id: &str) -> Option<String> {
    match provider {
        "neon" => Some(format!(
            "https://console.neon.tech/app/projects/{project_id}"
        )),
        _ => None,
    }
}

use async_trait::async_trait;

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("project {0:?} not found")]
    ProjectNotFound(String),
    #[error("role {0:?} not found on branch {1:?}")]
    RoleNotFound(String, String),
    /// A branch operation named a branch the provider does not have.
    ///
    /// Only `reset_branch` raises it — `delete_branch` is idempotent and
    /// `create_branch` makes what is missing — so a caller resetting a branch
    /// that vanished provider-side can recreate it rather than fail.
    #[error("branch {0:?} not found")]
    BranchNotFound(String),
    /// A branch carries Oxy's derived name but was not cut from the branch it
    /// should have been — so it is not the half-finished branch adoption
    /// exists to recover, and taking it over would serve staging from
    /// somewhere else's data.
    #[error("branch {name:?} exists but was cut from {parent:?}, not from {expected:?}")]
    BranchParentMismatch {
        name: String,
        parent: String,
        expected: String,
    },
    /// A branch operation aimed at production. Raised before any destructive
    /// call is sent.
    #[error("refusing a branch operation on {0:?}: it is the production branch or database")]
    BranchIsProduction(String),
    /// The provider rejected a create because the name is taken.
    ///
    /// Never silently adopt the existing resource: `airhouse` shipped that and
    /// it could have granted cross-tenant data access when two orgs picked the
    /// same name. Surface it and let an operator resolve it.
    /// Carries whatever the provider names the object — a project on Neon, a
    /// database on `LocalProvider` — so the message says "name", not "project
    /// name", which read wrongly for `oxy_org_<uuid>`.
    #[error("the name {0:?} is already taken")]
    ProjectNameTaken(String),
    /// The project exists under the name Oxy derives, and is not Oxy's.
    ///
    /// Distinct from [`Self::ProjectNameTaken`] because the remedies differ and
    /// the states are not distinguishable from a name alone. A taken name is a
    /// caller passing something it chose; this is a database sitting where a
    /// tenant belongs — a restored dump, a hand-made database, one left by an
    /// older owner-naming scheme — and the fix is a rename or a drop, by
    /// someone who can see WHO owns it. So it carries the observed owner, which
    /// the query that detects it has already read.
    ///
    /// Same reasoning that gave `assert_schema_owned_sql` its own `OXY02`
    /// rather than reusing a nearby error: "exists" is not "is ours", and the
    /// caller cannot act on a message that will not say which it hit.
    #[error("database {name:?} exists but is owned by {owner:?}, not by Oxy")]
    ProjectNotOwned { name: String, owner: String },
    #[error("provider rate-limited the request")]
    RateLimited,
    #[error("provider API error ({status}): {message}")]
    Api { status: u16, message: String },
    #[error("provider transport error: {0}")]
    Transport(String),
}

impl ProviderError {
    /// Whether retrying the same call unchanged could succeed.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            ProviderError::RateLimited
                | ProviderError::Transport(_)
                | ProviderError::Api {
                    status: 500..=599,
                    ..
                }
        )
    }
}

/// Control-plane operations against the managed-Postgres provider.
///
/// Implementations must be **idempotent where the verb allows it**: `delete_*`
/// succeeds whether or not the resource exists, and `get_*` returns `Ok(None)`
/// rather than an error for a missing resource. `create_*` is the exception —
/// it reports [`ProviderError::ProjectNameTaken`] so the caller can decide.
#[async_trait]
pub trait OltpProvider: Send + Sync {
    /// Human-readable provider name, for logs and the `oltp_tenants.provider`
    /// column.
    fn name(&self) -> &'static str;

    /// A DSN that may administer roles the tenant owner cannot, if the provider
    /// has one.
    ///
    /// `oxy_analyst_ro` is a fixed name while roles are **cluster-global**, so
    /// on a provider that puts several tenants on one cluster the second
    /// tenant's owner cannot touch the role the first tenant's owner created —
    /// `permission denied to alter role … ADMIN option`. Neon does not have
    /// this problem (one project per tenant, so the name is not shared) and
    /// returns `None`, keeping role DDL on the least-privileged connection that
    /// can do it.
    fn role_admin_dsn(&self) -> Option<String> {
        None
    }

    async fn create_project(&self, req: CreateProjectRequest) -> Result<Project, ProviderError>;

    async fn get_project(&self, project_id: &str) -> Result<Option<Project>, ProviderError>;

    /// Idempotent: deleting an already-absent project is `Ok(())`.
    async fn delete_project(&self, project_id: &str) -> Result<(), ProviderError>;

    /// Create a role. The returned [`Role::password`] is `Some` — this is the
    /// only time the provider discloses it, so the caller must seal it before
    /// dropping the value.
    async fn create_role(
        &self,
        project_id: &str,
        branch_id: &str,
        role_name: &str,
    ) -> Result<Role, ProviderError>;

    /// Look up a role. [`Role::password`] is `None`: the provider does not
    /// re-disclose it. A caller that has lost the password must reset it.
    async fn get_role(
        &self,
        project_id: &str,
        branch_id: &str,
        role_name: &str,
    ) -> Result<Option<Role>, ProviderError>;

    /// Rotate a role's password. Returns the new one, disclosed once.
    async fn reset_role_password(
        &self,
        project_id: &str,
        branch_id: &str,
        role_name: &str,
    ) -> Result<Role, ProviderError>;

    /// Idempotent: deleting an already-absent role is `Ok(())`.
    async fn delete_role(
        &self,
        project_id: &str,
        branch_id: &str,
        role_name: &str,
    ) -> Result<(), ProviderError>;

    /// Cut a copy-on-write branch of the tenant database from the head of
    /// `req.parent_branch_id`, with an endpoint of its own.
    ///
    /// **Adopts** a branch already carrying `req.name` in this project rather
    /// than failing or duplicating it: the name is derived from the branch kind
    /// and the project is one org's, so a match is the half-finished result of
    /// an earlier attempt that crashed before recording it — the same argument
    /// `create_project` makes for a derived project name.
    ///
    /// See [`ProjectBranch::owner_role`] for when the owner password is
    /// disclosed.
    async fn create_branch(&self, req: &BranchRequest) -> Result<ProjectBranch, ProviderError>;

    /// Throw away everything written to `branch_id` since it was cut and
    /// re-cut it from the parent's current head. The endpoint (host) and the
    /// branch id survive where the provider allows it.
    ///
    /// Every role on the branch reverts to production's password along with
    /// the data, so the owner credential is re-minted here and the caller must
    /// re-mint every other one it holds for the branch.
    ///
    /// [`ProviderError::BranchNotFound`] when the branch is gone.
    async fn reset_branch(
        &self,
        req: &BranchRequest,
        branch_id: &str,
    ) -> Result<ProjectBranch, ProviderError>;

    /// Idempotent: deleting an already-absent branch is `Ok(())`.
    ///
    /// Takes the same [`BranchRequest`] as the other two so each provider can
    /// prove, on its own terms, that `branch_id` is the branch `req` names and
    /// not production: Neon asks whether it is the project's default/protected
    /// branch or the parent; `LocalProvider` requires the exact derived branch
    /// database name, since there a wrong id is a `DROP DATABASE` of someone's
    /// data. Deleting production is never a branch operation.
    async fn delete_branch(
        &self,
        req: &BranchRequest,
        branch_id: &str,
    ) -> Result<(), ProviderError>;

    /// Whether `delete_project` takes the project's branches with it.
    ///
    /// True on Neon, where a branch lives inside the project, so a branch that
    /// cannot be deleted first must not stop the project delete that removes it
    /// anyway. False — the default, and the strict answer — where a branch is
    /// an independent object the project delete would strand (`LocalProvider`'s
    /// sibling database).
    fn project_delete_takes_branches(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod console_url_tests {
    use super::console_url;

    /// The shape is Neon's live console URL (`/app/projects/<id>`) — stable, but
    /// not part of their documented API, so this test is the record of it,
    /// verified against the live console on 2026-08-24. If Neon moves the path,
    /// this is where the break surfaces.
    #[test]
    fn neon_projects_get_a_console_link_others_do_not() {
        assert_eq!(
            console_url("neon", "cold-sky-123").as_deref(),
            Some("https://console.neon.tech/app/projects/cold-sky-123"),
            "the Neon console URL is a contract with a third party"
        );
        assert_eq!(console_url("local", "oxy_org_x"), None);
        assert_eq!(console_url("mock", "whatever"), None);
    }
}
