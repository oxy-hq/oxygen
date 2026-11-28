//! [`CompileChecker`]: the guard on the staging compile routes
//! (`POST /{workspace_id}/compile/staging`, `GET …/compile/staging/status`).
//!
//! Before it, both routes took [`WorkspaceEditor`](crate::role_guards::WorkspaceEditor).
//! A person still needs exactly that here — a workspace role above Viewer — and the
//! model's `Action::CompileCheck` adds the one caller the customer CI gate needs: the
//! org's service account acting through a CI run token, whose trust policy grants it
//! no more than `viewer` on the workspace. It may check a branch compiles and nothing
//! else; every other workspace guard still refuses it (open question 2 of
//! `internal-docs/oxy-cli-to-oxyc.md`, decided 2026-11-21).

use axum::extract::FromRequestParts;
use axum::http::StatusCode;
use axum::http::request::Parts;
use entity::workspace_members::WorkspaceRole;
use std::future::Future;

use crate::workspace_role::EffectiveWorkspaceRole;
use crate::{Action, Caller, Resource, enforce, request_facts};

const LABEL: &str = "guard.compile_check";

/// Caller may queue a staging compile of a branch and read where it stands.
///
/// The decision is `existing_allow && allows(CompileCheck)`. `existing_allow` is the
/// shipped check: [`WorkspaceEditor`](crate::role_guards::WorkspaceEditor)'s term
/// (a role above Viewer) OR the request being the workspace org's service account
/// through a CI run token ([`Caller::is_ci_run`](contracts_authz::Caller::is_ci_run)).
///
/// **The account term fails closed.** When the facts cannot be read, a role guard
/// defers to its shipped check — but the account term has no shipped check behind it
/// (it is new with this action), so with unknown facts only the editor term counts.
#[derive(Debug)]
pub struct CompileChecker(pub WorkspaceRole);

impl<S> FromRequestParts<S> for CompileChecker
where
    S: Send + Sync,
{
    type Rejection = StatusCode;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        async move { verdict(parts).await.map(CompileChecker) }
    }
}

async fn verdict(parts: &mut Parts) -> Result<WorkspaceRole, StatusCode> {
    let Some(EffectiveWorkspaceRole(role)) =
        parts.extensions.get::<EffectiveWorkspaceRole>().cloned()
    else {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    };
    let editor = role > WorkspaceRole::Viewer;
    let workspace = parts
        .extensions
        .get::<entity::workspaces::Model>()
        .and_then(|ws| ws.org_id.map(|org_id| (ws.id, org_id)));
    let allowed = match workspace {
        Some((workspace_id, org_id)) => {
            let ci_account = Caller::from_extensions(&parts.extensions).is_some_and(|caller| {
                caller.is_ci_run() && caller.account_role_in(org_id).is_some()
            });
            match request_facts(parts).await {
                Some(facts) => enforce(
                    LABEL,
                    &facts,
                    Action::CompileCheck,
                    &Resource::workspace(workspace_id, org_id),
                    editor || ci_account,
                ),
                // Unknown facts: defer to the check that shipped — the editor's.
                None => editor,
            }
        }
        // No org to decide in: the editor's term alone, as `WorkspaceEditor` does.
        None => editor,
    };
    if allowed {
        Ok(role)
    } else {
        Err(StatusCode::FORBIDDEN)
    }
}

#[cfg(test)]
#[path = "compile_check_guard_tests.rs"]
mod tests;
