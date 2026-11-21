//! `oxy oltp` — the one OLTP operator verb that still needs the server's own
//! database and secrets.
//!
//! ```bash
//! oxy oltp apply  --org <uuid|email>              # run schemas/*.sql not yet applied
//! ```
//!
//! `status`, `provision`, `dsn`, `connect`, `expose`, `deprovision`, `audit` and
//! `rotate` moved to `oxyc oltp` (`sdk/cli/src/commands/oltp*.ts`) over the
//! admin routes, and were deleted here. `apply` waits on open question 5 of
//! `internal-docs/oxy-cli-to-oxyc.md` (is a workspace `schemas/` still a path?).
//!
//! `--org` takes a user's email as well as an org id, because during a demo
//! nobody remembers a UUID.

use clap::{Args, Subcommand};
use oxy::database::client::establish_connection;
use oxy_oltp::migrator;
use oxy_shared::errors::OxyError;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};
use uuid::Uuid;

#[derive(Debug, Args)]
pub struct OltpArgs {
    #[command(subcommand)]
    pub command: OltpCommand,
}

#[derive(Debug, Subcommand)]
pub enum OltpCommand {
    /// Apply any `schemas/*.sql` the org's database has not run yet.
    Apply(TargetArgs),
}

#[derive(Debug, Args)]
pub struct TargetArgs {
    /// Org UUID, or the email of a member of it.
    #[arg(long)]
    pub org: String,

    /// Which workspace claims the schema namespaces and supplies the
    /// migrations. Required when the org has more than one.
    #[arg(long)]
    pub workspace: Option<Uuid>,
}

pub async fn oltp(args: OltpArgs) -> Result<(), OxyError> {
    let db = establish_connection().await?;
    match args.command {
        OltpCommand::Apply(t) => apply(&db, t).await,
    }
}

/// Resolve `--org` from a UUID or a member's email.
async fn resolve_org(db: &DatabaseConnection, target: &str) -> Result<Uuid, OxyError> {
    if let Ok(id) = Uuid::parse_str(target) {
        return Ok(id);
    }
    let user = entity::prelude::Users::find()
        .filter(entity::users::Column::Email.eq(target))
        .one(db)
        .await
        .map_err(|e| OxyError::DBError(e.to_string()))?
        .ok_or_else(|| {
            OxyError::ConfigurationError(format!("no user with email {target}, and not a UUID"))
        })?;
    // Every org, not `.one()`. A user in two orgs resolved to whichever row came
    // back first — and every verb here then acted on that org's real database.
    let orgs: Vec<Uuid> = entity::prelude::OrgMembers::find()
        .filter(entity::org_members::Column::UserId.eq(user.id))
        .all(db)
        .await
        .map_err(|e| OxyError::DBError(e.to_string()))?
        .into_iter()
        .map(|m| m.org_id)
        .collect();

    match orgs.len() {
        0 => Err(OxyError::ConfigurationError(format!(
            "{target} is not in any org"
        ))),
        1 => Ok(orgs[0]),
        _ => Err(OxyError::ConfigurationError(format!(
            "{target} is in {} orgs; pass --org <uuid> to say which one:\n{}",
            orgs.len(),
            orgs.iter()
                .map(|o| format!("  {o}"))
                .collect::<Vec<_>>()
                .join("\n")
        ))),
    }
}

/// The org's workspace, refusing to guess when there is more than one.
///
/// Both call sites used `.one(db)` with no ordering, so for a multi-workspace
/// org Postgres decided which workspace CLAIMS each schema namespace and whose
/// `schemas/*.sql` reach the tenant. That undercuts `claim_namespace` directly:
/// a re-run could stamp the claim on a different workspace, or trip
/// `SchemaNamespaceClaimed` against itself.
///
/// Per-org grain means `workspace_id` is not the containment mechanism here, so
/// "whichever row came back first" is exactly the wrong default. Name it.
async fn resolve_workspace(
    db: &DatabaseConnection,
    org_id: Uuid,
    requested: Option<Uuid>,
) -> Result<entity::workspaces::Model, OxyError> {
    let all = entity::prelude::Workspaces::find()
        .filter(entity::workspaces::Column::OrgId.eq(Some(org_id)))
        .order_by_asc(entity::workspaces::Column::Id)
        .all(db)
        .await
        .map_err(|e| OxyError::DBError(e.to_string()))?;

    if let Some(id) = requested {
        return all.into_iter().find(|w| w.id == id).ok_or_else(|| {
            OxyError::ConfigurationError(format!("workspace {id} is not in org {org_id}"))
        });
    }

    match all.len() {
        0 => Err(OxyError::ConfigurationError(format!(
            "org {org_id} has no workspace"
        ))),
        1 => Ok(all.into_iter().next().expect("length checked")),
        _ => {
            let listed = all
                .iter()
                .map(|w| format!("  {}  {}", w.id, w.name))
                .collect::<Vec<_>>()
                .join("\n");
            Err(OxyError::ConfigurationError(format!(
                "org {org_id} has {} workspaces; pass --workspace to say which one \
                 claims the schema namespaces and supplies the migrations:\n{listed}",
                all.len()
            )))
        }
    }
}

async fn apply(db: &DatabaseConnection, t: TargetArgs) -> Result<(), OxyError> {
    let org_id = resolve_org(db, &t.org).await?;
    let tenant = migrator::tenant_for_org(db, org_id)
        .await
        .map_err(|e| OxyError::RuntimeError(e.to_string()))?;
    let revision = latest_ready_revision(db, org_id, t.workspace).await?;

    let dsn = migrator::owner_dsn(&tenant).map_err(|e| OxyError::RuntimeError(e.to_string()))?;
    let outcome = migrator::apply_to_org(db, org_id, revision, &dsn, tenant.id, &tenant.owner_role)
        .await
        .map_err(|e| OxyError::RuntimeError(e.to_string()))?;

    if outcome.applied.is_empty() {
        println!(
            "✓ already up to date ({} applied previously)",
            outcome.already_applied
        );
    } else {
        println!("✓ applied {}:", outcome.applied.len());
        for f in &outcome.applied {
            println!("    {f}");
        }
    }
    Ok(())
}

/// Newest **ready** revision for the org's workspace. A failed compile must
/// never reach a customer's database.
async fn latest_ready_revision(
    db: &DatabaseConnection,
    org_id: Uuid,
    workspace: Option<Uuid>,
) -> Result<Uuid, OxyError> {
    let workspace = resolve_workspace(db, org_id, workspace).await?;

    entity::prelude::Revisions::find()
        .filter(entity::revisions::Column::WorkspaceId.eq(workspace.id))
        .filter(entity::revisions::Column::Status.eq("ready"))
        .order_by_desc(entity::revisions::Column::StartedAt)
        .one(db)
        .await
        .map_err(|e| OxyError::DBError(e.to_string()))?
        .map(|r| r.revision_id)
        .ok_or_else(|| {
            OxyError::ConfigurationError(
                "workspace has no successful compile — run `oxy compile` first".into(),
            )
        })
}
