use crate::server::api::middlewares::role_guards::WorkspaceEditor;
use crate::server::api::middlewares::workspace_context::WorkspaceManagerReadOnly;
use crate::server::router::WorkspaceExtractor;
use crate::server::service::task_manager::TASK_MANAGER;
use axum::{
    extract::{self, Path, Query},
    http::StatusCode,
};
use entity::logs;
use entity::prelude::Logs;
use entity::prelude::Threads;
use entity::threads;
use oxy::{
    database::client::establish_connection,
    exec_types::{ReferenceKind, event::SandboxInfo},
};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::ExprTrait;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, Condition, EntityTrait, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect,
    prelude::DateTimeWithTimeZone,
    sea_query::{Expr, extension::postgres::PgExpr},
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Serialize, ToSchema)]
pub struct ThreadItem {
    pub id: String,
    pub title: String,
    pub input: String,
    pub output: String,
    pub source_type: String,
    pub source: String,
    pub created_at: DateTimeWithTimeZone,
    pub references: Vec<ReferenceKind>,
    pub sandbox_info: Option<SandboxInfo>,
    pub is_processing: bool,
}

#[derive(Serialize, ToSchema)]
pub struct ThreadsResponse {
    pub threads: Vec<ThreadItem>,
    pub pagination: PaginationInfo,
}

#[derive(Serialize, ToSchema)]
pub struct PaginationInfo {
    pub page: u64,
    pub limit: u64,
    pub total: u64,
    pub total_pages: u64,
    pub has_next: bool,
    pub has_previous: bool,
}

#[derive(Deserialize, ToSchema)]
pub struct PaginationQuery {
    pub page: Option<u64>,
    pub limit: Option<u64>,
    /// Optional case-insensitive substring filter over thread title + input.
    pub search: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct CreateThreadRequest {
    pub title: String,
    pub input: String,
    pub source: String,
    pub source_type: String,
}

/// Get paginated list of threads for the authenticated user
#[utoipa::path(
    get,
    path = "/{workspace_id}/threads",
    params(
        ("workspace_id" = Uuid, Path, description = "Workspace UUID"),
        ("page" = Option<u64>, Query, description = "Page number (default: 1)"),
        ("limit" = Option<u64>, Query, description = "Items per page (default: 100, max: 100)"),
        ("search" = Option<String>, Query, description = "Case-insensitive title/input filter")
    ),
    responses(
        (status = 200, description = "List of threads with pagination", body = ThreadsResponse),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error")
    ),
    security(
        ("ApiKey" = [])
    ),
    tag = "Threads"
)]
pub async fn get_threads(
    WorkspaceExtractor(project): WorkspaceExtractor,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Query(pagination): Query<PaginationQuery>,
) -> Result<extract::Json<ThreadsResponse>, StatusCode> {
    let connection = establish_connection().await.map_err(|e| {
        tracing::error!("Failed to establish database connection: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // `Ord::max` is spelled out: `ExprTrait` (in scope for the query builders
    // below) blanket-implements a `max` of its own on every `Into<Expr>` type.
    let page = Ord::max(pagination.page.unwrap_or(1), 1);
    let limit = pagination.limit.unwrap_or(100).clamp(1, 100);

    // Shared filter: the user's own non-onboarding threads in this project,
    // optionally narrowed by a case-insensitive search over title + input.
    let mut base = Condition::all()
        .add(threads::Column::UserId.eq(Some(user.id)))
        .add(threads::Column::ProjectId.eq(project.id))
        .add(threads::Column::Source.ne("__onboarding__"));
    if let Some(term) = pagination
        .search
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let pattern = format!("%{term}%");
        base = base.add(
            Condition::any()
                .add(Expr::col(threads::Column::Title).ilike(pattern.clone()))
                .add(Expr::col(threads::Column::Input).ilike(pattern)),
        );
    }

    let total = Threads::find()
        .filter(base.clone())
        .count(&connection)
        .await
        .map_err(|e| {
            tracing::error!("Failed to count threads: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if total == 0 {
        return Ok(extract::Json(ThreadsResponse {
            threads: vec![],
            pagination: PaginationInfo {
                page,
                limit,
                total: 0,
                total_pages: 0,
                has_next: false,
                has_previous: false,
            },
        }));
    }

    let total_pages = total.div_ceil(limit); // Ceiling division for pagination
    let has_next = page < total_pages;
    let has_previous = page > 1;
    let offset = (page - 1) * limit;

    let threads = Threads::find()
        .filter(base)
        .order_by_desc(threads::Column::CreatedAt)
        .offset(offset)
        .limit(limit)
        .all(&connection)
        .await
        .map_err(|e| {
            tracing::error!("Failed to fetch threads: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let thread_items = threads
        .into_iter()
        .map(|t| ThreadItem {
            id: t.id.to_string(),
            title: t.title.clone(),
            input: t.input.clone(),
            output: t.output.clone(),
            source: t.source.clone(),
            source_type: t.source_type.clone(),
            created_at: t.created_at,
            references: serde_json::from_str(&t.references).unwrap_or_default(),
            sandbox_info: t
                .sandbox_info
                .as_ref()
                .and_then(|info| serde_json::from_value(info.clone()).ok()),
            is_processing: t.is_processing,
        })
        .collect();

    Ok(extract::Json(ThreadsResponse {
        threads: thread_items,
        pagination: PaginationInfo {
            page,
            limit,
            total,
            total_pages,
            has_next,
            has_previous,
        },
    }))
}

/// Builds the thread-detail lookup with operator-aware scoping.
///
/// Always scopes by `project_id` — the authoritative tenant boundary already
/// enforced by `WorkspaceExtractor`. For ordinary callers it additionally
/// scopes by `user_id`, so a member can't read another member's thread.
///
/// A Global Owner (`OXY_OWNER`) or Global Admin (`app_admins`) opening a
/// tenant for support/triage (`is_operator`) bypasses the `user_id` filter.
/// The admin explorer deep-links operators straight into any tenant's
/// conversation (see `admin/explorer.rs`) and the workspace gate already
/// synthesizes their membership (see `workspace_context.rs`); without this
/// bypass those deep links 404 even though the operator was authorized into
/// the workspace.
fn scoped_thread_query(
    thread_id: Uuid,
    project_id: Uuid,
    user_id: Uuid,
    is_operator: bool,
) -> sea_orm::Select<Threads> {
    let query = Threads::find_by_id(thread_id).filter(threads::Column::ProjectId.eq(project_id));
    if is_operator {
        query
    } else {
        query.filter(threads::Column::UserId.eq(Some(user_id)))
    }
}

/// Get a specific thread by ID
///
/// Retrieves detailed information about a single thread including its input, output,
/// references, and processing status. The thread must belong to the authenticated
/// user, except for Global Owner / Global Admin operators (support/triage).
#[utoipa::path(
    get,
    path = "/{workspace_id}/threads/{id}",
    params(
        ("workspace_id" = Uuid, Path, description = "Workspace UUID"),
        ("id" = String, Path, description = "Thread ID (UUID)")
    ),
    responses(
        (status = 200, description = "Thread details", body = ThreadItem),
        (status = 400, description = "Invalid thread ID format"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Thread not found"),
        (status = 500, description = "Internal server error")
    ),
    security(
        ("ApiKey" = [])
    ),
    tag = "Threads"
)]
pub async fn get_thread(
    WorkspaceExtractor(project): WorkspaceExtractor,
    Path((_workspace_id, id)): Path<(Uuid, String)>,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
) -> Result<extract::Json<ThreadItem>, StatusCode> {
    let connection = establish_connection().await.map_err(|e| {
        tracing::error!("Failed to establish database connection: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let thread_id = Uuid::parse_str(&id).map_err(|_| StatusCode::BAD_REQUEST)?;

    // SECURITY: scope by user_id AND project_id (workspace) so a member can't
    // read another member's thread or one from a workspace they're not scoped
    // to. project.id is the authoritative tenant boundary (enforced by
    // WorkspaceExtractor).
    //
    // Try the user-scoped lookup first: the overwhelmingly common case is a
    // member opening their own thread, and that path must not pay the operator
    // (`app_admins`) check. Only on a miss do we consider whether the caller is
    // a Global Owner / Global Admin opening another user's thread for
    // support/triage (the admin explorer deep-links them in — see
    // `scoped_thread_query`) and retry without the user_id filter.
    let thread = match scoped_thread_query(thread_id, project.id, user.id, false)
        .one(&connection)
        .await
        .map_err(|e| {
            tracing::error!("Failed to fetch thread {}: {}", thread_id, e);
            StatusCode::INTERNAL_SERVER_ERROR
        })? {
        Some(thread) => thread,
        None => {
            // Falling back to the unscoped query is an operator power, so ask the
            // ring rather than re-deciding what "operator" means here.
            //
            // Unknown standing denies. There is no legacy check beside this one to defer
            // to, so unlike `enforce`, a fail-open here would be a real grant — a blip
            // must not hand anyone another tenant's thread.
            let is_operator = match crate::server::authz::loader::load_platform_facts(
                &connection,
                &crate::server::authz::Caller::from_user(&user),
            )
            .await
            {
                Some(facts) => crate::server::authz::allows(
                    &facts,
                    crate::server::authz::Action::PlatformOps,
                    &crate::server::authz::Resource::platform(),
                ),
                None => false,
            };
            if !is_operator {
                return Err(StatusCode::NOT_FOUND);
            }
            scoped_thread_query(thread_id, project.id, user.id, true)
                .one(&connection)
                .await
                .map_err(|e| {
                    tracing::error!("Failed to fetch thread {} (operator): {}", thread_id, e);
                    StatusCode::INTERNAL_SERVER_ERROR
                })?
                .ok_or(StatusCode::NOT_FOUND)?
        }
    };

    let thread_item = ThreadItem {
        id: thread.id.to_string(),
        title: thread.title,
        input: thread.input,
        output: thread.output,
        source_type: thread.source_type,
        source: thread.source,
        created_at: thread.created_at,
        references: serde_json::from_str(&thread.references).unwrap_or_default(),
        sandbox_info: thread
            .sandbox_info
            .as_ref()
            .and_then(|info| serde_json::from_value(info.clone()).ok()),
        is_processing: thread.is_processing,
    };
    Ok(extract::Json(thread_item))
}

/// Create a new thread
#[utoipa::path(
    post,
    path = "/{workspace_id}/threads",
    params(
        ("workspace_id" = Uuid, Path, description = "Workspace UUID")
    ),
    request_body = CreateThreadRequest,
    responses(
        (status = 200, description = "Thread created successfully", body = ThreadItem),
        (status = 400, description = "Invalid request data"),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error")
    ),
    security(
        ("ApiKey" = [])
    ),
    tag = "Threads"
)]
pub async fn create_thread(
    WorkspaceExtractor(project): WorkspaceExtractor,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    extract::Json(thread_request): extract::Json<CreateThreadRequest>,
) -> Result<extract::Json<ThreadItem>, StatusCode> {
    let connection = establish_connection().await.map_err(|e| {
        tracing::error!("Failed to establish database connection: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let new_thread = entity::threads::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        user_id: ActiveValue::Set(Some(user.id)),
        created_at: ActiveValue::not_set(),
        title: ActiveValue::Set(thread_request.title),
        input: ActiveValue::Set(thread_request.input.clone()),
        output: ActiveValue::Set("".to_string()),
        source_type: ActiveValue::Set(thread_request.source_type),
        source: ActiveValue::Set(thread_request.source),
        references: ActiveValue::Set("[]".to_string()),
        is_processing: ActiveValue::Set(false),
        project_id: ActiveValue::Set(project.id),
        sandbox_info: ActiveValue::Set(None),
    };
    let thread = new_thread.insert(&connection).await.map_err(|e| {
        tracing::error!("Failed to create thread: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let thread_item = ThreadItem {
        id: thread.id.to_string(),
        title: thread.title,
        input: thread.input,
        output: thread.output,
        source_type: thread.source_type,
        source: thread.source,
        created_at: thread.created_at,
        references: serde_json::from_str(&thread.references).unwrap_or_default(),
        sandbox_info: thread
            .sandbox_info
            .as_ref()
            .and_then(|info| serde_json::from_value(info.clone()).ok()),
        is_processing: thread.is_processing,
    };
    Ok(extract::Json(thread_item))
}

/// Delete a specific thread
#[utoipa::path(
    delete,
    path = "/{workspace_id}/threads/{id}",
    params(
        ("workspace_id" = Uuid, Path, description = "Workspace UUID"),
        ("id" = String, Path, description = "Thread ID (UUID)")
    ),
    responses(
        (status = 200, description = "Thread deleted successfully"),
        (status = 400, description = "Invalid thread ID format"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Thread not found"),
        (status = 500, description = "Internal server error")
    ),
    security(
        ("ApiKey" = [])
    ),
    tag = "Threads"
)]
pub async fn delete_thread(
    WorkspaceExtractor(project): WorkspaceExtractor,
    Path((_workspace_id, id)): Path<(Uuid, String)>,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
) -> Result<StatusCode, StatusCode> {
    let connection = establish_connection().await.map_err(|e| {
        tracing::error!("Failed to establish database connection: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let thread_id = Uuid::parse_str(&id).map_err(|e| {
        tracing::warn!("Invalid thread ID format '{}': {}", id, e);
        StatusCode::BAD_REQUEST
    })?;

    // SECURITY: scope by user_id AND project_id (workspace) to prevent
    // cross-workspace deletion. See get_thread.
    let thread = Threads::find_by_id(thread_id)
        .filter(threads::Column::UserId.eq(Some(user.id)))
        .filter(threads::Column::ProjectId.eq(project.id))
        .one(&connection)
        .await
        .map_err(|e| {
            tracing::error!(
                "Database error finding thread {} for deletion by user {}: {}",
                thread_id,
                user.id,
                e
            );
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if let Some(thread) = thread {
        if thread.is_processing {
            tracing::warn!(
                "Attempted to delete thread {} that is currently being processed",
                thread_id
            );
            return Err(StatusCode::CONFLICT);
        }

        let active_thread: entity::threads::ActiveModel = thread.into();
        active_thread.delete(&connection).await.map_err(|e| {
            tracing::error!(
                "Failed to delete thread {} for user {}: {}",
                thread_id,
                user.id,
                e
            );
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

        tracing::info!(
            "Successfully deleted thread {} for user {}",
            thread_id,
            user.id
        );
    } else {
        tracing::warn!(
            "Thread {} not found or doesn't belong to user {}",
            thread_id,
            user.id
        );
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(StatusCode::OK)
}

fn remove_all_files_in_dir<P: AsRef<std::path::Path>>(dir: P) {
    if dir.as_ref().exists()
        && let Ok(entries) = std::fs::read_dir(&dir)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

/// Delete all threads for the authenticated user
#[utoipa::path(
    delete,
    path = "/{workspace_id}/threads",
    params(
        ("workspace_id" = Uuid, Path, description = "Workspace UUID")
    ),
    responses(
        (status = 200, description = "All threads deleted successfully"),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error")
    ),
    security(
        ("ApiKey" = [])
    ),
    tag = "Threads"
)]
pub async fn delete_all_threads(
    _: WorkspaceEditor,
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    WorkspaceExtractor(project): WorkspaceExtractor,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
) -> Result<StatusCode, StatusCode> {
    let connection = establish_connection().await.map_err(|e| {
        tracing::error!("Failed to establish database connection: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Threads::delete_many()
        .filter(threads::Column::UserId.eq(Some(user.id)))
        .filter(threads::Column::ProjectId.eq(project.id))
        .exec(&connection)
        .await
        .map_err(|e| {
            tracing::error!("Failed to delete threads for user {}: {}", user.id, e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Chart cleanup is node-local and best-effort, and deliberately does NOT
    // pin this route to the ide: deleting threads is a Postgres operation and
    // must stay available on every replica.
    //
    // Two consequences worth knowing rather than discovering. The charts
    // directory only exists on a node that owns a disk, so on a serve replica
    // this is a no-op — `remove_all_files_in_dir` starts with `dir.exists()`
    // and returns. And charts are mirrored to S3 (`runtime_artifact::chart_key`)
    // to survive the round-robin, so even on the ide the local sweep leaves the
    // mirrored copies behind. Thread rows are gone either way; what lingers is
    // orphaned image data.
    //
    // The durable fix is deleting through the same store that writes them,
    // rather than reaching for a directory. Tracked separately — doing it here
    // would widen a thread-deletion change into an artifact-lifecycle one.
    //
    // Best-effort means best-effort: the rows above are already committed, so a
    // node without a charts dir must not turn a successful deletion into a 500.
    // `charts_dir()` resolves through `runtime_state_dir()`, whose working copy
    // is an `Option`, so it answers on a replica too — and the sweep is a no-op
    // there because `remove_all_files_in_dir` starts with `dir.exists()`. That
    // is the whole reason this route does not need a working copy to delete
    // Postgres rows.
    //
    // "No-op" only holds because `runtime_state_dir` now declines the
    // workspace-root fallback when the root is not a directory. It used to take
    // that fallback and CREATE it, so the sweep reached `dir.exists()` on a
    // directory it had just made — and left the workspace root behind.
    let charts_dir = workspace_manager.config_manager.charts_dir();
    // The dir-walk + per-file removals are blocking fs calls; run them on a
    // blocking thread so they don't stall a Tokio worker.
    let _ = tokio::task::spawn_blocking(move || remove_all_files_in_dir(charts_dir)).await;

    Ok(StatusCode::OK)
}

/// Stop a running thread
#[utoipa::path(
    post,
    path = "/{workspace_id}/threads/{id}/stop",
    params(
        ("workspace_id" = Uuid, Path, description = "Workspace UUID"),
        ("id" = String, Path, description = "Thread ID (UUID)")
    ),
    responses(
        (status = 200, description = "Thread stopped successfully"),
        (status = 400, description = "Invalid thread ID format"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Thread not found"),
        (status = 500, description = "Internal server error")
    ),
    security(
        ("ApiKey" = [])
    ),
    tag = "Threads"
)]
pub async fn stop_thread(
    WorkspaceExtractor(project): WorkspaceExtractor,
    Path((_workspace_id, id)): Path<(Uuid, String)>,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
) -> Result<StatusCode, StatusCode> {
    let thread_id = Uuid::parse_str(&id).map_err(|_| StatusCode::BAD_REQUEST)?;

    // Verify the user owns this thread AND that it belongs to the workspace
    // in the URL. See SECURITY comment on get_thread.
    let connection = establish_connection().await.map_err(|e| {
        tracing::error!("Failed to establish database connection: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let thread = Threads::find_by_id(thread_id)
        .filter(threads::Column::UserId.eq(Some(user.id)))
        .filter(threads::Column::ProjectId.eq(project.id))
        .one(&connection)
        .await
        .map_err(|e| {
            tracing::error!("Failed to fetch thread {} for stop: {}", thread_id, e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if thread.is_none() {
        return Err(StatusCode::NOT_FOUND);
    }

    let thread = thread.unwrap();
    let mut thread_model: entity::threads::ActiveModel = thread.clone().into();
    thread_model.is_processing = ActiveValue::Set(false);

    if let Err(e) = thread_model.update(&connection).await {
        tracing::error!(
            "Failed to unlock thread {} during stop operation: {}",
            thread.id,
            e
        );
        // Continue with cancellation even if update fails
    }

    match TASK_MANAGER.cancel_task(thread_id).await {
        Ok(true) => {
            TASK_MANAGER.remove_task(thread_id).await;
            tracing::info!("Successfully stopped and unlocked thread {}", thread_id);
            Ok(StatusCode::OK)
        }
        Ok(false) => {
            tracing::warn!("Task not found for thread {}", thread_id);
            Err(StatusCode::NOT_FOUND)
        }
        Err(e) => {
            tracing::error!("Error cancelling task for thread {}: {}", thread_id, e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

#[derive(Deserialize, ToSchema)]
pub struct BulkDeleteThreadsRequest {
    pub thread_ids: Vec<String>,
}

/// Bulk delete multiple threads by their IDs
///
/// Efficiently deletes multiple threads in a single operation. All threads must belong
/// to the authenticated user. Invalid UUIDs will result in a 400 Bad Request error.
/// Empty thread_ids array will also return 400 Bad Request.
#[utoipa::path(
    post,
    path = "/{workspace_id}/threads/bulk-delete",
    params(
        ("workspace_id" = Uuid, Path, description = "Workspace UUID")
    ),
    request_body = BulkDeleteThreadsRequest,
    responses(
        (status = 200, description = "Threads deleted successfully"),
        (status = 400, description = "Invalid request data or thread IDs"),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error")
    ),
    security(
        ("ApiKey" = [])
    ),
    tag = "Threads"
)]
pub async fn bulk_delete_threads(
    _: WorkspaceEditor,
    WorkspaceExtractor(project): WorkspaceExtractor,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    extract::Json(request): extract::Json<BulkDeleteThreadsRequest>,
) -> Result<StatusCode, StatusCode> {
    let connection = establish_connection().await.map_err(|e| {
        tracing::error!("Failed to establish database connection: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let mut thread_uuids = Vec::new();
    for thread_id in request.thread_ids {
        let uuid = Uuid::parse_str(&thread_id).map_err(|_| StatusCode::BAD_REQUEST)?;
        thread_uuids.push(uuid);
    }

    if thread_uuids.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    Threads::delete_many()
        .filter(
            threads::Column::UserId
                .eq(Some(user.id))
                .and(threads::Column::ProjectId.eq(project.id))
                .and(threads::Column::Id.is_in(thread_uuids)),
        )
        .exec(&connection)
        .await
        .map_err(|e| {
            tracing::error!("Failed to bulk delete threads: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(StatusCode::OK)
}

#[derive(Serialize, ToSchema)]
pub struct LogItem {
    pub id: String,
    pub user_id: String,
    pub prompts: String,
    pub thread_id: String,
    pub log: serde_json::Value,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
    pub thread: Option<ThreadInfo>,
}

#[derive(Serialize, ToSchema)]
pub struct ThreadInfo {
    pub title: String,
    pub input: String,
    pub output: String,
    pub source: String,
    pub source_type: String,
    pub is_processing: bool,
}

#[derive(Serialize, ToSchema)]
pub struct LogsResponse {
    pub logs: Vec<LogItem>,
}

/// Get execution logs with associated thread information
///
/// Retrieves all execution logs for the authenticated user, including associated thread
/// details such as title, input, output, and processing status. Logs are ordered by
/// creation time in descending order (most recent first).
#[utoipa::path(
    get,
    path = "/logs",
    responses(
        (status = 200, description = "List of logs with thread information", body = LogsResponse),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error")
    ),
    security(
        ("ApiKey" = [])
    ),
    tag = "Threads"
)]
pub async fn get_logs(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
) -> Result<extract::Json<LogsResponse>, StatusCode> {
    let connection = establish_connection().await.map_err(|e| {
        tracing::error!("Failed to establish database connection: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let logs_with_threads = Logs::find()
        .filter(logs::Column::UserId.eq(user.id))
        .find_also_related(Threads)
        .order_by_desc(logs::Column::CreatedAt)
        .all(&connection)
        .await
        .map_err(|e| {
            tracing::error!("Failed to fetch logs for user {}: {}", user.id, e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let log_items = logs_with_threads
        .into_iter()
        .map(|(log, thread)| LogItem {
            id: log.id.to_string(),
            user_id: log.user_id.to_string(),
            prompts: log.prompts,
            thread_id: log.thread_id.to_string(),
            log: log.log,
            created_at: log.created_at,
            updated_at: log.updated_at,
            thread: thread.map(|t| ThreadInfo {
                title: t.title,
                input: t.input,
                output: t.output,
                source: t.source,
                source_type: t.source_type,
                is_processing: t.is_processing,
            }),
        })
        .collect();

    Ok(extract::Json(LogsResponse { logs: log_items }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{DatabaseBackend, QueryTrait};

    // A `<col>" = $` fragment only appears in the WHERE clause: the SELECT list
    // emits `"threads"."<col>",` / ` FROM`, never `= $`. Matching it therefore
    // asserts on the actual filter rather than the column merely appearing in
    // the (always-present) SELECT list — see the entity's `user_id` column.
    const USER_FILTER: &str = "user_id\" = $";
    const PROJECT_FILTER: &str = "project_id\" = $";

    // The `.as_str()` on every `contains` below is load-bearing: `use super::*`
    // pulls in `PgExpr`, which SeaQuery 1.0 blanket-implements for everything
    // convertible into an `Expr` — `String` included. Its `contains` (the
    // Postgres JSON operator) wins method resolution on a `String` receiver;
    // through `&str` the inherent `str::contains` wins.
    fn thread_query_sql(is_operator: bool) -> String {
        scoped_thread_query(Uuid::nil(), Uuid::nil(), Uuid::nil(), is_operator)
            .build(DatabaseBackend::Postgres)
            .sql
    }

    #[test]
    fn non_operator_query_scopes_by_user_and_project() {
        // Regression: an ordinary caller must stay scoped to their own
        // threads within the workspace.
        let sql = thread_query_sql(false);
        assert!(
            sql.as_str().contains(PROJECT_FILTER),
            "must filter by project_id: {sql}"
        );
        assert!(
            sql.as_str().contains(USER_FILTER),
            "must filter by user_id: {sql}"
        );
    }

    #[test]
    fn operator_query_drops_user_filter_keeps_project() {
        // Regression for the admin-explorer 404: a Global Owner / Global Admin
        // opening another user's thread must NOT be filtered by user_id (the
        // thread belongs to a different user), but MUST still be scoped to the
        // workspace tenant boundary.
        let sql = thread_query_sql(true);
        assert!(
            sql.as_str().contains(PROJECT_FILTER),
            "operator query must still filter by project_id: {sql}"
        );
        assert!(
            !sql.as_str().contains(USER_FILTER),
            "operator query must NOT filter by user_id: {sql}"
        );
    }
}
