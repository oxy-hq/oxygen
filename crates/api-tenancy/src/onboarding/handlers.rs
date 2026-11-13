use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use oxy::config::ResolveWorkspaceFile;
use oxy::github::{GitHubClient, github_token_for_namespace};
use oxy::service::secret_manager::SecretManagerService;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_project::{copy_airhouse_demo_files_to, copy_demo_files_to};
use tracing::{error, info};
use uuid::Uuid;

use oxy_app::surface::role_guards::{OrgWorkspaceCreator, WorkspaceAdmin};
use oxy_app::surface::{WorkspaceManagerReadOnly, WorkspacePath};
use oxy_app_core::AppState;

use super::dto::*;
use super::github_import::{ImportRequest, place_import, spawn_clone};
use super::ops::*;

/// POST /orgs/{org_id}/onboarding/demo — copy embedded demo workspace files.
pub async fn setup_demo(
    OrgWorkspaceCreator(ctx): OrgWorkspaceCreator,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    body: Option<Json<DemoSetupRequest>>,
) -> Result<Json<OnboardingResult>, (StatusCode, String)> {
    let req = body.map(|b| b.0).unwrap_or_default();

    let workspace_id = Uuid::new_v4();
    let project_dir = resolve_project_dir(workspace_id).map_err(|(status, msg)| {
        error!("{}", msg);
        (status, msg)
    })?;

    // A cloud role keeps the demo's data in the workspace's Airhouse: the
    // checkout gets no `.db/`, and `local` is `type: airhouse_managed`.
    let in_airhouse =
        super::airhouse::data_in_airhouse(oxy_app_core::process_role::current_process_role());
    let copied = if in_airhouse {
        copy_airhouse_demo_files_to(&project_dir)
            .await
            .map_err(|e| e.to_string())
    } else {
        copy_demo_files_to(&project_dir)
            .await
            .map_err(|e| e.to_string())
    };
    if let Err(e) = copied {
        error!("Failed to copy demo workspace files: {e}");
        let _ = std::fs::remove_dir_all(&project_dir);
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to copy demo workspace files: {e}"),
        ));
    }

    info!("Demo workspace files copied to {:?}", project_dir);

    let display_name = match req.name.as_deref() {
        Some(n) => n.to_string(),
        None => unique_display_name("Demo workspace", Some(ctx.org.id))
            .await
            .map_err(|(s, m)| {
                error!("{}", m);
                (s, m)
            })?,
    };
    let workspace_id = match register_project(
        &project_dir,
        &display_name,
        workspace_id,
        Some(user.id),
        Some(ctx.org.id),
        entity::workspaces::WorkspaceStatus::Ready,
        None,
    )
    .await
    {
        Ok(id) => id,
        Err((status, msg)) => {
            error!("{}", msg);
            let _ = std::fs::remove_dir_all(&project_dir);
            return Err((status, msg));
        }
    };

    if in_airhouse {
        // Registering gave the workspace its tenant when it could; the load
        // provisions it if that did not happen.
        let db = oxy::database::client::establish_connection()
            .await
            .map_err(|e| {
                let msg = format!("Database connection failed: {e}");
                error!("{}", msg);
                (StatusCode::INTERNAL_SERVER_ERROR, msg)
            })?;
        let importer = airhouse::import::TenantImporter::new(db);
        if let Err(e) = super::airhouse::load_demo_dataset(&importer, workspace_id).await {
            let msg = format!(
                "The demo workspace was created, but its data could not be loaded into \
                 Airhouse: {e}"
            );
            error!(%workspace_id, "{}", msg);
            // Shown on the workspace, as a failed GitHub import is.
            if let Err(status_err) = update_workspace_status(
                workspace_id,
                entity::workspaces::WorkspaceStatus::Failed,
                Some(msg.clone()),
            )
            .await
            {
                error!(%workspace_id, "could not mark the demo workspace failed: {status_err}");
            }
            return Err((super::airhouse::import_status(&e), msg));
        }
    }

    Ok(Json(OnboardingResult {
        workspace_type: "demo".to_string(),
        workspace_id,
    }))
}

/// POST /orgs/{org_id}/onboarding/new — create a Ready blank workspace (a minimal
/// `config.yml`). The same implementation every new org's `Default` workspace
/// comes from: `oxy_app`'s `workspace_provisioning::create_blank_workspace`.
pub async fn setup_new(
    OrgWorkspaceCreator(ctx): OrgWorkspaceCreator,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    body: Option<Json<NewSetupRequest>>,
) -> Result<Json<OnboardingResult>, (StatusCode, String)> {
    use crate::workspace_provisioning::{BlankWorkspace, WorkspaceName, create_blank_workspace};

    let req = body.map(|b| b.0).unwrap_or_default();
    let name = match req.name.as_deref() {
        Some(name) => WorkspaceName::Exact(name),
        None => WorkspaceName::UniqueFrom("New workspace"),
    };
    let spec = BlankWorkspace {
        org_id: ctx.org.id,
        created_by: user.id,
        name,
    };
    let db = oxy::database::client::establish_connection()
        .await
        .map_err(|e| {
            let msg = format!("Database connection failed: {e}");
            error!("{}", msg);
            (StatusCode::INTERNAL_SERVER_ERROR, msg)
        })?;
    // A plain connection autocommits, so the row is already durable here.
    let staged = create_blank_workspace(&db, spec).await.map_err(|e| {
        error!("{}", e);
        <(StatusCode, String)>::from(e)
    })?;
    let workspace_id = staged.finish().await;

    Ok(Json(OnboardingResult {
        workspace_type: "new".to_string(),
        workspace_id,
    }))
}

/// POST /orgs/{org_id}/onboarding/github — connect a GitHub repository to the org's
/// workspace and clone it in the background. The org's blank workspace is adopted
/// (same id); an org with none gets one; an org whose workspace already has a
/// repository is refused `409 org_has_workspace`. The row reads Cloning at once;
/// the clone runs asynchronously so large repositories don't hit the global
/// request timeout. See `github_import`.
pub async fn setup_github(
    OrgWorkspaceCreator(ctx): OrgWorkspaceCreator,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    axum::Json(req): axum::Json<GitHubSetupRequest>,
) -> Result<Json<OnboardingResult>, (StatusCode, String)> {
    // Verify the namespace belongs to the caller's org — unconditional now that the org is
    // always available from the path. Closes the cross-org namespace bypass (security #2).
    let ns = {
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
        let db = oxy::database::client::establish_connection()
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {e}")))?;
        let ns = entity::git_namespaces::Entity::find()
            .filter(entity::git_namespaces::Column::Id.eq(req.namespace_id))
            .one(&db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {e}")))?
            .ok_or((StatusCode::NOT_FOUND, "Namespace not found".to_string()))?;
        if ns.org_id != Some(ctx.org.id) {
            return Err((
                StatusCode::FORBIDDEN,
                "Namespace does not belong to this org".to_string(),
            ));
        }
        ns
    };

    let token = github_token_for_namespace(&ns).await.map_err(|e| {
        let msg = format!(
            "Failed to load token from git namespace {}: {}",
            req.namespace_id, e
        );
        error!("{}", msg);
        (StatusCode::INTERNAL_SERVER_ERROR, msg)
    })?;

    let client = GitHubClient::from_token(token.clone()).map_err(|e| {
        let msg = format!("Failed to create GitHub client: {}", e);
        error!("{}", msg);
        (StatusCode::INTERNAL_SERVER_ERROR, msg)
    })?;

    let repo = client.get_repository(req.repo_id).await.map_err(|e| {
        let msg = format!("Failed to get GitHub repository {}: {}", req.repo_id, e);
        error!("{}", msg);
        (StatusCode::NOT_FOUND, msg)
    })?;

    // Validate the subdirectory before touching the filesystem.
    let subdir = req
        .subdir
        .as_deref()
        .map(parse_subdir)
        .transpose()
        .map_err(|(status, msg)| {
            error!("{}", msg);
            (status, msg)
        })?
        .flatten();

    let placed = place_import(ImportRequest {
        org_id: ctx.org.id,
        user_id: user.id,
        namespace_id: req.namespace_id,
        repo: &repo,
        subdir: subdir.as_deref(),
        name: req.name.as_deref(),
    })
    .await?;
    let workspace_id = placed.workspace_id;
    spawn_clone(placed, repo.clone_url.clone(), req.branch.clone(), token);

    Ok(Json(OnboardingResult {
        workspace_type: "github".to_string(),
        workspace_id,
    }))
}

/// GET /{workspace_id}/onboarding-readiness — check which LLM API keys are needed by
/// the workspace's config.yml. Runs behind workspace_middleware so access to the
/// workspace is already verified.
pub async fn onboarding_readiness(
    State(app_state): State<AppState>,
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    Path(WorkspacePath { workspace_id }): Path<WorkspacePath>,
) -> Json<ReadinessResponse> {
    // Route is under /{workspace_id}/ behind workspace_middleware, which has
    // already enforced org membership. No separate access check needed.
    //
    // Presence is resolved exactly as the runtime resolves the key — DB-only in
    // cloud (an env var on the server must not mask a genuinely-missing
    // workspace secret), DB + env fallback in local. Checking `std::env` alone
    // reported every cloud-stored key as missing; see `secret_is_set`, which
    // `github_setup` shares so the two checks can't drift.
    let config_manager = &workspace_manager.config_manager;
    let db_secret_manager = SecretManagerService::new(workspace_id);
    let is_local = app_state.mode.is_local();

    // Deduplicate by var name — two models sharing a key_var only surface once.
    let mut seen = std::collections::HashSet::new();
    let mut llm_keys_present = Vec::new();
    let mut llm_keys_missing = Vec::new();

    for model in config_manager.models() {
        let Some(key) = model.key_var() else {
            continue;
        };
        if !seen.insert(key.to_string()) {
            continue;
        }
        if secret_is_set(
            &workspace_manager.secrets_manager,
            &db_secret_manager,
            is_local,
            key,
        )
        .await
        {
            llm_keys_present.push(key.to_string());
        } else {
            llm_keys_missing.push(key.to_string());
        }
    }

    let has_llm_key = !llm_keys_present.is_empty();

    Json(ReadinessResponse {
        has_llm_key,
        llm_keys_present,
        llm_keys_missing,
    })
}

/// POST /{workspace_id}/onboarding/test-llm-key — verify an LLM API key
/// against the provider before the user advances past the key-entry step in
/// onboarding.
///
/// Provider-specific probes (URLs, auth headers, status interpretation)
/// live in the corresponding `oxy-{provider}` infrastructure crate; this
/// handler only owns input validation, dispatch, and response shaping. We
/// do not persist the key here — the caller still owns saving it as a
/// project secret on success. Decoupling validation from persistence keeps
/// this endpoint reusable from a Settings "Test key" affordance later.
pub async fn test_llm_key(
    Path(WorkspacePath { workspace_id: _ }): Path<WorkspacePath>,
    AuthenticatedUserExtractor(_user): AuthenticatedUserExtractor,
    Json(req): Json<TestLlmKeyRequest>,
) -> Json<TestLlmKeyResponse> {
    let api_key = req.api_key.trim();
    if api_key.is_empty() {
        return Json(TestLlmKeyResponse {
            success: false,
            message: Some("API key is empty.".to_string()),
        });
    }

    match oxy_llm::validate_provider_key(&req.provider, api_key).await {
        Ok(()) => Json(TestLlmKeyResponse {
            success: true,
            message: None,
        }),
        Err(err) => Json(TestLlmKeyResponse {
            success: false,
            message: Some(err.user_message()),
        }),
    }
}

/// GET /{workspace_id}/onboarding/github-setup — return the setup work needed
/// before a GitHub-imported workspace can be queried. See `GithubSetupResponse`.
pub async fn github_setup(
    State(app_state): State<AppState>,
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    Path(WorkspacePath { workspace_id }): Path<WorkspacePath>,
    AuthenticatedUserExtractor(_user): AuthenticatedUserExtractor,
) -> Result<Json<GithubSetupResponse>, (StatusCode, String)> {
    let config_manager = &workspace_manager.config_manager;
    // Cloud mode uses a DB-only SecretManagerService so that env vars set on
    // the server don't silently suppress the "add key" prompt.
    let db_secret_manager = SecretManagerService::new(workspace_id);
    // Local mode uses the workspace's SecretsManager (DB + env fallback) so
    // keys set in .env are treated as present, matching runtime behaviour.
    let is_local = app_state.mode.is_local();

    // ── LLM keys ──────────────────────────────────────────────────────────────
    // Deduplicate by var name — two models sharing a key_var only surface once.
    let mut seen_llm_keys = std::collections::HashSet::new();
    let mut missing_llm_key_vars: Vec<GithubSetupKeyVar> = Vec::new();
    for model in config_manager.models() {
        let Some(key_var) = model.key_var() else {
            continue;
        };
        if !seen_llm_keys.insert(key_var.to_string()) {
            continue;
        }
        let is_set = secret_is_set(
            &workspace_manager.secrets_manager,
            &db_secret_manager,
            is_local,
            key_var,
        )
        .await;
        if is_set {
            continue;
        }
        missing_llm_key_vars.push(GithubSetupKeyVar {
            var_name: key_var.to_string(),
            vendor: vendor_label_for_model(model),
            sample_model_name: Some(model.name().to_string()),
        });
    }

    // ── Warehouses ───────────────────────────────────────────────────────────
    let mut warehouses: Vec<GithubSetupWarehouse> = Vec::new();
    for database in config_manager.list_databases() {
        let vars = collect_warehouse_vars(&database);
        let mut missing_vars: Vec<GithubSetupMissingVar> = Vec::new();
        for var in vars {
            let is_set = secret_is_set(
                &workspace_manager.secrets_manager,
                &db_secret_manager,
                is_local,
                &var.var_name,
            )
            .await;
            if is_set {
                continue;
            }
            missing_vars.push(var);
        }
        if !missing_vars.is_empty() {
            warehouses.push(GithubSetupWarehouse {
                name: database.name.clone(),
                dialect: database.dialect(),
                missing_vars,
            });
        }
    }

    // All models, set or not — `missing_llm_key_vars` dedupes by var and
    // can't be used to resolve a specific model -> key_var.
    let models: Vec<GithubSetupModel> = config_manager
        .models()
        .iter()
        .map(|model| GithubSetupModel {
            name: model.name().to_string(),
            key_var: model.key_var().map(|k| k.to_string()),
        })
        .collect();

    Ok(Json(GithubSetupResponse {
        missing_llm_key_vars,
        warehouses,
        models,
    }))
}

/// POST /workspaces/{id}/onboarding/upload-warehouse-files — take uploaded
/// CSV/Parquet files as the workspace's data.
///
/// On a cloud role each file is loaded into the workspace's Airhouse
/// (`destination: airhouse`, see `super::airhouse::upload_into_airhouse`), and
/// the client then registers `type: airhouse_managed`. That needs no working
/// copy, so a serve pod answers it.
///
/// On `OXY_ROLE=all` the files are streamed into `<workspace_root>/<subdir>/`
/// (default `.db/`, `destination: workspace_files`), and the client then
/// submits a warehouse config with `file_search_path = subdir`, which the
/// connector scans for the just-uploaded files.
pub async fn upload_warehouse_files(
    // An upload exists only to feed the next step, `POST /databases` with
    // `dataset` pointed at it, which is `WorkspaceAdmin`: an Editor who could
    // upload could never use the files. Same guard as that step.
    _: WorkspaceAdmin,
    // `ReadOnly`, not `<WorkingCopy>`: this crate builds a `Router<AppState>`
    // and the working-copy extractor resolves only from `IdeState`.
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    Path(WorkspacePath { workspace_id }): Path<WorkspacePath>,
    AuthenticatedUserExtractor(_user): AuthenticatedUserExtractor,
    mut multipart: axum::extract::Multipart,
) -> Result<(StatusCode, Json<UploadWarehouseFilesResponse>), (StatusCode, String)> {
    if super::airhouse::data_in_airhouse(oxy_app_core::process_role::current_process_role()) {
        let db = oxy::database::client::establish_connection()
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Database connection failed: {e}"),
                )
            })?;
        let importer = airhouse::import::TenantImporter::new(db);
        return super::airhouse::upload_into_airhouse(&importer, workspace_id, multipart).await;
    }

    let workspace_root = workspace_manager
        .config_manager
        .workspace_file_resolver()
        .ok_or_else(|| {
            tracing::error!("warehouse upload refused: no working copy on this node");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "this instance holds no working copy for the workspace".to_string(),
            )
        })?
        .workspace_path()
        .to_path_buf();

    let mut subdir: Option<std::path::PathBuf> = None;
    let mut files_out: Vec<String> = Vec::new();
    let mut skipped: Vec<SkippedUpload> = Vec::new();
    // Processed lazily on the first `file` field so callers can send `subdir`
    // either before or after the files in the form.
    let mut target_dir: Option<std::path::PathBuf> = None;
    let mut total_bytes: u64 = 0;
    let mut files_seen: usize = 0;

    while let Some(field) = multipart.next_field().await.map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("Failed to parse multipart body: {e}"),
        )
    })? {
        let field_name = field.name().unwrap_or("").to_string();
        match field_name.as_str() {
            "subdir" => {
                let raw = field.text().await.map_err(|e| {
                    (
                        StatusCode::BAD_REQUEST,
                        format!("Failed to read subdir field: {e}"),
                    )
                })?;
                subdir = parse_subdir(&raw)?;
            }
            "file" => {
                files_seen += 1;
                if files_seen > MAX_FILES_PER_REQUEST {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        format!("Too many files in upload request (max {MAX_FILES_PER_REQUEST})"),
                    ));
                }
                // Resolve the target dir once, on the first file.
                if target_dir.is_none() {
                    let dir_rel = subdir
                        .clone()
                        .unwrap_or_else(|| std::path::PathBuf::from(DEFAULT_UPLOAD_SUBDIR));
                    let dir_abs = workspace_root.join(&dir_rel);
                    tokio::fs::create_dir_all(&dir_abs).await.map_err(|e| {
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            format!("Failed to create upload directory: {e}"),
                        )
                    })?;
                    target_dir = Some(dir_abs);
                }

                let raw_name = field.file_name().unwrap_or("").to_string();
                let sanitised = match sanitise_upload_filename(&raw_name) {
                    Ok(n) => n,
                    Err(reason) => {
                        skipped.push(SkippedUpload {
                            name: raw_name,
                            reason: reason.to_string(),
                        });
                        // Drain the field so the stream stays in sync.
                        drain_field(field).await?;
                        continue;
                    }
                };

                if !has_supported_extension(&sanitised) {
                    skipped.push(SkippedUpload {
                        name: sanitised,
                        reason: "unsupported_extension".to_string(),
                    });
                    drain_field(field).await?;
                    continue;
                }

                let dir_abs = target_dir.as_ref().expect("target_dir set above");
                let final_path = dir_abs.join(&sanitised);
                if tokio::fs::try_exists(&final_path).await.unwrap_or(false) {
                    return Err((
                        StatusCode::CONFLICT,
                        format!(
                            "A file named '{sanitised}' already exists in the upload directory. \
                            Rename or remove the existing file before re-uploading."
                        ),
                    ));
                }

                let tmp_path = dir_abs.join(format!("{sanitised}.upload-tmp"));
                let bytes_written =
                    stream_field_to_file(field, &tmp_path, MAX_FILE_BYTES, &mut total_bytes)
                        .await?;

                tracing::info!(
                    workspace_id = %workspace_id,
                    filename = %sanitised,
                    bytes = bytes_written,
                    "Uploaded warehouse file"
                );

                tokio::fs::rename(&tmp_path, &final_path)
                    .await
                    .map_err(|e| {
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            format!("Failed to finalise upload '{sanitised}': {e}"),
                        )
                    })?;

                let rel = target_dir
                    .as_ref()
                    .and_then(|d| d.strip_prefix(&workspace_root).ok())
                    .map(|r| r.join(&sanitised))
                    .unwrap_or_else(|| std::path::PathBuf::from(&sanitised));
                files_out.push(rel.to_string_lossy().replace('\\', "/"));
            }
            _ => {
                // Ignore unknown fields, but drain them to keep the stream in sync.
                drain_field(field).await?;
            }
        }
    }

    if files_out.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "No supported files uploaded. Upload at least one .csv or .parquet file.".to_string(),
        ));
    }

    let resolved_subdir = subdir
        .unwrap_or_else(|| std::path::PathBuf::from(DEFAULT_UPLOAD_SUBDIR))
        .to_string_lossy()
        .replace('\\', "/");

    Ok((
        StatusCode::CREATED,
        Json(UploadWarehouseFilesResponse {
            subdir: resolved_subdir,
            files: files_out,
            skipped,
            destination: UploadDestination::WorkspaceFiles,
            tables: Vec::new(),
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitise_rejects_empty() {
        assert_eq!(sanitise_upload_filename(""), Err("empty_filename"));
        assert_eq!(sanitise_upload_filename("   "), Err("empty_filename"));
    }

    #[test]
    fn sanitise_rejects_separators() {
        assert_eq!(
            sanitise_upload_filename("foo/bar.csv"),
            Err("path_separator_in_filename")
        );
        assert_eq!(
            sanitise_upload_filename("foo\\bar.csv"),
            Err("path_separator_in_filename")
        );
    }

    #[test]
    fn sanitise_rejects_traversal() {
        // `.` / `..` are caught by the explicit invalid-filename guard, which
        // runs before the hidden-file check.
        assert_eq!(sanitise_upload_filename(".."), Err("invalid_filename"));
        assert_eq!(sanitise_upload_filename("."), Err("invalid_filename"));
    }

    #[test]
    fn sanitise_rejects_hidden() {
        assert_eq!(
            sanitise_upload_filename(".DS_Store"),
            Err("hidden_filename")
        );
        assert_eq!(
            sanitise_upload_filename(".env.parquet"),
            Err("hidden_filename")
        );
    }

    #[test]
    fn sanitise_rejects_null_byte() {
        assert_eq!(
            sanitise_upload_filename("foo\0.csv"),
            Err("null_byte_in_filename")
        );
    }

    #[test]
    fn sanitise_accepts_plain_filenames() {
        assert_eq!(
            sanitise_upload_filename("orders.csv"),
            Ok("orders.csv".to_string())
        );
        assert_eq!(
            sanitise_upload_filename("  Orders-2024.Parquet  "),
            Ok("Orders-2024.Parquet".to_string())
        );
    }

    #[test]
    fn supported_extension_accepts_csv_parquet_case_insensitive() {
        assert!(has_supported_extension("foo.csv"));
        assert!(has_supported_extension("foo.CSV"));
        assert!(has_supported_extension("foo.Parquet"));
        assert!(has_supported_extension("foo.PARQUET"));
    }

    #[test]
    fn supported_extension_rejects_others() {
        assert!(!has_supported_extension("foo"));
        assert!(!has_supported_extension("foo.txt"));
        assert!(!has_supported_extension("foo.json"));
        assert!(!has_supported_extension("notes.md"));
    }
}
