//! Workspace secrets shared with custom apps — the tenant routes over
//! `app_secret_grants` (`oxy_custom_apps::secret_grants`), and the mark that
//! tells an app's Secrets view a key is shared rather than missing.
//!
//! | Route (under `/api/{workspace_id}`) | |
//! | --- | --- |
//! | `GET /shared-secrets` | which apps each workspace secret is shared with, and what each app still lacks |
//! | `PUT /shared-secrets/{secret_id}` | share one workspace secret with exactly these apps |
//!
//! Both are `WorkspaceAdmin`, the guard on the reveal route: sharing a secret
//! with an app hands its value to that app's code, so only someone who could
//! already read the value may do it. A sandbox agent token or an app publish
//! token is refused (404) even so — neither ever reads a secret's value.
//!
//! **The workspace in the path bounds everything**: the secret must be an
//! active, shareable workspace secret of that workspace and every app must
//! belong to it, or the answer is 404 — "not yours" and "does not exist" stay
//! indistinguishable, as on [`super::scoped_to_workspace`].

use std::collections::{BTreeMap, BTreeSet};

use axum::extract::Path;
use axum::http::StatusCode;
use axum::{Extension, Json};
use entity::{apps, secrets};
use futures::StreamExt;
use oxy::service::secret_manager::SecretInfo;
use oxy_app_core::audit::{self, RequestActor};
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_auth::types::AppPublishTokenAuth;
use oxy_custom_apps::secret_grants::{self, GrantChange};
use oxy_server_authz::role_guards::WorkspaceAdmin;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::super::custom_apps_functions::env_cache;
use super::declared::{AppSecretEntry, DeclaredKey};
use super::{Failure, agent, connect, declarations, scope, workspace_rows};

/// The most apps one request may share a secret with — far above any real
/// workspace, low enough that a request cannot ask for an unbounded insert.
const MAX_APPS: usize = 200;

/// How many apps' declarations [`list`] reads at once: each is two short
/// queries on its own pooled connection.
const DECLARATION_READS: usize = 4;

// ── DTOs ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct SharedSecretsResponse {
    /// Every workspace secret shared with at least one app.
    pub shared: Vec<SharedSecret>,
    /// Every app of the workspace, with the keys its production build declares
    /// that nothing provides yet.
    pub apps: Vec<AppNeeds>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct SharedSecret {
    pub secret_id: Uuid,
    pub app_ids: Vec<Uuid>,
}

#[derive(Debug, Serialize)]
pub struct AppNeeds {
    pub app_id: Uuid,
    pub app_name: String,
    pub app_slug: String,
    /// Declared, and nothing a production run would read.
    pub missing: Vec<MissingKey>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct MissingKey {
    pub key: String,
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ShareRequest {
    /// The apps that should read this secret, all of them: an app left out
    /// stops reading it.
    pub app_ids: Vec<Uuid>,
}

// ── The view mark ────────────────────────────────────────────────────────────

/// Mark, in a production view, each key the app stores no value of but reads
/// from a workspace secret shared with it. `rows` is the workspace's secrets as
/// the view already listed them.
///
/// Production only. Elsewhere a shared secret is read only for a `shared` key
/// ([`secret_grants::plan_granted`]), which `inherits_production` already
/// reports as not missing.
pub(super) async fn mark_shared(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &AppEnvironment,
    rows: &[SecretInfo],
    entries: &mut [AppSecretEntry],
) {
    if *environment != AppEnvironment::Production {
        return;
    }
    let shared = match secret_grants::shared_with_app(db, app.id).await {
        Ok(ids) => shared_names(&ids, rows),
        Err(e) => {
            // Under-reports (a shared key reads as missing) rather than failing
            // the view.
            tracing::warn!(app_id = %app.id, "shared workspace secrets could not be read: {e}");
            return;
        }
    };
    if apply_shared(&shared, entries) {
        super::declared::sort_entries(entries);
    }
}

/// Set `shared_from_workspace` on each unset entry `shared` names. Whether any
/// changed, so the caller re-sorts only then. Pure.
pub(super) fn apply_shared(
    shared: &BTreeMap<String, Uuid>,
    entries: &mut [AppSecretEntry],
) -> bool {
    let mut changed = false;
    for entry in entries.iter_mut().filter(|e| !e.is_set) {
        entry.shared_from_workspace = shared.get(&entry.key).copied();
        changed |= entry.shared_from_workspace.is_some();
    }
    changed
}

/// The names production reads from workspace secrets shared with `app`, for a
/// check that only needs "is it provided". Empty, logged, when the grants
/// cannot be read — the check then under-reports a shared key as unset.
pub(super) async fn shared_keys(
    db: &DatabaseConnection,
    app: &apps::Model,
    rows: &[SecretInfo],
) -> BTreeSet<String> {
    match secret_grants::shared_with_app(db, app.id).await {
        Ok(ids) => shared_names(&ids, rows).into_keys().collect(),
        Err(e) => {
            tracing::warn!(app_id = %app.id, "shared workspace secrets could not be read: {e}");
            BTreeSet::new()
        }
    }
}

/// Name → id of the secrets in `rows` (the workspace's, active) that `ids`
/// shares and that may be shared at all. Pure.
fn shared_names(ids: &BTreeSet<Uuid>, rows: &[SecretInfo]) -> BTreeMap<String, Uuid> {
    rows.iter()
        .filter(|s| ids.contains(&s.id) && secret_grants::is_shareable(&s.name))
        .map(|s| (s.name.clone(), s.id))
        .collect()
}

// ── Handlers ─────────────────────────────────────────────────────────────────

/// `GET /api/{workspace_id}/shared-secrets`
///
/// The workspace's secrets and grants are read once; per app only its
/// production declarations are, a few at a time, so the page costs a constant
/// number of scans of the secret rows however many apps the workspace has.
pub async fn list(
    _: WorkspaceAdmin,
    Path(workspace_id): Path<Uuid>,
) -> Result<Json<SharedSecretsResponse>, Failure> {
    let db = connect().await?;
    let rows = workspace_rows(&db, workspace_id).await?;
    let grants = secret_grants::in_workspace(&db, workspace_id)
        .await
        .map_err(internal("listing shared secrets"))?;
    let shared = shared_by_app(&grants, &rows);
    // Built up front, not mapped inside the stream: a closure over borrowed
    // items in the stream's type is not `Send` for every lifetime axum asks
    // (the same shape as `home_widgets::list`).
    let reads: Vec<_> = workspace_apps(&db, workspace_id)
        .await?
        .into_iter()
        .map(|app| app_needs(&db, app, &rows, &shared))
        .collect();
    let needs: Vec<AppNeeds> = futures::stream::iter(reads)
        .buffered(DECLARATION_READS)
        .collect()
        .await;
    Ok(Json(SharedSecretsResponse {
        shared: group_grants(grants),
        apps: needs,
    }))
}

/// One app's missing keys: what its production build declares, less its own
/// production keys and the workspace secrets shared with it.
async fn app_needs(
    db: &DatabaseConnection,
    app: apps::Model,
    rows: &[SecretInfo],
    shared: &BTreeMap<Uuid, BTreeSet<String>>,
) -> AppNeeds {
    let declared = declarations::for_environment(db, &app, &AppEnvironment::Production).await;
    let provided: BTreeSet<String> = own_keys(app.id, rows)
        .into_iter()
        .chain(shared.get(&app.id).into_iter().flatten().cloned())
        .collect();
    AppNeeds {
        missing: missing_from(declared.keys.into_values(), &provided),
        app_id: app.id,
        app_name: app.name,
        app_slug: app.slug,
    }
}

/// `PUT /api/{workspace_id}/shared-secrets/{secret_id}`
pub async fn share(
    _: WorkspaceAdmin,
    Path((workspace_id, secret_id)): Path<(Uuid, Uuid)>,
    actor: RequestActor,
    marker: Option<Extension<AppPublishTokenAuth>>,
    Json(body): Json<ShareRequest>,
) -> Result<Json<SharedSecret>, Failure> {
    if marker.is_some() || agent::token_of(&actor.user).is_some() {
        return Err(not_found("secret"));
    }
    let wanted: BTreeSet<Uuid> = body.app_ids.into_iter().collect();
    if wanted.len() > MAX_APPS {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("a secret can be shared with at most {MAX_APPS} apps per request"),
        ));
    }
    let db = connect().await?;
    let secret = shareable_secret(&db, workspace_id, secret_id).await?;
    let apps = apps_in_workspace(&db, workspace_id, &wanted).await?;
    let change =
        secret_grants::set_apps(&db, workspace_id, secret_id, &wanted, Some(actor.user.id))
            .await
            .map_err(internal("sharing a secret"))?;
    for app_id in change.touched() {
        env_cache::invalidate(app_id, &AppEnvironment::Production);
    }
    audit_change(&db, &actor, &secret, &apps, &change).await;
    Ok(Json(SharedSecret {
        secret_id,
        app_ids: wanted.into_iter().collect(),
    }))
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// The keys an app stores for production (`apps/<app_id>/<KEY>`, one segment —
/// never an environment's `staging/KEY`), out of the workspace's rows. Pure.
fn own_keys(app_id: Uuid, rows: &[SecretInfo]) -> BTreeSet<String> {
    let prefix = scope::secret_prefix(app_id, &AppEnvironment::Production);
    rows.iter()
        .filter_map(|s| s.name.strip_prefix(&prefix))
        .filter(|key| !key.is_empty() && !key.contains('/'))
        .map(str::to_string)
        .collect()
}

/// App → names of the active, shareable workspace secrets shared with it. A
/// grant whose secret is not among `rows` (deleted since) shares nothing. Pure.
fn shared_by_app(grants: &[(Uuid, Uuid)], rows: &[SecretInfo]) -> BTreeMap<Uuid, BTreeSet<String>> {
    let names: BTreeMap<Uuid, &str> = rows
        .iter()
        .filter(|s| secret_grants::is_shareable(&s.name))
        .map(|s| (s.id, s.name.as_str()))
        .collect();
    let mut by_app: BTreeMap<Uuid, BTreeSet<String>> = BTreeMap::new();
    for (secret_id, app_id) in grants {
        if let Some(name) = names.get(secret_id) {
            by_app
                .entry(*app_id)
                .or_default()
                .insert((*name).to_string());
        }
    }
    by_app
}

/// The declared keys nothing `provided` covers, required first, then by name.
/// Pure.
fn missing_from(
    declared: impl IntoIterator<Item = DeclaredKey>,
    provided: &BTreeSet<String>,
) -> Vec<MissingKey> {
    let mut missing: Vec<MissingKey> = declared
        .into_iter()
        .filter(|d| !provided.contains(&d.key))
        .map(|d| MissingKey {
            key: d.key,
            required: d.required,
            description: d.description,
        })
        .collect();
    missing.sort_by(|a, b| b.required.cmp(&a.required).then_with(|| a.key.cmp(&b.key)));
    missing
}

/// `(secret_id, app_id)` rows grouped by secret, both sides sorted.
fn group_grants(grants: Vec<(Uuid, Uuid)>) -> Vec<SharedSecret> {
    let mut by_secret: BTreeMap<Uuid, BTreeSet<Uuid>> = BTreeMap::new();
    for (secret_id, app_id) in grants {
        by_secret.entry(secret_id).or_default().insert(app_id);
    }
    by_secret
        .into_iter()
        .map(|(secret_id, apps)| SharedSecret {
            secret_id,
            app_ids: apps.into_iter().collect(),
        })
        .collect()
}

async fn workspace_apps(
    db: &DatabaseConnection,
    workspace_id: Uuid,
) -> Result<Vec<apps::Model>, Failure> {
    apps::Entity::find()
        .filter(apps::Column::ProjectId.eq(workspace_id))
        .order_by_asc(apps::Column::Name)
        .all(db)
        .await
        .map_err(internal("listing apps"))
}

/// The secret `secret_id`, when it is an active, shareable secret of
/// `workspace_id`; 404 otherwise — an app's own `apps/<id>/KEY` row included.
async fn shareable_secret(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    secret_id: Uuid,
) -> Result<secrets::Model, Failure> {
    let secret = secrets::Entity::find_by_id(secret_id)
        .one(db)
        .await
        .map_err(internal("secret lookup"))?
        .ok_or_else(|| not_found("secret"))?;
    let shareable = secret.project_id == workspace_id
        && secret.is_active
        && secret_grants::is_shareable(&secret.name);
    shareable
        .then_some(secret)
        .ok_or_else(|| not_found("secret"))
}

/// Every app in `wanted`, when all of them belong to `workspace_id`; 404 when
/// any does not exist or is another workspace's.
async fn apps_in_workspace(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    wanted: &BTreeSet<Uuid>,
) -> Result<Vec<apps::Model>, Failure> {
    if wanted.is_empty() {
        return Ok(Vec::new());
    }
    let found = apps::Entity::find()
        .filter(apps::Column::Id.is_in(wanted.iter().copied()))
        .filter(apps::Column::ProjectId.eq(workspace_id))
        .all(db)
        .await
        .map_err(internal("app lookup"))?;
    if found.len() != wanted.len() {
        tracing::warn!(
            %workspace_id,
            asked = wanted.len(),
            found = found.len(),
            "share request names an app outside the path workspace — answering 404"
        );
        return Err(not_found("app"));
    }
    Ok(found)
}

/// One `custom_app.secret.shared` / `.unshared` row per app the change moved.
/// Best-effort: a failed audit write never fails the share.
async fn audit_change(
    db: &DatabaseConnection,
    actor: &RequestActor,
    secret: &secrets::Model,
    apps: &[apps::Model],
    change: &GrantChange,
) {
    let mut org_of: BTreeMap<Uuid, (Uuid, String)> = apps
        .iter()
        .map(|a| (a.id, (a.org_id, a.slug.clone())))
        .collect();
    // A revoked app is not in `apps` (it was left out of the request).
    if !change.removed.is_empty()
        && let Ok(gone) = apps::Entity::find()
            .filter(apps::Column::Id.is_in(change.removed.clone()))
            .all(db)
            .await
    {
        org_of.extend(gone.into_iter().map(|a| (a.id, (a.org_id, a.slug))));
    }
    let moves = change
        .added
        .iter()
        .map(|id| (id, "custom_app.secret.shared"))
        .chain(
            change
                .removed
                .iter()
                .map(|id| (id, "custom_app.secret.unshared")),
        );
    for (app_id, action) in moves {
        let Some((org_id, slug)) = org_of.get(app_id) else {
            continue;
        };
        let entry = audit::AuditEntry::for_request(actor, action)
            .org(*org_id)
            .workspace(secret.project_id)
            .target(
                "workspace_secret",
                secret.id.to_string(),
                format!("{} → {slug}", secret.name),
            )
            .environment(AppEnvironment::Production.name());
        audit::record_best_effort(db, entry).await;
    }
}

fn not_found(what: &str) -> Failure {
    (StatusCode::NOT_FOUND, format!("{what} not found"))
}

fn internal(doing: &'static str) -> impl Fn(sea_orm::DbErr) -> Failure {
    move |e| {
        tracing::error!("{doing} failed: {e}");
        (StatusCode::INTERNAL_SERVER_ERROR, format!("{doing} failed"))
    }
}

#[cfg(test)]
#[path = "workspace_shared_tests.rs"]
mod tests;
