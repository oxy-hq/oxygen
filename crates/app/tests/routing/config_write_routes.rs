//! Routes that write `config.yml` must be `IdeOnly`, and must say so twice.
//!
//! `POST /api/{ws}/databases` wrote the working copy while classified `FleetOk`
//! by the unlisted-route default. On a stateless replica the parent directory
//! does not exist, so adding a warehouse succeeded or failed depending on which
//! pod the load balancer picked.
//!
//! Two independent guards, because either alone has a hole:
//!
//! 1. **Classification** — `role_manifest` routes the request to a node that
//!    owns a disk. Fails open: forget an entry and the default is `FleetOk`.
//! 2. **`ensure_fs_writable`** in the handler — classification-independent, so a
//!    future misclassification fails loudly instead of writing to an ephemeral
//!    filesystem. That function existed with a precise error message and zero
//!    production call sites until this change.
//!
//! This test pins both. It is a source scan rather than a request test because
//! the failure it guards against is a *missing* entry, which no request against
//! the current router can exercise.

use std::path::{Path, PathBuf};

fn repo_file(rel: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Handlers that mutate `config.yml`, and the `ConfigManager` method that does
/// it. Adding a write method to `ConfigManager` without adding it here is the
/// gap this list exists to close — see the counter-check below, which fails if
/// the set of writing methods grows.
const WRITE_HANDLERS: &[(&str, &str)] = &[
    ("src/server/api/database.rs", "add_databases"),
    ("src/server/api/apps.rs", "upsert_integration"),
    ("src/server/api/apps.rs", "remove_integration_by_kind"),
    ("src/server/api/data_repo.rs", "add_repository"),
    ("src/server/api/data_repo.rs", "remove_repository"),
    // Moved out of oxy-app into the `oxy-api-onboarding` sibling crate, now
    // `oxy-api-tenancy`'s `onboarding` module. The
    // path is relative to `crates/app`, and the guard still applies: crossing a
    // crate line does not stop a handler rewriting `config.yml`.
    (
        "../api-tenancy/src/onboarding/handlers.rs",
        "remove_database",
    ),
    ("../api-tenancy/src/onboarding/handlers.rs", "remove_model"),
];

#[test]
fn every_config_writing_handler_guards_the_filesystem() {
    let mut missing = Vec::new();
    for (file, method) in WRITE_HANDLERS {
        let src = repo_file(file);
        if !src.contains(&format!(".{method}(")) {
            // The call moved or was renamed; the list is stale rather than the
            // code being wrong, but a stale list silently stops guarding.
            missing.push(format!(
                "{file}: `{method}` no longer called — update this list"
            ));
            continue;
        }
        if !src.contains("ensure_fs_writable") {
            missing.push(format!(
                "{file}: calls `{method}` (writes config.yml) but never calls \
                 `ensure_fs_writable`"
            ));
        }
    }
    assert!(
        missing.is_empty(),
        "config.yml writes without a filesystem guard:\n  {}\n\n\
         On a stateless serve replica the write lands nowhere useful. Add \
         `crate::server::role_manifest::ensure_fs_writable(\"…\")?` at the top of \
         the handler.",
        missing.join("\n  ")
    );
}

/// The routes those handlers sit behind, as `(method, path)` pairs the manifest
/// must classify `IdeOnly`.
const WRITE_ROUTES: &[(&str, &str)] = &[
    ("POST", "/api/{workspace_id}/databases"),
    ("POST", "/api/{workspace_id}/app-integrations"),
    ("DELETE", "/api/{workspace_id}/app-integrations/{kind}"),
    ("POST", "/api/{workspace_id}/repositories"),
    ("DELETE", "/api/{workspace_id}/repositories/{name}"),
];

#[test]
fn every_config_writing_route_is_ide_only() {
    oxy_app::server::role_manifest::install_route_declarations_for_tests();
    // Ask the real classifier rather than grepping the manifest: a route can be
    // correctly `IdeOnly` through a `{*rest}` wildcard with no verbatim entry of
    // its own, and a verbatim entry can still be shadowed by a `FleetOk`
    // carve-out scanned first. Only `classify` knows which.
    let unclassified: Vec<String> = WRITE_ROUTES
        .iter()
        .filter(|(method, pattern)| {
            let concrete = concrete_path(pattern);
            oxy_app::server::role_manifest::classify(method, &concrete)
                != oxy_app::server::role_manifest::RouteRole::IdeOnly
        })
        .map(|(method, path)| format!("{method} {path}"))
        .collect();

    assert!(
        unclassified.is_empty(),
        "these routes write `config.yml` in the working copy but do not classify \
         `IdeOnly`:\n  {}\n\n\
         Unlisted routes default to FleetOk (the last line of `classify`), so they \
         run on stateless replicas that have no working copy.",
        unclassified.join("\n  ")
    );
}

/// Turn a manifest-style pattern into a URI the classifier can match, the way a
/// real request would arrive.
fn concrete_path(pattern: &str) -> String {
    pattern
        .split('/')
        .map(|seg| if seg.starts_with('{') { "x" } else { seg })
        .collect::<Vec<_>>()
        .join("/")
}

/// Counter-check: if `ConfigManager` grows a new method that writes
/// `config.yml`, `WRITE_HANDLERS` above is now incomplete and silently guards
/// less than it claims.
#[test]
fn the_set_of_config_writing_methods_has_not_grown() {
    let manager = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../core/src/config/manager.rs"),
    )
    .expect("read core's manager.rs");

    let known: Vec<&str> = vec![
        "update_databases",
        "add_database",
        "add_databases",
        "remove_database",
        "remove_model",
        "add_repository",
        "remove_repository",
        "upsert_integration",
        "remove_integration_by_kind",
    ];

    // Every method whose body reaches `write_config` is a config.yml writer.
    let mut found = Vec::new();
    let mut current: Option<String> = None;
    for line in manager.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed
            .strip_prefix("pub async fn ")
            .or_else(|| trimmed.strip_prefix("pub fn "))
        {
            current = Some(
                rest.chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect(),
            );
        } else if line.contains("write_config(")
            && let Some(name) = current.clone()
            && !found.contains(&name)
        {
            found.push(name);
        }
    }

    let unexpected: Vec<&String> = found
        .iter()
        .filter(|f| !known.contains(&f.as_str()))
        .collect();
    assert!(
        unexpected.is_empty(),
        "new `config.yml` writers found in ConfigManager: {unexpected:?}\n\n\
         Add the handler that calls each one to `WRITE_HANDLERS`, give it an \
         `ensure_fs_writable` guard, and classify its route `IdeOnly`."
    );

    // Without this the scan is fail-open: point it at a file that no longer
    // contains `write_config` and it finds nothing, reports nothing unexpected,
    // and passes — guarding less than it claims while looking green.
    let missing: Vec<&&str> = known
        .iter()
        .filter(|k| !found.iter().any(|f| f == *k))
        .collect();
    assert!(
        missing.is_empty(),
        "expected `config.yml` writers not found in ConfigManager: {missing:?}\n\n\
         If the writers moved to another type, repoint this scan at that file. \
         Until then it passes by finding nothing at all."
    );
}

/// The external API mounts the same handlers under `/external/api`. Being a
/// sibling of the main router, it sat outside `enforce_role` entirely, so every
/// route there was unclassified — including `/world-model/events`, which the
/// main surface classified while this one did not. `classify` normalises the
/// prefix so one entry governs both, whatever that entry says.
#[test]
fn the_external_api_surface_classifies_like_the_main_one() {
    use oxy_app::server::role_manifest::{RouteRole, classify};

    for (method, main, external) in [
        (
            "GET",
            "/api/x/world-model/events",
            "/external/api/x/world-model/events",
        ),
        ("GET", "/api/x/secrets/env", "/external/api/x/secrets/env"),
        ("POST", "/api/x/databases", "/external/api/x/databases"),
        ("GET", "/api/x/threads", "/external/api/x/threads"),
    ] {
        assert_eq!(
            classify(method, main),
            classify(method, external),
            "`{external}` must classify the same as `{main}` — it is the same \
             handler behind a different prefix"
        );
    }

    // And the normalisation must not have made everything IdeOnly by accident.
    assert_eq!(
        classify("GET", "/external/api/x/threads"),
        RouteRole::FleetOk,
        "a Postgres read on the external surface must stay FleetOk"
    );
}
