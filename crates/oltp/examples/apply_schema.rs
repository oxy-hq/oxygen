//! Apply a workspace's compiled `schemas/*.sql` to an org's OLTP database.
//!
//! ```bash
//! export OXY_DATABASE_URL=postgresql://postgres:postgres@localhost:15432/oxy
//! cargo run -p oxy-oltp --example apply_schema -- --email luong@oxy.tech
//! ```
//!
//! Resolves the org from the user, takes that workspace's newest successful
//! revision, and applies whatever it has not applied yet. Idempotent — re-run
//! it freely.

use oxy_oltp::migrator;
use sea_orm::{ColumnTrait, ConnectOptions, Database, EntityTrait, QueryFilter, QueryOrder};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let email = args
        .iter()
        .position(|a| a == "--email")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "luong@oxy.tech".to_string());

    let db = control_plane().await?;

    let user = entity::prelude::Users::find()
        .filter(entity::users::Column::Email.eq(email.clone()))
        .one(&db)
        .await?
        .ok_or_else(|| format!("no user with email {email}"))?;
    let membership = entity::prelude::OrgMembers::find()
        .filter(entity::org_members::Column::UserId.eq(user.id))
        .one(&db)
        .await?
        .ok_or("user is not a member of any org")?;
    let org_id = membership.org_id;

    // Newest revision for any workspace in this org. A single workspace is the
    // common case; the namespace claim is what keeps several honest.
    let workspace = entity::prelude::Workspaces::find()
        .filter(entity::workspaces::Column::OrgId.eq(Some(org_id)))
        .one(&db)
        .await?
        .ok_or("org has no workspace")?;
    let revision = entity::prelude::Revisions::find()
        .filter(entity::revisions::Column::WorkspaceId.eq(workspace.id))
        // Only a `ready` revision — a failed compile must never be applied to
        // a customer's database.
        .filter(entity::revisions::Column::Status.eq("ready"))
        .order_by_desc(entity::revisions::Column::StartedAt)
        .one(&db)
        .await?
        .ok_or("workspace has no successful compile — run `oxy compile` first")?;

    let tenant = migrator::tenant_for_org(&db, org_id).await?;
    let dsn = migrator::owner_dsn(&tenant)?;

    println!("→ org {org_id}");
    println!("→ revision {}", revision.revision_id);
    println!("→ database {}", tenant.database_name);

    let outcome = migrator::apply_to_org(
        &db,
        org_id,
        revision.revision_id,
        &dsn,
        tenant.id,
        &tenant.owner_role,
    )
    .await?;

    if outcome.applied.is_empty() {
        println!(
            "\n✓ already up to date ({} migration(s) previously applied)",
            outcome.already_applied
        );
    } else {
        println!("\n✓ applied {} migration(s):", outcome.applied.len());
        for f in &outcome.applied {
            println!("    {f}");
        }
    }
    Ok(())
}

/// Oxy's control-plane Postgres, with the platform port installed first: the
/// owner password is sealed under the master key through it, and `oxy`
/// installs it at boot.
async fn control_plane() -> Result<sea_orm::DatabaseConnection, Box<dyn std::error::Error>> {
    oxy_platform::ProcessPlatformDb::install();
    let url = std::env::var("OXY_DATABASE_URL")
        .map_err(|_| "OXY_DATABASE_URL must point at Oxy's control-plane Postgres")?;
    Ok(Database::connect(ConnectOptions::new(url)).await?)
}
