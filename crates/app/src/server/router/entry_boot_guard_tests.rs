//! The router builders start no background work (decompose BL-W, 2026-11-23).
//!
//! `oxy serve` starts the boot jobs itself (`oxy-server`'s
//! `commands::serve_background`) from the `AgenticState` the builders hand
//! back. A builder that started one again would run it twice in `serve`, and
//! in every test that builds the router. So this reads the three builders'
//! bodies and fails on any call that starts a loop, a driver, a hook or a job.
//!
//! What stays and is not flagged: `new_agentic_state` (the task-router listener,
//! the reaper and the boot cleanup), which the routes need the state for and
//! which leaves with the router-assembly lift. The dead
//! `spawn_billing_reconciler` left `entry.rs` for `oxy-server`'s
//! `drivers::billing_reconciler` in decompose BL1; its needle stays below, so a
//! builder that called one again would still fail.

/// `entry.rs` up to its first column-0 `#[cfg(test)]`, so this file's own
/// needles are never read as calls, with whole-line comments dropped: a comment
/// may name a call, or hold a lone brace that would throw off [`fn_body`].
fn entry_code() -> String {
    let src = include_str!("entry.rs");
    src.split("\n#[cfg(test)]")
        .next()
        .unwrap_or(src)
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The body of `fn <name>(`: from the first `{` after the signature to its
/// matching `}`.
fn fn_body<'a>(src: &'a str, name: &str) -> &'a str {
    let needle = format!("fn {name}(");
    let at = src
        .find(&needle)
        .unwrap_or_else(|| panic!("entry.rs has no `{needle}`"));
    let open = at + src[at..].find('{').expect("a body");
    let mut depth = 0usize;
    for (i, c) in src[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &src[open..=open + i];
                }
            }
            _ => {}
        }
    }
    panic!("`{needle}` has an unbalanced body");
}

/// Calls that start background work, as the boot section spelled them.
const BOOT_CALLS: &[&str] = &[
    "tokio::spawn(",
    "spawn_recovery(",
    "spawn_shutdown_hook(",
    "register_shutdown_hook(",
    "install_global_ticks(",
    "start_fleet_jobs(",
    "spawn_compile_maintenance(",
    "spawn_audit_prune_loop(",
    "spawn_token_usage_flush(",
    "spawn_billing_reconciler(",
    "(job.start)(",
    "oxy_cameras::service::",
];

#[test]
fn no_router_builder_starts_background_work() {
    let src = entry_code();
    for builder in ["api_router", "api_router_with_state", "internal_api_router"] {
        let code = fn_body(&src, builder);
        for call in BOOT_CALLS {
            assert!(
                !code.contains(call),
                "`{builder}` calls `{call}`: a router builder starts no background \
                 work. `oxy serve` starts it from the state the builder returns \
                 (`oxy-server`'s `commands::serve_background`)"
            );
        }
    }
}

/// The guard above reads real bodies: each builder still builds the state
/// `serve` starts from, so an empty match is a broken cut, not a pass.
#[test]
fn the_guard_reads_the_builders() {
    let src = entry_code();
    assert!(fn_body(&src, "api_router").contains("api_router_with_state("));
    assert!(fn_body(&src, "api_router_with_state").contains("new_agentic_state("));
    assert!(fn_body(&src, "internal_api_router").contains("new_agentic_state("));
}
