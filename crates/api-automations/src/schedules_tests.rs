use super::*;

const APP: &str = "6f1c1f5e-4b2a-4d8e-9d59-0b8f8a3e2c11";

/// An environment's function row is the platform's; production's and
/// every other kind's — whatever its ref — stay the tenant's.
#[test]
fn only_a_function_ref_naming_an_environment_is_hidden() {
    assert!(names_environment(
        "function",
        &format!("{APP}/staging/refresh")
    ));
    assert!(names_environment(
        "function",
        &format!("{APP}/dev-a1/refresh")
    ));
    assert!(!names_environment("function", &format!("{APP}/refresh")));
    assert!(!names_environment(
        "workflow",
        "automations/a/b.automation.yml"
    ));
    assert!(!names_environment(
        "airway",
        &format!("{APP}/staging/refresh")
    ));
}

#[test]
fn a_tenant_cannot_create_an_environments_row() {
    let input = |target_ref: String| ScheduleInput {
        name: "x".into(),
        target_kind: "function".into(),
        target_ref,
        question: None,
        variables: None,
        cron_expr: "0 * * * *".into(),
        timezone: "UTC".into(),
        enabled: true,
    };
    assert!(matches!(
        tenant_input(&input(format!("{APP}/dev-a1/refresh"))),
        Err(ScheduleError::Invalid(_))
    ));
    assert!(tenant_input(&input(format!("{APP}/refresh"))).is_ok());
}
