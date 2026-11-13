use axum::extract::{Extension, Json, Path, Query, State};
use axum::response::Json as ResponseJson;
use axum::response::{IntoResponse, Response};
use reqwest::StatusCode;
use tracing::{error, info};
use uuid::Uuid;

use crate::server::api::middlewares::org_context::OrgContextExtractor;
use crate::server::api::middlewares::role_guards::{
    OrgAdmin, ensure_org_admin_or_workspace_creator,
};
use crate::server::api::middlewares::workspace_context::{
    EffectiveWorkspaceRole, WorkspaceRootWorkingCopy,
};
use crate::server::authz;
use crate::server::router::AppState;
use oxy::api_types::{BranchOrigin, BranchType, ProjectBranch};
use oxy::config::ConfigBuilder;
use oxy::github::default_git_client;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_git::GitClient;

use super::detached::active_head;
use super::dto::*;
use super::ops::*;

/// Detect the workspace's git mode from disk + environment. The
/// `GIT_REPOSITORY_URL` env var is treated as "remote configured" — cloud
/// deployments inject the remote that way.
pub async fn detect_git_mode(workspace_root: &std::path::Path) -> GitMode {
    let git = default_git_client();
    if !git.is_git_repo(workspace_root) {
        return GitMode::None;
    }
    let has_remote =
        std::env::var("GIT_REPOSITORY_URL").is_ok() || git.has_remote(workspace_root).await;
    if has_remote {
        GitMode::Connected
    } else {
        GitMode::Local
    }
}

// ─── Handlers ────────────────────────────────────────────────────────────

/// The repository facts `/meta` serves (see `repository_facts`), for any other
/// reader that names the repository. One credential-stripping path, not two.
pub fn workspace_repository_facts(
    row: &entity::workspaces::Model,
) -> (Option<String>, Option<String>, Option<String>) {
    repository_facts(row)
}

/// `GET /{workspace_id}/meta` — the Postgres half of `/details`.
///
/// Additive: `/details` is untouched and still serves both halves. This exists
/// so the frontend can move one query at a time, and so the half that needs no
/// working copy stops being unavailable whenever the ide is.
///
/// It delegates rather than duplicating the local-mode short-circuit and the
/// storage-key computation, both of which have earned their comments. Splitting
/// the *surface* is what unblocks the frontend; splitting the *bodies* can wait
/// until the old route is retired.
pub async fn get_workspace_meta(
    state: State<AppState>,
    user: AuthenticatedUserExtractor,
    EffectiveWorkspaceRole(role): EffectiveWorkspaceRole,
    project: Extension<entity::workspaces::Model>,
    path: Path<Uuid>,
    crate::server::factory_access::FactoryAccess(factory_access): crate::server::factory_access::FactoryAccess,
) -> Result<ResponseJson<WorkspaceMetaResponse>, Response> {
    // Local mode keeps the delegation. Both of the fields that can only be
    // answered from disk — `requires_local_setup` and `workspace_error` — are
    // set nowhere else, and the short-circuit that computes them has earned
    // its comments.
    if state.mode.is_local() {
        let ResponseJson(d) =
            get_workspace(state, user, EffectiveWorkspaceRole(role), project, path).await?;
        return Ok(ResponseJson(WorkspaceMetaResponse {
            id: d.id,
            name: d.name,
            workspace_id: d.workspace_id,
            created_at: d.created_at,
            updated_at: d.updated_at,
            workspace_error: d.workspace_error,
            storage_key: d.storage_key,
            current_user_role: d.current_user_role,
            requires_local_setup: d.requires_local_setup,
            // Local mode is one guest owner on their own machine: no Factory to gate.
            factory_access: true,
            // `--local` is the unmaintained no-remote mode: nothing to clone.
            git_remote_url: None,
            default_branch: None,
            repo_subdir: None,
        }));
    }

    // Cloud: not one field of this response comes off the filesystem.
    // `workspace_id`, `created_at` and `updated_at` are the same constants in
    // both arms of `build_workspace_details_response`; `workspace_error` is
    // only ever `Some` under `is_local`; `requires_local_setup` is `false`.
    //
    // Delegating anyway meant inheriting `get_workspace`'s work AND its
    // failures: four git subprocesses and an `Origin::Disk` ConfigManager per
    // request on a node that owns the files, and — worse — the 503 that
    // `build_workspace_details_response` raises when the workspace root is not
    // there yet on an fs-writable pod. `/meta` was returning "materializing"
    // for data no volume holds. FleetOk was safe only because a replica has no
    // files to trip over; now it is safe because there is nothing to ask.
    let Path(workspace_id) = path;
    let Extension(project) = project;
    let now = chrono::Utc::now().to_string();
    let (git_remote_url, default_branch, repo_subdir) = repository_facts(&project);
    Ok(ResponseJson(WorkspaceMetaResponse {
        id: workspace_id,
        name: project.name,
        workspace_id: Uuid::nil(),
        created_at: now.clone(),
        updated_at: now,
        workspace_error: None,
        storage_key: compute_workspace_storage_key(workspace_id, None),
        current_user_role: role.as_str().to_string(),
        requires_local_setup: false,
        factory_access,
        git_remote_url,
        default_branch,
        repo_subdir,
    }))
}

/// The repository facts `/meta` serves, off the Postgres row: the remote with
/// any `user:password@` removed (callers show the URL, never authenticate with
/// it), the default branch, and the workspace's subdirectory when it is not the
/// repository root. Empty strings read as absent.
fn repository_facts(
    row: &entity::workspaces::Model,
) -> (Option<String>, Option<String>, Option<String>) {
    let remote = row
        .git_remote_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(strip_url_userinfo);
    let branch = oxy_app_core::workspace_repo_facts::stored_default_branch(row);
    let subdir = oxy_app_core::workspace_repo_facts::stored_repo_subdir(row).map(str::to_string);
    (remote, branch, subdir)
}

/// `https://user:pw@host/x` -> `https://host/x`. Leaves scp-style
/// (`git@github.com:o/r.git`) and credential-free URLs untouched.
fn strip_url_userinfo(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    let (authority, tail) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    match authority.rsplit_once('@') {
        Some((_, host)) => format!("{scheme}://{host}{tail}"),
        None => url.to_string(),
    }
}

/// Get detailed information about a specific workspace including its active branch.
///
/// This endpoint retrieves complete workspace details along with the currently active branch information.
/// Requires authentication and returns workspace metadata, provider info, and active branch details.
#[utoipa::path(
    get,
    path = "/workspaces/{workspace_id}",
    params(
        ("workspace_id" = Uuid, Path, description = "Workspace ID")
    ),
    responses(
        (status = 200, description = "Workspace details retrieved successfully", body = WorkspaceDetailsResponse),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Workspace not found"),
        (status = 500, description = "Internal server error"),
        (status = 503, description = "Workspace working copy is not on disk yet \
            (pod restart / rolling update). Transient — carries \
            `x-oxy-unavailable: workspace-materializing` and `Retry-After`; \
            retry rather than surfacing an error. Cloud only.")
    ),
    security(
        ("ApiKey" = [])
    ),
    tag = "Workspaces"
)]
pub async fn get_workspace(
    State(app_state): State<AppState>,
    AuthenticatedUserExtractor(_user): AuthenticatedUserExtractor,
    EffectiveWorkspaceRole(role): EffectiveWorkspaceRole,
    Extension(project): Extension<entity::workspaces::Model>,
    Path(workspace_id): Path<Uuid>,
) -> Result<ResponseJson<WorkspaceDetailsResponse>, Response> {
    info!("Getting workspace details for ID: {}", workspace_id);

    let role_str = role.as_str().to_string();

    // Local-mode short-circuit: when there is no resolvable config.yml,
    // skip the normal path (which 500s on path: None) and return the
    // "setup required" shape so the FE renders the bootstrap dialog.
    if app_state.mode.is_local() {
        let config_exists = match &project.path {
            Some(p) => tokio::fs::try_exists(std::path::Path::new(p).join("config.yml"))
                .await
                .unwrap_or(false),
            None => false,
        };
        if !config_exists {
            // `setup_demo` / `setup_empty` write into `startup_cwd`, so hashing
            // it here keeps the storage_key stable across the bootstrap → ready
            // transition (the workspace path resolves to the same dir post-setup).
            let storage_key =
                compute_workspace_storage_key(workspace_id, Some(app_state.startup_cwd.as_path()));
            return Ok(build_workspace_details_response_for_uninitialized_local(
                workspace_id,
                &project.name,
                role_str,
                storage_key,
            ));
        }
    }

    let workspace_root = workspace_root(&project)
        .await
        .map_err(IntoResponse::into_response)?;

    let local_path_for_key = app_state
        .mode
        .is_local()
        .then_some(workspace_root.as_path());
    let storage_key = compute_workspace_storage_key(workspace_id, local_path_for_key);

    // A missing working copy is "not ready yet" only when THIS instance owns the
    // workspace filesystem. A stateless serve replica has no PVC, so `exists()`
    // is false there in normal operation; answering 503 would turn a benign
    // degraded page into an unusable one.
    let missing_copy_is_transient =
        !app_state.mode.is_local() && crate::server::role_manifest::process_is_fs_writable();

    build_workspace_details_response(
        workspace_id,
        &project.name,
        &workspace_root,
        app_state.mode.is_local(),
        missing_copy_is_transient,
        role_str,
        storage_key,
    )
    .await
}

/// How long the FE should wait before retrying a `workspace-materializing` 503.
/// Matches `ide_proxy`'s ide-down `Retry-After` — same class of "come back in a
/// moment", and a shared number is one fewer thing to reconcile.
const MATERIALIZING_RETRY_AFTER_SECS: &str = "5";

/// The working copy is not on disk on an instance that OWNS one (ide / all).
/// That is a readiness state, not a failure: git is the source of truth, so the
/// checkout is by definition re-clonable and the only honest answer is "not
/// yet". Say so with `503` + `Retry-After` and let the caller back off.
///
/// Only reachable when `missing_copy_is_transient` — a serve replica with no
/// PVC hits the same `exists()` check in NORMAL operation while degrading for a
/// down ide, and must keep its flagged 200.
///
/// It previously returned `200` with a `workspace_error` string, which was
/// wrong in three compounding ways during a rolling update:
///   1. A `200` is invisible to every ide-down detector (they key on `502` +
///      `x-oxy-required-role: ide`), so nothing throttled it.
///   2. That `200` carries `x-oxy-served-by: ide`, so the FE's success
///      interceptor fired `reportIdeReachable()` — the response that should
///      raise the "unavailable" UI was retiring it instead.
///   3. The FE toasted the string AND navigated away, which remounted the
///      shell, refetched, and toasted again. That loop is the toast spam.
///
/// Deliberately NOT `502` and NOT `x-oxy-required-role: ide`: the ide is
/// REACHABLE and serving its other routes fine, so borrowing the ide-down
/// signal would conflate two states — and because `reportIdeReachable()` fires
/// on any successful ide response, a concurrent healthy request would flap the
/// banner off while this one was still raising it.
fn workspace_materializing_response(workspace_root: &std::path::Path) -> Response {
    tracing::info!(
        path = %workspace_root.display(),
        "workspace details: working copy not materialised yet — 503 (transient)"
    );
    (
        StatusCode::SERVICE_UNAVAILABLE,
        [
            (
                crate::server::ide_proxy::HEADER_UNAVAILABLE,
                "workspace-materializing",
            ),
            (
                axum::http::header::RETRY_AFTER.as_str(),
                MATERIALIZING_RETRY_AFTER_SECS,
            ),
        ],
        ResponseJson(serde_json::json!({
            "error": "Workspace is still starting up",
            "code": "workspace_materializing",
        })),
    )
        .into_response()
}

pub async fn build_workspace_details_response(
    workspace_id: Uuid,
    name: &str,
    workspace_root: &std::path::Path,
    // Set to true in local mode so the response reports `GitMode::None`
    // even when a `.git` folder exists on disk. Opposite polarity from
    // the router's `include_git_features` — matches `ServeMode::is_local`.
    is_local: bool,
    // Whether a MISSING working copy should be reported as transient (503) or
    // as the flagged degrade (200). Deliberately a separate flag rather than
    // derived from `is_local`: three distinct situations produce a missing
    // directory and only one of them is transient.
    //   - ide/all in cloud, volume not populated yet → TRANSIENT. Ours to own.
    //   - serve replica in cloud → the ide is down and we are serving the
    //     documented graceful degrade. Not ours, not transient.
    //   - local mode → no upstream to restore from, so it is genuinely gone.
    missing_copy_is_transient: bool,
    current_user_role: String,
    storage_key: String,
) -> Result<ResponseJson<WorkspaceDetailsResponse>, Response> {
    let now = chrono::Utc::now().to_string();

    if !workspace_root.exists() {
        if missing_copy_is_transient {
            return Err(workspace_materializing_response(workspace_root));
        }
        // The 200 with safe defaults and git shown as unavailable. Whether it
        // carries a user-visible message depends on who is asking:
        //
        //   - LOCAL: the directory really is gone and the path is the user's
        //     own filesystem, so naming it is the actionable thing to say.
        //   - CLOUD serve replica: this is the DOCUMENTED degrade for a down
        //     ide, i.e. expected operation — not an error. A server-side path
        //     is our vocabulary, not the user's; they can neither verify nor
        //     act on it. And the FE toasts `workspace_error` AND redirects to
        //     the org root, which would defeat the very degrade this path
        //     exists to serve. `git_mode: None` already tells the UI that git
        //     is unavailable, and the ide-unavailable banner (raised by the
        //     other IdeOnly requests 502ing) explains why.
        let workspace_error = is_local.then(|| {
            format!(
                "Workspace directory not found: {}",
                workspace_root.display()
            )
        });
        return Ok(no_git_response(
            workspace_id,
            name,
            now,
            workspace_error,
            false,
            current_user_role,
            storage_key,
        ));
    }

    // Local-mode servers disable all git features — routes are not
    // mounted, capabilities must match. Force None regardless of
    // what lives on disk.
    if is_local {
        return Ok(no_git_response(
            workspace_id,
            name,
            now,
            None,
            false,
            current_user_role,
            storage_key,
        ));
    }

    let git = default_git_client();
    let git_mode = detect_git_mode(workspace_root).await;
    let has_local_repo = !matches!(git_mode, GitMode::None);

    let default_branch = if has_local_repo {
        git.get_default_branch(workspace_root).await
    } else {
        "main".to_string()
    };

    let (current_branch, detached_head) = active_head(workspace_root, &default_branch).await;

    // Fall back to [default_branch] on any error or when there's no local repo.
    let protected_branches: Vec<String> = if has_local_repo {
        let config_branches = match ConfigBuilder::new().with_workspace_path(workspace_root) {
            Ok(builder) => match builder
                .build_with_working_copy(oxy::config::Origin::Disk, oxy::config::OnMissing::Empty)
                .await
            {
                Ok(manager) => manager.protected_branches().map(|b| b.to_vec()),
                Err(err) => {
                    tracing::warn!(
                        workspace_path = %workspace_root.display(),
                        error = %err,
                        "failed to build config for protected_branches; falling back to default"
                    );
                    None
                }
            },
            Err(err) => {
                tracing::warn!(
                    workspace_path = %workspace_root.display(),
                    error = %err,
                    "failed to build config for protected_branches; falling back to default"
                );
                None
            }
        };
        config_branches.unwrap_or_else(|| vec![default_branch.clone()])
    } else {
        vec![default_branch.clone()]
    };

    Ok(ResponseJson(WorkspaceDetailsResponse {
        id: workspace_id,
        name: name.to_string(),
        workspace_id: Uuid::nil(),
        created_at: now.clone(),
        updated_at: now.clone(),
        active_branch: Some(ProjectBranch {
            id: Uuid::nil(),
            name: current_branch,
            revision: String::new(),
            workspace_id,
            branch_type: BranchType::Local,
            origin: BranchOrigin::Both,
            created_at: now.clone(),
            updated_at: now,
        }),
        detached_head,
        workspace_error: None,
        git_mode,
        capabilities: git_mode.into(),
        default_branch,
        protected_branches,
        requires_local_setup: false,
        current_user_role,
        storage_key,
    }))
}

/// GET /orgs/{org_id}/workspaces — list workspaces in the given org.
/// Membership is enforced by org_middleware; this handler just scopes the query.
/// The workspaces in one organization.
///
/// The second step of turning a customer into ids: `{workspace_id}` is the
/// path parameter almost every other endpoint takes. Membership is enforced by
/// `org_middleware`, so this only ever returns workspaces the caller can reach.
#[utoipa::path(
    method(get),
    path = "/orgs/{org_id}/workspaces",
    params(
        ("org_id" = Uuid, Path, description = "Organization UUID")
    ),
    responses(
        (status = OK, description = "Workspaces in the organization", body = Vec<WorkspaceSummary>, content_type = "application/json")
    ),
    security(
        ("BearerAuth" = [])
    )
)]
pub async fn list_workspaces(
    crate::server::api::middlewares::org_context::OrgContextExtractor(ctx): crate::server::api::middlewares::org_context::OrgContextExtractor,
    caller: authz::Caller,
    State(_app_state): State<AppState>,
) -> Result<ResponseJson<Vec<WorkspaceSummary>>, StatusCode> {
    use entity::prelude::Workspaces;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

    let db = oxy::database::client::establish_connection()
        .await
        .map_err(|e| {
            error!("Failed to connect to database: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Never the nil UUID: it is the legacy `--local` workspace id, which this
    // (cloud-only) surface's own `workspace_middleware` refuses with a 404. A
    // database a `--local` run once touched keeps a nil "Local" row in its org,
    // and listing it lands the post-login dispatcher on a workspace that spins.
    let workspaces = Workspaces::find()
        .filter(entity::workspaces::Column::OrgId.eq(ctx.org.id))
        .filter(entity::workspaces::Column::Id.ne(Uuid::nil()))
        .all(&db)
        .await
        .map_err(|e| {
            error!("Failed to list workspaces: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Discovery follows the grant (API-tokens design §4.5): an API token lists
    // only the workspaces it covers. A session lists them all.
    let workspaces: Vec<_> = workspaces
        .into_iter()
        .filter(|ws| caller.workspace_ceiling(ctx.org.id, ws.id).is_some())
        .collect();

    // Batch-fetch creator names for all workspaces that have a created_by set.
    let creator_ids: Vec<Uuid> = workspaces
        .iter()
        .filter_map(|p| p.created_by)
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    let creator_names: std::collections::HashMap<Uuid, String> = if creator_ids.is_empty() {
        std::collections::HashMap::new()
    } else {
        use entity::prelude::Users;
        use sea_orm::{ColumnTrait, QueryFilter};
        Users::find()
            .filter(entity::users::Column::Id.is_in(creator_ids))
            .all(&db)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|u| {
                let display = if u.name.is_empty() {
                    u.email.unwrap_or_default()
                } else {
                    u.name
                };
                (u.id, display)
            })
            .collect()
    };

    // Agents are compiled rows: counted from the promoted revisions in one
    // query, on any pod, rather than by walking a working copy.
    let agent_counts = super::ops::compiled_agent_counts(&workspaces).await;

    let summary_futures = workspaces.into_iter().map(|p| {
        let creator_names = creator_names.clone();
        let agent_count = agent_counts.get(&p.id).copied();
        async move {
            // Gather automation counts and git metadata when the path exists on disk.
            let (workflow_count, git_remote, git_commit, git_updated_at) =
                if let Some(ref path_str) = p.path {
                    let dir = std::path::PathBuf::from(path_str);
                    if dir.exists() {
                        let workflow_count = tokio::task::spawn_blocking({
                            let dir = dir.clone();
                            move || Some(count_automation_files(&dir))
                        })
                        .await
                        .unwrap_or(None);
                        let git = default_git_client();
                        let (remote, (sha, msg), updated_at) = tokio::join!(
                            git.get_remote_url(&dir),
                            git.get_branch_commit(&dir, "HEAD"),
                            git.get_head_commit_relative_date(&dir)
                        );
                        let commit = if sha.is_empty() {
                            None
                        } else {
                            Some(format!("{} — {}", &sha[..sha.len().min(7)], msg))
                        };
                        (workflow_count, remote, commit, updated_at)
                    } else {
                        (None, None, None, None)
                    }
                } else {
                    (None, None, None, None)
                };

            WorkspaceSummary {
                id: p.id,
                org_id: p.org_id,
                name: p.name,
                path: p.path,
                created_at: p.created_at.into(),
                last_opened_at: p.last_opened_at.map(|t| t.into()),
                created_by_name: p.created_by.and_then(|id| creator_names.get(&id).cloned()),
                agent_count,
                workflow_count,
                git_remote,
                git_commit,
                git_updated_at,
                status: p.status,
                error: p.error,
            }
        }
    });

    let summaries = futures::future::join_all(summary_futures).await;

    Ok(ResponseJson(summaries))
}

// SECURITY: the access rule is "Org Owner/Admin OR the workspace creator".
// The creator branch needs `ctx.membership.user_id` plus `workspace.created_by`,
// which is only available after a DB lookup — so we can't use a pure typed
// guard like `OrgAdmin` here. Instead we use `OrgContextExtractor` to surface
// the membership and delegate the role check to
// `ensure_org_admin_or_workspace_creator`, which lives in `role_guards.rs`
// alongside the other role checks.
pub async fn rename_workspace(
    OrgContextExtractor(ctx): OrgContextExtractor,
    actor: oxy_app_core::audit::RequestActor,
    Path((org_id, workspace_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<RenameWorkspaceRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    use entity::prelude::Workspaces;
    use entity::workspaces;
    use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter, Set};

    let name = body.name.trim().to_string();
    if name.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "Workspace name cannot be empty".to_string(),
        ));
    }

    // Guard against control characters and non-printable input.
    if !name.chars().all(|c| c.is_ascii_graphic() || c == ' ') {
        return Err((
            StatusCode::BAD_REQUEST,
            "Workspace name contains invalid characters".to_string(),
        ));
    }

    let db = oxy::database::client::establish_connection()
        .await
        .map_err(|e| {
            error!("Failed to connect to database: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Database connection failed: {e}"),
            )
        })?;

    // Fetch the workspace first so we can check ownership before doing anything else.
    let workspace = Workspaces::find_by_id(workspace_id)
        .one(&db)
        .await
        .map_err(|e| {
            error!("Failed to find workspace {}: {}", workspace_id, e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to find workspace: {e}"),
            )
        })?
        .ok_or((StatusCode::NOT_FOUND, "Workspace not found".to_string()))?;

    if workspace.org_id != Some(org_id) {
        return Err((StatusCode::NOT_FOUND, "Workspace not found".to_string()));
    }

    // Org admins/owners can rename any workspace; Org Members can rename only those
    // they created (role_guards::ensure_org_admin_or_workspace_creator) — the one
    // authorization rule outside the role guards, now enforced through the unified
    // WorkspaceRename ring. `existing && unified`, so the model can only tighten it.
    let legacy = ensure_org_admin_or_workspace_creator(&ctx, &workspace).is_ok();
    let allowed = authz::enforce_for(
        &db,
        &crate::server::authz::caller_of(&actor),
        "workspace.rename",
        authz::Action::WorkspaceRename,
        authz::Resource::workspace_with_creator(workspace.id, org_id, workspace.created_by),
        legacy,
    )
    .await;
    if !allowed {
        return Err((
            StatusCode::FORBIDDEN,
            "Only admins or workspace creator can rename".to_string(),
        ));
    }

    // Reject duplicate names within the same org.
    let name_taken = Workspaces::find()
        .filter(workspaces::Column::Name.eq(&name))
        .filter(workspaces::Column::Id.ne(workspace_id))
        .filter(workspaces::Column::OrgId.eq(org_id))
        .one(&db)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to query workspaces: {e}"),
            )
        })?
        .is_some();

    if name_taken {
        return Err((
            StatusCode::CONFLICT,
            format!("A workspace named '{name}' already exists. Please choose a different name."),
        ));
    }

    let mut active: workspaces::ActiveModel = workspace.into_active_model();
    active.name = Set(name.clone());
    active.updated_at = Set(chrono::Utc::now().into());
    active.save(&db).await.map_err(|e| {
        error!("Failed to rename workspace {}: {}", workspace_id, e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to rename workspace: {e}"),
        )
    })?;

    info!("Renamed workspace {} to '{}'", workspace_id, name);
    Ok(StatusCode::OK)
}

pub async fn delete_workspace(
    // `OrgAdmin` FIRST: axum runs extractors in argument order, and resolving a
    // workspace path before checking authority would leak existence.
    _: OrgAdmin,
    State(_app_state): State<AppState>,
    Path((org_id, workspace_id)): Path<(Uuid, Uuid)>,
    Query(query): Query<DeleteProjectQuery>,
    root: WorkspaceRootWorkingCopy,
) -> Result<StatusCode, StatusCode> {
    use sea_orm::{ModelTrait, TransactionTrait};

    let db = oxy::database::client::establish_connection()
        .await
        .map_err(|e| {
            error!("Failed to connect to database: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // The extractor already loaded it, and building the manager needed it — so
    // querying again here would be a second round trip for the same row.
    let workspace = root.workspace.clone().ok_or(StatusCode::NOT_FOUND)?;

    if workspace.org_id != Some(org_id) {
        return Err(StatusCode::NOT_FOUND);
    }

    // From the extractor, so the signature states that this handler reaches a
    // working copy. It used to resolve the path here by hand, which is how a
    // filesystem delete came to be described by a signature naming no manager.
    let workspace_path = root
        .manager
        .as_ref()
        .map(|m| m.config_manager.workspace_path().to_path_buf());

    // The delete cascades the tenant row away, not the tenant in Airhouse: record
    // it for removal in the same transaction, so both commit or neither does.
    let tenants = airhouse::workspace_delete::tenants_of(&db, &[workspace_id]).await;
    let delete_failed = |e: sea_orm::DbErr| {
        error!("Failed to delete workspace {}: {}", workspace_id, e);
        StatusCode::INTERNAL_SERVER_ERROR
    };
    let txn = db.begin().await.map_err(delete_failed)?;
    workspace.delete(&txn).await.map_err(delete_failed)?;
    airhouse::workspace_delete::record(&txn, &tenants)
        .await
        .map_err(delete_failed)?;
    txn.commit().await.map_err(delete_failed)?;

    info!("Deleted workspace {}", workspace_id);

    // Clean up the now-orphaned schedule rows *after* the workspace is gone: if
    // the delete above had failed we'd have stripped a still-live workspace of
    // its workflow/monitor schedules (which, unlike `health_eval`, startup
    // reconcile never recreates).
    cleanup_workspace_schedules(&db, workspace_id).await;

    // Only remove files from disk when the caller explicitly opts in.
    // Without `?delete_files=true` we only remove the DB record, leaving
    // the directory intact so an accidental delete can be recovered.
    if query.delete_files
        && let Some(path) = workspace_path
    {
        if path.exists() {
            if let Err(e) = std::fs::remove_dir_all(&path) {
                tracing::warn!("Failed to delete workspace directory {:?}: {}", path, e);
            } else {
                info!("Deleted workspace directory {:?}", path);
            }
        } else if !oxy::workspace_fs_probe::process_owns_workspace_files() {
            // The route is IdeOnly so this should be unreachable, but a missing
            // directory means two different things and only one of them is
            // "nothing to delete". On a pod with no working copy it means the
            // files are elsewhere, still there, and the 200 below is a lie.
            tracing::error!(
                %workspace_id,
                path = %path.display(),
                "delete_files requested on a process that owns no working copy; the \
                 directory was NOT deleted and still exists on the ide. This route is \
                 classified IdeOnly — reaching here means routing did not hold."
            );
        }
    }

    Ok(StatusCode::OK)
}

#[cfg(test)]
mod strip_url_userinfo_tests {
    use super::strip_url_userinfo;

    #[test]
    fn credentials_are_removed_and_the_rest_kept() {
        assert_eq!(
            strip_url_userinfo("https://x-access-token:abc@github.com/acme/repo.git"),
            "https://github.com/acme/repo.git"
        );
        assert_eq!(
            strip_url_userinfo("https://token@github.com"),
            "https://github.com"
        );
    }

    #[test]
    fn urls_without_credentials_are_untouched() {
        assert_eq!(
            strip_url_userinfo("https://github.com/acme/repo.git"),
            "https://github.com/acme/repo.git"
        );
        assert_eq!(
            strip_url_userinfo("git@github.com:acme/repo.git"),
            "git@github.com:acme/repo.git"
        );
    }
}
