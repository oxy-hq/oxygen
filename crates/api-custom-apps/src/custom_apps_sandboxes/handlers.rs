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

use std::sync::Arc;

use axum::extract::Path;
use axum::http::StatusCode;
use axum::{Extension, Json};
use chrono::Utc;
use entity::apps;
use oxy::database::client::establish_connection;
use oxy_app_core::audit::RequestActor;
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_app_core::semantic_pin_seam::SemanticPinner;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_auth::types::{AppPublishTokenAuth, AuthenticatedUser};
use sea_orm::{DatabaseConnection, EntityTrait};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::branch::{self, NewBranch};
use super::copy_build;
use super::env_diff::{EnvDiffDto, environment_diff};
use super::planes_pin::{self, Pinner};
use super::{EnvironmentDto, SandboxError, TeardownReason, ops};
use crate::custom_apps_env_resolve::{
    may_open_environment, may_open_new_sandbox, may_open_non_production,
};

#[derive(Debug, Default, Deserialize)]
pub struct CreateEnvironmentRequest {
    /// The full name, `dev-<handle>`.
    pub name: String,
    /// The environment to cut the sandbox from: production, staging, or an
    /// active sandbox of this app, among those the caller may open (a sandbox
    /// agent token: only a sandbox it created). Default: the default parent of
    /// the kind.
    pub from: Option<String>,
    /// The fork point on the parent, RFC 3339 and not in the future. Default:
    /// when the sandbox is created.
    pub at: Option<String>,
    /// `light` (every default) or `full` (`PRESET_FULL`), under `planes`.
    pub preset: Option<String>,
    /// Plane key → mode, over the preset.
    pub planes: Option<serde_json::Value>,
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

/// A request the three checks let through: the app, its org's slug (an
/// environment's URL is built from it), and the sandbox agent token the
/// request used, when that is its credential.
pub(crate) struct Admitted {
    pub(crate) db: DatabaseConnection,
    pub(crate) app: apps::Model,
    pub(crate) org_slug: String,
    /// `Some` narrows what is shown to the sandboxes that token created.
    pub(crate) own: Option<Uuid>,
}

/// Which environment a request is about, as far as its route says.
pub(crate) enum Door<'a> {
    /// List and create: no one sandbox yet.
    New,
    /// Show: the name in the path, before it is parsed.
    Named(&'a str),
    /// Delete: the name in the path, which must be a sandbox's.
    Sandbox(&'a str),
}

pub(crate) async fn admit(
    user: &AuthenticatedUser,
    marker: Option<Extension<AppPublishTokenAuth>>,
    app_id: Uuid,
    door: Door<'_>,
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
    let caller = oxy_server_authz::Caller::from_user(user);
    if !opens(&db, &caller, &app, &door).await {
        return Err(refusal(&caller, &door));
    }
    let org_slug = entity::organizations::Entity::find_by_id(app.org_id)
        .one(&db)
        .await
        .map_err(|e| SandboxError::db("load the app's org", e))?
        .map(|org| org.slug)
        .unwrap_or_default();
    let own = caller.sandbox_agent().map(|reach| reach.token_id);
    Ok(Admitted {
        db,
        app,
        org_slug,
        own,
    })
}

/// Whether the caller may open what `door` names. A sandbox agent token is
/// asked about the one environment (`may_open_environment`); a name that does
/// not parse names none, so it is refused. Everyone else is asked about the
/// app, as before, and told `InvalidName` by the handler afterwards.
///
/// `GET …/environments/staging` therefore opens for a token granted the
/// app's staging, and for no other token.
async fn opens(
    db: &DatabaseConnection,
    caller: &oxy_server_authz::Caller,
    app: &apps::Model,
    door: &Door<'_>,
) -> bool {
    let name = match door {
        Door::New => return may_open_new_sandbox(db, caller, app).await,
        Door::Named(name) | Door::Sandbox(name) => name,
    };
    match AppEnvironment::parse(name) {
        // A sandbox agent token deletes a sandbox and nothing else: a token
        // granted the app's staging may open staging, and is still answered
        // here as it always was — staging is not its to delete.
        Some(environment) if deletes_a_fixed_environment(caller, door, &environment) => false,
        Some(environment) => may_open_environment(db, caller, app, &environment).await,
        None => !caller.is_sandbox_agent() && may_open_non_production(db, caller, app).await,
    }
}

/// Whether a sandbox agent token is asking to delete production or staging.
/// `false` for every other caller, who is told `not_a_sandbox` further on.
fn deletes_a_fixed_environment(
    caller: &oxy_server_authz::Caller,
    door: &Door<'_>,
    environment: &AppEnvironment,
) -> bool {
    caller.is_sandbox_agent()
        && matches!(door, Door::Sandbox(_))
        && !matches!(environment, AppEnvironment::Dev { .. })
}

/// A sandbox agent token is answered as if what it asked for did not exist:
/// another creator's sandbox, production and staging are not its to learn of.
fn refusal(caller: &oxy_server_authz::Caller, door: &Door<'_>) -> SandboxError {
    match (caller.is_sandbox_agent(), door) {
        (true, Door::Named(name) | Door::Sandbox(name)) => {
            SandboxError::NotFound((*name).to_string())
        }
        (true, Door::New) => SandboxError::AppNotFound,
        (false, _) => SandboxError::NotStaff,
    }
}

/// The environment `name` names; anything `AppEnvironment::parse` rejects is
/// `InvalidName`.
pub(crate) fn parse(name: &str) -> Result<AppEnvironment, SandboxError> {
    AppEnvironment::parse(name).ok_or_else(|| SandboxError::InvalidName(name.to_string()))
}

/// `GET /api/customer-apps/{id}/environments`
pub async fn list(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    marker: Option<Extension<AppPublishTokenAuth>>,
    Path(id): Path<Uuid>,
) -> Result<Json<EnvironmentList>, SandboxError> {
    let Admitted {
        db,
        app,
        org_slug,
        own,
    } = admit(&user, marker, id, Door::New).await?;
    let environments = ops::list_for(&db, &app, &org_slug, own).await?;
    Ok(Json(EnvironmentList { environments }))
}

/// `POST /api/customer-apps/{id}/environments` — `201` with the new sandbox.
/// Optionally cut from an environment (`from`, `at`) and with planes chosen
/// (`preset`, `planes`); see [`CreateEnvironmentRequest`].
pub async fn create(
    actor: RequestActor,
    marker: Option<Extension<AppPublishTokenAuth>>,
    pinner: Option<Extension<Arc<dyn SemanticPinner>>>,
    Path(id): Path<Uuid>,
    Json(body): Json<CreateEnvironmentRequest>,
) -> Result<(StatusCode, Json<EnvironmentDto>), SandboxError> {
    let Admitted {
        db,
        app,
        org_slug,
        own,
    } = admit(&actor.user, marker, id, Door::New).await?;
    let environment = parse(&body.name)?;
    let pinner = pinner.as_ref().map(|Extension(pinner)| pinner);
    let caller = oxy_server_authz::Caller::from_user(&actor.user);
    let (branch, semantic_branch) =
        requested_branch(&db, &caller, &app, &environment, &body, pinner).await?;
    ops::create_with(&db, &app, &environment, &actor, branch).await?;
    // A new sandbox holds no pin, so only a branch choice calls the port.
    let after = planes_pin::PinAfter::Create;
    planes_pin::settle(
        pinner,
        &app,
        &environment,
        semantic_branch.as_deref(),
        after,
    )
    .await?;
    let created = ops::get_for(&db, &app, &org_slug, &environment, own).await?;
    Ok((StatusCode::CREATED, Json(created)))
}

/// What a create asked for beyond its name, checked: the planes parse and
/// validate (`400 invalid_planes`, a `semantic` branch found in the workspace),
/// and `at` is a past RFC 3339 time (`400 invalid_fork_at`). `from` must be an
/// environment the caller may open (see [`may_fork_from`]); that it exists is
/// checked by the create itself, under the app's lock. Either way the answer
/// is `400 invalid_parent`. Also the `semantic` branch to pin once the sandbox
/// exists.
async fn requested_branch(
    db: &DatabaseConnection,
    caller: &oxy_server_authz::Caller,
    app: &apps::Model,
    environment: &AppEnvironment,
    body: &CreateEnvironmentRequest,
    pinner: Pinner<'_>,
) -> Result<(NewBranch, Option<String>), SandboxError> {
    if let Some(from) = &body.from {
        may_fork_from(db, caller, app, from).await?;
    }
    let fork_at = body
        .at
        .as_deref()
        .map(|at| branch::parse_fork_at(at, Utc::now()))
        .transpose()?;
    let asked = body.preset.is_some() || body.planes.is_some();
    let (planes, semantic_branch) = if asked {
        let ask = (body.preset.as_deref(), body.planes.as_ref());
        let build = match &body.from {
            Some(from) => copy_build::forked_build(db, app, from).await?,
            None => None,
        };
        let at = fork_at.map(|at| at.with_timezone(&Utc));
        let requested =
            planes_pin::requested(db, app, environment, ask, (build, at), pinner).await?;
        let semantic_branch = requested.spec.semantic_branch().map(str::to_string);
        (Some(requested.stored), semantic_branch)
    } else {
        (None, None)
    };
    let new = NewBranch {
        from: body.from.clone(),
        fork_at,
        planes,
    };
    Ok((new, semantic_branch))
}

/// A sandbox starts on its parent's build and copies its configuration, so a
/// caller forks only from an environment it may open. A sandbox agent token's
/// reach is the `dev-*` sandboxes it created: production, staging and a
/// colleague's sandbox are refused, and answered as a parent that does not
/// exist, so a token learns nothing about them. A person is asked the same
/// question as the door (`may_open_non_production`). A name that does not
/// parse is left to the create's own `invalid_parent`.
async fn may_fork_from(
    db: &DatabaseConnection,
    caller: &oxy_server_authz::Caller,
    app: &apps::Model,
    from: &str,
) -> Result<(), SandboxError> {
    match AppEnvironment::parse(from) {
        Some(parent) if !may_open_environment(db, caller, app, &parent).await => {
            Err(SandboxError::InvalidParent(from.to_string()))
        }
        _ => Ok(()),
    }
}

/// `GET /api/customer-apps/{id}/environments/{name}`
pub async fn show(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    marker: Option<Extension<AppPublishTokenAuth>>,
    Path((id, name)): Path<(Uuid, String)>,
) -> Result<Json<EnvironmentDto>, SandboxError> {
    let Admitted {
        db,
        app,
        org_slug,
        own,
    } = admit(&user, marker, id, Door::Named(&name)).await?;
    let environment = parse(&name)?;
    Ok(Json(
        ops::get_for(&db, &app, &org_slug, &environment, own).await?,
    ))
}

/// `GET /api/customer-apps/{id}/environments/{name}/diff` — the OLTP schema
/// difference between the environment's own branch and production, under the
/// same door as [`show`].
pub async fn diff(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    marker: Option<Extension<AppPublishTokenAuth>>,
    Path((id, name)): Path<(Uuid, String)>,
) -> Result<Json<EnvDiffDto>, SandboxError> {
    let Admitted { db, app, .. } = admit(&user, marker, id, Door::Named(&name)).await?;
    let environment = parse(&name)?;
    Ok(Json(environment_diff(&db, &app, &environment).await?))
}

/// `DELETE /api/customer-apps/{id}/environments/{name}` — `202`; also when
/// the sandbox is already being deleted: it answers the teardown still on its
/// way, and queues a new one only once that run has ended (`ops::delete`).
pub async fn delete(
    actor: RequestActor,
    marker: Option<Extension<AppPublishTokenAuth>>,
    Path((id, name)): Path<(Uuid, String)>,
) -> Result<(StatusCode, Json<DeleteAccepted>), SandboxError> {
    let Admitted { db, app, .. } = admit(&actor.user, marker, id, Door::Sandbox(&name)).await?;
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
    use crate::custom_apps_agent_fixture as fixture;

    /// A sandbox agent token deletes a sandbox and nothing else, whatever it
    /// may open: the delete door refuses it production and staging by name.
    /// Showing one is not deleting it, and no other caller is held here.
    #[test]
    fn a_token_is_refused_the_delete_of_a_fixed_environment_by_name() {
        let credential = fixture::credential(
            Uuid::from_u128(0x70),
            Uuid::from_u128(2),
            Uuid::from_u128(1),
            Uuid::from_u128(3),
        );
        let agent = oxy_server_authz::Caller::from_user(&fixture::user(Some(credential)));
        let person = oxy_server_authz::Caller::from_user(&fixture::user(None));
        let sandbox = AppEnvironment::parse("dev-a").expect("a sandbox");
        for fixed in [AppEnvironment::Production, AppEnvironment::Staging] {
            let name = fixed.name();
            assert!(deletes_a_fixed_environment(
                &agent,
                &Door::Sandbox(&name),
                &fixed
            ));
            assert!(!deletes_a_fixed_environment(
                &agent,
                &Door::Named(&name),
                &fixed
            ));
            assert!(!deletes_a_fixed_environment(
                &person,
                &Door::Sandbox(&name),
                &fixed
            ));
        }
        assert!(!deletes_a_fixed_environment(
            &agent,
            &Door::Sandbox("dev-a"),
            &sandbox
        ));
        assert!(!deletes_a_fixed_environment(&agent, &Door::New, &sandbox));
    }

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
