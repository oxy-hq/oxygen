//! `GET /api/customer-apps/{id}/environments/{name}/diff` — what an
//! environment's planes differ from production by, one entry per plane that
//! has a difference to show (`oxyc env diff`).
//!
//! Today that is the OLTP plane of a sandbox on its own branch (`branch` or
//! `empty`): the app's writer schema on the branch against production's, both
//! read from `pg_catalog` (types through `format_type`) in a `READ ONLY`
//! transaction. An environment
//! with no such branch answers `planes: {}`. The Airhouse plane adds its entry
//! beside `oltp` when it grows a branch.
//!
//! **Refused the way `ctx.oltp` is.** A branch that is not cut, being cut or
//! migrated, failed, or cut in the other mode than the planes now ask for is
//! `409 oltp_branch_unready` with the fix `ctx.oltp` would name
//! ([`branch_for`]); a diff of a half-migrated branch would be a wrong answer.

use entity::{app_environments, apps};
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_custom_apps::planes::OltpMode;
use oxy_oltp::WriterRef;
use oxy_oltp::resolver::{WriterConnection, resolve_writer_connection_for_org};
use sea_orm::{DatabaseConnection, EntityTrait};
use serde::Serialize;

use super::SandboxError;
use crate::custom_apps_env_branch::diff::{CatalogColumn, OltpDiff, diff, read_catalog};
use crate::custom_apps_env_branch::home::branch_for;
use crate::custom_apps_planes::PlaneSpec;

/// The response: the environment, and its differences by plane key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EnvDiffDto {
    pub environment: String,
    pub planes: Planes,
}

/// The planes that have a difference to show. A plane with none is absent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Planes {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oltp: Option<OltpDiff>,
}

/// The diff of `environment`'s planes against production.
pub async fn environment_diff(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &AppEnvironment,
) -> Result<EnvDiffDto, SandboxError> {
    let mut planes = Planes::default();
    if let Some(mode) = own_branch_mode(db, app, environment).await? {
        planes.oltp = Some(oltp_diff(db, app, environment, mode).await?);
    }
    Ok(EnvDiffDto {
        environment: environment.name(),
        planes,
    })
}

/// The mode of a sandbox whose planes ask for an OLTP branch of its own; `None`
/// for staging, production and a sandbox on `schema`, which have no branch.
/// `NotFound` for a sandbox with no row.
async fn own_branch_mode(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &AppEnvironment,
) -> Result<Option<OltpMode>, SandboxError> {
    if !matches!(environment, AppEnvironment::Dev { .. }) {
        return Ok(None);
    }
    let row = app_environments::Entity::find_by_id((app.id, environment.name()))
        .one(db)
        .await
        .map_err(|e| SandboxError::db("read the environment", e))?;
    let row = row.ok_or_else(|| SandboxError::NotFound(environment.name()))?;
    // A stored value that does not parse is a defect to see, not "no planes".
    let spec = PlaneSpec::parse(&row.planes)
        .map_err(|e| SandboxError::db("read the environment's planes", e))?;
    Ok(spec
        .resolve(environment)
        .map(|planes| planes.oltp)
        .filter(|m| *m != OltpMode::Schema))
}

/// The app's writer schema on the sandbox's branch against production's.
async fn oltp_diff(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &AppEnvironment,
    mode: OltpMode,
) -> Result<OltpDiff, SandboxError> {
    // The branch is resolved once, here: what is compared is the connection
    // the readiness decision was made on, and a refusal anywhere is the same
    // 409 `ctx.oltp` would give.
    let branch = branch_for(db, app, environment, mode)
        .await
        .map_err(|e| SandboxError::db("decide the environment's OLTP home", e))?
        .map_err(SandboxError::OltpBranchUnready)?;
    let writer = oxy_oltp::schema::app_writer_name(&app.slug)
        .and_then(|name| WriterRef::app(name).ok())
        .ok_or_else(|| SandboxError::db("name the app's writer", &app.slug))?;
    let production = resolve_writer_connection_for_org(db, app.org_id, &writer)
        .await
        .map_err(|e| SandboxError::db("resolve production's writer", e))?;
    let before = catalog(&production, "env diff (production)").await?;
    let after = catalog(&branch.connection, "env diff (branch)").await?;
    Ok(OltpDiff {
        schema: production.schema.clone(),
        ..diff(&before, &after)
    })
}

async fn catalog(
    connection: &WriterConnection,
    label: &'static str,
) -> Result<Vec<CatalogColumn>, SandboxError> {
    let mut client = oxy_oltp::connect::connect(&connection.dsn, label)
        .await
        .map_err(|e| {
            SandboxError::db(
                "connect to the OLTP database",
                oxy_oltp::connect::pg_detail(&e),
            )
        })?;
    read_catalog(&mut client, &connection.schema)
        .await
        .map_err(|e| SandboxError::db("read the OLTP catalog", oxy_oltp::connect::pg_detail(&e)))
}
