use axum::{
    extract::{Json, Query},
    response::Json as ResponseJson,
};
use reqwest::StatusCode;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use tracing::error;
use uuid::Uuid;

use oxy::database::client::establish_connection;
use oxy::github::app_auth::GitHubAppAuth;
use oxy::github::client::GitHubClient;
use oxy::github::github_token_for_namespace;
use oxy::github::types::{GitHubBranch, GitHubRepository};
use oxy_auth::extractor::AuthenticatedUserExtractor;

use oxy_server_authz::org_context::OrgContextExtractor;

#[derive(Debug, Serialize)]
pub struct GitHubNamespace {
    pub id: Uuid,
    pub owner_type: String,
    pub slug: String,
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct GitHubNamespacesResponse {
    pub installations: Vec<GitHubNamespace>,
}

#[derive(Debug, Deserialize)]
pub struct GitHubRepositoriesQuery {
    pub git_namespace_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct GitHubBranchesQuery {
    pub git_namespace_id: Uuid,
    pub repo_name: String,
}

#[derive(Debug, Serialize)]
pub struct GitHubRepositoriesResponse {
    pub repositories: Vec<GitHubRepository>,
}

#[derive(Debug, Serialize)]
pub struct GitHubBranchesResponse {
    pub branches: Vec<GitHubBranch>,
}

#[derive(Debug, Deserialize)]
pub struct CreatePATNamespaceRequest {
    pub token: String,
}

/// POST /github/namespaces/installation — register an existing GitHub App installation by ID.
///
/// Bypasses the OAuth callback flow entirely: uses the App's private key to fetch installation
/// info directly from the GitHub API. Intended for dev environments where only one callback URL
/// is registered and the app is already installed on the org.
///
/// The installation ID is visible in the GitHub settings URL:
/// https://github.com/settings/installations/{installation_id}
#[derive(Debug, Deserialize)]
pub struct CreateInstallationNamespaceRequest {
    pub installation_id: i64,
}

/// POST /github/namespaces/pat — register a Personal Access Token as a GitHub connection.
/// Validates the token against the GitHub API and stores it as a namespace.
pub async fn create_pat_namespace(
    OrgContextExtractor(org_ctx): OrgContextExtractor,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Json(payload): Json<CreatePATNamespaceRequest>,
) -> Result<ResponseJson<GitHubNamespace>, axum::http::StatusCode> {
    let client = GitHubClient::from_token(payload.token.clone()).map_err(|e| {
        error!("Failed to create GitHub client from PAT: {}", e);
        StatusCode::BAD_REQUEST
    })?;

    let username = client.get_current_user().await.map_err(|e| {
        error!("PAT validation failed: {}", e);
        StatusCode::UNAUTHORIZED
    })?;

    let db = establish_connection().await.map_err(|e| {
        error!("Failed to establish database connection: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // If this user already has a PAT namespace, update it with the new token and
    // return it.  PAT namespaces use slug="pat", so (user_id, slug="pat") is unique.
    // We always update the token so that rotating a PAT is reflected immediately
    // without requiring the user to delete and re-add the namespace.
    {
        use entity::git_namespaces;
        use sea_orm::{ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter};
        if let Some(existing) = entity::prelude::GitNamespaces::find()
            .filter(git_namespaces::Column::CreatedBy.eq(user.id))
            .filter(git_namespaces::Column::Slug.eq("pat"))
            .one(&db)
            .await
            .map_err(|e| {
                error!("DB error checking existing PAT namespace: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?
        {
            let mut active = existing.clone().into_active_model();
            active.name = Set(username.clone());
            active.oauth_token = Set(payload.token.clone());
            active.save(&db).await.map_err(|e| {
                error!("DB error updating PAT namespace token: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
            return Ok(ResponseJson(GitHubNamespace {
                id: existing.id,
                owner_type: existing.owner_type,
                slug: existing.slug,
                name: username,
            }));
        }
    }

    let git_namespace = entity::git_namespaces::ActiveModel {
        id: Set(Uuid::new_v4()),
        created_by: Set(user.id),
        name: Set(username.clone()),
        slug: Set("pat".to_string()),
        owner_type: Set("User".to_string()),
        installation_id: Set(0),
        provider: Set("github".to_string()),
        org_id: Set(Some(org_ctx.org.id)),
        oauth_token: Set(payload.token.clone()),
    };

    let ns = git_namespace.insert(&db).await.map_err(|e| {
        error!("Database error inserting PAT namespace: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(ResponseJson(GitHubNamespace {
        id: ns.id,
        owner_type: ns.owner_type,
        slug: ns.slug,
        name: ns.name,
    }))
}

pub async fn create_installation_namespace(
    OrgContextExtractor(org_ctx): OrgContextExtractor,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Json(payload): Json<CreateInstallationNamespaceRequest>,
) -> Result<ResponseJson<GitHubNamespace>, axum::http::StatusCode> {
    let app_auth = GitHubAppAuth::from_env()?;
    let installation = app_auth
        .get_installation_info(&payload.installation_id.to_string())
        .await?;

    let db = establish_connection().await.map_err(|e| {
        error!("Failed to establish database connection: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Return existing namespace if this installation is already registered for this user.
    use entity::git_namespaces;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    let existing = entity::prelude::GitNamespaces::find()
        .filter(git_namespaces::Column::InstallationId.eq(installation.id))
        .filter(git_namespaces::Column::OrgId.eq(org_ctx.org.id))
        .one(&db)
        .await
        .map_err(|e| {
            error!("DB query failed looking up existing namespace: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if let Some(ns) = existing {
        return Ok(ResponseJson(GitHubNamespace {
            id: ns.id,
            owner_type: ns.owner_type,
            slug: ns.slug,
            name: ns.name,
        }));
    }

    let git_namespace = entity::git_namespaces::ActiveModel {
        id: Set(Uuid::new_v4()),
        created_by: Set(user.id),
        name: Set(installation.name),
        slug: Set(installation.slug),
        owner_type: Set(installation.owner_type),
        installation_id: Set(installation.id),
        provider: Set("github".to_string()),
        org_id: Set(Some(org_ctx.org.id)),
        oauth_token: Set(String::new()),
    };

    let ns = git_namespace.insert(&db).await.map_err(|e| {
        error!("Database error inserting installation namespace: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(ResponseJson(GitHubNamespace {
        id: ns.id,
        owner_type: ns.owner_type,
        slug: ns.slug,
        name: ns.name,
    }))
}

pub async fn delete_git_namespace(
    OrgContextExtractor(org_ctx): OrgContextExtractor,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    axum::extract::Path((_org_id, id)): axum::extract::Path<(Uuid, Uuid)>,
) -> Result<StatusCode, StatusCode> {
    let db = establish_connection().await.map_err(|e| {
        error!("Failed to establish database connection: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let ns = entity::git_namespaces::Entity::find_by_id(id)
        .one(&db)
        .await
        .map_err(|e| {
            error!("Database error fetching namespace {}: {}", id, e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    // Verify the namespace belongs to the current org.
    if ns.org_id != Some(org_ctx.org.id) {
        return Err(StatusCode::NOT_FOUND);
    }

    // Allow deletion by org owners/admins, or the original creator — the same
    // admin-or-creator ring as workspace rename, enforced through the policy instead
    // of re-derived here (an inline copy drifts from the model it mirrors).
    let is_admin = matches!(
        org_ctx.membership.role,
        entity::org_members::OrgRole::Owner | entity::org_members::OrgRole::Admin
    );
    let legacy = is_admin || ns.created_by == user.id;
    let allowed = oxy_server_authz::enforce_for(
        &db,
        &oxy_server_authz::Caller::from_user(&user),
        "namespace.delete",
        oxy_server_authz::Action::NamespaceDelete,
        oxy_server_authz::Resource::namespace_with_creator(
            ns.id,
            org_ctx.org.id,
            Some(ns.created_by),
        ),
        legacy,
    )
    .await;
    if !allowed {
        return Err(StatusCode::FORBIDDEN);
    }

    entity::git_namespaces::Entity::delete_by_id(id)
        .exec(&db)
        .await
        .map_err(|e| {
            error!("Database error deleting namespace {}: {}", id, e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_git_namespaces(
    OrgContextExtractor(org_ctx): OrgContextExtractor,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
) -> Result<ResponseJson<GitHubNamespacesResponse>, axum::http::StatusCode> {
    let db = establish_connection().await.map_err(|e| {
        error!("Failed to establish database connection: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    use sea_orm::{ColumnTrait, QueryFilter};
    // Filter namespaces by the current organization. Within an org, GitHub App
    // installations are shared by all members, but PAT namespaces are per-user.
    let git_namespaces = entity::git_namespaces::Entity::find()
        .filter(entity::git_namespaces::Column::OrgId.eq(org_ctx.org.id))
        .filter(
            sea_orm::Condition::any()
                .add(entity::git_namespaces::Column::Slug.ne("pat"))
                .add(entity::git_namespaces::Column::CreatedBy.eq(user.id)),
        )
        .all(&db)
        .await
        .map_err(|e| {
            error!("Database error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let response = GitHubNamespacesResponse {
        installations: git_namespaces
            .into_iter()
            .map(|ns| GitHubNamespace {
                id: ns.id,
                owner_type: ns.owner_type,
                name: ns.name,
                slug: ns.slug,
            })
            .collect(),
    };

    Ok(ResponseJson(response))
}

pub async fn list_repositories(
    OrgContextExtractor(org_ctx): OrgContextExtractor,
    _user: AuthenticatedUserExtractor,
    Query(query): Query<GitHubRepositoriesQuery>,
) -> Result<Json<Vec<GitHubRepository>>, StatusCode> {
    let db = establish_connection().await.map_err(|e| {
        error!("Failed to establish database connection: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let git_namespace = entity::git_namespaces::Entity::find()
        .filter(entity::git_namespaces::Column::Id.eq(query.git_namespace_id))
        .filter(entity::git_namespaces::Column::OrgId.eq(org_ctx.org.id))
        .one(&db)
        .await
        .map_err(|e| {
            error!("Database error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    let Some(git_namespace) = git_namespace else {
        error!("Git namespace not found for id {}", query.git_namespace_id);
        return Err(StatusCode::BAD_REQUEST);
    };

    // PAT slug indicates a personal access token namespace; otherwise use GitHub App auth.
    let is_pat = git_namespace.slug == "pat";
    let token = github_token_for_namespace(&git_namespace)
        .await
        .map_err(|e| {
            error!("Failed to load token from git namespace: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let client = match GitHubClient::from_token(token) {
        Ok(client) => client,
        Err(e) => {
            error!("Failed to create GitHub client: {}", e);
            return Err(StatusCode::BAD_REQUEST);
        }
    };

    // PAT tokens use /user/repos; installation tokens use /installation/repositories.
    let repositories = if is_pat {
        client.list_user_repositories().await
    } else {
        client.list_repositories().await
    };

    match repositories {
        Ok(repos) => Ok(Json(repos)),
        Err(e) => {
            let msg = e.to_string();
            error!("Failed to fetch repositories: {}", msg);
            // Map GitHub auth/installation errors to distinct HTTP codes so the
            // client can surface a "reinstall" prompt instead of a generic error.
            // Note: we use 403 (not 401) to avoid triggering the client's global
            // "session expired → redirect to login" interceptor.
            if msg.contains("401") || msg.contains("403") {
                // Token expired, app permissions revoked, or PAT invalid.
                Err(StatusCode::FORBIDDEN)
            } else if msg.contains("404") {
                // Installation no longer exists (app was uninstalled).
                Err(StatusCode::NOT_FOUND)
            } else {
                Err(StatusCode::INTERNAL_SERVER_ERROR)
            }
        }
    }
}

pub async fn list_branches(
    OrgContextExtractor(org_ctx): OrgContextExtractor,
    _user: AuthenticatedUserExtractor,
    Query(query): Query<GitHubBranchesQuery>,
) -> Result<Json<Vec<GitHubBranch>>, StatusCode> {
    let db = establish_connection().await.map_err(|e| {
        error!("Failed to establish database connection: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let git_namespace = entity::git_namespaces::Entity::find()
        .filter(entity::git_namespaces::Column::Id.eq(query.git_namespace_id))
        .filter(entity::git_namespaces::Column::OrgId.eq(org_ctx.org.id))
        .one(&db)
        .await
        .map_err(|e| {
            error!("Database error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    let Some(git_namespace) = git_namespace else {
        error!("Git namespace not found for id {}", query.git_namespace_id);
        return Err(StatusCode::BAD_REQUEST);
    };

    let token = github_token_for_namespace(&git_namespace)
        .await
        .map_err(|e| {
            error!("Failed to load token from git namespace: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let client = match GitHubClient::from_token(token) {
        Ok(client) => client,
        Err(e) => {
            error!("Failed to create GitHub client: {}", e);
            return Err(StatusCode::BAD_REQUEST);
        }
    };

    match client.list_branches(query.repo_name).await {
        Ok(branches) => Ok(Json(branches)),
        Err(e) => {
            error!("Failed to fetch branches: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}
