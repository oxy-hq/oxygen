//! `oxy oltp` — provision an org's OLTP database and apply its schema.
//!
//! The operator surface for the per-org OLTP plane. Two verbs, both
//! idempotent, both safe to re-run:
//!
//! ```bash
//! oxy oltp provision --org <uuid|email>   # create the database + writers
//! oxy oltp apply     --org <uuid|email>   # run schemas/*.sql not yet applied
//! oxy oltp status    --org <uuid|email>   # what exists and what is pending
//! ```
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
    /// Create the org's Postgres, its writers, and the analyst credential.
    Provision(ProvisionArgs),
    /// Apply any `schemas/*.sql` the org's database has not run yet.
    Apply(TargetArgs),
    /// Show what is provisioned and what is pending.
    Status(OrgArgs),
    /// Print a connection string for a role, read from the sealed credentials.
    Dsn(DsnArgs),
    /// Open `psql` against the org's database. Debugging, not routine access.
    Connect(DsnArgs),
    /// Audit every role: what it is, whether it is still confined, and what it
    /// can reach outside its own schema (writers, the analyst and PUBLIC). Note
    /// confinement checks what each role is a member OF, not what is a member
    /// of IT — so a lingering owner→writer membership left by a failed
    /// `deprovision --writer` is invisible here (harmless: the owner already
    /// dominates the writer).
    Audit(OrgArgs),
    /// Rotate a role's password and reseal it.
    Rotate(DsnArgs),
    /// Let analytics read a writer's schema, or withdraw it.
    Expose(ExposeArgs),
    /// Deprovision one writer (`--writer app:<slug>`, drops that schema only) or,
    /// without it, destroy the org's whole database at the provider. Irreversible.
    Deprovision(DeprovisionArgs),
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
pub struct ProvisionArgs {
    #[command(flatten)]
    pub target: TargetArgs,

    /// Writers to ensure exist, as `app:<slug>` or `pipeline:<source>`.
    /// Repeatable.
    #[arg(long = "writer")]
    pub writers: Vec<String>,
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

#[derive(Debug, Args)]
pub struct ExposeArgs {
    /// Org UUID, or the email of a member of it.
    #[arg(long)]
    pub org: String,
    /// The writer, as `app:<slug>` or `pipeline:<source>`.
    #[arg(long)]
    pub writer: String,
    /// Withdraw analytics access instead of granting it.
    #[arg(long)]
    pub revoke: bool,
}

#[derive(Debug, Args)]
pub struct DeprovisionArgs {
    /// Org UUID, or the email of a member of it.
    #[arg(long)]
    pub org: String,
    /// Deprovision a SINGLE writer (`app:<slug>` or `pipeline:<source>`), dropping
    /// its schema + role and leaving the rest of the tenant intact. This is what
    /// an app-delete/rename asks for when it refuses. Omit to destroy the whole
    /// tenant database instead.
    #[arg(long)]
    pub writer: Option<String>,
    /// Required — this is destructive. With `--writer`, it drops that one
    /// schema and its data. Without it, it destroys the whole provider project,
    /// which on Neon cannot be undone from here.
    #[arg(long)]
    pub yes: bool,
}

pub async fn oltp(args: OltpArgs) -> Result<(), OxyError> {
    let db = establish_connection().await?;
    match args.command {
        OltpCommand::Provision(t) => provision(&db, t).await,
        OltpCommand::Apply(t) => apply(&db, t).await,
        OltpCommand::Status(t) => status(&db, t).await,
        OltpCommand::Dsn(a) => dsn(&db, a).await,
        OltpCommand::Connect(a) => connect(&db, a).await,
        OltpCommand::Audit(t) => audit(&db, t).await,
        OltpCommand::Rotate(a) => rotate(&db, a).await,
        OltpCommand::Expose(a) => expose(&db, a).await,
        OltpCommand::Deprovision(a) => deprovision(&db, a).await,
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

async fn provision(db: &DatabaseConnection, args: ProvisionArgs) -> Result<(), OxyError> {
    let ProvisionArgs { target: t, writers } = args;
    let org_id = resolve_org(db, &t.org).await?;
    let p = provisioner(db.clone())?;

    let tenant = p
        .provision(org_id)
        .await
        .map_err(|e| OxyError::RuntimeError(e.to_string()))?;
    println!("✓ database {} on {}", tenant.database_name, tenant.host);

    // The workspace that claims each schema namespace. One database serves an
    // org; schema definitions compile per workspace, so somebody has to own
    // each namespace.
    let workspace = resolve_workspace(db, org_id, t.workspace).await?;

    for spec in &writers {
        let writer = parse_writer(spec)?;
        let creds = p
            .ensure_writer(org_id, &writer, GrantLevel::ReadWrite, Some(workspace.id))
            .await
            .map_err(|e| OxyError::RuntimeError(e.to_string()))?;
        println!("✓ writer {writer} → schema {}", creds.schema_name);
    }

    p.ensure_analyst(org_id)
        .await
        .map_err(|e| OxyError::RuntimeError(e.to_string()))?;
    println!("✓ analyst credential");
    println!("\nNext: oxy compile … && oxy oltp apply --org {}", t.org);
    Ok(())
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

/// Print a usable DSN, decrypting the sealed password.
///
/// The demo tooling used to build these by hand from `LocalProvider`'s
/// deterministic passwords, which meant nothing could be verified against a
/// managed provider — every script silently assumed `pw_<role>` and
/// `sslmode=disable`. Reading the real credential makes the boundary tests and
/// `oltp-psql` provider-agnostic, and gives an engineer the connection string
/// the admin console shows without needing a running server.
async fn dsn(db: &DatabaseConnection, a: DsnArgs) -> Result<(), OxyError> {
    let out = resolve_dsn(db, a).await?;
    // Bare stdout, no decoration: this is meant to be captured into a variable.
    println!("{out}");
    Ok(())
}

async fn resolve_dsn(db: &DatabaseConnection, a: DsnArgs) -> Result<String, OxyError> {
    let org_id = resolve_org(db, &a.org).await?;
    let out = if a.role == "owner" {
        // The owner credential is what runs migrations; useful for verifying a
        // boundary from the one role that is allowed to cross it.
        let tenant = migrator::tenant_for_org(db, org_id)
            .await
            .map_err(|e| OxyError::RuntimeError(e.to_string()))?;
        migrator::owner_dsn(&tenant).map_err(|e| OxyError::RuntimeError(e.to_string()))?
    } else if a.role == "analyst" {
        oxy_oltp::resolver::resolve_analyst_connection_for_org(db, org_id)
            .await
            .map_err(|e| OxyError::RuntimeError(e.to_string()))?
            .dsn()
    } else {
        let writer = parse_writer(&a.role)?;
        oxy_oltp::resolver::resolve_writer_connection_for_org(db, org_id, &writer)
            .await
            .map_err(|e| OxyError::RuntimeError(e.to_string()))?
            .dsn
    };
    Ok(out)
}

/// `psql` straight into the org's database as `--role`.
///
/// Defaults to the analyst, so the reflex action is the read-only one — you
/// have to ask for a credential that can write. On a managed provider the DSN
/// carries `sslmode=require`, so this also proves TLS end to end.
async fn connect(db: &DatabaseConnection, a: DsnArgs) -> Result<(), OxyError> {
    let role = a.role.clone();
    let target = resolve_dsn(db, a).await?;

    if which_psql().is_none() {
        return Err(OxyError::ConfigurationError(
            "psql is not on PATH — install libpq, or use: oxy oltp dsn --org <org>".into(),
        ));
    }
    eprintln!("connecting as {role} (^D to exit)");
    // The password goes in the ENVIRONMENT, not argv. A DSN passed as an
    // argument is world-readable for the life of the session — `ps auxww` shows
    // every user on the box a working credential for a tenant's database, and
    // shells log it besides. `PGPASSWORD` is libpq's own answer and is readable
    // only by the same user (and root) through /proc.
    let (sanitised, password) = split_password(&target);
    // exec-style handoff: psql owns the terminal, and its exit code is ours.
    let mut cmd = std::process::Command::new("psql");
    cmd.arg(&sanitised);
    if let Some(pw) = password {
        cmd.env("PGPASSWORD", pw);
    }
    let status = cmd
        .status()
        .map_err(|e| OxyError::RuntimeError(format!("could not start psql: {e}")))?;
    if !status.success() {
        return Err(OxyError::RuntimeError(format!("psql exited with {status}")));
    }
    Ok(())
}

/// A DSN with its password removed, and the password.
///
/// Only the `userinfo` half of a URI is touched: everything else — host, port,
/// database, and every query parameter, `sslmode` included — is passed through
/// unchanged, because dropping one of those would silently change how the
/// connection is made rather than merely how it is spelled.
///
/// Returns the DSN untouched and `None` when there is no password to move,
/// which is the local-cluster case.
fn split_password(dsn: &str) -> (String, Option<String>) {
    let Some((scheme, rest)) = dsn.split_once("://") else {
        return (dsn.to_string(), None);
    };
    // The LAST `@` before the path: a password may legally contain one, and
    // splitting on the first would cut the credential in half and hand psql a
    // hostname made of the rest of it.
    let host_start = rest.find('/').unwrap_or(rest.len());
    let Some(at) = rest[..host_start].rfind('@') else {
        return (dsn.to_string(), None);
    };
    let (userinfo, tail) = rest.split_at(at);
    let Some((user, password)) = userinfo.split_once(':') else {
        return (dsn.to_string(), None);
    };
    (
        format!("{scheme}://{user}{tail}"),
        Some(decode_userinfo(password)),
    )
}

/// Percent-decoding for a userinfo field.
///
/// The DSN carries the password percent-encoded (`encode_userinfo` puts it
/// there); `PGPASSWORD` takes the raw bytes, so handing over the encoded form
/// would authenticate with the wrong string for any password containing a
/// reserved character — and fail as "password authentication failed", which
/// reads as a wrong credential rather than a mangled one.
fn decode_userinfo(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn which_psql() -> Option<()> {
    std::process::Command::new("psql")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()
        .filter(|s| s.success())
        .map(|_| ())
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
            "\n{findings} role(s) hold authority they should not. Re-run `oxy oltp provision` \
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
             as the owner; `oxy oltp provision` refuses a writer that still holds any (OXY03)."
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

/// Opt a writer's schema into (or out of) analytics.
///
/// `raw_*` is readable by the analyst on creation because ETL data exists to be
/// analysed; `app_*` is not, because live application state may be bookings or
/// patient records. This is the only way to change that, and it is deliberately
/// a separate verb rather than a flag on `provision` — widening who can read a
/// tenant's application data should be its own decision with its own audit line.
async fn expose(db: &DatabaseConnection, a: ExposeArgs) -> Result<(), OxyError> {
    let org_id = resolve_org(db, &a.org).await?;
    let writer = parse_writer(&a.writer)?;
    let p = provisioner(db.clone())?;
    p.set_analytics_visibility(org_id, &writer, !a.revoke)
        .await
        .map_err(|e| OxyError::RuntimeError(e.to_string()))?;
    println!(
        "✓ {} is {} the read-only analyst",
        writer.schema_name(),
        if a.revoke {
            "hidden from"
        } else {
            "readable by"
        }
    );
    Ok(())
}

/// Destroy the org's database at the provider and forget it locally.
///
/// Deleting an org already does this (`org_handlers`), so this is for the case
/// the org survives and its database should not — a demo tenant, a mis-created
/// one, or an orphan whose local row was lost and re-created.
///
/// Requires `--yes`: every other verb here is idempotent and safe to re-run,
/// and this one is neither.
async fn deprovision(db: &DatabaseConnection, a: DeprovisionArgs) -> Result<(), OxyError> {
    let org_id = resolve_org(db, &a.org).await?;
    let tenant = migrator::tenant_for_org(db, org_id).await.ok();

    let Some(tenant) = tenant else {
        println!("no OLTP database for org {org_id} — nothing to do");
        return Ok(());
    };

    // Per-writer: drop ONE app/pipeline's schema + role, leaving the tenant. This
    // is the verb the app-delete/rename guard points an operator at.
    if let Some(spec) = &a.writer {
        let writer = parse_writer(spec)?;
        if !a.yes {
            // Host, not just database — on a fleet with more than one cluster
            // reachable it is the line that says you are pointed at the right one.
            println!(
                "This DROPS schema {} and all its data in {} on {} ({})",
                writer.schema_name(),
                tenant.database_name,
                tenant.host,
                tenant.provider
            );
            println!("\nRe-run with --yes to proceed.");
            return Err(OxyError::ConfigurationError("refused without --yes".into()));
        }
        let p = provisioner(db.clone())?;
        p.deprovision_writer(org_id, &writer)
            .await
            .map_err(|e| OxyError::RuntimeError(e.to_string()))?;
        println!(
            "✓ deprovisioned writer {writer} (schema {})",
            writer.schema_name()
        );
        return Ok(());
    }

    if !a.yes {
        println!("This DESTROYS {} on {}", tenant.database_name, tenant.host);
        println!("  provider: {}", tenant.provider);
        if tenant.provider != "local" {
            println!("  the provider project is deleted and cannot be recovered from here");
        }
        println!("\nRe-run with --yes to proceed.");
        return Err(OxyError::ConfigurationError("refused without --yes".into()));
    }

    let p = provisioner(db.clone())?;
    p.deprovision(org_id)
        .await
        .map_err(|e| OxyError::RuntimeError(e.to_string()))?;
    println!(
        "✓ deprovisioned {} ({})",
        tenant.database_name, tenant.provider
    );
    Ok(())
}

async fn status(db: &DatabaseConnection, t: OrgArgs) -> Result<(), OxyError> {
    let org_id = resolve_org(db, &t.org).await?;
    match migrator::tenant_for_org(db, org_id).await {
        Err(_) => {
            println!("no OLTP database for org {org_id}");
            println!("  run: oxy oltp provision --org {}", t.org);
        }
        Ok(tenant) => {
            println!("database  {}", tenant.database_name);
            println!("host      {}", tenant.host);
            println!("provider  {}", tenant.provider);
            // `oltp_tenants.pg_version` was written by provisioning and read by
            // nothing, so a tenant left on an older major than the one Oxy now
            // provisions was invisible everywhere — the record existed and no
            // surface showed it.
            // The drift note is for MANAGED providers only. There the default
            // is what Oxy requests, so a row below it names a real action. On
            // `local` the row holds the cluster's OWN major and the requested
            // version is not honoured at all, so a developer on 17 would get a
            // permanent note pointing at something they cannot do.
            // `differs`, not `behind`: this is `!=`, so it is also true for a
            // tenant AHEAD of the default. The rendered text survives that; the
            // name should not claim a comparison it does not make.
            let differs =
                u8::try_from(tenant.pg_version).ok() != Some(oxy_oltp::config::DEFAULT_PG_VERSION);
            println!(
                "pg        {}{}",
                tenant.pg_version,
                if differs && tenant.provider != "local" {
                    format!(
                        " (provisioning now uses {})",
                        oxy_oltp::config::DEFAULT_PG_VERSION
                    )
                } else {
                    String::new()
                }
            );
            println!("status    {}", tenant.status.as_str());
            println!(
                "platform  v{}/{}",
                tenant.platform_schema_version,
                oxy_oltp::platform::PLATFORM_SCHEMA_VERSION
            );
            println!(
                "analyst   {}",
                if tenant.analyst_password_ciphertext.is_some() {
                    "ready"
                } else {
                    "NOT MINTED — postgres_managed cannot resolve"
                }
            );

            let roles = oxy_oltp::entity::roles::Entity::find()
                .filter(oxy_oltp::entity::roles::Column::TenantRowId.eq(tenant.id))
                .all(db)
                .await
                .map_err(|e| OxyError::DBError(e.to_string()))?;
            println!("\nschemas");
            for r in roles {
                println!("  {:<24} {}", r.schema_name, r.role_name);
            }
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

#[cfg(test)]
mod connect_tests {
    use super::{decode_userinfo, split_password};

    /// The password must not survive into the argument vector: `ps auxww` is
    /// world-readable, so a DSN passed as an argument hands every user on the
    /// box a working credential for a tenant's database.
    #[test]
    fn the_password_leaves_the_dsn() {
        let (dsn, pw) =
            split_password("postgresql://app_bookings_rw:s3cret@db.neon.tech/oxy_org_x");
        assert_eq!(dsn, "postgresql://app_bookings_rw@db.neon.tech/oxy_org_x");
        assert_eq!(pw.as_deref(), Some("s3cret"));
        assert!(!dsn.contains("s3cret"));
    }

    /// Everything that decides HOW the connection is made has to survive, or
    /// sanitising the DSN would quietly change the connection rather than its
    /// spelling. `sslmode` is the one that matters most.
    #[test]
    fn every_other_part_of_the_dsn_survives() {
        let (dsn, _) = split_password(
            "postgresql://u:p@db.neon.tech:5433/oxy_org_x?sslmode=require&options=-csearch_path%3Dapp_x",
        );
        assert_eq!(
            dsn,
            "postgresql://u@db.neon.tech:5433/oxy_org_x?sslmode=require&options=-csearch_path%3Dapp_x"
        );
    }

    /// A **literal** `@` in the password must not split the host.
    ///
    /// Oxy's own DSNs percent-encode the userinfo, so this shape only arrives
    /// from a hand-written `--dsn`. That is exactly why it is tested with a raw
    /// `@`: the encoded form `pa%40ss` contains no literal `@` at all, so it
    /// passes whether the split takes the first separator or the last, and an
    /// earlier version of this test used it — proving nothing about the
    /// behaviour its name describes.
    #[test]
    fn a_literal_at_sign_in_the_password_does_not_split_the_host() {
        let (dsn, pw) = split_password("postgresql://u:pa@ss@host/db");
        assert_eq!(dsn, "postgresql://u@host/db");
        assert_eq!(pw.as_deref(), Some("pa@ss"));
    }

    /// The encoded form is the one Oxy actually produces, and it must decode.
    #[test]
    fn an_encoded_at_sign_round_trips() {
        let (dsn, pw) = split_password("postgresql://u:pa%40ss@host/db");
        assert_eq!(dsn, "postgresql://u@host/db");
        assert_eq!(pw.as_deref(), Some("pa@ss"));
    }

    /// The DSN carries the password percent-encoded; `PGPASSWORD` takes raw
    /// bytes. Handing over the encoded form fails as "password authentication
    /// failed", which reads as a wrong credential rather than a mangled one.
    #[test]
    fn the_password_is_decoded_on_the_way_out() {
        assert_eq!(decode_userinfo("a%2Fb%3Ac%40d"), "a/b:c@d");
        assert_eq!(decode_userinfo("plain"), "plain");
        // A stray `%` is not an escape; it must survive rather than eat bytes.
        assert_eq!(decode_userinfo("100%"), "100%");
    }

    /// A local cluster DSN has no password to move.
    #[test]
    fn a_dsn_without_a_password_is_passed_through() {
        for dsn in [
            "postgresql://postgres@localhost:5432/oxy_org_x",
            "postgresql://localhost/oxy_org_x",
            "not-a-url",
        ] {
            assert_eq!(split_password(dsn), (dsn.to_string(), None), "{dsn}");
        }
    }
}
