use std::path::{Path, PathBuf};

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

/// The four entry points that admit a sandbox agent token, and no other.
///
/// Every authenticator is built with `Admit` or `Refuse`, so a new entry point
/// has to choose; this is what makes choosing `Admit` a reviewed decision.
/// Each admitting site sits behind a fence: `api_auth_layers` mounts
/// `app_grant_scope_middleware`, and `/fn` runs after `serve_tree_refuses`
/// (`oxy_custom_apps::sandbox_reach`), as does the sandbox's page
/// (`serve/mod.rs`, the same fence, then `resolve::authorize` per
/// environment). `/logs` is checked by its handler.
#[test]
fn only_four_entry_points_admit_a_sandbox_agent_token() {
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
            "api-custom-apps/src/custom_apps_functions/mod.rs",
            "api-custom-apps/src/logs.rs",
            "api-custom-apps/src/serve/mod.rs",
            "app/src/server/router/protected.rs",
        ],
        "a new entry point admits a sandbox agent token: put it behind the route allow-list \
         (`app_grant_scope`), then list it here"
    );
}
