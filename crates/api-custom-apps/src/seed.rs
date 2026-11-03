//! The demo seed's custom-app half, handed to `oxy-app`'s seed command
//! (`SurfaceSeams::example_apps`): the example apps, their storage figures, and
//! the teardown. `oxy-app` seeds the rows they belong to (orgs, workspaces,
//! teams) and calls back here.

use airhouse::LOCAL_ORG_ID;
use oxy::theme::StyledText;
use oxy_app::surface::seams::ExampleAppsSeam;
use oxy_shared::errors::OxyError;
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use crate::seed_apps::{AppTarget, clear_example_apps, seed_example_apps, seeded_app_workspace_of};
use crate::seed_fixture;
use crate::seed_storage::{clear_storage_usage, seed_storage_usage};

/// Deploy the example custom app to the demo workspace, and to Acme's (so the
/// admin cockpit and the partner console both have a real app to show).
///
/// **Warns instead of failing.** The rest of the seed is the part a developer
/// can't work without; the example app is a bonus on top. The one failure that's
/// actually likely — `OXY_ROLE` exported in the shell, which makes the build
/// store refuse a filesystem write — would otherwise turn a stray env var into a
/// completely failed seed.
pub async fn deploy_example_apps(
    conn: &DatabaseConnection,
    workspace_id: Uuid,
    path: &str,
    acme_restricted_team: Option<Uuid>,
) {
    let mut targets = vec![AppTarget::open(
        LOCAL_ORG_ID,
        "local".to_string(),
        workspace_id,
    )];
    // Absent when the partner seed skipped (non-local DB) — deploy to the demo
    // workspace anyway rather than treating that as an error.
    match seeded_app_workspace_of(conn, "acme").await {
        Ok(Some(acme)) => {
            // A SECOND, restricted app beside the open one — never instead of it.
            // Acme's only app being invisible to most of Acme would read as a broken
            // seed; two apps show the contrast the feature is about, with the open
            // one still on everyone's launcher.
            if let Some(team) = acme_restricted_team {
                targets.push(acme.restricted(team));
            }
            targets.push(acme);
        }
        Ok(None) => {}
        Err(e) => println!("{} could not resolve Acme's workspace: {e}", "⚠️".warning()),
    }

    if let Err(e) = seed_example_apps(conn, path, &targets).await {
        println!(
            "{} example app not deployed: {e}\n  \
             Everything else seeded. Re-run `oxy seed` once the cause is fixed.",
            "⚠️".warning()
        );
    }

    // After the apps exist — storage usage hangs off `apps.id` (with an
    // ON DELETE CASCADE foreign key), so seeding it earlier would insert
    // nothing. Warn rather than fail: demo storage numbers are a nicety, and
    // the rest of the seed is what a developer cannot work without.
    if let Err(e) = seed_storage_usage(conn).await {
        println!("{} storage usage not seeded: {e}", "⚠️".warning());
    }
}

/// The seam `oxy seed` and the integration-test fixture pass to `oxy-app`.
pub fn example_apps() -> ExampleAppsSeam {
    ExampleAppsSeam {
        deploy: |conn, workspace_id, path, team| {
            Box::pin(deploy_example_apps(conn, workspace_id, path, team))
        },
        fixtures: |conn, workspace_id, compiled| {
            Box::pin(seed_fixture::seed_demo_fixture(
                conn,
                workspace_id,
                compiled,
            ))
        },
        clear_storage: |conn| Box::pin(clear_storage_usage(conn)),
        clear_apps: |conn| Box::pin(clear_apps(conn)),
    }
}

/// The example apps, then the demo fixture (whose clear retires its schedules
/// before it deletes the app).
async fn clear_apps(conn: &DatabaseConnection) -> Result<u64, OxyError> {
    let examples = clear_example_apps(conn).await?;
    let fixture = seed_fixture::clear_fixture(conn)
        .await
        .map_err(|e| OxyError::RuntimeError(format!("clear the demo fixture: {e}")))?;
    Ok(examples + fixture)
}

/// Drop the demo workspace row and every seeded example app (`oxy seed --clear`).
pub async fn clear_demo() -> Result<(), OxyError> {
    oxy_app::surface::seed::clear_demo(Some(example_apps())).await
}

/// The fixture the integration tests are built on: the demo seed's rows and the
/// example apps, no compile, no LLM keys and no Airhouse tenants. `oxy-app`'s `seed_demo` seeds the
/// rows alone.
pub async fn seed_demo(workspace_path: Option<std::path::PathBuf>) -> Result<(), OxyError> {
    oxy_app::surface::seed::seed_demo_with(
        workspace_path,
        oxy_app::surface::seed::SeedOptions {
            compile: false,
            llm_keys: false,
            fixtures: false,
            airhouse: false,
        },
        Some(example_apps()),
    )
    .await
}
