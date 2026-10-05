//! The Tenancy HTTP surface: who belongs to an org, and how orgs and their
//! workspaces come to exist.
//!
//! One bounded context, one crate (`internal-docs/domain-boundaries.md` S1).
//! It began as two sibling crates that split along the CALLER, not the
//! context — `oxy-api-onboarding` (an org admin creating a workspace) and
//! `oxy-api-partner-console` (a partner managing its client orgs) — which
//! left the org/workspace provisioning they share with nowhere to live but
//! `oxy-app`. Merged, that shared code had one owner, which is what let the
//! rest of tenancy (`organizations`, the org-team handlers, the org logo,
//! the staff console's org and workspace sections) move here too.
//!
//! `oxy-server` mounts it through the seams: [`routes`] and the onboarding /
//! partner-console routers through `SurfaceSeams::api`, onboarding's
//! workspace half through `::workspace`, the staff console's org and
//! workspace sections through `::admin` ([`admin_sections`]), and the
//! documented operations through `::openapi` ([`openapi`]).

// Laying out `create_org`'s future takes 103 of rustc's default query depth of
// 128 at opt-level 3 (measured in #3476), and the `--release` build that produces
// the image takes more — a build PR CI never runs. Raised before it overflows there.
#![recursion_limit = "256"]

pub mod admin;
pub mod onboarding;
pub mod org_logo;
pub mod org_teams;
pub mod organizations;
pub mod partner_console;
pub mod workspace_provisioning;

use axum::Router;
use axum::middleware::from_fn;
use axum::routing::{delete, get, patch, post, put};
use oxy_app::surface::{AdminSection, org_middleware, subscription_guard_middleware};
use oxy_app_core::AppState;

/// The org-facing tenancy routes: the caller's orgs and invitations at the
/// root, and one org's settings, members, invitations, teams and app access
/// under `/orgs/{org_id}`.
///
/// Moved out of `oxy-app`'s `build_global_routes` / `build_org_routes` with
/// their middleware unchanged: the org subtree runs `org_middleware` (which
/// resolves `{org_id}` into the `OrgContext` the guards read) and then the
/// subscription guard — `.layer` is outermost-last, so the order below is the
/// request order reversed. All Postgres, so all FleetOk: no declarations.
pub fn routes() -> Router<AppState> {
    Router::new()
        // Read-only. Customers do not create orgs: Oxy staff (`POST
        // /admin/orgs`) and partners (`POST /partners/{id}/orgs`) onboard them.
        .route("/orgs", get(organizations::list_orgs))
        .route("/invitations/mine", get(organizations::list_my_invitations))
        .route(
            "/invitations/{token}/accept",
            post(organizations::accept_invitation),
        )
        .merge(
            Router::new()
                .nest("/orgs/{org_id}", org_routes())
                .layer(from_fn(subscription_guard_middleware))
                .layer(from_fn(org_middleware)),
        )
}

fn org_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/",
            get(organizations::get_org)
                .patch(organizations::update_org)
                .delete(organizations::delete_org),
        )
        .route(
            "/logo",
            put(org_logo::upload_org_logo).delete(org_logo::delete_org_logo),
        )
        .route("/members", get(organizations::list_members))
        .route(
            "/members/{user_id}",
            patch(organizations::update_member_role).delete(organizations::remove_member),
        )
        .route(
            "/invitations",
            post(organizations::create_invitation).get(organizations::list_invitations),
        )
        .route(
            "/invitations/bulk",
            post(organizations::create_bulk_invitations),
        )
        .route(
            "/invitations/{invitation_id}",
            delete(organizations::revoke_invitation),
        )
        .route(
            "/teams",
            get(org_teams::handlers::list_teams).post(org_teams::handlers::create_team),
        )
        .route(
            "/teams/{team_id}",
            get(org_teams::handlers::get_team)
                .patch(org_teams::handlers::update_team)
                .delete(org_teams::handlers::delete_team),
        )
        .route(
            "/teams/{team_id}/members",
            post(org_teams::handlers::add_team_member),
        )
        .route(
            "/teams/{team_id}/members/{user_id}",
            delete(org_teams::handlers::remove_team_member),
        )
        .route("/apps", get(org_teams::app_access::list_org_apps))
        .route(
            "/apps/{app_id}/access",
            get(org_teams::app_access::get_app_access).put(org_teams::app_access::set_app_access),
        )
}

/// The staff console's org and workspace administration, mounted under
/// `/api/admin` through the admin seam. One section: both sit behind
/// `PlatformOrgs`, the capability they had in `oxy-app`; `create_org`
/// additionally asks for `PlatformOrgCreate` inside the handler.
pub fn admin_sections() -> Vec<AdminSection> {
    vec![AdminSection {
        capability: oxy_app::surface::Action::PlatformOrgs,
        routes: admin_routes(),
        decls: vec![admin::orgs::CREATE_ORG_ROLE],
    }]
}

/// Relative to `/admin`. Its own function so the route catalog can seed it.
pub fn admin_routes() -> Router<AppState> {
    admin::orgs::router().merge(admin::workspaces::router())
}

/// The OpenAPI operations this crate documents, merged into the served
/// document by `oxy-server`. `/orgs` is one of the lookups an agent needs to
/// turn a customer into the ids everything else takes.
pub fn openapi() -> utoipa::openapi::OpenApi {
    utoipa_axum::router::OpenApiRouter::<AppState>::new()
        .routes(utoipa_axum::routes!(organizations::list_orgs))
        .into_openapi()
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    /// Customers do not create orgs — staff and partners onboard them — so
    /// `POST /orgs` is a 405: the path stays mounted for the list, and nothing
    /// handles the write. Moved from `oxy-app`'s router tests with the route.
    #[tokio::test]
    async fn self_serve_org_creation_is_not_mounted() {
        let router = super::routes().with_state(oxy_app::surface::bare_app_state());
        let req = Request::builder()
            .method("POST")
            .uri("/orgs")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.expect("oneshot");
        assert_eq!(
            resp.status(),
            StatusCode::METHOD_NOT_ALLOWED,
            "POST /orgs must not reach a handler"
        );
    }
}
