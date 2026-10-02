use super::*;

fn task() -> SandboxMigrationsTask {
    SandboxMigrationsTask {
        app_id: Uuid::from_u128(7),
        app_slug: "store-ops".into(),
        workspace_id: Uuid::from_u128(9),
        build_pk: Uuid::from_u128(11),
        environment: "dev-a1".into(),
        migrations: vec![QueuedMigration {
            filename: "0001_visits.sql".into(),
            checksum: "abc".into(),
            sql: "CREATE TABLE app_store_ops.visits (id VARCHAR);".into(),
        }],
    }
}

#[test]
fn the_payload_round_trips_through_its_spec() {
    let task = task();
    let spec = task.spec().expect("serializes");
    let TaskSpec::Custom { kind, payload } = &spec else {
        panic!("a custom task");
    };
    assert_eq!(kind, SANDBOX_MIGRATIONS_KIND);
    assert_eq!(kind, "custom_app_sandbox_migrations");
    assert_eq!(payload["environment"], "dev-a1");
    let object = payload.as_object().expect("an object");
    let mut fields: Vec<&str> = object.keys().map(String::as_str).collect();
    fields.sort_unstable();
    assert_eq!(
        fields,
        [
            "app_id",
            "app_slug",
            "build_pk",
            "environment",
            "migrations",
            "workspace_id"
        ]
    );
    assert_eq!(SandboxMigrationsTask::from_spec(&spec), Ok(task.clone()));
    let declared = task.declared();
    assert_eq!(declared.len(), 1);
    assert_eq!(declared[0].filename, "0001_visits.sql");
    assert_eq!(declared[0].checksum, "abc");
}

/// A staging task is not a sandbox task, and the reverse: the kinds differ so
/// that neither executor can be handed the other's payload.
#[test]
fn a_staging_task_or_a_broken_payload_is_refused() {
    let staging = TaskSpec::Custom {
        kind: "custom_app_staging_migrations".into(),
        payload: serde_json::to_value(task()).unwrap(),
    };
    assert!(SandboxMigrationsTask::from_spec(&staging).is_err());
    let broken = TaskSpec::Custom {
        kind: SANDBOX_MIGRATIONS_KIND.into(),
        payload: json!({ "app_id": "not-a-uuid" }),
    };
    assert!(SandboxMigrationsTask::from_spec(&broken).is_err());
    use crate::server::api::custom_apps_nonproduction::staging_task::{
        STAGING_MIGRATIONS_KIND, StagingMigrationTask,
    };
    assert_ne!(SANDBOX_MIGRATIONS_KIND, STAGING_MIGRATIONS_KIND);
    assert!(
        StagingMigrationTask::from_spec(&task().spec().unwrap()).is_err(),
        "the staging executor must not take a sandbox's task"
    );
}

#[test]
fn the_run_id_names_the_app_the_sandbox_and_the_build() {
    let first = task();
    assert_eq!(
        first.run_id(),
        format!(
            "custom_app_sandbox_migrations:{}:dev-a1:{}",
            Uuid::from_u128(7),
            Uuid::from_u128(11)
        )
    );
    let mut other_sandbox = task();
    other_sandbox.environment = "dev-b2".into();
    let mut other_build = task();
    other_build.build_pk = Uuid::from_u128(12);
    assert_ne!(first.run_id(), other_sandbox.run_id());
    assert_ne!(first.run_id(), other_build.run_id());
}

/// Staging's sibling is the staging task's: a payload naming a fixed
/// environment is refused before anything is applied.
#[test]
fn a_payload_that_names_no_sandbox_is_refused() {
    assert_eq!(
        task().sandbox(),
        Ok(AppEnvironment::Dev {
            handle: "a1".into()
        })
    );
    for name in ["production", "staging", "dev--x", ""] {
        let mut bad = task();
        bad.environment = name.into();
        let refused = bad.sandbox().expect_err(name);
        assert!(refused.contains("is not a sandbox"), "{refused}");
    }
}
