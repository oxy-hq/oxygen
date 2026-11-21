//! `oxy oltp` — the OLTP operator verbs that still need the server's own
//! database and secrets.
//!
//! ```bash
//! oxy oltp apply  --org <uuid|email>              # run schemas/*.sql not yet applied
//! oxy oltp audit  --org <uuid|email>              # every role's real authority
//! oxy oltp rotate --org <uuid|email> --role <w>   # rotate a writer's password
//! ```
//!
//! `status`, `provision`, `dsn`, `connect`, `expose` and `deprovision` moved to
//! `oxyc oltp` (`sdk/cli/src/commands/oltp*.ts`) over the admin routes, and were
//! deleted here. These three wait on routes of their own
//! (`internal-docs/oxy-cli-to-oxyc.md`, steps 12 and open question 5).
//!
//! `--org` takes a user's email as well as an org id, because during a demo
//! nobody remembers a UUID.

use clap::{Args, Subcommand};
use oxy::database::client::establish_connection;
use oxy_oltp::migrator;
use oxy_oltp::provisioner::OltpProvisioner;
use oxy_oltp::schema::{GrantLevel, WriterRef};
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
    /// Audit every role: what it is, whether it is still confined, and what it
    /// can reach outside its own schema (writers, the analyst and PUBLIC). Note
    /// confinement checks what each role is a member OF, not what is a member
    /// of IT — so a lingering owner→writer membership left by a failed
    /// `deprovision --writer` is invisible here (harmless: the owner already
    /// dominates the writer).
    Audit(OrgArgs),
    /// Rotate a role's password and reseal it.
    Rotate(DsnArgs),
}

/// Just the org — for the verbs that read per-org state and nothing else.
///
/// Deliberately NOT `TargetArgs`: `status` and `audit` neither claim a schema
/// namespace nor read migrations, so `--workspace` and `--writer` had no
/// meaning there. Sharing one struct made clap ACCEPT both flags on both verbs
/// and then ignore them, which reads as "the flag did something" — the exact
/// silent-no-op the rest of this crate is built to avoid. Each verb now takes
/// the flags it honours, so an unsupported one is a clap error at the prompt.
#[derive(Debug, Args)]
pub struct OrgArgs {
    /// Org UUID, or the email of a member of it.
    #[arg(long)]
    pub org: String,
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

#[derive(Debug, Args)]
pub struct DsnArgs {
    /// Org UUID, or the email of a member of it.
    #[arg(long)]
    pub org: String,
    /// `analyst` (read-only), `owner` (runs migrations), or a writer as
    /// `app:<slug>` / `pipeline:<source>`.
    #[arg(long, default_value = "analyst")]
    pub role: String,
}

pub async fn oltp(args: OltpArgs) -> Result<(), OxyError> {
    let db = establish_connection().await?;
    match args.command {
        OltpCommand::Apply(t) => apply(&db, t).await,
        OltpCommand::Audit(t) => audit(&db, t).await,
        OltpCommand::Rotate(a) => rotate(&db, a).await,
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

/// Build a provisioner. Delegates to `oxy_oltp::provisioner::from_env` so the
/// CLI and the admin console cannot diverge on which provider an org lands on —
/// an operator clicking Provision must get the database an engineer would get
/// from here.
fn provisioner(db: DatabaseConnection) -> Result<OltpProvisioner, OxyError> {
    // Blocking on a future in a sync fn is fine here: this is CLI startup, and
    // the alternative is threading async through every subcommand for one call.
    futures::executor::block_on(oxy_oltp::provisioner::from_env(db))
        .map_err(|e| OxyError::ConfigurationError(e.to_string()))
}

fn parse_writer(spec: &str) -> Result<WriterRef, OxyError> {
    let bad = || {
        OxyError::ConfigurationError(format!(
            "writer {spec:?} must look like `app:<slug>` or `pipeline:<source>`"
        ))
    };
    let (kind, name) = spec.split_once(':').ok_or_else(bad)?;
    match kind {
        "app" => WriterRef::app(name),
        "pipeline" => WriterRef::pipeline(name),
        _ => return Err(bad()),
    }
    .map_err(|e| OxyError::ConfigurationError(e.to_string()))
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

/// What every role in this tenant can actually do, read from the database.
///
/// The local `oltp_roles` table records what Oxy *intended*. This reads
/// `pg_roles` and `pg_auth_members` for what is true — the two diverged on
/// Neon, where API-created roles silently carried `neon_superuser`, and nothing
/// in Oxy's own tables would ever have shown it.
async fn audit(db: &DatabaseConnection, t: OrgArgs) -> Result<(), OxyError> {
    let org_id = resolve_org(db, &t.org).await?;
    let tenant = migrator::tenant_for_org(db, org_id)
        .await
        .map_err(|e| OxyError::RuntimeError(e.to_string()))?;
    let owner_dsn =
        migrator::owner_dsn(&tenant).map_err(|e| OxyError::RuntimeError(e.to_string()))?;

    // `pg_detail`, not `{e}`: a refused session displays as the literal "db
    // error", which hid Neon's `exceeded the quota` for two days.
    let client = oxy_oltp::connect::connect(&owner_dsn, "oltp audit")
        .await
        .map_err(|e| {
            OxyError::RuntimeError(format!("connect: {}", oxy_oltp::connect::pg_detail(&e)))
        })?;
    // Columns generated from `roles::CONFINEMENT_ATTRIBUTES`, so this command
    // and `assert_confined_sql` can never check different things again — the
    // divergence that let a REPLICATION-carrying role print `risk: none`.
    let attr_columns = oxy_oltp::roles::CONFINEMENT_ATTRIBUTES
        .iter()
        .map(|(_, column)| format!("r.{column}"))
        .collect::<Vec<_>>()
        .join(", ");
    let n_attrs = oxy_oltp::roles::CONFINEMENT_ATTRIBUTES.len();
    let rows = client
        .query(
            &format!(
            "SELECT r.rolname, {attr_columns},                     r.rolcanlogin,                     coalesce((SELECT string_agg(g.rolname, ',') FROM pg_auth_members m                               JOIN pg_roles g ON g.oid = m.roleid                               WHERE m.member = r.oid), '') AS memberships              FROM pg_roles r              WHERE r.rolname = $1 OR r.rolname LIKE 'app\\_%' OR r.rolname LIKE 'raw\\_%'                 OR r.rolname = $2              ORDER BY r.rolname"
            ),
            // The tenant's OWN analyst, not the bare constant. On a cluster
            // where tenants share a role namespace the real login is
            // `oxy_analyst_ro_<tag>`, so auditing the constant reported on a
            // decoy — and reported it clean, which is the worst way to be
            // wrong about a privilege audit.
            &[
                &oxy_oltp::schema::analyst_role_for(&tenant.provider, &tenant.database_name),
                &tenant.owner_role,
            ],
        )
        .await
        .map_err(|e| OxyError::RuntimeError(format!("query: {e}")))?;

    println!(
        "{:<28} {:<7} {:<6} {}",
        "role", "login", "risk", "memberships"
    );
    let mut findings = 0usize;
    for row in rows {
        let name: String = row.get(0);
        // Must stay the same list `roles::assert_confined_sql` checks. This
        // command exists to vouch for containment, so an attribute the check
        // rejects and the audit ignores means `risk: none` and exit 0 for a
        // credential provisioning would refuse to hand out. `replication` was
        // exactly that gap: it streams the whole WAL, every other writer's rows
        // included, reading past table ACLs rather than through them.
        let flags: Vec<(&str, bool)> = oxy_oltp::roles::CONFINEMENT_ATTRIBUTES
            .iter()
            .enumerate()
            .map(|(i, (label, _))| (*label, row.get::<_, bool>(i + 1)))
            .collect();
        let login: bool = row.get(n_attrs + 1);
        let memberships: String = row.get(n_attrs + 2);
        let risky: Vec<&str> = flags.iter().filter(|(_, v)| *v).map(|(k, _)| *k).collect();

        // The owner is *meant* to be powerful — it runs migrations. Everything
        // else carrying authority is a finding.
        let is_owner = name == tenant.owner_role;
        let bad = !is_owner && (!risky.is_empty() || !memberships.is_empty());
        if bad {
            findings += 1;
        }
        println!(
            "{:<28} {:<7} {:<6} {}",
            name,
            if login { "yes" } else { "no" },
            if is_owner {
                "admin".to_string()
            } else if bad {
                risky.join("+")
            } else {
                "none".to_string()
            },
            if memberships.is_empty() {
                "-"
            } else {
                &memberships
            }
        );
    }

    if findings > 0 {
        println!(
            "\n{findings} role(s) hold authority they should not. Re-run `oxyc oltp provision` \
             to re-mint them, or `oxy oltp rotate --role <r>` for one."
        );
    } else {
        println!("\nall non-admin roles confined");
    }

    // What each role can reach, not just what it is: a grant outside its own
    // schema is how one writer reads another's rows (`grant_audit`).
    let grants = oxy_oltp::grant_audit::tenant_findings(db, &client, &tenant)
        .await
        .map_err(|e| OxyError::RuntimeError(e.to_string()))?;
    let drifted = print_grant_findings(&grants);

    match (findings, drifted) {
        (0, 0) => Ok(()),
        (f, 0) => Err(OxyError::RuntimeError(format!(
            "{f} role(s) are not confined"
        ))),
        (0, d) => Err(OxyError::RuntimeError(format!(
            "{d} role(s) hold privileges outside their scope"
        ))),
        (f, d) => Err(OxyError::RuntimeError(format!(
            "{f} role(s) are not confined and {d} hold privileges outside their scope"
        ))),
    }
}

/// Print each role's grant findings; returns how many roles have any.
fn print_grant_findings(report: &[oxy_oltp::grant_audit::RoleFindings]) -> usize {
    println!("\ngrants outside each role's scope");
    let mut drifted = 0usize;
    for role in report {
        if role.findings.is_empty() {
            println!("  {:<28} none", role.role);
            continue;
        }
        drifted += 1;
        println!("  {:<28} {} finding(s)", role.role, role.findings.len());
        for f in &role.findings {
            println!("    - {f}");
        }
    }
    if drifted > 0 {
        println!(
            "\n{drifted} role(s) hold privileges outside their scope. Revoke each one above \
             as the owner; `oxyc oltp provision` refuses a writer that still holds any (OXY03)."
        );
    } else {
        println!("\nno role holds privileges outside its scope");
    }
    drifted
}

/// Rotate one role's password, reseal it, and re-assert confinement.
async fn rotate(db: &DatabaseConnection, a: DsnArgs) -> Result<(), OxyError> {
    let org_id = resolve_org(db, &a.org).await?;
    let p = provisioner(db.clone())?;
    let writer = parse_writer(&a.role)?;
    let creds = p
        .rotate_writer(org_id, &writer, GrantLevel::ReadWrite)
        .await
        .map_err(|e| OxyError::RuntimeError(e.to_string()))?;
    println!("✓ rotated {} ({})", creds.role_name, creds.schema_name);
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
