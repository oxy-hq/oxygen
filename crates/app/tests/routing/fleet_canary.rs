//! The dynamic half of the compile-boundary enforcement.
//!
//! Everything else guarding it is static — route classification, the
//! `ConfigManager<WorkingCopy>` capability, the allowlist tests. Static checks share one
//! weakness that matters here: they tell you about the sites you thought to
//! check, never the ones you didn't. Both production incidents were sites nobody
//! had thought to check.
//!
//! `internal-docs/compile-boundary.md` calls the fix "the target enforcement":
//! boot a process configured as a diskless `serve` replica, drive traffic at it,
//! and fail on anything that reaches for a working copy. The instrument is
//! `oxy::workspace_fs_probe`, wired into the path resolvers themselves — the one
//! thing every filesystem read needs — so it does not depend on anyone
//! remembering to instrument their own call site.
//!
//! ## What this covers, and what it does not
//!
//! Driving *every registered route* needs a live server, real auth, and a valid
//! body per route; axum also exposes no way to enumerate its routes, so the list
//! would have to be hand-maintained — the very thing that failed. What this does
//! instead is verify the instrument on the paths a test can reach, and pin the
//! wiring that makes it work in production, where it runs against real traffic
//! on every replica.
//!
//! The gap is deliberate and worth stating: **this is the detector, not yet the
//! sweep.** A leak on a route no test drives is caught by the WARN + counter in
//! production rather than in CI — which is still the difference between four
//! days and a dashboard.

use oxy::workspace_fs_probe::{
    leaks, process_owns_workspace_files, reset_leaks, set_process_owns_workspace_files,
};

/// The probe's owns-files flag and leak counter are process-global, so the four
/// cases that read or write them are serialized against each other.
///
/// `AsDisklessReplica` below restores the flag on drop, which is enough while a
/// test runs alone and worth nothing while another runs beside it. This file was
/// its own binary when that guard was written; in `tests/routing/` it shares a
/// process with six other modules, and a `Drop` impl private to one file
/// excludes exactly nobody.
use serial_test::serial;

/// Restores the global on drop so one test's configuration can't leak into
/// another's, whatever the outcome.
struct AsDisklessReplica;

impl AsDisklessReplica {
    fn enter() -> Self {
        set_process_owns_workspace_files(false);
        reset_leaks();
        AsDisklessReplica
    }
}

impl Drop for AsDisklessReplica {
    fn drop(&mut self) {
        set_process_owns_workspace_files(true);
        reset_leaks();
    }
}

#[test]
#[serial]
fn the_probe_defaults_to_owning_files() {
    // An un-configured process is a single-process instance — `oxy run`, a test,
    // `oxy serve --local` — and those genuinely own their files. Defaulting the
    // other way would make every local run warn, and a warning everyone learns
    // to ignore is worse than none.
    assert!(
        process_owns_workspace_files(),
        "the probe must default to owning files; only the split fleet opts out"
    );
}

#[tokio::test]
#[serial]
async fn resolving_a_workspace_path_on_a_replica_is_recorded() {
    let _guard = AsDisklessReplica::enter();

    // The workspace row a replica would have: a path from the database column,
    // pointing at a directory this pod does not have.
    let row = entity::workspaces::Model {
        id: uuid::Uuid::new_v4(),
        path: Some("/nonexistent/workspace/oxy".to_string()),
        ..fake_workspace()
    };

    let resolved = oxy::adapters::workspace::effective_workspace_path(&row, None)
        .await
        .expect("resolving a path never fails — that is the whole problem");

    assert_eq!(
        resolved,
        std::path::PathBuf::from("/nonexistent/workspace/oxy"),
        "the resolver returns the database column verbatim without stat-ing it"
    );
    assert_eq!(
        leaks(),
        1,
        "a workspace path resolved on a diskless process must be recorded — this \
         is the signal that a route is reaching for a working copy that is not \
         here, and it is what fires in production when a static check missed one"
    );
}

#[tokio::test]
#[serial]
async fn the_same_resolution_is_silent_on_a_node_that_owns_files() {
    // Counter-guard. A probe that fires on the ide too would be noise, and noise
    // is how a real signal gets muted.
    set_process_owns_workspace_files(true);
    reset_leaks();

    let row = entity::workspaces::Model {
        id: uuid::Uuid::new_v4(),
        path: Some("/nonexistent/workspace/oxy".to_string()),
        ..fake_workspace()
    };
    let _ = oxy::adapters::workspace::effective_workspace_path(&row, None).await;

    assert_eq!(
        leaks(),
        0,
        "the ide resolving its own workspace path is normal operation"
    );
}

#[test]
fn every_route_that_owns_a_disk_is_classified_ide_only() {
    // The static counterpart, phrased as the canary would: for each route the
    // manifest claims needs the ide, `classify` must actually say so. Catches a
    // `FleetOk` carve-out silently shadowing an `IdeOnly` entry — the two lists
    // are scanned in order, and the carve-outs win.
    use oxy_app::server::role_manifest::{
        RouteRole, classify, dump_manifest, install_route_declarations_for_tests,
    };

    // `DECLARED` is a `OnceLock` and `dump_manifest()` is `unwrap_or_default()`,
    // so without this the filter below runs over an EMPTY vec and the assertion
    // passes having inspected nothing. That is how it behaved while this file
    // was its own binary — every case here reads the manifest, none installed
    // it. Six of the six other modules now sharing this binary do install it, so
    // under `cargo test` the outcome would otherwise depend on which ran first:
    // vacuous when this one leads, live when a sibling does. `#[serial]` cannot
    // repair that — a `OnceLock` has no restore point the way the probe flag
    // does — so the fix is to install it here and refuse to run vacuously.
    install_route_declarations_for_tests();

    let manifest = dump_manifest();
    assert!(
        !manifest.is_empty(),
        "the manifest is empty, so the check below would inspect nothing and \
         pass — install_route_declarations_for_tests() did not take"
    );

    let shadowed: Vec<String> = manifest
        .into_iter()
        .filter(|(_, _, role)| *role == "ide-only")
        .filter(|(method, pattern, _)| {
            let concrete: String = pattern
                .split('/')
                .map(|seg| if seg.starts_with('{') { "x" } else { seg })
                .collect::<Vec<_>>()
                .join("/");
            let m = if *method == "*" { "GET" } else { method };
            classify(m, &concrete) != RouteRole::IdeOnly
        })
        .map(|(method, pattern, _)| format!("{method} {pattern}"))
        .collect();

    assert!(
        shadowed.is_empty(),
        "these are listed IdeOnly but do not classify that way — a FleetOk \
         carve-out is shadowing them, and they will run on a replica with no \
         working copy:\n  {}",
        shadowed.join("\n  ")
    );
}

/// A workspace row carrying only what path resolution reads: `id` and `path`.
/// The rest is inert here.
/// `/meta` is mounted `route_fleet`, so a replica answers it — and until this
/// was split it answered by delegating to `get_workspace`, which resolves the
/// workspace root and then runs `detect_git_mode` / `get_default_branch` /
/// `get_current_branch` and builds an `Origin::Disk` ConfigManager, before
/// `/meta` threw every git field away.
///
/// Two costs, and the second is the one that showed: on an fs-writable pod
/// whose volume is not populated yet, `build_workspace_details_response` raises
/// the "materializing" 503 — so `/meta` reported unavailable for data no volume
/// holds. Its whole response is DB row plus constants.
///
/// Asserting through the probe rather than on the status code: a 200 could
/// still have walked the filesystem to produce it.
#[tokio::test]
#[serial]
async fn the_meta_route_reads_no_filesystem_on_a_replica() {
    let _guard = AsDisklessReplica::enter();

    let workspace_id = uuid::Uuid::new_v4();
    let row = entity::workspaces::Model {
        id: workspace_id,
        name: "canary".to_string(),
        path: Some("/nonexistent/workspace/oxy".to_string()),
        ..fake_workspace()
    };

    let meta = oxy_app::server::api::workspaces::get_workspace_meta(
        axum::extract::State(oxy_app::server::router::bare_app_state()),
        oxy_auth::extractor::AuthenticatedUserExtractor(oxy_auth::types::AuthenticatedUser {
            id: uuid::Uuid::new_v4(),
            email: Some("canary@example.com".to_string()),
            name: "canary".to_string(),
            picture: None,
            status: entity::users::UserStatus::Active,
            credential: None,
        }),
        oxy_server_authz::workspace_role::EffectiveWorkspaceRole(
            entity::workspace_members::WorkspaceRole::Admin,
        ),
        axum::extract::Extension(row),
        axum::extract::Path(workspace_id),
    )
    .await
    .expect("meta needs no working copy, so it cannot fail for want of one");

    assert_eq!(meta.0.id, workspace_id);
    assert_eq!(meta.0.name, "canary");
    assert_eq!(
        leaks(),
        0,
        "`/meta` resolved a workspace path on a diskless replica — it is \
         mounted FleetOk, so every field it returns must come from the database \
         row or a constant"
    );
}

fn fake_workspace() -> entity::workspaces::Model {
    let now = chrono::Utc::now().into();
    entity::workspaces::Model {
        id: uuid::Uuid::nil(),
        name: String::new(),
        git_namespace_id: None,
        git_remote_url: None,
        created_at: now,
        updated_at: now,
        path: None,
        last_opened_at: None,
        created_by: None,
        org_id: None,
        status: entity::workspaces::WorkspaceStatus::Ready,
        error: None,
        monthly_vlm_budget_micros: None,
        current_revision_id: None,
        default_branch: None,
        repo_subdir: None,
    }
}

/// The branch-hint counterpart of the leak probe.
///
/// `role_manifest` says which routes may reach a replica; this says which ones
/// arrived carrying a question only the ide can answer. A replica skips the
/// branch gate by design — it has no working copy, so the promoted revision is
/// the only thing it can serve — which means a caller asking for a feature
/// branch is answered with `main` and no error. That is indistinguishable from
/// working software, so it is counted.
///
/// Zero on a healthy fleet. Non-zero means a route takes `?branch=` and is not
/// classified `IdeOnly`.
///
/// **This asserts an absolute on a monotonic process-global**, which is a claim
/// about the whole binary rather than about this test: nothing anywhere in
/// `tests/routing/` may drive traffic through `compiled_reader`. That held
/// trivially while this file was its own binary and is now an unwritten
/// invariant across seven modules. `#[serial]` would not protect it — a counter
/// that only goes up has no restore point — so the constraint is stated here
/// instead: a case added to this group that reaches `compiled_reader` has to
/// convert this into a delta (read before, read after) rather than leave it
/// asserting zero.
#[test]
fn the_dropped_branch_hint_counter_starts_at_zero() {
    assert_eq!(
        oxy_app::server::api::compiled_reader::branch_hints_dropped(),
        0,
        "nothing in this process has dropped a branch hint yet"
    );
}
