use super::*;

fn task() -> SandboxOltpTask {
    SandboxOltpTask {
        app_id: Uuid::from_u128(7),
        app_slug: "store-ops".into(),
        org_id: Uuid::from_u128(8),
        workspace_id: Uuid::from_u128(9),
        build_pk: Uuid::from_u128(11),
        environment: "dev-a1".into(),
        migrations: vec![QueuedMigration {
            filename: "0001_visits.sql".into(),
            checksum: "abc".into(),
            sql: "CREATE TABLE visits (id INT PRIMARY KEY);".into(),
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
    assert_eq!(kind, SANDBOX_OLTP_KIND);
    assert_eq!(kind, "custom_app_sandbox_oltp");
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
            "org_id",
            "workspace_id"
        ]
    );
    assert_eq!(SandboxOltpTask::from_spec(&spec), Ok(task.clone()));
    let declared = task.declared();
    assert_eq!(declared.len(), 1);
    assert_eq!(declared[0].filename, "0001_visits.sql");
}

/// The kinds differ so that no executor can be handed another's payload: an
/// older worker, which knows neither this kind nor this executor, fails the
/// task — it never applies a sandbox's OLTP files to staging's schema.
#[test]
fn no_other_executor_takes_a_sandbox_oltp_task_and_it_takes_no_other() {
    use crate::server::api::custom_apps_nonproduction::staging_task::{
        STAGING_MIGRATIONS_KIND, StagingMigrationTask,
    };
    use crate::server::api::custom_apps_sandboxes::migrations_task::{
        SANDBOX_MIGRATIONS_KIND, SandboxMigrationsTask,
    };
    let spec = task().spec().unwrap();
    assert!(StagingMigrationTask::from_spec(&spec).is_err());
    assert!(SandboxMigrationsTask::from_spec(&spec).is_err());
    for kind in [STAGING_MIGRATIONS_KIND, SANDBOX_MIGRATIONS_KIND] {
        assert_ne!(SANDBOX_OLTP_KIND, kind);
        let theirs = TaskSpec::Custom {
            kind: kind.into(),
            payload: serde_json::to_value(task()).unwrap(),
        };
        assert!(SandboxOltpTask::from_spec(&theirs).is_err(), "{kind}");
    }
    let broken = TaskSpec::Custom {
        kind: SANDBOX_OLTP_KIND.into(),
        payload: json!({ "app_id": "not-a-uuid" }),
    };
    assert!(SandboxOltpTask::from_spec(&broken).is_err());
}

#[test]
fn the_run_id_names_the_app_the_sandbox_and_the_build() {
    let first = task();
    assert_eq!(
        first.run_id(),
        format!(
            "custom_app_sandbox_oltp:{}:dev-a1:{}",
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

/// Staging's schema on the branch is the staging task's: a payload naming a
/// fixed environment is refused before anything is created or applied.
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
