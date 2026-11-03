//! Give each seeded workspace its Airhouse tenant, as creating a workspace
//! does, so the demo's data has the place it has in a deployment.
//!
//! Uses the Airhouse `AIRHOUSE_*` names or, with none set, the one `oxy start`
//! runs, found where [`airhouse::config::autodetect_local_airhouse`] looks.
//! With neither it says so and the rest of the seed goes on, and a failure
//! warns rather than failing the seed, like the LLM keys. Only on a database
//! that is local by its URL: a tenant is a real database in that Airhouse, and
//! `OXY_SEED_ALLOW_REMOTE` consents to demo rows, not to that.

use airhouse::provisioner::ProvisionerError;
use airhouse::{EnsureTenantError, EnsuredTenant, ensure_tenant};
use oxy::theme::StyledText;
use sea_orm::DatabaseConnection;

use super::seed_compile::SeededWorkspace;

/// Look for `oxy start`'s Airhouse before anything reads the Airhouse config,
/// which is read once per process.
pub(crate) async fn find_local_airhouse() {
    airhouse::config::autodetect_local_airhouse().await;
}

pub(crate) async fn provision_tenants(conn: &DatabaseConnection, targets: &[SeededWorkspace]) {
    if super::seed_partners::remote_seeding_allowed()
        || !super::seed_partners::database_url_looks_local()
    {
        println!(
            "{} skipping Airhouse tenants — not an unambiguously local database",
            "⏭️".info()
        );
        return;
    }
    if airhouse::AirhouseConfig::cached().as_runtime().is_none() {
        println!(
            "{} skipping Airhouse tenants — no Airhouse (`oxy start` runs one)",
            "⏭️".info()
        );
        return;
    }
    let (mut provisioned, mut ready) = (0, 0);
    for target in targets {
        match ensure_tenant(conn, target.id).await {
            Ok(EnsuredTenant::Provisioned) => provisioned += 1,
            Ok(EnsuredTenant::AlreadyActive) => ready += 1,
            // The tenant exists in Airhouse but not in this database: a
            // `--clean` that wiped oxy-postgres but not an Airhouse it adopted.
            Err(EnsureTenantError::Provision(ProvisionerError::TenantNameTaken(name))) => {
                println!(
                    "{} {}: Airhouse already has tenant {name} from before this database \
                     was reset; reset that Airhouse with `{}` and seed again",
                    "⚠️".warning(),
                    target.label,
                    oxy::database::docker::airhouse::RESET_EXTERNAL_HINT
                )
            }
            Err(e) => println!(
                "{} {}: no Airhouse tenant: {e}",
                "⚠️".warning(),
                target.label
            ),
        }
    }
    println!(
        "{} Airhouse tenants: {provisioned} provisioned, {ready} already there",
        "✅".success()
    );
}
