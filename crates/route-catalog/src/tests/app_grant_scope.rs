//! The sandbox agent token's route allow-list, checked against the generated
//! route table (sandbox agent credential design §3.1).
//!
//! The walk exercises the *matcher* against every real route shape, so a
//! matcher that drifts permissive fails here. Testing only the listed routes
//! would miss that.

use axum::http::Method;
use oxy_app::server::api::middlewares::app_grant_scope::{
    PUBLIC_ROUTES, SANDBOX_ALLOWED, sandbox_admits,
};
use uuid::Uuid;

use crate::catalog;

const GRANTED: &str = "11111111-1111-4111-8111-111111111111";

fn granted(app: Uuid) -> bool {
    app.to_string() == GRANTED
}

/// A template with its parameter names dropped, so `{name}` and
/// `{sandbox_name}` compare equal. `{id}` is kept: it is the granted app.
pub(super) fn shape_of(template: &str) -> String {
    template
        .split('/')
        .map(|segment| match segment {
            "{id}" => "{id}",
            s if s.starts_with('{') => "{}",
            s => s,
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// A template with the granted app as `{id}`, a uuid for every other id-like
/// parameter, and a plain segment for the rest.
fn instantiate(template: &str) -> String {
    template
        .split('/')
        .map(|segment| match segment {
            s if s.starts_with('{') && s.ends_with("id}") => GRANTED.to_string(),
            s if s.starts_with('{') => "dev-x1".to_string(),
            s => s.to_string(),
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn listed(method: &str, path: &str) -> bool {
    SANDBOX_ALLOWED
        .iter()
        .any(|allowed| allowed.method == method && shape_of(allowed.shape) == shape_of(path))
}

/// Every route the fence is mounted in front of, as `(method, path)` relative
/// to `/api`: the protected surfaces. The public surface carries no fence and
/// is walked by its own test.
fn fenced_routes() -> Vec<(&'static str, &'static str)> {
    catalog()
        .routes
        .iter()
        .filter(|route| route.surface != "public")
        .filter_map(|route| Some((route.method, route.path.strip_prefix("/api")?)))
        .collect()
}

#[test]
fn every_route_is_refused_unless_listed() {
    let routes = fenced_routes();
    assert!(
        routes.len() > 200,
        "the walk lost the routes ({})",
        routes.len()
    );
    let mut admitted = 0;
    for (method, template) in routes {
        let path = instantiate(template);
        let verb = Method::from_bytes(method.as_bytes()).expect("a method");
        let admits = sandbox_admits(&verb, &path, granted);
        assert_eq!(
            admits,
            listed(method, template),
            "{method} {template} (as {path}): a sandbox agent token is refused on every route \
             that is not a row of SANDBOX_ALLOWED"
        );
        admitted += usize::from(admits);
    }
    assert!(
        admitted >= SANDBOX_ALLOWED.len() - 1,
        "{admitted} rows matched a route"
    );
}

#[test]
fn every_allowed_route_is_still_mounted() {
    let routes = fenced_routes();
    for allowed in SANDBOX_ALLOWED {
        assert!(
            routes
                .iter()
                .any(|(method, template)| *method == allowed.method
                    && shape_of(template) == shape_of(allowed.shape)),
            "{} {} {} is no longer mounted: drop the row, or a future route of that shape \
             inherits it",
            allowed.row,
            allowed.method,
            allowed.shape
        );
    }
}

/// No fence is mounted on the public router, so every `/customer-apps/…` route
/// there states whether it admits the token, in `PUBLIC_ROUTES`.
#[test]
fn every_public_custom_app_route_declares_its_sandbox_treatment() {
    let public: Vec<&str> = catalog()
        .routes
        .iter()
        .filter(|route| route.surface == "public")
        .filter_map(|route| route.path.strip_prefix("/api"))
        .filter(|path| path.starts_with("/customer-apps/"))
        .collect();
    assert!(public.len() >= 5, "the walk lost the public routes");
    for path in &public {
        assert!(
            PUBLIC_ROUTES
                .iter()
                .any(|(listed, _)| shape_of(listed) == shape_of(path)),
            "{path} is a public custom-app route with no stated answer to a sandbox agent \
             token: list it in PUBLIC_ROUTES, refused unless it is part of the sandbox loop"
        );
    }
    for (listed, _) in PUBLIC_ROUTES {
        assert!(
            public.iter().any(|path| shape_of(path) == shape_of(listed)),
            "{listed} is no longer a public route — drop it from PUBLIC_ROUTES"
        );
    }
    let admitting: Vec<&str> = PUBLIC_ROUTES
        .iter()
        .filter(|(_, admits)| *admits)
        .map(|(path, _)| *path)
        .collect();
    assert_eq!(admitting, ["/customer-apps/{org_slug}/{app_slug}/logs"]);
}
