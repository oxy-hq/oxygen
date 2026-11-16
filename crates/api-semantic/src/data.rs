use oxy_app::surface::sql_exec::{
    SQLParams, SemanticQueryResponse, SqlErrorResponse, execute_sql_response,
};
use oxy_app::surface::{EffectiveWorkspaceRole, WorkspaceManagerReadOnly, WorkspacePath};

use axum::extract::{self, Path};
use axum::http::StatusCode;
use oxy_auth::extractor::AuthenticatedUserExtractor;

pub async fn execute_sql(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    EffectiveWorkspaceRole(role): EffectiveWorkspaceRole,
    Path(WorkspacePath {
        workspace_id: _workspace_id,
    }): Path<WorkspacePath>,
    extract::Json(payload): extract::Json<SQLParams>,
) -> Result<extract::Json<SemanticQueryResponse>, (StatusCode, extract::Json<SqlErrorResponse>)> {
    execute_sql_response(&workspace_manager, user.id, role, &payload).await
}

/// Run SQL against one of the workspace's configured databases.
///
/// The workhorse of the API for anything data-shaped. `database` names a
/// connection from the workspace's `config.yml`; `GET /{workspace_id}/databases`
/// lists them. Mounted on BOTH the user-token surface
/// (`/api/{workspace_id}/sql/query`) and the API-key surface
/// (`/external/api/{workspace_id}/sql/query`) — same handler, same body.
///
/// **The response shape depends on `result_format`, and the default is not an
/// object.** With `json` (the default) the body is an array of arrays of
/// strings, **header row first**: `[["id","name"],["1","ada"]]`. With `parquet`
/// it is an object naming a file, and only that variant carries `truncated`.
/// The two are an untagged enum, so a client must not assume a field is there.
#[utoipa::path(
    method(post),
    path = "/{workspace_id}/sql/query",
    params(
        ("workspace_id" = Uuid, Path, description = "Workspace UUID")
    ),
    request_body = SQLParams,
    responses(
        (status = OK, description = "`json` (default): rows as arrays of strings, header row first. `parquet`: an object naming the written file.", body = SemanticQueryResponse, content_type = "application/json"),
        (status = BAD_REQUEST, description = "The query failed; `message` always set, vendor fields when the driver exposes them", body = SqlErrorResponse, content_type = "application/json")
    ),
    security(
        ("ApiKey" = []),
        ("BearerAuth" = [])
    )
)]
pub async fn execute_sql_query(
    workspace: WorkspaceManagerReadOnly,
    user: AuthenticatedUserExtractor,
    role: EffectiveWorkspaceRole,
    path: Path<WorkspacePath>,
    payload: extract::Json<SQLParams>,
) -> Result<extract::Json<SemanticQueryResponse>, (StatusCode, extract::Json<SqlErrorResponse>)> {
    execute_sql(workspace, user, role, path, payload).await
}
