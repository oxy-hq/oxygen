use agentic_core::delegation::TaskSpec;
use chrono::TimeZone;
use oxy_oltp::entity::env_branches::EnvBranchMode;
use serde_json::json;

use super::*;
use crate::custom_apps_env_branch::wants_branch;

fn task(op: BranchOp) -> EnvBranchTask {
    EnvBranchTask {
        app_id: Uuid::from_u128(7),
        app_slug: "store-ops".into(),
        org_id: Uuid::from_u128(8),
        workspace_id: Uuid::from_u128(9),
        environment: "dev-a1".into(),
        op,
    }
}

fn each_op() -> [BranchOp; 4] {
    let at = Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap();
    [
        BranchOp::Cut { at: None },
        BranchOp::Cut { at: Some(at) },
        BranchOp::Reset { at: Some(at) },
        BranchOp::Migrate {
            build_pk: Uuid::from_u128(11),
        },
    ]
}

#[test]
fn the_spec_round_trips_each_op() {
    for op in each_op() {
        let task = task(op);
        let spec = task.spec().expect("serializes");
        let TaskSpec::Custom { kind, payload } = &spec else {
            panic!("a custom task");
        };
        assert_eq!(kind, ENV_OLTP_BRANCH_KIND);
        assert_eq!(kind, "custom_app_env_oltp_branch");
        assert_eq!(payload["environment"], "dev-a1");
        assert_eq!(payload["op"]["op"], task.op.tag());
        assert_eq!(EnvBranchTask::from_spec(&spec), Ok(task));
    }
}

#[test]
fn no_other_kind_is_taken_and_a_broken_payload_is_refused() {
    let theirs = TaskSpec::Custom {
        kind: "custom_app_sandbox_oltp".into(),
        payload: serde_json::to_value(task(BranchOp::Cut { at: None })).unwrap(),
    };
    assert!(EnvBranchTask::from_spec(&theirs).is_err());
    let broken = TaskSpec::Custom {
        kind: ENV_OLTP_BRANCH_KIND.into(),
        payload: json!({ "app_id": "not-a-uuid" }),
    };
    assert!(EnvBranchTask::from_spec(&broken).is_err());
}

#[test]
fn the_run_id_names_the_op() {
    for op in each_op() {
        let task = task(op);
        let id = task.run_id();
        let prefix = format!(
            "custom_app_env_oltp_branch:{}:dev-a1:{}:",
            Uuid::from_u128(7),
            task.op.tag()
        );
        assert!(id.starts_with(&prefix), "{id}");
        let nonce = &id[prefix.len()..];
        assert_eq!(nonce.len(), 32, "a simple uuid: {id}");
    }
}

/// A reset asked for twice must run twice, so no two ids repeat.
#[test]
fn the_run_id_is_fresh_for_every_enqueue() {
    let task = task(BranchOp::Reset { at: None });
    assert_ne!(task.run_id(), task.run_id());
}

#[test]
fn a_payload_that_names_no_sandbox_is_refused() {
    assert!(task(BranchOp::Cut { at: None }).sandbox().is_ok());
    for name in ["production", "staging", "dev--x", ""] {
        let mut bad = task(BranchOp::Cut { at: None });
        bad.environment = name.into();
        let refused = bad.sandbox().expect_err(name);
        assert!(refused.contains("is not a sandbox"), "{refused}");
    }
}

fn row_with(planes: serde_json::Value) -> entity::app_environments::Model {
    let now = Utc::now().fixed_offset();
    entity::app_environments::Model {
        app_id: Uuid::from_u128(7),
        name: "dev-a1".to_string(),
        kind: "dev".to_string(),
        owner_user_id: None,
        build_id: None,
        updated_by: None,
        updated_at: now,
        created_at: now,
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

#[test]
fn wants_branch_reads_the_planes_column() {
    assert_eq!(wants_branch(&row_with(json!({}))), None);
    assert_eq!(wants_branch(&row_with(json!({"oltp": "schema"}))), None);
    assert_eq!(
        wants_branch(&row_with(json!({"oltp": "branch"}))),
        Some(EnvBranchMode::Branch)
    );
    assert_eq!(
        wants_branch(&row_with(json!({"oltp": "empty"}))),
        Some(EnvBranchMode::Empty)
    );
    assert_eq!(wants_branch(&row_with(json!({"oltp": "bogus"}))), None);
}
