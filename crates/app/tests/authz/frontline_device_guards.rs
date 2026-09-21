//! The kiosk update route stands behind the same door its siblings do.
//!
//! `update_device` is a write on the tenant's kiosks: it changes the label an
//! admin reads in the settings list and the number a tablet arms its sign-out
//! timer with. Create, revoke and reissue all take `OrgAdmin`, and a plain org
//! Member must not be able to reach past any of them.
//!
//! Asserted through a router rather than by reading the handler's signature,
//! for the reason `thread_role_guards` gives: axum runs extractors in
//! declaration order and short-circuits on the first rejection, so mounting the
//! handler and sending a request is what proves the guard is *first* — a source
//! scan would pass just as happily on a handler that took `OrgAdmin` after the
//! body it should never have parsed.
//!
//! Nothing here opens a database. The Member never gets past the guard, and the
//! Admin is stopped by `AuthenticatedUserExtractor` (there is no authenticated
//! user on a hand-built request), which is an extension read. Both rejections
//! happen before `update_device` reaches `establish_connection()`.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::patch;
use entity::org_members::OrgRole;
use oxy_app::server::api::frontline_devices::update_device;
use oxy_app::server::api::middlewares::org_context::OrgContext;
use tower::ServiceExt;
use uuid::Uuid;

const DEVICE: &str = "33333333-3333-3333-3333-333333333333";

fn org_context(org_id: Uuid, role: OrgRole) -> OrgContext {
    let now = chrono::Utc::now().fixed_offset();
    OrgContext {
        org: entity::organizations::Model {
            id: org_id,
            name: "Poke House".into(),
            slug: "poke-house".into(),
            logo: None,
            logo_content_type: None,
            created_at: now,
            updated_at: now,
        },
        membership: entity::org_members::Model {
            id: Uuid::new_v4(),
            org_id,
            user_id: Uuid::new_v4(),
            role,
            created_at: now,
            updated_at: now,
        },
        // A real membership, not the cross-tenant operator fallback — the
        // question here is what a tenant's own Member may do.
        is_global_override: false,
    }
}

/// Stands in for `org_middleware`, which is what populates this extension on
/// the real router.
fn inject_org(
    org_id: Uuid,
    role: OrgRole,
) -> impl Clone + Fn(Request<Body>, Next) -> futures::future::BoxFuture<'static, Response> {
    move |mut req: Request<Body>, next: Next| {
        let ctx = org_context(org_id, role.clone());
        Box::pin(async move {
            req.extensions_mut().insert(ctx);
            next.run(req).await
        })
    }
}

async fn patch_kiosk_as(role: OrgRole) -> StatusCode {
    let org_id = Uuid::new_v4();
    let router = Router::new()
        .route(
            "/orgs/{org_id}/frontline/devices/{id}",
            patch(update_device),
        )
        .layer(middleware::from_fn(inject_org(org_id, role)));
    let req = Request::builder()
        .method("PATCH")
        .uri(format!("/orgs/{org_id}/frontline/devices/{DEVICE}"))
        .header("content-type", "application/json")
        .body(Body::from(r#"{"idle_timeout_seconds":900}"#))
        .unwrap();
    router.oneshot(req).await.expect("oneshot").status()
}

#[tokio::test]
async fn a_plain_member_cannot_change_a_kiosk() {
    assert_eq!(
        patch_kiosk_as(OrgRole::Member).await,
        StatusCode::FORBIDDEN,
        "changing a kiosk's sign-out is an OrgAdmin act, like enrolling and revoking one"
    );
}

#[tokio::test]
async fn an_owner_and_an_admin_are_not_turned_away_by_the_guard() {
    // They fail further down — there is no authenticated user on a hand-built
    // request — but the failure must not be 403, which would mean the ring
    // rejected the very people who enrol these tablets.
    for role in [OrgRole::Owner, OrgRole::Admin] {
        let status = patch_kiosk_as(role.clone()).await;
        assert_ne!(
            status,
            StatusCode::FORBIDDEN,
            "{role:?} must pass OrgAdmin on update_device, got {status}"
        );
    }
}
