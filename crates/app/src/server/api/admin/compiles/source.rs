//! Where an operator-requested compile reads the workspace from.

use axum::http::StatusCode;
use axum::response::Response;
use sea_orm::{DatabaseConnection, EntityTrait};
use serde::Deserialize;
use uuid::Uuid;

use super::{error_body, insert_run_and_enqueue_compile};
use crate::server::compile_git;
use crate::server::compile_request::{MainSource, Refusal, main_source};

/// `source` on `POST /admin/compiles/run`.
#[derive(Deserialize, Debug, Default, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CompileSource {
    /// The working copy, on the node that holds one. What this route has
    /// always done, and still the default: `git_sha` is only a label.
    #[default]
    WorkingCopy,
    /// The commit `git_sha` names, fetched from the workspace's GitHub
    /// remote. Any pod can run it, and `git_sha` must be a full commit id.
    Git,
}

/// Enqueue the compile `source` asks for. Returns the task id.
///
/// A `git` request that can never succeed — no remote, not a commit — is
/// answered here with a 400 rather than queued to fail: there is an operator
/// waiting on the response. Whether GitHub has the commit is only known once a
/// worker asks, and is reported on the task.
///
/// `owns_working_copy`: whether this process can claim a working-copy compile
/// (`process_can_compile()`); only the handler reads the real value.
pub(super) async fn enqueue(
    db: &DatabaseConnection,
    source: CompileSource,
    workspace_id: Uuid,
    git_sha: Option<String>,
    branch: Option<String>,
    promote: bool,
    owns_working_copy: bool,
) -> Result<String, Response> {
    match source {
        CompileSource::WorkingCopy => enqueue_main(
            db,
            workspace_id,
            git_sha,
            branch,
            promote,
            owns_working_copy,
        )
        .await
        .map_err(refusal_response),
        CompileSource::Git => {
            let sha = compile_git::validate_request(db, workspace_id, git_sha.as_deref())
                .await
                .map_err(|e| error_body(StatusCode::BAD_REQUEST, e.code(), Some(e.to_string())))?;
            let kind = oxy_compile::RevisionKind::Main;
            compile_git::enqueue(db, workspace_id, &sha, branch.as_deref(), kind, promote)
                .await
                .map_err(|e| refusal_response(e.into()))
        }
    }
}

/// Queue a compile of `main` from wherever this process can have it compiled:
/// the commit GitHub has at the branch head (any pod claims it), else this
/// process's working copy, else a [`Refusal`] — never a working-copy compile no
/// node here can claim, which would sit `queued` forever (INC-164).
///
/// `git_sha` is only a label for a working-copy compile and is ignored when
/// GitHub answers: the commit compiled is the one GitHub reports for the branch.
pub async fn enqueue_main(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    git_sha: Option<String>,
    branch: Option<String>,
    promote: bool,
    owns_working_copy: bool,
) -> Result<String, Refusal> {
    let workspace = entity::workspaces::Entity::find_by_id(workspace_id)
        .one(db)
        .await?
        .ok_or_else(|| Refusal::Internal(format!("workspace {workspace_id} not found")))?;
    match main_source(&workspace, branch.as_deref(), owns_working_copy).await? {
        MainSource::Git { git_sha, branch } => {
            let kind = oxy_compile::RevisionKind::Main;
            Ok(
                compile_git::enqueue(db, workspace_id, &git_sha, Some(&branch), kind, promote)
                    .await?,
            )
        }
        MainSource::WorkingCopy => {
            Ok(insert_run_and_enqueue_compile(db, workspace_id, git_sha, branch, promote).await?)
        }
    }
}

/// The operator-facing HTTP answer for a refusal.
pub(super) fn refusal_response(refusal: Refusal) -> Response {
    let code = match &refusal {
        Refusal::NeedsWorkingCopy { .. } => "needs_working_copy",
        Refusal::GitHubUnavailable { .. } => "github_unavailable",
        Refusal::InvalidBranch(_) => "invalid_branch",
        Refusal::Conflict(_) => "conflict",
        Refusal::Internal(_) => {
            tracing::error!(%refusal, "admin/compiles: enqueue failed");
            "enqueue_failed"
        }
        _ => "refused",
    };
    error_body(refusal.status(), code, Some(refusal.to_string()))
}

#[cfg(test)]
mod tests {
    use super::CompileSource;

    #[derive(serde::Deserialize)]
    struct Body {
        #[serde(default)]
        source: CompileSource,
    }

    #[test]
    fn a_request_that_names_no_source_compiles_the_working_copy() {
        let body: Body = serde_json::from_str("{}").unwrap();
        assert_eq!(body.source, CompileSource::WorkingCopy);
    }

    #[test]
    fn git_is_opt_in_by_name() {
        let body: Body = serde_json::from_str(r#"{"source":"git"}"#).unwrap();
        assert_eq!(body.source, CompileSource::Git);
        assert!(serde_json::from_str::<Body>(r#"{"source":"github"}"#).is_err());
    }
}
