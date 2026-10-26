//! Attaching a GitHub App installation to an org: who may, and which one.
//!
//! An attached installation decides what the org can read — its repositories
//! are listed and cloned with an installation token the Oxy App mints — so both
//! halves are asked every time, on every route that attaches one (the org
//! route, and the install callback):
//!
//! - **Who:** an owner or admin of the org. On the org route that is the
//!   `OrgAdmin` guard; on a per-user route, which names the org in a query or a
//!   signed `state` instead of its path, it is [`require_org_admin`] — the org
//!   resolved as `org_middleware` resolves one, then the same guard's decision.
//! - **Which:** an installation the caller's own GitHub account lists
//!   ([`callers_installation`]). The App's credential reads every installation
//!   of the App, and ids are sequential integers, so the App answering for an
//!   id says nothing about who typed it.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use sea_orm::DatabaseConnection;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, QueryFilter};
use serde::Serialize;
use uuid::Uuid;

use entity::{git_namespaces, github_accounts};
use oxy::database::client::establish_connection;
use oxy::github::app_auth::GitHubInstallation;
use oxy::github::client::GitHubClient;
use oxy::shared::errors::OxyError;
use oxy_app::surface::role_guards::OrgAdmin;
use oxy_app::surface::{OrgContext, resolve_org_context};
use oxy_auth::types::AuthenticatedUser;
use oxy_server_authz::Caller;

/// The `code` of the refusal a caller with no usable GitHub account gets.
pub const GITHUB_ACCOUNT_NOT_CONNECTED: &str = "github_account_not_connected";

/// How a route that attaches an installation refuses.
#[derive(Debug)]
pub enum AttachError {
    Status(StatusCode),
    /// Nothing can show an installation is the caller's: no GitHub account is
    /// connected, or GitHub no longer accepts its token. The way through is to
    /// (re)connect it, so this is a `409` with a code the UI keys on, never a
    /// `401` — that one signs the person out of Oxy.
    GitHubAccountNotConnected,
}

impl From<StatusCode> for AttachError {
    fn from(status: StatusCode) -> Self {
        Self::Status(status)
    }
}

#[derive(Serialize)]
struct Refusal {
    code: &'static str,
    error: &'static str,
}

impl IntoResponse for AttachError {
    fn into_response(self) -> Response {
        match self {
            Self::Status(status) => status.into_response(),
            Self::GitHubAccountNotConnected => (
                StatusCode::CONFLICT,
                Json(Refusal {
                    code: GITHUB_ACCOUNT_NOT_CONNECTED,
                    error: "Connect your GitHub account to choose one of its installations.",
                }),
            )
                .into_response(),
        }
    }
}

/// The `OrgAdmin` door for a route whose org is not in its path.
///
/// First the credential's own reach (`require_org_reach`, 404 for a token the
/// org is outside), then the org resolved exactly as `org_middleware` resolves
/// one — 404 for no such org, 403 for a non-member without a live assume-role
/// session — then `OrgAdmin`'s own decision on it: a plain member is 403.
pub async fn require_org_admin(
    user: &AuthenticatedUser,
    org_id: Uuid,
) -> Result<OrgContext, StatusCode> {
    user.require_org_reach(org_id)?;
    let ctx = resolve_org_context(user, org_id).await?;
    let db = connect().await?;
    let OrgAdmin(ctx) = OrgAdmin::of(&db, &Caller::from_user(user), ctx).await?;
    Ok(ctx)
}

/// The installation `installation_id`, if the caller's own GitHub account
/// lists it (`GET /user/installations`, every page, with the token they
/// connected). One it does not list is `404` — exactly the answer for an id
/// that exists nowhere, so the refusal cannot be used to find ids.
pub async fn callers_installation(
    db: &DatabaseConnection,
    user_id: Uuid,
    installation_id: i64,
) -> Result<GitHubInstallation, AttachError> {
    let account = github_accounts::Entity::find()
        .filter(github_accounts::Column::UserId.eq(user_id))
        .one(db)
        .await
        .map_err(|e| {
            tracing::error!("failed to query github_accounts for user {user_id}: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(AttachError::GitHubAccountNotConnected)?;

    let client = GitHubClient::from_token(account.oauth_token).map_err(|e| {
        tracing::error!("failed to initialize GitHub client for user {user_id}: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // GitHub rejecting the token is the client's `AuthenticationError`; anything
    // else is GitHub failing, and nothing is attached while it does.
    let listed = client
        .list_user_installations()
        .await
        .map_err(|e| match e {
            OxyError::AuthenticationError(msg) => {
                tracing::warn!("GitHub no longer accepts the token of user {user_id}: {msg}");
                AttachError::GitHubAccountNotConnected
            }
            e => {
                tracing::error!("failed to list github user installations for user {user_id}: {e}");
                AttachError::Status(StatusCode::BAD_GATEWAY)
            }
        })?;

    listed
        .into_iter()
        .find(|i| i.id == installation_id && is_callers_to_attach(i, &account.github_login))
        .ok_or_else(|| {
            tracing::warn!(
                "user {user_id} named installation {installation_id}, which their GitHub \
                 account does not list, or lists on someone else's personal account"
            );
            AttachError::Status(StatusCode::NOT_FOUND)
        })
}

/// Whether a listed installation is one the caller may attach.
///
/// `GET /user/installations` lists every installation the user can reach
/// through *any* repository: one they own, one they are a collaborator on,
/// one their organization membership grants. So a personal account's
/// installation is listed to everyone its owner made a collaborator on a
/// single repository. For a `User` account, then, listed is not owned: its
/// login must be the caller's own connected login (GitHub logins are
/// case-insensitive). An `Organization` installation is accepted as listed;
/// requiring a role in that GitHub organization is a product decision this
/// check does not make.
fn is_callers_to_attach(installation: &GitHubInstallation, github_login: &str) -> bool {
    installation.owner_type != "User" || installation.slug.eq_ignore_ascii_case(github_login)
}

/// The org's namespace for `installation`: the one already attached, or a new
/// one created by `user_id`.
pub async fn attach_namespace(
    db: &DatabaseConnection,
    org_id: Uuid,
    user_id: Uuid,
    installation: GitHubInstallation,
) -> Result<git_namespaces::Model, StatusCode> {
    let existing = git_namespaces::Entity::find()
        .filter(git_namespaces::Column::InstallationId.eq(installation.id))
        .filter(git_namespaces::Column::OrgId.eq(org_id))
        .one(db)
        .await
        .map_err(|e| {
            tracing::error!(
                "failed to query git_namespaces for installation {} org {org_id}: {e}",
                installation.id
            );
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    if let Some(ns) = existing {
        return Ok(ns);
    }

    let installation_id = installation.id;
    git_namespaces::ActiveModel {
        id: Set(Uuid::new_v4()),
        installation_id: Set(installation.id),
        name: Set(installation.name),
        owner_type: Set(installation.owner_type),
        provider: Set("github".into()),
        slug: Set(installation.slug),
        oauth_token: Set(String::new()),
        created_by: Set(user_id),
        org_id: Set(Some(org_id)),
    }
    .insert(db)
    .await
    .map_err(|e| {
        tracing::error!(
            "failed to insert git_namespace for installation {installation_id} org {org_id}: {e}"
        );
        StatusCode::INTERNAL_SERVER_ERROR
    })
}

pub async fn connect() -> Result<DatabaseConnection, StatusCode> {
    establish_connection().await.map_err(|e| {
        tracing::error!("failed to establish database connection: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(owner_type: &str, login: &str) -> GitHubInstallation {
        GitHubInstallation {
            id: 1,
            owner_type: owner_type.into(),
            slug: login.into(),
            name: login.into(),
        }
    }

    #[test]
    fn a_personal_installation_is_attachable_only_by_its_own_login() {
        assert!(is_callers_to_attach(&listed("User", "octocat"), "octocat"));
        assert!(is_callers_to_attach(&listed("User", "OctoCat"), "octocat"));
        assert!(!is_callers_to_attach(
            &listed("User", "someone-else"),
            "octocat"
        ));
    }

    #[test]
    fn an_organization_installation_is_attachable_as_listed() {
        assert!(is_callers_to_attach(
            &listed("Organization", "acme"),
            "octocat"
        ));
    }
}
