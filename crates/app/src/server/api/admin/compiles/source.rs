//! Where an operator-requested compile reads the workspace from.

use axum::http::StatusCode;
use axum::response::Response;
use sea_orm::DatabaseConnection;
use serde::Deserialize;
use uuid::Uuid;

use super::{error_body, insert_run_and_enqueue_compile};
use crate::server::compile_git;

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
pub(super) async fn enqueue(
    db: &DatabaseConnection,
    source: CompileSource,
    workspace_id: Uuid,
    git_sha: Option<String>,
    branch: Option<String>,
    promote: bool,
) -> Result<String, Response> {
    let enqueued = match source {
        CompileSource::WorkingCopy => {
            insert_run_and_enqueue_compile(db, workspace_id, git_sha, branch, promote).await
        }
        CompileSource::Git => {
            let sha = compile_git::validate_request(db, workspace_id, git_sha.as_deref())
                .await
                .map_err(|e| error_body(StatusCode::BAD_REQUEST, e.code(), Some(e.to_string())))?;
            compile_git::enqueue(db, workspace_id, &sha, branch.as_deref(), promote).await
        }
    };
    enqueued.map_err(|e| {
        tracing::error!(?e, "admin/compiles: enqueue failed");
        error_body(
            StatusCode::INTERNAL_SERVER_ERROR,
            "enqueue_failed",
            Some(format!("{e}")),
        )
    })
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
