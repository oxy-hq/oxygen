//! The staff-console handlers that take no extractor of their own, pinned behind
//! the guard their section relies on, on the router `serve` builds.
//!
//! Every handler below reads or writes with nothing that refuses a caller: no
//! `AuthenticatedUserExtractor`, no `role_guards::*`, and for the two airway
//! `DELETE`s not even a body to reject. The only thing between a request and
//! the handler is the capability layer `oxy-server`'s `admin::extracted_sections`
//! puts on each section (or, for the Billing queue, the strict owner guard).
//! Drop that layer and the handler serves anyone the staff door admits, which
//! is any platform standing at all. `oxy-api-airway`'s `admin_mount` test
//! builds the section bare and cannot see the layer; this one can.
//!
//! Three callers, each sent every route:
//!
//! * the guest, who is not staff: 403, the staff door;
//! * the guest with a grant that lacks the route's capability (an App Operator
//!   for `PlatformOperate` and `PlatformPartners`; an App Operator and a Global
//!   Admin for the owner-only Billing queue): 403, the section's layer. A
//!   positive control proves the door admitted the grant, so the 403 is the
//!   section's and not the door's;
//! * the guest as the Global Owner, TRACE only: 405, so each path is mounted
//!   and reachable by someone who holds the capability.
//!
//! **No handler runs, for any caller, and that is load-bearing** (the reason
//! for #4587). A read is sent its own verb, since the guard answers it before
//! the handler, and a regression would only read. A write is sent TRACE, which
//! the same route layer answers: axum layers a route's method-not-allowed
//! fallback with its methods, so TRACE is refused exactly where the write is,
//! and no handler takes it. Each write path here is mounted by one section
//! only, so its fallback is that section's. (`/admin/orgs` is two sections'
//! path, which is why its Billing verb, a read, is sent as itself.) The airway
//! source kind is one no other test writes, not `toast`.
//!
//! What it writes: only the guest's own grant row, inserted and removed around
//! each pass, as the `served_router_tests` admin cases do. It builds
//! `api_router(..)`, the unscoped `cleanup_stale_runs` writer, so it skips
//! without a database and `.config/nextest.toml` pins it into `serial-db`.

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use oxy_app_core::serve_mode::ServeMode;
use oxy_server::router::api_router;
use oxy_server_authz::Action;
use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter};
use tower::ServiceExt;

const GUEST: &str = oxy_auth::user::LOCAL_GUEST_EMAIL;

/// A path no section mounts: the outer router's 404 for every caller.
const UNMOUNTED: &str = "/admin/nothing-mounts-this";

/// `(verb the handler takes, path, the guard its section relies on)`. `{k}` is
/// an airway source kind, `{w}` a workspace, `{o}` an org and `{p}` a partner.
const NO_EXTRACTOR_ROUTES: &[(&str, &str, Action)] = &[
    // `oxy-api-airway`: `get/put/delete_deployment_config` and
    // `put/delete_global_config` take no extractor that refuses a caller; the
    // rest of the section is pinned beside them.
    ("GET", "/admin/airway/config", Action::PlatformOperate),
    ("PUT", "/admin/airway/config/{k}", Action::PlatformOperate),
    (
        "DELETE",
        "/admin/airway/config/{k}",
        Action::PlatformOperate,
    ),
    (
        "PUT",
        "/admin/airway/config/{k}/workspaces/{w}",
        Action::PlatformOperate,
    ),
    (
        "DELETE",
        "/admin/airway/config/{k}/workspaces/{w}",
        Action::PlatformOperate,
    ),
    (
        "GET",
        "/admin/airway/config/{k}/preview",
        Action::PlatformOperate,
    ),
    (
        "GET",
        "/admin/airway/deployment-config",
        Action::PlatformOperate,
    ),
    (
        "PUT",
        "/admin/airway/deployment-config",
        Action::PlatformOperate,
    ),
    (
        "DELETE",
        "/admin/airway/deployment-config",
        Action::PlatformOperate,
    ),
    // `oxy-api-admin`: the flag console's `list` and `evaluate`, the route-role
    // table, and the task fleet's schedule list (a nest, not a section).
    ("GET", "/admin/feature-flags", Action::PlatformOperate),
    (
        "GET",
        "/admin/feature-flags/evaluate",
        Action::PlatformOperate,
    ),
    ("GET", "/admin/routing-manifest", Action::PlatformOperate),
    (
        "GET",
        "/admin/internal-jobs/scheduled",
        Action::PlatformOperate,
    ),
    // `oxy-api-tenancy`: the partner registry's `list_partners` / `get_partner`.
    ("GET", "/admin/partners", Action::PlatformPartners),
    ("GET", "/admin/partners/{p}", Action::PlatformPartners),
    // `oxy-api-billing`'s `admin/billing.rs`: the Billing queue's nine.
    ("GET", "/admin/orgs", Action::PlatformOwnerOnly),
    ("GET", "/admin/billing/prices", Action::PlatformOwnerOnly),
    (
        "GET",
        "/admin/orgs/{o}/billing/subscription",
        Action::PlatformOwnerOnly,
    ),
    (
        "POST",
        "/admin/orgs/{o}/billing/provision-subscription",
        Action::PlatformOwnerOnly,
    ),
    (
        "POST",
        "/admin/orgs/{o}/billing/provision-checkout",
        Action::PlatformOwnerOnly,
    ),
    (
        "GET",
        "/admin/orgs/{o}/billing/checkout",
        Action::PlatformOwnerOnly,
    ),
    (
        "POST",
        "/admin/orgs/{o}/billing/checkout/resend",
        Action::PlatformOwnerOnly,
    ),
    (
        "POST",
        "/admin/orgs/{o}/billing/checkout/cancel",
        Action::PlatformOwnerOnly,
    ),
    (
        "POST",
        "/admin/orgs/{o}/billing/resync",
        Action::PlatformOwnerOnly,
    ),
];

/// A staff grant and where the door demonstrably admits it: a TRACE to a
/// section whose capability the role holds answers 405, not the door's 403.
struct StaffWithout {
    role: &'static str,
    admitted_at: &'static str,
}

const APP_OPERATOR: StaffWithout = StaffWithout {
    role: "app_operator",
    // `oxy-api-custom-apps`' CI publish tokens, behind `PlatformApps`.
    admitted_at: "/admin/app-publish-tokens",
};

const GLOBAL_ADMIN: StaffWithout = StaffWithout {
    role: "global_admin",
    admitted_at: "/admin/feature-flags",
};

/// The staff roles that lack `guard`. A Global Admin holds every capability,
/// so only the owner-only ring refuses it.
fn lacking(guard: Action) -> &'static [StaffWithout] {
    match guard {
        Action::PlatformOwnerOnly => &[APP_OPERATOR, GLOBAL_ADMIN],
        _ => &[APP_OPERATOR],
    }
}

/// `{k}`, `{w}`, `{o}` and `{p}` filled in. The ids exist nowhere.
fn concrete(path: &str) -> String {
    path.replace("{k}", "guard-pin-probe")
        .replace("{w}", &uuid::Uuid::new_v4().to_string())
        .replace("{o}", &uuid::Uuid::new_v4().to_string())
        .replace("{p}", &uuid::Uuid::new_v4().to_string())
}

/// What a refused caller is sent: a read as itself, a write as TRACE (see the
/// module docs).
fn probe_verb(verb: &str) -> Method {
    match verb {
        "GET" => Method::GET,
        _ => Method::TRACE,
    }
}

async fn status(router: &Router, method: Method, path: &str) -> StatusCode {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .body(Body::empty())
        .unwrap();
    router.clone().oneshot(req).await.expect("oneshot").status()
}

struct OwnerEnv(Option<String>);

impl OwnerEnv {
    fn set(email: &str) -> Self {
        let prev = std::env::var("OXY_OWNER").ok();
        // SAFETY: nextest runs each test in its own process.
        unsafe { std::env::set_var("OXY_OWNER", email) };
        Self(prev)
    }
}

impl Drop for OwnerEnv {
    fn drop(&mut self) {
        match &self.0 {
            // SAFETY: as above.
            Some(v) => unsafe { std::env::set_var("OXY_OWNER", v) },
            None => unsafe { std::env::remove_var("OXY_OWNER") },
        }
    }
}

/// Every route's answer to the guest as it stands, as `(verb, path, got)`.
async fn answers(router: &Router, routes: &[(&str, String, Action)]) -> Vec<(Method, String, u16)> {
    let mut out = Vec::new();
    for (verb, path, _) in routes {
        let method = probe_verb(verb);
        let got = status(router, method.clone(), path).await;
        out.push((method, path.clone(), got.as_u16()));
    }
    out
}

/// The guest holding `role`, unbounded: `(the admitted-at control, every
/// route's answer)`. The grant is removed before this returns, so a failed
/// assertion afterwards leaves nothing behind.
async fn as_staff(
    router: &Router,
    db: &sea_orm::DatabaseConnection,
    role: &str,
    admitted_at: &str,
    routes: &[(&str, String, Action)],
) -> (StatusCode, Vec<(Method, String, u16)>) {
    let now = chrono::Utc::now();
    let grant = entity::app_admins::ActiveModel {
        id: ActiveValue::Set(uuid::Uuid::new_v4()),
        email: ActiveValue::Set(GUEST.into()),
        granted_by: ActiveValue::Set(None),
        created_at: ActiveValue::Set(now.into()),
        role: ActiveValue::Set(role.into()),
        scope_all: ActiveValue::Set(true),
        updated_at: ActiveValue::Set(now.into()),
    }
    .insert(db)
    .await
    .expect("seed the grant");
    // The grant read is cached per process, and the non-staff pass put the
    // guest's "no grant" in it.
    oxy_server_authz::globals::invalidate_admin_cache();
    let control = status(router, Method::TRACE, admitted_at).await;
    let got = answers(router, routes).await;
    entity::app_admins::Entity::delete_by_id(grant.id)
        .exec(db)
        .await
        .expect("remove the grant");
    oxy_server_authz::globals::invalidate_admin_cache();
    (control, got)
}

fn assert_refused(who: &str, got: &[(Method, String, u16)]) {
    for (method, path, status) in got {
        assert_eq!(
            *status, 403,
            "{who} {method} {path}: must be refused before the handler, got {status}"
        );
    }
}

#[tokio::test]
async fn no_extractor_admin_handlers_refuse_callers_without_their_guard() {
    if std::env::var("OXY_DATABASE_URL").is_err() {
        return;
    }
    let (router, _external, _preagg) = api_router(
        ServeMode::Cloud,
        false,
        None,
        std::path::PathBuf::new(),
        tokio_util::sync::CancellationToken::new(),
        false,
        oxy_server::surface_seams(),
    )
    .await
    .expect("router built");
    let routes: Vec<(&str, String, Action)> = NO_EXTRACTOR_ROUTES
        .iter()
        .map(|(verb, path, guard)| (*verb, concrete(path), *guard))
        .collect();

    // A run that panicked mid-way leaves a grant behind, which would make the
    // guest staff below; this module is serial (`serial-db`).
    let db = oxy::database::client::establish_connection()
        .await
        .expect("database");
    entity::app_admins::Entity::delete_many()
        .filter(entity::app_admins::Column::Email.eq(GUEST))
        .exec(&db)
        .await
        .expect("clear a stale guest grant");
    oxy_server_authz::globals::invalidate_admin_cache();

    assert_eq!(
        status(&router, Method::GET, UNMOUNTED).await,
        StatusCode::NOT_FOUND,
        "control: an unmounted path is the router's 404"
    );
    assert_refused("non-staff", &answers(&router, &routes).await);

    for staff in [APP_OPERATOR, GLOBAL_ADMIN] {
        let refused: Vec<_> = routes
            .iter()
            .filter(|(_, _, guard)| lacking(*guard).iter().any(|s| s.role == staff.role))
            .cloned()
            .collect();
        let (control, got) = as_staff(&router, &db, staff.role, staff.admitted_at, &refused).await;
        assert_eq!(
            control,
            StatusCode::METHOD_NOT_ALLOWED,
            "{}: the door must admit the grant at TRACE {}, or the refusals below are \
             the door's and prove nothing about the section's guard",
            staff.role,
            staff.admitted_at
        );
        assert!(!got.is_empty(), "{}: no route lacks its grant", staff.role);
        assert_refused(staff.role, &got);
    }

    let owner = OwnerEnv::set(GUEST);
    let mut mounted = Vec::new();
    for (_, path, _) in &routes {
        mounted.push((path.clone(), status(&router, Method::TRACE, path).await));
    }
    drop(owner);
    for (path, got) in mounted {
        assert_eq!(
            got,
            StatusCode::METHOD_NOT_ALLOWED,
            "owner TRACE {path}: must be mounted and reachable by a holder, got {got}"
        );
    }
}
