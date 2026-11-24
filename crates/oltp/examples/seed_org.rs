//! POC demo: provision a per-org OLTP database for a **real Oxy org**, through
//! the real [`OltpProvisioner`], so `type: postgres_managed` resolves in the IDE.
//!
//! Unlike `seed_local`, this writes to Oxy's control plane (`oltp_tenants`,
//! `oltp_roles`) — which is what `postgres_managed` reads. Use it when you want
//! the database wired into a running Oxy, not just a standalone Postgres.
//!
//! ```bash
//! # Oxy's own Postgres (control plane) — the same one `oxy serve` uses.
//! export OXY_DATABASE_URL=postgres://postgres:postgres@localhost:5432/oxy
//! # Where tenant databases get created. Defaults to OXY_DATABASE_URL's cluster.
//! export OXY_OLTP_ADMIN_URL=postgresql://postgres:postgres@localhost:15432/postgres
//! export OXY_ENCRYPTION_KEY=<same key oxy serve uses>
//!
//! cargo run -p oxy-oltp --example seed_org -- --email luong@oxy.tech
//! cargo run -p oxy-oltp --example seed_org -- --email luong@oxy.tech --reset
//! ```
//!
//! Migrations must have run first (`oxy serve` does it, or
//! `cargo run -p migration --bin migration` then start oxy once).

use std::sync::Arc;

use oxy_oltp::platform::PLATFORM_SCHEMA_VERSION;
use oxy_oltp::provider::LocalProvider;
use oxy_oltp::provisioner::OltpProvisioner;
use oxy_oltp::schema::{GrantLevel, WriterRef};
use oxy_oltp::sql::PgSqlExecutor;
use sea_orm::{ColumnTrait, ConnectOptions, Database, EntityTrait, QueryFilter};

/// Workspace that claims the demo schema namespaces. Matches the id
/// `oxy dev seed` uses: Uuid::new_v5(NAMESPACE_DNS, "demo.oxy.local").
const DEMO_WORKSPACE_ID: uuid::Uuid = uuid::uuid!("70787bb2-e11b-5488-b2c3-02e60d5fc7d3");

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let email = arg_value(&args, "--email").unwrap_or_else(|| "luong@oxy.tech".to_string());
    let reset = args.iter().any(|a| a == "--reset");

    let control_url = std::env::var("OXY_DATABASE_URL")
        .map_err(|_| "OXY_DATABASE_URL must point at Oxy's control-plane Postgres")?;
    // Tenant databases can live on a different cluster than the control plane —
    // and should, so a demo can't scribble on Oxy's own database.
    let admin_url = std::env::var("OXY_OLTP_ADMIN_URL").unwrap_or_else(|_| control_url.clone());

    let db = Database::connect(ConnectOptions::new(control_url)).await?;

    // 1. Find the user, then the org they belong to.
    let user = entity::prelude::Users::find()
        .filter(entity::users::Column::Email.eq(email.clone()))
        .one(&db)
        .await?
        .ok_or_else(|| {
            format!("no user with email {email} — sign in once, or run `oxy dev seed`")
        })?;

    let membership = entity::prelude::OrgMembers::find()
        .filter(entity::org_members::Column::UserId.eq(user.id))
        .one(&db)
        .await?
        .ok_or_else(|| format!("{email} is not a member of any org"))?;

    let org = entity::prelude::Organizations::find_by_id(membership.org_id)
        .one(&db)
        .await?
        .ok_or("org row vanished between queries")?;

    println!("→ user  {email} ({})", user.id);
    println!(
        "→ org   {} ({}) role={:?}",
        org.name, org.id, membership.role
    );

    let provider = Arc::new(LocalProvider::new(admin_url.clone(), host_of(&admin_url)));
    let provisioner = OltpProvisioner::new(
        db.clone(),
        provider.clone(),
        Arc::new(PgSqlExecutor),
        "local",
        17,
    );

    if reset {
        println!("→ deprovisioning existing tenant");
        provisioner.deprovision(org.id).await?;
    }

    // 2. Provision. Idempotent, and applies every platform step from 0.
    let tenant = provisioner.provision(org.id).await?;
    println!(
        "→ database {} on {} (platform v{}/{})",
        tenant.database_name, tenant.host, tenant.platform_schema_version, PLATFORM_SCHEMA_VERSION
    );

    // 3. Writers, mirroring the seed_local demo.
    for (writer, expose) in [
        (WriterRef::pipeline("toast")?, true),
        (WriterRef::app("bookings")?, true),
    ] {
        let creds = provisioner
            .ensure_writer(
                org.id,
                &writer,
                GrantLevel::ReadWrite,
                // The seeded demo workspace claims both namespaces.
                Some(DEMO_WORKSPACE_ID),
            )
            .await?;
        println!(
            "→ writer {writer} → schema {} role {}",
            creds.schema_name, creds.role_name
        );

        // Both exposed here so the IDE demo has something to query. In
        // production `app_*` stays hidden unless the app opts in.
        provisioner
            .set_analytics_visibility(org.id, &writer, expose)
            .await?;
    }

    // Tables are NOT created here. `schemas/*.sql` owns them, applied by
    // `oxyc oltp apply` — a table created by the writer cannot later be altered
    // by a migration, because migrations run as the database owner.

    // 4. The analyst login `postgres_managed` resolves to.
    provisioner.ensure_analyst(org.id).await?;
    println!("→ analyst credential minted and sealed");

    // Machine-readable, so a script can pick it up.
    println!("OLTP_DATABASE={}", tenant.database_name);
    println!("OLTP_ORG_ID={}", org.id);

    println!("\n✓ done. Add to your workspace config.yml:\n");
    println!("    databases:");
    println!("      - name: oltp");
    println!("        type: postgres_managed\n");
    println!("  Then query it in the IDE — it resolves as oxy_analyst_ro (read-only by design):\n");
    println!("    SELECT location, sum(net_sales) FROM raw_toast.sales GROUP BY 1;");
    println!("    SELECT count(*) FROM app_bookings.orders WHERE status = 'open';\n");

    Ok(())
}

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn host_of(dsn: &str) -> String {
    dsn.split("://")
        .nth(1)
        .and_then(|rest| rest.split('@').nth(1).or(Some(rest)))
        .map(|h| h.split('/').next().unwrap_or(h))
        .map(|s| s.split('?').next().unwrap_or(s).to_string())
        .unwrap_or_else(|| "localhost:5432".to_string())
}
