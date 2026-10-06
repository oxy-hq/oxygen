//! Every write the staff console mounts on a custom app says what refuses a
//! sandbox agent token there once the route allow-list is not in front of it
//! (sandbox agent credential design, decision 6).
//!
//! The console's platform doors admit the token, and its scope check is by
//! org. So a write route is closed to the token by the allow-list — and, in
//! its handler, by one of two things: the `RefuseSandboxAgent` extractor
//! (`REFUSED_WRITES`), or a decision on the environment the request names
//! (`ENVIRONMENT_DECIDED`). A new write on these prefixes fails here until it
//! is in one of the two.

use oxy_app::server::api::custom_apps_agent_refusal::{ENVIRONMENT_DECIDED, REFUSED_WRITES};
use oxy_app::server::api::middlewares::app_grant_scope::SANDBOX_ALLOWED;

use super::app_grant_scope::shape_of;
use crate::catalog;

/// The mounts the staff console's platform doors stand in front of.
const CONSOLE: [&str; 3] = ["/customer-apps", "/admin/apps", "/admin/app-publish-tokens"];

fn on_console(path: &str) -> bool {
    CONSOLE.iter().any(|prefix| {
        path.strip_prefix(prefix)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    })
}

/// Every write the console mounts, as `(method, path)` relative to `/api`.
fn console_writes() -> Vec<(&'static str, &'static str)> {
    catalog()
        .routes
        .iter()
        .filter(|route| route.surface != "public")
        .filter(|route| !matches!(route.method, "GET" | "HEAD"))
        .filter_map(|route| Some((route.method, route.path.strip_prefix("/api")?)))
        .filter(|(_, path)| on_console(path))
        .collect()
}

fn same(method: &str, shape: &str, route: (&str, &str)) -> bool {
    method == route.0 && shape_of(shape) == shape_of(route.1)
}

#[test]
fn every_console_write_says_what_refuses_the_token_in_its_handler() {
    let writes = console_writes();
    assert!(
        writes.len() >= 20,
        "the walk lost the console ({})",
        writes.len()
    );
    for route in writes {
        let refused = REFUSED_WRITES
            .iter()
            .any(|(method, shape)| same(method, shape, route));
        let decided = ENVIRONMENT_DECIDED
            .iter()
            .any(|(method, shape, _)| same(method, shape, route));
        assert!(
            refused != decided,
            "{} {}: a console write is in exactly one of REFUSED_WRITES (its handler takes \
             RefuseSandboxAgent) and ENVIRONMENT_DECIDED (its handler decides by environment); \
             refused={refused} decided={decided}",
            route.0,
            route.1
        );
    }
}

#[test]
fn every_listed_write_is_still_mounted() {
    let writes = console_writes();
    let listed = REFUSED_WRITES
        .iter()
        .map(|(method, shape)| (*method, *shape))
        .chain(
            ENVIRONMENT_DECIDED
                .iter()
                .map(|(method, shape, _)| (*method, *shape)),
        );
    for (method, shape) in listed {
        assert!(
            writes.iter().any(|route| same(method, shape, *route)),
            "{method} {shape} is no longer a console write: drop the row"
        );
    }
}

/// The writes that take no extractor are the sandbox loop's own and nothing
/// more: each is a write row of the allow-list, or the admin mount of one.
#[test]
fn only_the_loops_own_writes_decide_by_environment() {
    let loop_writes: Vec<(&str, &str)> = SANDBOX_ALLOWED
        .iter()
        .filter(|allowed| allowed.method != "GET" && allowed.shape.starts_with("/customer-apps"))
        .map(|allowed| (allowed.method, allowed.shape))
        .collect();
    for (method, shape, _) in ENVIRONMENT_DECIDED {
        let mirrored = shape.replacen("/admin/apps", "/customer-apps", 1);
        assert!(
            loop_writes.iter().any(|row| same(method, &mirrored, *row)),
            "{method} {shape} is not a write of the sandbox loop: its handler takes \
             RefuseSandboxAgent"
        );
    }
    for (method, shape) in loop_writes {
        assert!(
            ENVIRONMENT_DECIDED
                .iter()
                .any(|(m, s, _)| same(m, s, (method, shape))),
            "{method} {shape} is a write of the sandbox loop with no stated decision"
        );
    }
}
