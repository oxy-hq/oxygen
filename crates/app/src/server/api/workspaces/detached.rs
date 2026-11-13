//! What the workspace git surface does when the working copy is on a detached
//! HEAD — a `pull_request` checkout in CI, `git checkout <sha>`,
//! `git worktree add --detach`.
//!
//! There is no branch. Reads and saves treat the `HEAD@<sha>` label as the
//! workspace root as it is; that is decided once, in
//! `oxy::adapters::workspace::effective_workspace_path`, and nothing here
//! repeats it. What lives here is the other half: operations that need a real
//! branch — commit/push, force-push, pull, fetch, restore, switching *to* the
//! label, staging — answer `409 {"code":"detached_head"}` with the reason,
//! instead of handing git a `HEAD@<sha>` refspec. Switching to or creating a
//! real branch stays open: it is the way out the message points at.
//!
//! The label itself is produced and recognised in `oxy_git::cli::head`.

use std::path::Path;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use oxy::github::default_git_client;
use oxy_git::{DetachedHead, GitClient};

/// `code` of the refusal body. The frontend keys on it to show the message.
pub const DETACHED_HEAD_CODE: &str = "detached_head";

/// Why a workspace git handler refused: a bare status, as these handlers have
/// always answered, or the detached-HEAD refusal, which explains itself.
#[derive(Debug, PartialEq, Eq)]
pub enum GitRefusal {
    Status(StatusCode),
    DetachedHead(DetachedHead),
}

impl From<StatusCode> for GitRefusal {
    fn from(status: StatusCode) -> Self {
        Self::Status(status)
    }
}

impl From<DetachedHead> for GitRefusal {
    fn from(detached: DetachedHead) -> Self {
        Self::DetachedHead(detached)
    }
}

impl IntoResponse for GitRefusal {
    /// `409 {"code":"detached_head","message":"This workspace is on a detached
    /// HEAD at <sha>; switch to or create a branch first.","sha":"<sha>"}`.
    fn into_response(self) -> Response {
        match self {
            Self::Status(status) => status.into_response(),
            Self::DetachedHead(detached) => (
                StatusCode::CONFLICT,
                axum::Json(serde_json::json!({
                    "code": DETACHED_HEAD_CODE,
                    "message": detached.to_string(),
                    "sha": detached.short_sha,
                })),
            )
                .into_response(),
        }
    }
}

/// The status for a `?branch=` value the workspace middleware could not
/// resolve. A well-formed detached label that no longer matches the working
/// copy is a conflict with the workspace's state (the checkout moved since the
/// caller read it), not a malformed request; everything else stays a `400`.
pub(crate) fn unresolved_branch_status(branch: Option<&str>) -> StatusCode {
    match branch.map(str::trim) {
        Some(b) if oxy_git::is_detached_label(b) => StatusCode::CONFLICT,
        _ => StatusCode::BAD_REQUEST,
    }
}

/// What `HEAD` names in the workspace root, as the details response reports
/// it: the `active_branch` name (the label when detached) and, only when
/// detached, the short sha — so the frontend never has to parse the label.
pub(super) async fn active_head(root: &Path, default_branch: &str) -> (String, Option<String>) {
    match default_git_client().head_state(root).await {
        Ok(head) => {
            let label = head.label();
            (label, head.into_branch().err().map(|d| d.short_sha))
        }
        Err(_) => (default_branch.to_string(), None),
    }
}

#[cfg(test)]
#[path = "detached_tests.rs"]
mod tests;
