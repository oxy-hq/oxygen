//! The three task kinds a sandbox queues, `custom_app_sandbox_teardown`,
//! `custom_app_sandbox_migrations` and `custom_app_sandbox_oltp`: each is
//! registered with the worker fleet,
//! and each is filed as a platform daemon wherever a daemon is told from the
//! tenant's own work. Source scans — the registry and two of the three lists
//! are private to their crates.

/// A sandbox's task kinds are filed as platform daemons everywhere a
/// daemon is told from the tenant's own work: the coordinator's run feed
/// (backend and frontend lists, which must agree) and workspace health — a
/// failed teardown must never mark a customer's workspace unhealthy.
#[test]
fn the_sandbox_task_kinds_are_platform_daemons_in_every_list() {
    use oxy_app::server::api::custom_apps_sandboxes::migrations_task::SANDBOX_MIGRATIONS_KIND;
    use oxy_app::server::api::custom_apps_sandboxes::oltp_task::SANDBOX_OLTP_KIND;
    use oxy_app::server::api::custom_apps_sandboxes::teardown::SANDBOX_TEARDOWN_KIND;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for (file, list) in [
        (
            "../agentic/runtime/src/lifecycle/crud/queries.rs",
            "pub const SYSTEM_SOURCE_TYPES: &[&str] = &[",
        ),
        (
            "src/server/api/admin/workspace_health/queries.rs",
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
        ] {
            assert!(
                body.contains(&format!("\"{kind}\"")),
                "{file}: `{kind}` is missing from the daemon list"
            );
        }
    }
}

/// The worker fleet runs a sandbox's migrations only if the registry names
/// their kind.
#[test]
fn the_migrations_executor_is_registered_for_its_kind() {
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/server/router/recovery.rs"),
    )
    .expect("read router/recovery.rs");
    let registry = &source[source
        .find("fn build_custom_task_registry(")
        .expect("the registry builder")..];
    let squeezed: String = registry.split_whitespace().collect();
    assert!(
        squeezed.contains(
            "reg.register(migrations_task::SANDBOX_MIGRATIONS_KIND,\
             Arc::new(migrations_task::SandboxMigrationsExecutor{db:db.clone()}),);"
        ),
        "`build_custom_task_registry` no longer registers the sandbox migrations"
    );
}

/// The worker fleet creates, seeds and migrates a sandbox's OLTP schema only
/// if the registry names its kind.
#[test]
fn the_oltp_schema_executor_is_registered_for_its_kind() {
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/server/router/recovery.rs"),
    )
    .expect("read router/recovery.rs");
    let registry = &source[source
        .find("fn build_custom_task_registry(")
        .expect("the registry builder")..];
    let squeezed: String = registry.split_whitespace().collect();
    assert!(
        squeezed.contains(
            "reg.register(oltp_task::SANDBOX_OLTP_KIND,\
             Arc::new(oltp_task::SandboxOltpExecutor{db:db.clone()}),);"
        ),
        "`build_custom_task_registry` no longer registers the sandbox OLTP schema task"
    );
}

/// The worker fleet runs the task only if the registry names its kind.
#[test]
fn the_teardown_executor_is_registered_for_its_kind() {
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/server/router/recovery.rs"),
    )
    .expect("read router/recovery.rs");
    let registry = &source[source
        .find("fn build_custom_task_registry(")
        .expect("the registry builder")..];
    let squeezed: String = registry.split_whitespace().collect();
    assert!(
        squeezed.contains(
            "reg.register(teardown::SANDBOX_TEARDOWN_KIND,\
             Arc::new(teardown::SandboxTeardownExecutor{db:db.clone()}),);"
        ),
        "`build_custom_task_registry` no longer registers the sandbox teardown"
    );
}
