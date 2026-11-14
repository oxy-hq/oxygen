//! App-scoped secret management — the write path for `apps/<app_id>/<KEY>`, and
//! the reconciled view that says which keys an app is still missing.
//!
//! # Why this module exists
//!
//! `ctx.env` has always read secrets named `apps/<app_id>/<KEY>` out of the
//! project secret manager, but nothing could **create** one. The public secrets
//! API and the settings UI both reject `/` in a name (`validate_secret_name`),
//! so the only writer was `ctx.secrets.set` from inside a function — which
//! cannot bootstrap the secret the function needs in order to run. A `webhook:`
//! block was the sharpest form of it: 401 on every delivery, and no way to set
//! the signing key it named. (The docs told people to use Settings → Secrets;
//! that never worked.)
//!
//! Everything *else* about an app-scoped secret already worked through the
//! existing `/api/workspaces/{id}/secrets` routes, because those address by row
//! id rather than by name — list, reveal, rotate and delete all resolve fine.
//! So this module is deliberately narrow: it owns creation, and it owns the
//! app-shaped *view* (bare keys, grouped under one app, reconciled against what
//! the build declares) that made the raw `apps/<uuid>/KEY` rows unreadable.
//!
//! # Mounts
//!
//! One handler set, two mounts, each keeping the guard already on its surface:
//!
//! | Mount | Guard |
//! | --- | --- |
//! | `/api/customer-apps/{id}/secrets` | owner **or** app-admin, beside `/{id}/publish` |
//! | `/api/workspaces/{ws}/custom-apps/{app_id}/secrets` (POST only) | `WorkspaceAdmin` |
//!
//! No `/admin/apps/{id}/secrets` twin: `/customer-apps` is already reachable by
//! owners as well as app-admins (`oxy_owner_or_app_admin_guard`), and it is the
//! surface the staff console actually calls for app work — publish, builds,
//! functions, storage. A second mount would add a route, not reach.
//!
//! The staff mount reaches any app by id, exactly as the rest of that surface
//! does. **The workspace mount must not** — see [`scoped_to_workspace`].
//!
//! All of it is Postgres-only (no workspace FS, no git), so every route is
//! `FleetOk`.
//!
//! # Environments
//!
//! Every route takes an optional environment — `environment` in the POST body,
//! `?environment=` on the others — and acts on that environment's path
//! (`scope`): production's `apps/<app_id>/<KEY>` by default; staging's
//! `apps/<app_id>/staging/<KEY>` or a sandbox's `apps/<app_id>/dev-<handle>/<KEY>`
//! for staff only ([`environment::authorize`], oxy-authz `AppNonProduction`),
//! and a sandbox must exist (404 otherwise). A view lists that environment's
//! keys alone, reconciled against the build that environment serves. Outside
//! production an unset key says what a run would read instead: in a sandbox,
//! staging's value when staging holds one (`inherits_staging`); otherwise
//! production's for a `shared` key (`inherits_production`). Neither is missing.
//! A sandbox's teardown removes its whole path ([`delete_environment_secrets`]).
//!
//! # Workspace secrets shared with an app
//!
//! A workspace admin can share one workspace secret with any number of the
//! workspace's apps ([`workspace_shared`], `oxy_custom_apps::secret_grants`):
//! one stored value, read by each as `ctx.env.<name>`, rotated once. A
//! production view marks a key the app stores nothing for but reads that way
//! (`shared_from_workspace`), so it is not counted missing.

mod agent;
mod copy;
mod declarations;
pub(crate) mod declared;
pub mod environment;
mod ops;
pub(crate) use oxy_custom_apps::secret_audit as project_routes;
pub(crate) mod required;
pub(crate) use oxy_custom_apps::secret_scope as scope;
pub(crate) mod shared_env;
pub mod workspace_shared;

use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::{Extension, Json};
use entity::apps;
use oxy::database::client::establish_connection;
use oxy::service::secret_manager::{SecretInfo, SecretManagerService};
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_auth::types::AppPublishTokenAuth;
use oxy_server_authz::role_guards::WorkspaceAdmin;
use sea_orm::{DatabaseConnection, EntityTrait};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use declared::{AppSecretEntry, StoredSecret};
use environment::{Caller, EnvironmentQuery};
pub(crate) use ops::delete_environment_secrets;
#[doc(hidden)]
pub use ops::delete_named_secrets;

/// Storage-name prefix for one app's secrets in `environment`. The single
/// definition (`scope`) — every read strips it and every write is built from
/// it, so the two can't drift.
fn prefix_for(app_id: Uuid, environment: &AppEnvironment) -> String {
    scope::secret_prefix(app_id, environment)
}

// ── DTOs ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct AppSecretsResponse {
    pub app_id: Uuid,
    pub app_slug: String,
    pub app_name: String,
    /// The environment whose secrets these are (`production`, `staging`, or
    /// a sandbox's `dev-<handle>`).
    pub environment: String,
    /// Declared ∪ stored, action-first. See [`declared::reconcile`].
    pub entries: Vec<AppSecretEntry>,
    /// Count of `required` keys with nothing stored — the badge the UI shows on
    /// the tab, and the one number that says "this app is not ready".
    pub missing_required: usize,
    /// The build whose declarations were read (published, else draft). `None`
    /// when the app has never been built, in which case nothing is declared and
    /// every stored key reads as undeclared — which is correct, not a bug.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub declaring_build_id: Option<String>,
    /// Set when the manifest's `env` block exists but could not be parsed.
    /// Surfaced rather than swallowed, so a typo doesn't look like an app that
    /// declares nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub declaration_error: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SetSecretRequest {
    /// Bare key — `STRIPE_API_KEY`, not the prefixed storage name. The prefix is
    /// system-built by `set_app_secret`, which is what keeps an app's write
    /// inside its own namespace.
    pub key: String,
    /// Empty when `copy_from` names the value instead.
    #[serde(default)]
    pub value: String,
    /// `production` (the default), `staging`, or a sandbox's `dev-<handle>`
    /// — anything but production is staff only ([`environment::authorize`]).
    #[serde(default)]
    pub environment: Option<String>,
    /// Copy this environment's stored value of `key` instead of sending one —
    /// server-side, so it never reaches the caller ([`copy`]). Only downhill:
    /// production into staging or a sandbox, staging into a sandbox.
    #[serde(default)]
    pub copy_from: Option<String>,
}

/// `?environment=` and `?build=` on the list route.
#[derive(Debug, Default, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub environment: Option<String>,
    /// An `app_builds.id` of this app: reconcile against that build's
    /// declarations instead of the build the environment serves — "what would
    /// this environment be missing if this build went live here", asked before
    /// a promote, publish or rollback. Another app's build is a 404.
    #[serde(default)]
    pub build: Option<Uuid>,
}

#[derive(Debug, Serialize)]
pub struct RevealResponse {
    pub key: String,
    pub value: String,
}

// ── App resolution + scoping ─────────────────────────────────────────────────

type Failure = (StatusCode, String);

async fn connect() -> Result<DatabaseConnection, Failure> {
    establish_connection().await.map_err(|e| {
        tracing::error!("custom_apps_secrets DB connect failed: {e}");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "database unavailable".to_string(),
        )
    })
}

async fn load_app(db: &DatabaseConnection, app_id: Uuid) -> Result<apps::Model, Failure> {
    apps::Entity::find_by_id(app_id)
        .one(db)
        .await
        .map_err(|e| {
            tracing::error!("app lookup failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "app lookup failed".to_string(),
            )
        })?
        .ok_or((StatusCode::NOT_FOUND, "app not found".to_string()))
}

/// Confine a tenant-surface request to its own workspace.
///
/// **This is the one place a cross-tenant read could open.** An app's secrets
/// live in the project secret store keyed by `apps.project_id`; a workspace
/// admin passing another org's `app_id` on their own workspace's route would
/// otherwise read (and rotate) that app's secrets, because `WorkspaceAdmin`
/// only proves standing in the workspace named in the path — never that the app
/// belongs to it.
///
/// Answers **404, not 403**: on a scoped surface, "not yours" and "does not
/// exist" must be indistinguishable, or the endpoint becomes a probe for which
/// app ids exist. Same reasoning as the App Operator scope boundary.
fn scoped_to_workspace(app: apps::Model, workspace_id: Uuid) -> Result<apps::Model, Failure> {
    if app.project_id != workspace_id {
        tracing::warn!(
            app_id = %app.id,
            requested_workspace = %workspace_id,
            owning_workspace = %app.project_id,
            "app-secrets request for an app outside the path workspace — answering 404"
        );
        return Err((StatusCode::NOT_FOUND, "app not found".to_string()));
    }
    Ok(app)
}

// ── Core operations ──────────────────────────────────────────────────────────

/// Every active secret row of a workspace, listed once per request and handed to
/// everything that filters it (an app's own keys, the secrets shared with it).
async fn workspace_rows(
    db: &DatabaseConnection,
    project_id: Uuid,
) -> Result<Vec<SecretInfo>, Failure> {
    SecretManagerService::new(project_id)
        .list_secrets(db)
        .await
        .map_err(|e| {
            tracing::error!(%project_id, "listing secrets failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not list secrets".to_string(),
            )
        })
}

/// Everything stored directly under this app's prefix in `environment`, keys
/// bared and attributed. Directly: production's view never lists staging's
/// `staging/KEY`, the rule `ctx.env` follows too.
async fn stored_secrets(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &AppEnvironment,
) -> Result<Vec<StoredSecret>, Failure> {
    let rows = workspace_rows(db, app.project_id).await?;
    Ok(stored_secrets_in(db, app, environment, &rows).await)
}

/// [`stored_secrets`] over the workspace's rows, already listed.
async fn stored_secrets_in(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &AppEnvironment,
    rows: &[SecretInfo],
) -> Vec<StoredSecret> {
    let prefix = prefix_for(app.id, environment);
    let scoped: Vec<(String, &SecretInfo)> = rows
        .iter()
        .filter_map(|s| {
            let key = s.name.strip_prefix(&prefix)?;
            (!key.is_empty() && !key.contains('/')).then(|| (key.to_string(), s))
        })
        .collect();

    // One batch lookup for the "last changed by" column, same as `list_secrets`.
    let actor_ids: Vec<Uuid> = scoped
        .iter()
        .filter_map(|(_, s)| s.updated_by.or(Some(s.created_by)))
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    let emails = oxy_app::surface::secrets::resolve_user_emails(db, &actor_ids).await;

    scoped
        .into_iter()
        .map(|(key, s)| StoredSecret {
            key,
            secret_id: s.id,
            updated_at: s.updated_at,
            updated_by_email: s
                .updated_by
                .or(Some(s.created_by))
                .and_then(|id| emails.get(&id).cloned()),
        })
        .collect()
}

async fn view(
    db: &DatabaseConnection,
    app: apps::Model,
    environment: &AppEnvironment,
    build: Option<Uuid>,
) -> Result<AppSecretsResponse, Failure> {
    let rows = workspace_rows(db, app.project_id).await?;
    let stored = stored_secrets_in(db, &app, environment, &rows).await;
    let declarations = match build {
        Some(build) => declarations::for_requested_build(db, &app, build).await?,
        None => declarations::for_environment(db, &app, environment).await,
    };
    let mut entries = declared::reconcile(declarations.keys, stored);
    environment::mark_inherited(
        db,
        &app,
        environment,
        declarations.manifest.as_ref(),
        &mut entries,
    )
    .await;
    workspace_shared::mark_shared(db, &app, environment, &rows, &mut entries).await;
    Ok(AppSecretsResponse {
        missing_required: entries.iter().filter(|e| e.is_missing_required()).count(),
        app_id: app.id,
        app_slug: app.slug,
        app_name: app.name,
        environment: environment.name(),
        entries,
        declaring_build_id: declarations.build_id,
        declaration_error: declarations.error,
    })
}

// ── Staff-surface handlers (`/admin/apps/{id}`, `/customer-apps/{id}`) ───────
//
// Every handler takes the publish-token marker and hands it to
// `environment::resolve`, which refuses it outside production. A token's
// scope (`middlewares::app_publish_token_scope`) admits every `GET` under
// `/customer-apps/`, and a token is its minter — so without this a token
// minted by staff listed and revealed staging's, and a sandbox's, secrets.

pub async fn admin_list(
    Path(app_id): Path<Uuid>,
    Query(q): Query<ListQuery>,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    marker: Option<Extension<AppPublishTokenAuth>>,
) -> Result<Json<AppSecretsResponse>, Failure> {
    let db = connect().await?;
    let app = load_app(&db, app_id).await?;
    let caller = Caller::of(&user, &marker);
    let environment = environment::resolve(&db, &app, caller, q.environment.as_deref()).await?;
    Ok(Json(view(&db, app, &environment, q.build).await?))
}

pub async fn admin_set(
    Path(app_id): Path<Uuid>,
    actor: oxy_app_core::audit::RequestActor,
    marker: Option<Extension<AppPublishTokenAuth>>,
    Json(mut body): Json<SetSecretRequest>,
) -> Result<StatusCode, Failure> {
    let db = connect().await?;
    let app = load_app(&db, app_id).await?;
    let caller = Caller::of(&actor.user, &marker);
    let environment = environment::resolve(&db, &app, caller, body.environment.as_deref()).await?;
    let copied = copy::fill_from_source(&db, &app, caller, &environment, &mut body).await?;
    ops::set(&db, &app, &environment, body, copied.as_ref(), &actor).await
}

pub async fn admin_delete(
    Path((app_id, key)): Path<(Uuid, String)>,
    Query(q): Query<EnvironmentQuery>,
    actor: oxy_app_core::audit::RequestActor,
    marker: Option<Extension<AppPublishTokenAuth>>,
) -> Result<StatusCode, Failure> {
    let db = connect().await?;
    let app = load_app(&db, app_id).await?;
    let caller = Caller::of(&actor.user, &marker);
    let environment = environment::resolve(&db, &app, caller, q.environment.as_deref()).await?;
    ops::delete(&db, &app, &environment, &key, &actor).await
}

pub async fn admin_reveal(
    Path((app_id, key)): Path<(Uuid, String)>,
    Query(q): Query<EnvironmentQuery>,
    actor: oxy_app_core::audit::RequestActor,
    marker: Option<Extension<AppPublishTokenAuth>>,
) -> Result<Json<RevealResponse>, Failure> {
    let db = connect().await?;
    let app = load_app(&db, app_id).await?;
    // A sandbox agent token never reads a secret's value, in any environment
    // — its own sandbox included. The route allow-list does not admit this
    // route; this holds if that list is ever loosened.
    if agent::token_of(&actor.user).is_some() {
        return Err((StatusCode::NOT_FOUND, "secret not found".to_string()));
    }
    let caller = Caller::of(&actor.user, &marker);
    let environment = environment::resolve(&db, &app, caller, q.environment.as_deref()).await?;
    ops::reveal(&db, &app, &environment, &key, &actor).await
}

// ── Tenant-surface handler (`/workspaces/{ws}/custom-apps/{app_id}`) ────────

/// `POST /api/workspaces/{ws}/custom-apps/{app_id}/secrets` — the tenant write
/// path, and **the only tenant verb here**.
///
/// Everything else an app secret needs already works on the project-secrets
/// routes next door, because those address a row by **id** rather than by name:
/// `GET /secrets` lists it, `GET /secrets/{id}/value` reveals it, `PUT` rotates
/// it, `DELETE` removes it — the last two audited like this route's writes
/// ([`project_routes`]). Only creation was impossible, because
/// `validate_secret_name` rejects the `/` in `apps/<app_id>/<KEY>` — so that is
/// the one thing added, rather than a parallel CRUD surface whose other three
/// verbs would duplicate working routes and have to be kept in step with them.
pub async fn workspace_set(
    _: WorkspaceAdmin,
    Path((workspace_id, app_id)): Path<(Uuid, Uuid)>,
    actor: oxy_app_core::audit::RequestActor,
    marker: Option<Extension<AppPublishTokenAuth>>,
    Json(mut body): Json<SetSecretRequest>,
) -> Result<StatusCode, Failure> {
    let db = connect().await?;
    let app = scoped_to_workspace(load_app(&db, app_id).await?, workspace_id)?;
    // Staging here too is staff-only: a workspace admin is refused — and with
    // it every copy, since nothing is copied into production.
    let caller = Caller::of(&actor.user, &marker);
    let environment = environment::resolve(&db, &app, caller, body.environment.as_deref()).await?;
    let copied = copy::fill_from_source(&db, &app, caller, &environment, &mut body).await?;
    ops::set(&db, &app, &environment, body, copied.as_ref(), &actor).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_row(project_id: Uuid) -> apps::Model {
        apps::Model {
            id: Uuid::new_v4(),
            slug: "bookkeeping".to_string(),
            name: "Bookkeeping".to_string(),
            org_id: Uuid::new_v4(),
            project_id,
            branch: "main".to_string(),
            source_repo: "oxy-hq/customer-apps".to_string(),
            status: "created".to_string(),
            source_type: "s3".to_string(),
            source_config: serde_json::json!({}),
            last_synced_at: None,
            manifest_override: None,
            bootstrap_pr_url: None,
            published_at: None,
            repo_path: None,
            last_promoted_by: None,
            last_promoted_at: None,
            visibility: "org".to_string(),
            created_at: chrono::Utc::now().fixed_offset(),
            updated_at: chrono::Utc::now().fixed_offset(),
        }
    }

    #[test]
    fn prefix_is_the_namespace_ctx_env_reads() {
        let id = Uuid::nil();
        assert_eq!(
            prefix_for(id, &AppEnvironment::Production),
            format!("apps/{id}/")
        );
        assert_eq!(
            prefix_for(id, &AppEnvironment::Staging),
            format!("apps/{id}/staging/")
        );
    }

    #[test]
    fn an_app_in_this_workspace_passes() {
        let ws = Uuid::new_v4();
        assert!(scoped_to_workspace(app_row(ws), ws).is_ok());
    }

    /// The cross-tenant guard. `WorkspaceAdmin` proves standing in the workspace
    /// named in the PATH — never that the app belongs to it.
    #[test]
    fn an_app_from_another_workspace_is_not_found() {
        let (mine, theirs) = (Uuid::new_v4(), Uuid::new_v4());
        let err = scoped_to_workspace(app_row(theirs), mine).unwrap_err();
        assert_eq!(
            err.0,
            StatusCode::NOT_FOUND,
            "must be indistinguishable from a nonexistent app, or the route \
             becomes a probe for which app ids exist"
        );
    }

    // Which build declares (production's, else staging's for an app never
    // promoted) is `EnvironmentBuilds::resolve_for_functions`, tested beside it
    // in `custom_apps_env_resolve`.
}
