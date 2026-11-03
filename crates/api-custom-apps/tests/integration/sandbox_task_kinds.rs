//! The four task kinds a sandbox queues, `custom_app_sandbox_teardown`,
//! `custom_app_sandbox_migrations`, `custom_app_sandbox_oltp` and
//! `custom_app_env_oltp_branch`: each is
//! handed to the worker fleet by `oxy-server`,
//! and each is filed as a platform daemon wherever a daemon is told from the
//! tenant's own work. Source scans — the registry and two of the three lists
//! are private to their crates.

/// The task kinds `oxy-server` hands the worker fleet (`SurfaceSeams::tasks`),
/// read from `custom_apps_seams::tasks` with whitespace removed. The registry
/// builder in `oxy-app` registers whatever it is handed, so the kind is named
/// here, not there; `custom_app_functions_manual_run_guards` pins that the
/// builder registers every seam and that `surface_seams` fills the field.
fn handed_in_tasks() -> String {
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../server/src/custom_apps_seams.rs"),
    )
    .expect("read crates/server/src/custom_apps_seams.rs");
    let from = source
        .find("fn tasks(")
        .expect("`custom_apps_seams::tasks` is gone");
    let body = &source[from..];
    let body = &body[..body
        .find("fn background(")
        .expect("`background` follows `tasks`")];
    body.split_whitespace().collect()
}

/// A sandbox's task kinds are filed as platform daemons everywhere a
/// daemon is told from the tenant's own work: the coordinator's run feed
/// (backend and frontend lists, which must agree) and workspace health — a
/// failed teardown must never mark a customer's workspace unhealthy.
#[test]
fn the_sandbox_task_kinds_are_platform_daemons_in_every_list() {
    use oxy_api_custom_apps::custom_apps_env_branch::task::ENV_OLTP_BRANCH_KIND;
    use oxy_api_custom_apps::custom_apps_sandboxes::migrations_task::SANDBOX_MIGRATIONS_KIND;
    use oxy_api_custom_apps::custom_apps_sandboxes::oltp_task::SANDBOX_OLTP_KIND;
    use oxy_api_custom_apps::custom_apps_sandboxes::teardown::SANDBOX_TEARDOWN_KIND;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for (file, list) in [
        (
            "../agentic/runtime/src/lifecycle/crud/queries.rs",
            "pub const SYSTEM_SOURCE_TYPES: &[&str] = &[",
        ),
        (
            "../app-core/src/customer_runs.rs",
            "const NON_WORKSPACE_RUN_SOURCES: &[&str] = &[",
        ),
        (
            "../../web-app/src/pages/ide/coordinator/components/constants.ts",
            "const SYSTEM_SOURCE_TYPES: readonly string[] = [",
        ),
    ] {
        let source = std::fs::read_to_string(root.join(file)).expect(file);
        let start = source
            .find(list)
            .unwrap_or_else(|| panic!("{file} no longer declares `{list}`"));
        let body = &source[start..];
        let body = &body[..body.find("];").expect("the list ends")];
        for kind in [
            SANDBOX_TEARDOWN_KIND,
            SANDBOX_MIGRATIONS_KIND,
            SANDBOX_OLTP_KIND,
            ENV_OLTP_BRANCH_KIND,
        ] {
            assert!(
                body.contains(&format!("\"{kind}\"")),
                "{file}: `{kind}` is missing from the daemon list"
            );
        }
    }
}

/// The worker fleet runs a sandbox's migrations only if `oxy-server` hands in
/// their kind.
#[test]
fn the_migrations_executor_is_registered_for_its_kind() {
    let squeezed = handed_in_tasks();
    assert!(
        squeezed.contains(
            "TaskSeam{kind:migrations_task::SANDBOX_MIGRATIONS_KIND,\
             build:|deps|{Arc::new(migrations_task::SandboxMigrationsExecutor{db:deps.db.clone(),})},}"
        ),
        "`custom_apps_seams::tasks` no longer hands in \
         the executor for SANDBOX_MIGRATIONS_KIND"
    );
}

/// The worker fleet creates, seeds and migrates a sandbox's OLTP schema only
/// if `oxy-server` hands in its kind.
#[test]
fn the_oltp_schema_executor_is_registered_for_its_kind() {
    let squeezed = handed_in_tasks();
    assert!(
        squeezed.contains(
            "TaskSeam{kind:oltp_task::SANDBOX_OLTP_KIND,\
             build:|deps|{Arc::new(oltp_task::SandboxOltpExecutor{db:deps.db.clone(),})},}"
        ),
        "`custom_apps_seams::tasks` no longer hands in \
         the executor for SANDBOX_OLTP_KIND"
    );
}

/// The worker fleet cuts, resets and migrates a sandbox's own OLTP branch only
/// if `oxy-server` hands in its kind.
#[test]
fn the_env_branch_executor_is_registered_for_its_kind() {
    let squeezed = handed_in_tasks();
    assert!(
        squeezed.contains(
            "TaskSeam{kind:env_branch_task::ENV_OLTP_BRANCH_KIND,\
             build:|deps|{Arc::new(env_branch_task::EnvBranchExecutor{db:deps.db.clone(),})},}"
        ),
        "`custom_apps_seams::tasks` no longer hands in \
         the executor for ENV_OLTP_BRANCH_KIND"
    );
}

/// The worker fleet runs the task only if `oxy-server` hands in its kind.
#[test]
fn the_teardown_executor_is_registered_for_its_kind() {
    let squeezed = handed_in_tasks();
    assert!(
        squeezed.contains(
            "TaskSeam{kind:teardown::SANDBOX_TEARDOWN_KIND,\
             build:|deps|{Arc::new(teardown::SandboxTeardownExecutor{db:deps.db.clone(),})},}"
        ),
        "`custom_apps_seams::tasks` no longer hands in \
         the executor for SANDBOX_TEARDOWN_KIND"
    );
}
