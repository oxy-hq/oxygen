use axum::{
    http::{HeaderMap, StatusCode, header::SET_COOKIE},
    response::Json,
};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_auth::types::AuthenticatedUser;
use oxy_auth::user::UserService;
use serde::Serialize;

use crate::server::api::auth::clear_session_cookie;
use utoipa::ToSchema;

/// Global profile fields returned by `GET /user`. Role and admin status are
/// per-org, so they are intentionally omitted here — read them from
/// `OrgInfo` in the login response or from `GET /orgs`. Workspace-scoped
/// routes receive the resolved role via the `EffectiveWorkspaceRole`
/// extractor. `is_owner` mirrors the `OXY_OWNER` allow-list and lets the
/// frontend route Oxy staff to the admin shell. `is_app_admin` reflects
/// membership in the `app_admins` table and gates the customer-apps
/// surface.
#[derive(Serialize, ToSchema)]
pub struct UserInfo {
    pub id: String,
    pub email: String,
    pub name: String,
    pub picture: Option<String>,
    pub status: String,
    pub is_owner: bool,
    pub is_app_admin: bool,
    /// The capabilities this user's platform grant expands to (`Cap::as_str`), empty
    /// for a non-staff user. A Global Owner reports every capability.
    ///
    /// **UX only** — the admin console renders its nav from this so a staff member
    /// isn't shown rooms they'll be 403'd out of. Every route re-decides server-side
    /// (`platform_cap_guard`); hiding a nav item is not a security control, and the
    /// only reason this is safe to ship is that it never *grants* anything.
    ///
    /// Sent instead of a role name so the frontend never re-implements
    /// `PlatformRole::caps()` — one expansion, in the model, serialized outward.
    pub platform_capabilities: Vec<String>,
    /// Partners this user administers (empty for most users). Non-empty means
    /// the frontend should surface the partner console. Each entry carries the
    /// partner's capability snapshot so the UI can hide surfaces the partner
    /// can't use. UX-only — the server re-checks on every partner route.
    pub partner_memberships: Vec<PartnerMembershipInfo>,
}

#[derive(Serialize, ToSchema)]
pub struct PartnerMembershipInfo {
    pub partner_id: String,
    pub slug: String,
    /// The partner's ceiling — what any operator here may do. There are no
    /// per-person roles; this is the whole capability story.
    pub capabilities: PartnerCapabilitiesInfo,
}

#[derive(Serialize, ToSchema)]
pub struct PartnerCapabilitiesInfo {
    pub manage_members: bool,
    pub manage_apps: bool,
    pub develop_apps: bool,
    pub view_audit: bool,
    pub manage_billing: bool,
    pub manage_secrets: bool,
    pub create_orgs: bool,
    pub manage_org_settings: bool,
}

#[derive(Serialize)]
pub struct LogoutResponse {
    pub logout_url: Option<String>,
    pub success: bool,
    pub message: String,
}

/// Build a [`UserInfo`] from the authenticated user. Async because
/// `is_app_admin` is now a DB-backed check (see `app_admins` table).
pub async fn user_info_from(user: AuthenticatedUser) -> UserInfo {
    // Reported, not decided — so this takes the flag door, not a ring. With no
    // connection we still know the owner half (an env read), and reporting `is_owner:
    // false` at a Global Owner would hide their own UI over a DB blip.
    let standing = match oxy::database::client::establish_connection().await {
        Ok(db) => {
            crate::server::authz::globals::platform_standing(
                &db,
                &crate::server::authz::Caller::from_user(&user),
            )
            .await
        }
        Err(_) => crate::server::authz::globals::platform_standing_offline(
            &crate::server::authz::Caller::from_user(&user),
        ),
    };
    let is_owner = standing.is_global_owner;
    let is_app_admin = standing.is_global_admin;
    let platform_capabilities =
        platform_capabilities_for(&crate::server::authz::Caller::from_user(&user), is_owner).await;
    let partner_memberships = partner_memberships_for(&user).await;
    UserInfo {
        id: user.id.to_string(),
        email: user.label().to_string(),
        name: user.name,
        picture: user.picture,
        status: user.status.as_str().to_string(),
        is_owner,
        is_app_admin,
        platform_capabilities,
        partner_memberships,
    }
}

/// The capability list to report. The owner short-circuit mirrors the model, where
/// `is_global_owner` is still a boolean that satisfies every capability — reading the
/// grant table for an owner would report an empty list and blank their own console.
async fn platform_capabilities_for(
    caller: &crate::server::authz::Caller,
    is_owner: bool,
) -> Vec<String> {
    if is_owner {
        return oxy_authz::Cap::ALL
            .iter()
            .map(|c| c.as_str().to_string())
            .collect();
    }
    let Ok(db) = oxy::database::client::establish_connection().await else {
        return Vec::new();
    };
    match crate::server::authz::globals::platform_grant_checked(&db, caller).await {
        Ok(Some(grant)) => grant.caps.iter().map(|c| c.as_str().to_string()).collect(),
        // No grant, or an unreadable one. Reporting nothing hides nav items rather than
        // showing ones that would 403 — the safe direction for a display-only field.
        _ => Vec::new(),
    }
}

/// Resolve the partner memberships to expose on `UserInfo`. Fails closed to an
/// empty list on any DB error — a partner just won't see the console until the
/// DB recovers, rather than the whole `/user` call erroring.
async fn partner_memberships_for(user: &AuthenticatedUser) -> Vec<PartnerMembershipInfo> {
    use crate::server::api::middlewares::partner_authz::scopes_for_user;
    let Ok(db) = oxy::database::client::establish_connection().await else {
        return Vec::new();
    };
    scopes_for_user(&db, &crate::server::authz::Caller::from_user(&user))
        .await
        .into_iter()
        .map(|s| PartnerMembershipInfo {
            partner_id: s.partner_id.to_string(),
            slug: s.slug,
            capabilities: PartnerCapabilitiesInfo {
                manage_members: s.capabilities.manage_members,
                manage_apps: s.capabilities.manage_apps,
                develop_apps: s.capabilities.develop_apps,
                view_audit: s.capabilities.view_audit,
                manage_billing: s.capabilities.manage_billing,
                manage_secrets: s.capabilities.manage_secrets,
                create_orgs: s.capabilities.create_orgs,
                manage_org_settings: s.capabilities.manage_org_settings,
            },
        })
        .collect()
}

pub async fn logout() -> Result<(HeaderMap, Json<LogoutResponse>), StatusCode> {
    let mut headers = HeaderMap::new();
    if let Ok(value) = clear_session_cookie().parse() {
        headers.insert(SET_COOKIE, value);
    }
    Ok((
        headers,
        Json(LogoutResponse {
            logout_url: None,
            success: true,
            message: "Built-in logout successful".to_string(),
        }),
    ))
}

pub async fn get_current_user(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
) -> Result<Json<UserInfo>, StatusCode> {
    Ok(Json(user_info_from(user).await))
}

/// Public endpoint that returns current user if authenticated, null if not
/// This prevents redirect loops when auth is enabled
/// Who the caller is.
///
/// **A `200` with a `null` body means the token did not resolve to a user** —
/// an expired session answers exactly that rather than a `401`, so a client
/// must not read the null as "no such user". `oxyc whoami` exists to tell the
/// two apart.
#[utoipa::path(
    method(get),
    path = "/user",
    responses(
        (status = OK, description = "The authenticated user, or `null` when the credential did not resolve to one", body = Option<UserInfo>, content_type = "application/json")
    ),
    security(
        ("BearerAuth" = [])
    )
)]
pub async fn get_current_user_public(
    axum::extract::State(_app_state): axum::extract::State<crate::server::router::AppState>,
    headers: axum::http::HeaderMap,
) -> Result<Json<Option<UserInfo>>, StatusCode> {
    // Try to authenticate using the same logic as the middleware
    // If successful, return user info; if not, return null (not an error)
    use oxy_auth::authenticator::Authenticator;
    use oxy_auth::built_in::BuiltInAuthenticator;

    let authenticator = BuiltInAuthenticator::new();

    match authenticator.authenticate_with_credential(&headers).await {
        Ok((identity, credential)) => {
            // Look up existing user only — do not auto-create. Closes #16.
            // User rows are created by the auth/sign-up flow, not by a public GET.
            //
            // The user carries the key or token that authenticated the request,
            // so the standing reported is what that credential holds — the same
            // thing every other route will enforce for it.
            match UserService::find_user_by_identity(&identity).await {
                Ok(Some(user)) => Ok(Json(Some(
                    user_info_from(user.with_credential(credential)).await,
                ))),
                Ok(None) => Ok(Json(None)),
                Err(e) => {
                    tracing::error!("Failed to lookup user from identity: {}", e);
                    Ok(Json(None))
                }
            }
        }
        Err(_) => {
            // Not authenticated - return null instead of error
            Ok(Json(None))
        }
    }
}
