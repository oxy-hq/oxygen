use chrono::{TimeZone, Utc};
use entity::app_environments;
use oxy_app_core::custom_app_environment::AppEnvironment;
use serde_json::json;
use uuid::Uuid;

use super::branch_fields;

fn row(name: &str, kind: &str, planes: serde_json::Value) -> app_environments::Model {
    let created = Utc.with_ymd_and_hms(2026, 11, 1, 9, 0, 0).unwrap();
    app_environments::Model {
        app_id: Uuid::from_u128(1),
        name: name.into(),
        kind: kind.into(),
        owner_user_id: None,
        build_id: None,
        updated_by: None,
        updated_at: created.fixed_offset(),
        created_at: created.fixed_offset(),
        deleting_at: None,
        oltp_schema: None,
        created_by_token_id: None,
        schedules_enabled: false,
        parent_name: None,
        fork_at: None,
        planes,
        semantic_revision_id: None,
        semantic_git_sha: None,
    }
}

fn dev() -> AppEnvironment {
    AppEnvironment::parse("dev-a1").expect("a valid sandbox name")
}

#[test]
fn show_reports_planes_for_a_sandbox() {
    let row = row("dev-a1", "dev", json!({}));
    let fields = branch_fields(&dev(), Some(&row));
    let planes = fields.planes.expect("planes resolve");
    assert_eq!(planes["workspace-actions"], "held");
    assert_eq!(planes["oltp"], "schema");
    assert_eq!(planes.len(), 12);
    assert_eq!(fields.parent.as_deref(), Some("staging"));
    assert_eq!(fields.planes_error, None);
    assert_eq!(fields.fork_at, Some(row.created_at.with_timezone(&Utc)));
}

#[test]
fn a_recorded_parent_and_fork_point_win_over_the_defaults() {
    let mut row = row("dev-a1", "dev", json!({"workspace-actions": "held"}));
    row.parent_name = Some("dev-b2".into());
    row.fork_at = Some(
        Utc.with_ymd_and_hms(2026, 11, 2, 0, 0, 0)
            .unwrap()
            .fixed_offset(),
    );
    let fields = branch_fields(&dev(), Some(&row));
    assert_eq!(fields.parent.as_deref(), Some("dev-b2"));
    assert_eq!(
        fields.fork_at,
        Some(Utc.with_ymd_and_hms(2026, 11, 2, 0, 0, 0).unwrap())
    );
    assert_eq!(fields.planes.expect("planes")["workspace-actions"], "held");
}

#[test]
fn show_survives_a_bad_planes_value() {
    let row = row("dev-a1", "dev", json!({"oltp": 3}));
    let fields = branch_fields(&dev(), Some(&row));
    assert_eq!(fields.planes, None);
    assert!(fields.planes_error.is_some());
    // The rest is still filled.
    assert_eq!(fields.parent.as_deref(), Some("staging"));
    assert!(fields.fork_at.is_some());
}

#[test]
fn show_production_has_null_planes() {
    let plain = row("production", "production", json!({}));
    // A stray value in a column production never reads is still not reported.
    let mut stray = row("production", "production", json!({"oltp": 3}));
    stray.parent_name = Some("staging".into());
    stray.fork_at = Some(stray.created_at);
    for row in [Some(&plain), Some(&stray), None] {
        let fields = branch_fields(&AppEnvironment::Production, row);
        assert_eq!(fields.planes, None);
        assert_eq!(fields.planes_error, None);
        assert_eq!(fields.parent, None);
        assert_eq!(fields.fork_at, None);
    }
}

#[test]
fn a_fixed_environment_with_no_row_resolves_the_defaults() {
    let fields = branch_fields(&AppEnvironment::Staging, None);
    let planes = fields.planes.expect("planes resolve");
    assert_eq!(planes["workspace-actions"], "held");
    assert_eq!(fields.parent.as_deref(), Some("production"));
    assert_eq!(fields.fork_at, None, "no row, no recorded creation");
}

/// `planes_set` is the row's own object, as stored: not the resolved modes,
/// and shown even when it cannot be read, so a client can see what to replace.
#[test]
fn planes_set_is_the_stored_object() {
    let set = json!({"workspace-actions": "held"});
    let fields = branch_fields(&dev(), Some(&row("dev-a1", "dev", set.clone())));
    assert_eq!(fields.planes_set, Some(set));
    let empty = branch_fields(&dev(), Some(&row("dev-a1", "dev", json!({}))));
    assert_eq!(
        empty.planes_set,
        Some(json!({})),
        "`{{}}` when nothing is set"
    );
    let unreadable = branch_fields(&dev(), Some(&row("dev-a1", "dev", json!({"oltp": 3}))));
    assert_eq!(unreadable.planes_set, Some(json!({"oltp": 3})));
}

#[test]
fn planes_set_is_null_for_production_and_for_a_fixed_environment_with_no_row() {
    let stray = row("production", "production", json!({"oltp": 3}));
    assert_eq!(
        branch_fields(&AppEnvironment::Production, Some(&stray)).planes_set,
        None
    );
    assert_eq!(
        branch_fields(&AppEnvironment::Staging, None).planes_set,
        None
    );
}
