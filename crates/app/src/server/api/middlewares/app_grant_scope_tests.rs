use std::path::{Path, PathBuf};

use axum::http::HeaderValue;

use super::*;

const GRANTED: &str = "11111111-1111-4111-8111-111111111111";
const OTHER: &str = "22222222-2222-4222-8222-222222222222";
/// Detected by prefix; the fence never validates it.
const SBX: &str = "Bearer oxy_sbx_0123456789abcdefghijABCDEFGHIJ012345";

fn granted(app: Uuid) -> bool {
    app.to_string() == GRANTED
}

fn method(name: &str) -> Method {
    Method::from_bytes(name.as_bytes()).expect("a method")
}

/// `shape` with `{id}` set to `app` and every other parameter to one segment.
fn instantiate(shape: &str, app: &str) -> String {
    shape
        .split('/')
        .map(|segment| match segment {
            "{id}" => app.to_string(),
            s if s.starts_with('{') => "x1".to_string(),
            s => s.to_string(),
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
    let mut map = HeaderMap::new();
    for (name, value) in pairs {
        map.insert(*name, HeaderValue::from_str(value).expect("a header value"));
    }
    map
}

#[test]
fn every_row_is_admitted_for_a_granted_app_and_for_no_other() {
    for allowed in SANDBOX_ALLOWED {
        let own = instantiate(allowed.shape, GRANTED);
        assert!(
            sandbox_admits(&method(allowed.method), &own, granted),
            "{} {} {own}",
            allowed.row,
            allowed.method
        );
        if allowed.shape.contains("{id}") {
            let theirs = instantiate(allowed.shape, OTHER);
            assert!(
                !sandbox_admits(&method(allowed.method), &theirs, granted),
                "{}: another app's id must be refused ({theirs})",
                allowed.row
            );
            let malformed = instantiate(allowed.shape, "not-a-uuid");
            assert!(
                !sandbox_admits(&method(allowed.method), &malformed, |_| true),
                "{}: an id that is not a uuid must be refused",
                allowed.row
            );
        }
    }
}

/// The shapes that sit beside the loop's, each refused for a granted app:
/// one segment more, one fewer, an empty one, another method, another tree.
#[test]
fn a_shape_beside_the_loop_is_refused() {
    let app = GRANTED;
    let beside = [
        ("GET", format!("/customer-apps/{app}/secrets/KEY/value")),
        ("GET", format!("/customer-apps/{app}/secrets/KEY")),
        ("PUT", format!("/customer-apps/{app}/secrets/KEY")),
        ("POST", format!("/customer-apps/{app}/publish")),
        ("DELETE", format!("/customer-apps/{app}/publish")),
        ("POST", format!("/customer-apps/{app}/rollback")),
        ("GET", format!("/customer-apps/{app}")),
        ("PATCH", format!("/customer-apps/{app}")),
        ("DELETE", format!("/customer-apps/{app}")),
        ("GET", format!("/customer-apps/{app}/builds")),
        ("GET", format!("/customer-apps/{app}/publishers")),
        ("GET", format!("/customer-apps/{app}/staging/held")),
        ("GET", "/customer-apps".to_string()),
        ("POST", "/customer-apps".to_string()),
        ("GET", "/customer-apps/fleet-health".to_string()),
        ("POST", "/customer-apps/batch/publish".to_string()),
        ("GET", "/customer-apps/publish".to_string()),
        ("POST", "/customer-apps/publish/oidc-exchange".to_string()),
        ("POST", format!("/customer-apps/{app}/environments/dev-a")),
        ("PATCH", format!("/customer-apps/{app}/environments/dev-a")),
        (
            "GET",
            format!("/customer-apps/{app}/environments/dev-a/extra"),
        ),
        ("GET", format!("/customer-apps/{app}/environments/")),
        ("GET", "/customer-apps//environments".to_string()),
        ("GET", format!("/customer-apps/{app}/invocations/")),
        ("GET", format!("/customer-apps/{app}/invocations/x1")),
        ("POST", format!("/customer-apps/{app}/invocations/x1/held")),
        ("POST", format!("/customer-apps/{app}/functions")),
        ("GET", format!("/customer-apps/{app}/functions/ping/runs")),
        ("GET", format!("/admin/apps/{app}/invocations")),
        ("GET", format!("/admin/customer-apps/{app}/environments")),
        ("GET", format!("/api/customer-apps/{app}/environments")),
        ("HEAD", format!("/customer-apps/{app}/environments")),
        ("POST", "/auth/token".to_string()),
        ("GET", "/auth/token/extra".to_string()),
        ("GET", "/user/tokens".to_string()),
        ("POST", "/user/tokens".to_string()),
        ("GET", "/orgs".to_string()),
        ("GET", "/assume".to_string()),
    ];
    for (verb, path) in beside {
        assert!(
            !sandbox_admits(&method(verb), &path, |_| true),
            "{verb} {path} is not part of the sandbox loop"
        );
    }
}

#[test]
fn the_serve_tree_admits_fn_of_a_dev_sandbox_on_the_product_host_only() {
    let own = [
        ("authorization", SBX),
        ("x-oxy-app-env", "dev-a"),
        ("host", "app.oxygen-hq.com"),
    ];
    assert!(!serve_tree_refuses(
        &Method::POST,
        &headers(&own),
        "acme/store/fn/ping"
    ));
    // A leading slash is how axum hands over the capture.
    assert!(!serve_tree_refuses(
        &Method::POST,
        &headers(&own),
        "/acme/store/fn/ping"
    ));

    let refused_paths = [
        (Method::GET, "acme/store/fn/ping"),
        (Method::POST, "acme/store/fn/ping/extra"),
        (Method::POST, "acme/store/fn/"),
        (Method::POST, "acme/store/fn"),
        (Method::POST, "acme//fn/ping"),
        (Method::GET, "acme/store/"),
        (Method::GET, "acme/store/assets/index.js"),
        (Method::POST, "acme/store/api/anything"),
    ];
    for (verb, path) in refused_paths {
        assert!(
            serve_tree_refuses(&verb, &headers(&own), path),
            "{verb} {path}"
        );
    }

    let refused_headers: [&[(&'static str, &str)]; 6] = [
        // No environment named: that is production.
        &[("authorization", SBX), ("host", "app.oxygen-hq.com")],
        &[("authorization", SBX), ("x-oxy-app-env", "production")],
        &[("authorization", SBX), ("x-oxy-app-env", "staging")],
        &[
            ("authorization", SBX),
            ("x-oxy-app-env", "not an environment"),
        ],
        // An app subdomain decides the environment by its label, so the token
        // is refused there whatever the header says.
        &[
            ("authorization", SBX),
            ("x-oxy-app-env", "dev-a"),
            ("host", "acme--store.customer-apps.oxygen-hq.com"),
        ],
        &[
            ("authorization", SBX),
            ("x-oxy-app-env", "dev-a"),
            ("host", "dev-a--acme--store.customer-apps.oxygen-hq.com"),
        ],
    ];
    for pairs in refused_headers {
        assert!(
            serve_tree_refuses(&Method::POST, &headers(pairs), "acme/store/fn/ping"),
            "{pairs:?}"
        );
    }
}

#[test]
fn the_serve_tree_reads_the_token_from_x_api_key_too() {
    let presented = headers(&[("x-api-key", SBX.trim_start_matches("Bearer "))]);
    assert!(serve_tree_refuses(&Method::GET, &presented, "acme/store/"));
}

#[test]
fn the_serve_tree_half_leaves_every_other_credential_alone() {
    let others: [&[(&'static str, &str)]; 5] = [
        &[],
        &[("cookie", "oxy_session=a.b.c")],
        &[("authorization", "Bearer a.b.c")],
        &[(
            "authorization",
            "Bearer oxy_pat_0123456789abcdefghijABCDEFGHIJ012345",
        )],
        &[("x-api-key", "oxy_0123456789abcdef0123456789abcdef")],
    ];
    for pairs in others {
        for (verb, path) in [
            (Method::GET, "acme/store/"),
            (Method::POST, "acme/store/fn/ping"),
            (Method::GET, "acme/store/assets/index.js"),
        ] {
            assert!(
                !serve_tree_refuses(&verb, &headers(pairs), path),
                "{pairs:?} {verb} {path}"
            );
        }
    }
}

fn rust_sources(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if name != "target" && name != "tests" && !name.starts_with('.') {
                rust_sources(&path, found);
            }
        } else if name.ends_with(".rs") && !name.ends_with("_tests.rs") {
            found.push(path);
        }
    }
}

/// The three entry points that admit a sandbox agent token, and no other.
///
/// Every authenticator is built with `Admit` or `Refuse`, so a new entry point
/// has to choose; this is what makes choosing `Admit` a reviewed decision.
/// Each admitting site sits behind a fence: `api_auth_layers` mounts
/// [`app_grant_scope_middleware`], and `/fn` runs after
/// [`serve_tree_refuses`]. `/logs` is checked by its handler.
#[test]
fn only_three_entry_points_admit_a_sandbox_agent_token() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/");
    let mut sources = Vec::new();
    rust_sources(crates, &mut sources);
    assert!(sources.len() > 500, "the walk lost the workspace");

    let mut admitting: Vec<String> = sources
        .iter()
        .filter(|path| {
            std::fs::read_to_string(path).is_ok_and(|text| {
                text.lines().any(|line| {
                    let code = line.split("//").next().unwrap_or("");
                    code.contains("SandboxAgent::Admit")
                })
            })
        })
        .map(|path| {
            path.strip_prefix(crates)
                .expect("under crates/")
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    admitting.sort();
    assert_eq!(
        admitting,
        [
            "app/src/server/api/custom_apps_functions/mod.rs",
            "app/src/server/api/custom_apps_logs.rs",
            "app/src/server/router/protected.rs",
        ],
        "a new entry point admits a sandbox agent token: put it behind the route allow-list \
         (`app_grant_scope`), then list it here"
    );
}
