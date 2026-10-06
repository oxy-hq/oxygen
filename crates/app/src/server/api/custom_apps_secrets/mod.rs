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

pub(crate) mod declared;
pub mod environment;
mod ops;
pub(crate) mod scope;
pub(crate) mod shared_env;

use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::{Extension, Json};
use entity::{app_builds, app_functions, apps};
use oxy::database::client::establish_connection;
use oxy::service::secret_manager::SecretManagerService;
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_auth::types::AppPublishTokenAuth;
use oxy_server_authz::role_guards::WorkspaceAdmin;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
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
    pub value: String,
    /// `production` (the default), `staging`, or a sandbox's `dev-<handle>`
    /// — anything but production is staff only ([`environment::authorize`]).
    #[serde(default)]
    pub environment: Option<String>,
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

/// The build whose declarations are in force in `environment`: for
/// production, production's, else staging's for an app never promoted; for
/// staging, staging's. Matches how `/fn` and the storage sweeper pick a
/// manifest — the declarations have to come from the same build as the code
/// that reads them — so it asks the same resolver
/// (`custom_apps_env_resolve::resolve_function_environment`).
async fn declaring_build(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &AppEnvironment,
) -> Result<Option<Uuid>, String> {
    crate::server::api::custom_apps_env_resolve::resolve_function_environment(db, app, environment)
        .await
        .map(|resolved| resolved.build_id)
        .map_err(|e| e.to_string())
}

/// What the active build declares, plus why the answer might be incomplete.
///
/// A struct rather than a tuple because the third field is the interesting one:
/// every path that yields *no* declarations has to say whether that means "this
/// app declares nothing" or "we could not find out".
struct Declarations {
    keys: std::collections::BTreeMap<String, declared::DeclaredKey>,
    /// Non-`None` when the list is not the whole truth — a malformed `env`
    /// block, or a query that failed.
    error: Option<String>,
    build_id: Option<String>,
    /// The declaring build's manifest, for the `shared` overlay.
    manifest: Option<serde_json::Value>,
}

impl Declarations {
    /// The app has no build at all. Genuinely declares nothing — not an error.
    fn none() -> Self {
        Self {
            keys: Default::default(),
            error: None,
            build_id: None,
            manifest: None,
        }
    }

    /// We could not read the declarations. Distinct from `none()` on purpose:
    /// silently returning zero keys is the exact failure this module argues
    /// against for a malformed manifest — every stored key would flip to
    /// **Undeclared**, `missing_required` would fall to 0, and the header badge
    /// would vanish, all reading as a healthy app.
    fn unreadable(reason: &str) -> Self {
        Self {
            keys: Default::default(),
            error: Some(format!(
                "Could not read this app's declared keys ({reason}), so only what is \
                 already stored is listed below."
            )),
            build_id: None,
            manifest: None,
        }
    }
}

async fn build_declarations(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &AppEnvironment,
) -> Declarations {
    let build_pk = match declaring_build(db, app, environment).await {
        Ok(Some(build_pk)) => build_pk,
        Ok(None) => return Declarations::none(),
        Err(e) => {
            tracing::error!(app_id = %app.id, "environment lookup failed: {e}");
            return Declarations::unreadable("its environments could not be read");
        }
    };

    let build = match app_builds::Entity::find_by_id(build_pk).one(db).await {
        // A build row that has gone away declares nothing, which is true rather
        // than unknown — only the query FAILING is unknown.
        Ok(row) => row,
        Err(e) => {
            tracing::error!(app_id = %app.id, %build_pk, "build lookup failed: {e}");
            return Declarations::unreadable("its build could not be loaded");
        }
    };

    let (env, manifest_error) = declared::declared_env(
        build.as_ref().and_then(|b| b.manifest_json.as_ref()),
        app.id,
    );

    // Every function in the same build, for their `webhook.secretVar`s.
    let fn_manifests: Vec<serde_json::Value> = match app_functions::Entity::find()
        .filter(app_functions::Column::BuildId.eq(build_pk))
        .all(db)
        .await
    {
        Ok(rows) => rows.into_iter().filter_map(|r| r.manifest_json).collect(),
        Err(e) => {
            tracing::error!(app_id = %app.id, "function lookup failed: {e}");
            return Declarations::unreadable("its functions could not be loaded");
        }
    };

    Declarations {
        keys: declared::merge_declared(env, declared::webhook_secret_vars(fn_manifests.iter())),
        error: manifest_error,
        manifest: build.as_ref().and_then(|b| b.manifest_json.clone()),
        build_id: build.map(|b| b.build_id),
    }
}

/// Everything stored directly under this app's prefix in `environment`, keys
/// bared and attributed. Directly: production's view never lists staging's
/// `staging/KEY`, the rule `ctx.env` follows too.
async fn stored_secrets(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &AppEnvironment,
) -> Result<Vec<StoredSecret>, Failure> {
    let manager = SecretManagerService::new(app.project_id);
    let prefix = prefix_for(app.id, environment);

    let rows = manager.list_secrets(db).await.map_err(|e| {
        tracing::error!(app_id = %app.id, "listing secrets failed: {e}");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not list secrets".to_string(),
        )
    })?;

    let scoped: Vec<_> = rows
        .into_iter()
        .filter_map(|s| {
            let key = s.name.strip_prefix(&prefix)?.to_string();
            (!key.is_empty() && !key.contains('/')).then_some((key, s))
        })
        .collect();

    // One batch lookup for the "last changed by" column, same as `list_secrets`.
    let actor_ids: Vec<Uuid> = scoped
        .iter()
        .filter_map(|(_, s)| s.updated_by.or(Some(s.created_by)))
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    let emails = crate::server::api::secrets::resolve_user_emails(db, &actor_ids).await;

    Ok(scoped
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
        .collect())
}

async fn view(
    db: &DatabaseConnection,
    app: apps::Model,
    environment: &AppEnvironment,
) -> Result<AppSecretsResponse, Failure> {
    let stored = stored_secrets(db, &app, environment).await?;
    let declarations = build_declarations(db, &app, environment).await;
    let mut entries = declared::reconcile(declarations.keys, stored);
    environment::mark_inherited(
        db,
        &app,
        environment,
        declarations.manifest.as_ref(),
        &mut entries,
    )
    .await;
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
    Query(q): Query<EnvironmentQuery>,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    marker: Option<Extension<AppPublishTokenAuth>>,
) -> Result<Json<AppSecretsResponse>, Failure> {
    let db = connect().await?;
    let app = load_app(&db, app_id).await?;
    let caller = Caller::of(&user, &marker);
    let environment = environment::resolve(&db, &app, caller, q.environment.as_deref()).await?;
    Ok(Json(view(&db, app, &environment).await?))
}

pub async fn admin_set(
    Path(app_id): Path<Uuid>,
    actor: oxy_app_core::audit::RequestActor,
    marker: Option<Extension<AppPublishTokenAuth>>,
    Json(body): Json<SetSecretRequest>,
) -> Result<StatusCode, Failure> {
    let db = connect().await?;
    let app = load_app(&db, app_id).await?;
    let caller = Caller::of(&actor.user, &marker);
    let environment = environment::resolve(&db, &app, caller, body.environment.as_deref()).await?;
    ops::set(&db, &app, &environment, body, &actor).await
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
/// it, `DELETE` removes it. Only creation was impossible, because
/// `validate_secret_name` rejects the `/` in `apps/<app_id>/<KEY>` — so that is
/// the one thing added, rather than a parallel CRUD surface whose other three
/// verbs would duplicate working routes and have to be kept in step with them.
pub async fn workspace_set(
    _: WorkspaceAdmin,
    Path((workspace_id, app_id)): Path<(Uuid, Uuid)>,
    actor: oxy_app_core::audit::RequestActor,
    marker: Option<Extension<AppPublishTokenAuth>>,
    Json(body): Json<SetSecretRequest>,
) -> Result<StatusCode, Failure> {
    let db = connect().await?;
    let app = scoped_to_workspace(load_app(&db, app_id).await?, workspace_id)?;
    // Staging here too is staff-only: a workspace admin is refused.
    let caller = Caller::of(&actor.user, &marker);
    let environment = environment::resolve(&db, &app, caller, body.environment.as_deref()).await?;
    ops::set(&db, &app, &environment, body, &actor).await
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
            draft_build_id: None,
            published_build_id: None,
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
