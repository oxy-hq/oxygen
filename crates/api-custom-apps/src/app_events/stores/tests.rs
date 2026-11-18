use serde_json::{Value, json};

use super::*;

const APP: Uuid = Uuid::from_u128(1);

fn sandbox_row(planes: Value) -> app_environments::Model {
    let now = chrono::Utc::now().fixed_offset();
    app_environments::Model {
        app_id: APP,
        name: "dev-luong".to_string(),
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
        events_enabled: true,
        parent_name: None,
        fork_at: None,
        planes,
        semantic_revision_id: None,
        semantic_git_sha: None,
    }
}

fn store(row: Option<&app_environments::Model>) -> OutboxTarget {
    let environment = AppEnvironment::parse("dev-luong").expect("a sandbox");
    sandbox_store(APP, row, &environment, "dev-luong")
}

/// A sandbox whose planes put `ctx.oltp` on a branch of its own (`branch`
/// or `empty`) keeps its events there; one on a schema of the staging
/// branch, the default, keeps them on the staging branch.
#[test]
fn a_sandbox_on_its_own_branch_has_its_own_store() {
    let own = OutboxTarget::EnvBranch {
        app_id: APP,
        environment: "dev-luong".into(),
    };
    let staging_branch = OutboxTarget::Branch("dev-luong".into());
    for mode in ["branch", "empty"] {
        assert_eq!(
            store(Some(&sandbox_row(json!({ "oltp": mode })))),
            own,
            "{mode}"
        );
    }
    assert_eq!(
        store(Some(&sandbox_row(json!({ "oltp": "schema" })))),
        staging_branch
    );
    assert_eq!(store(Some(&sandbox_row(json!({})))), staging_branch);
    assert_eq!(store(None), staging_branch, "no row: the default planes");
}

/// Production and staging never depend on an app's planes.
#[test]
fn production_and_staging_have_one_shared_store() {
    assert_eq!(
        shared_store(&AppEnvironment::Production, "production"),
        OutboxTarget::Production
    );
    assert_eq!(
        shared_store(&AppEnvironment::Staging, "staging"),
        OutboxTarget::Branch("staging".into())
    );
}

#[test]
fn only_an_environment_is_relayed() {
    for environment in ["production", "staging", "dev-a1b2c3"] {
        assert!(is_relayed(environment), "{environment}");
    }
    for nothing in ["Production", "dev-", "dev-UPPER", "", "nowhere"] {
        assert!(!is_relayed(nothing), "{nothing:?}");
    }
}
