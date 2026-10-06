//! The sandbox management routes, `/api/customer-apps/{id}/environments`
//! (`internal-docs/custom-app-sandboxes.md` §5.1), mounted in
//! `server::router::global` beside the app's secrets.
//!
//! The staff console's layers have already run (owner-or-app-admin,
//! `Action::PlatformApps`, the grant's app scope, no active assume session).
//! Every handler adds the same three checks, in this order ([`admit`]): a
//! publish token is refused, the app must exist, and the caller must be
//! allowed to open its non-production environments (`Action::AppNonProduction`
//! — the rule that opens staging).

use axum::extract::Path;
use axum::http::StatusCode;
use axum::{Extension, Json};
use entity::apps;
use oxy::database::client::establish_connection;
use oxy_app_core::audit::RequestActor;
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_auth::types::{AppPublishTokenAuth, AuthenticatedUser};
use sea_orm::{DatabaseConnection, EntityTrait};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{EnvironmentDto, SandboxError, TeardownReason, ops};
use crate::server::api::custom_apps_env_resolve::may_open_non_production;

#[derive(Debug, Deserialize)]
pub struct CreateEnvironmentRequest {
    /// The full name, `dev-<handle>`.
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct EnvironmentList {
    pub environments: Vec<EnvironmentDto>,
}

#[derive(Debug, Serialize)]
pub struct DeleteAccepted {
    pub name: String,
    /// Always `deleting`: the row goes when the teardown finishes.
    pub status: &'static str,
    pub teardown_run_id: String,
}

/// A request the three checks let through: the app, and its org's slug (an
/// environment's URL is built from it).
struct Admitted {
    db: DatabaseConnection,
    app: apps::Model,
    org_slug: String,
}

async fn admit(
    user: &AuthenticatedUser,
    marker: Option<Extension<AppPublishTokenAuth>>,
    app_id: Uuid,
) -> Result<Admitted, SandboxError> {
    if marker.is_some() {
        return Err(SandboxError::PublishToken);
    }
    let db = establish_connection()
        .await
        .map_err(|e| SandboxError::db("connect", e))?;
    let app = apps::Entity::find_by_id(app_id)
        .one(&db)
        .await
        .map_err(|e| SandboxError::db("load the app", e))?
        .ok_or(SandboxError::AppNotFound)?;
    // Credential-aware: a token that carries no staff standing is no staff.
    let caller = crate::server::authz::Caller::from_user(user);
    if !may_open_non_production(&db, &caller, &app).await {
        return Err(SandboxError::NotStaff);
    }
    let org_slug = entity::organizations::Entity::find_by_id(app.org_id)
        .one(&db)
        .await
        .map_err(|e| SandboxError::db("load the app's org", e))?
        .map(|org| org.slug)
        .unwrap_or_default();
    Ok(Admitted { db, app, org_slug })
}

/// The environment `name` names; anything `AppEnvironment::parse` rejects is
/// `InvalidName`.
fn parse(name: &str) -> Result<AppEnvironment, SandboxError> {
    AppEnvironment::parse(name).ok_or_else(|| SandboxError::InvalidName(name.to_string()))
}

/// `GET /api/customer-apps/{id}/environments`
pub async fn list(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    marker: Option<Extension<AppPublishTokenAuth>>,
    Path(id): Path<Uuid>,
) -> Result<Json<EnvironmentList>, SandboxError> {
    let Admitted { db, app, org_slug } = admit(&user, marker, id).await?;
    let environments = ops::list(&db, &app, &org_slug).await?;
    Ok(Json(EnvironmentList { environments }))
}

/// `POST /api/customer-apps/{id}/environments` — `201` with the new sandbox.
pub async fn create(
    actor: RequestActor,
    marker: Option<Extension<AppPublishTokenAuth>>,
    Path(id): Path<Uuid>,
    Json(body): Json<CreateEnvironmentRequest>,
) -> Result<(StatusCode, Json<EnvironmentDto>), SandboxError> {
    let Admitted { db, app, org_slug } = admit(&actor.user, marker, id).await?;
    let environment = parse(&body.name)?;
    ops::create(&db, &app, &environment, &actor).await?;
    let created = ops::get(&db, &app, &org_slug, &environment).await?;
    Ok((StatusCode::CREATED, Json(created)))
}

/// `GET /api/customer-apps/{id}/environments/{name}`
pub async fn show(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    marker: Option<Extension<AppPublishTokenAuth>>,
    Path((id, name)): Path<(Uuid, String)>,
) -> Result<Json<EnvironmentDto>, SandboxError> {
    let Admitted { db, app, org_slug } = admit(&user, marker, id).await?;
    let environment = parse(&name)?;
    Ok(Json(ops::get(&db, &app, &org_slug, &environment).await?))
}

/// `DELETE /api/customer-apps/{id}/environments/{name}` — `202`; also when
/// the sandbox is already being deleted: it answers the teardown still on its
/// way, and queues a new one only once that run has ended (`ops::delete`).
pub async fn delete(
    actor: RequestActor,
    marker: Option<Extension<AppPublishTokenAuth>>,
    Path((id, name)): Path<(Uuid, String)>,
) -> Result<(StatusCode, Json<DeleteAccepted>), SandboxError> {
    let Admitted { db, app, .. } = admit(&actor.user, marker, id).await?;
    let environment = parse(&name)?;
    let teardown_run_id = ops::begin_delete(
        &db,
        &app,
        &environment,
        Some(&actor),
        TeardownReason::Deleted,
    )
    .await?;
    let accepted = DeleteAccepted {
        name: environment.name(),
        status: "deleting",
        teardown_run_id,
    };
    Ok((StatusCode::ACCEPTED, Json(accepted)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_an_environment_name_or_invalid() {
        assert_eq!(parse("staging"), Ok(AppEnvironment::Staging));
        assert_eq!(
            parse("dev-a1"),
            Ok(AppEnvironment::Dev {
                handle: "a1".into()
            })
        );
        for bad in ["a1", "dev-", "dev--x", "DEV-a1", "dev-abcdefghijklm", ""] {
            assert_eq!(parse(bad), Err(SandboxError::InvalidName(bad.to_string())));
        }
    }
}
