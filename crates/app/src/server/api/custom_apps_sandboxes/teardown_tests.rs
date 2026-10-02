use super::*;

fn task() -> SandboxTeardownTask {
    SandboxTeardownTask {
        app_id: Uuid::from_u128(7),
        app_slug: "store-ops".into(),
        org_id: Uuid::from_u128(8),
        workspace_id: Uuid::from_u128(9),
        environment: "dev-a1".into(),
        reason: "expired".into(),
        marked_at_micros: 1_790_000_000_123_456,
    }
}

#[test]
fn the_payload_round_trips_through_its_spec() {
    let task = task();
    let spec = task.spec().expect("serializes");
    let TaskSpec::Custom { kind, payload } = &spec else {
        panic!("a custom task");
    };
    assert_eq!(kind, SANDBOX_TEARDOWN_KIND);
    assert_eq!(payload["environment"], "dev-a1");
    assert_eq!(SandboxTeardownTask::from_spec(&spec), Ok(task));
}

#[test]
fn another_kind_or_a_broken_payload_is_refused() {
    let other = TaskSpec::Custom {
        kind: "custom_app_staging_migrations".into(),
        payload: serde_json::to_value(task()).unwrap(),
    };
    assert!(SandboxTeardownTask::from_spec(&other).is_err());
    let broken = TaskSpec::Custom {
        kind: SANDBOX_TEARDOWN_KIND.into(),
        payload: json!({ "app_id": "not-a-uuid" }),
    };
    assert!(SandboxTeardownTask::from_spec(&broken).is_err());
}

/// Only a sandbox is ever torn down: a payload naming a fixed environment,
/// or nothing at all, is refused before any home is touched.
#[test]
fn a_payload_that_names_no_sandbox_is_refused() {
    assert_eq!(
        task().sandbox(),
        Ok(AppEnvironment::Dev {
            handle: "a1".into()
        })
    );
    for name in ["production", "staging", "dev--x", "", "main"] {
        let mut bad = task();
        bad.environment = name.into();
        let refused = bad.sandbox().expect_err(name);
        assert!(refused.contains("is not a sandbox"), "{refused}");
    }
}

#[test]
fn the_summary_says_what_went_and_what_airhouse_kept() {
    let none = summary(&task(), 0, &DropOutcome::default(), 1);
    assert!(none.contains("no Airhouse sibling"), "{none}");
    assert!(none.contains("0 secrets deleted, ") && none.ends_with("row removed"));
    let kept = DropOutcome {
        schema: Some("app_store_ops__dev_a1".into()),
        relations_dropped: 2,
        schema_dropped: false,
        ledger_rows_cleared: 2,
    };
    let text = summary(&task(), 3, &kept, 0);
    assert!(text.contains("emptied (2 relations)"), "{text}");
    assert!(text.contains("3 secrets deleted") && text.ends_with("already gone"));
}

/// One run per time the sandbox was marked: a second `DELETE`, or the
/// retry sweep, marks it again and so queues a run of its own.
#[test]
fn the_run_id_names_the_app_the_sandbox_and_when_it_was_marked() {
    let first = task();
    assert_eq!(
        first.run_id(),
        format!(
            "custom_app_sandbox_teardown:{}:dev-a1:1790000000123456",
            Uuid::from_u128(7)
        )
    );
    let mut again = task();
    again.marked_at_micros += 1;
    assert_ne!(first.run_id(), again.run_id());
}
