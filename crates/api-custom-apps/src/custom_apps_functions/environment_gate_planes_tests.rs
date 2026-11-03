//! `with_planes`'s pure part: which planes an environment's row gives it
//! (`planes_for`), and that an unreadable row is a refusal, not a fallback.

use super::*;
use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

fn sandbox() -> AppEnvironment {
    AppEnvironment::parse("dev-a1").expect("a sandbox")
}

fn row(name: &str, planes: serde_json::Value) -> app_environments::Model {
    let now = Utc::now().fixed_offset();
    app_environments::Model {
        app_id: Uuid::from_u128(9),
        name: name.to_string(),
        kind: if name == "staging" { "staging" } else { "dev" }.to_string(),
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

/// A staging environment with no row resolves the defaults: its legacy-column
/// build keeps serving, and its policy is the one `for_environment` builds.
#[test]
fn staging_with_no_row_resolves_the_defaults() {
    let planes = planes_for(None, &AppEnvironment::Staging)
        .expect("not refused")
        .expect("staging has planes");
    assert_eq!(
        Some(&planes),
        EnvPolicy::for_environment(AppEnvironment::Staging).planes()
    );
}

#[test]
fn a_row_with_no_choices_resolves_the_defaults_of_its_kind() {
    let environment = sandbox();
    let planes = planes_for(Some(&row("dev-a1", json!({}))), &environment)
        .expect("not refused")
        .expect("a sandbox has planes");
    assert_eq!(
        Some(&planes),
        EnvPolicy::for_environment(environment).planes()
    );
}

#[test]
fn a_rows_choice_is_read_over_the_defaults() {
    let environment = sandbox();
    let held = json!({"workspace-actions": "held"});
    let planes = planes_for(Some(&row("dev-a1", held)), &environment)
        .expect("not refused")
        .expect("a sandbox has planes");
    assert_eq!(
        planes.workspace_actions,
        crate::custom_apps_planes::WorkspaceActionsMode::Held,
        "a sandbox's default is `refused`"
    );
}

/// Production has no planes, with or without a row, and a production row is
/// not even parsed.
#[test]
fn production_has_no_planes() {
    assert_eq!(planes_for(None, &AppEnvironment::Production), Ok(None));
    let unreadable = row("production", json!({"nope": "x"}));
    assert_eq!(
        planes_for(Some(&unreadable), &AppEnvironment::Production),
        Ok(None)
    );
}

/// A row whose `planes` cannot be read refuses the run, naming the error and
/// the fix — it does not resolve the defaults.
#[test]
fn an_unreadable_row_is_refused_not_defaulted() {
    for planes in [json!({"olpt": "schema"}), json!({"oltp": "teleport"})] {
        for (name, environment) in [("staging", AppEnvironment::Staging), ("dev-a1", sandbox())] {
            let refused =
                planes_for(Some(&row(name, planes.clone())), &environment).expect_err("refused");
            assert_eq!(refused.reason, RefusedReason::PlanesUnreadable);
            assert_eq!(refused.environment, environment);
            let detail = refused.detail.clone().expect("the parse error");
            let message = refused.message();
            assert!(message.contains(&detail), "{message}");
            assert!(
                message.contains(&format!("oxyc env update <app> {name} --plane")),
                "{message}"
            );
        }
    }
}

/// The caller may open the environment, so this is not a `403`: `409`, with
/// the gate's body.
#[tokio::test]
async fn unreadable_planes_answer_409() {
    let refused = planes_for(
        Some(&row("staging", json!({"x": 1}))),
        &AppEnvironment::Staging,
    )
    .expect_err("refused");
    let response = refused.into_response();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("body");
    let body: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(body["error"], "EnvironmentRefused");
    assert_eq!(body["environment"], "staging");
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|m| m.contains("oxyc env update"))
    );
}

/// Production returns before any query: a disconnected connection panics on
/// the first statement.
#[tokio::test]
async fn production_asks_the_database_nothing() {
    let db = sea_orm::DatabaseConnection::default();
    let production = Admission {
        environment: ResolvedEnvironment {
            environment: AppEnvironment::Production,
            build_id: None,
        },
        policy: EnvPolicy::production(),
    };
    let app = entity::apps::Model {
        visibility: "org".to_string(),
        id: Uuid::from_u128(9),
        slug: "x".to_string(),
        name: "X".to_string(),
        org_id: Uuid::from_u128(7),
        project_id: Uuid::nil(),
        branch: "main".to_string(),
        source_repo: String::new(),
        status: "created".to_string(),
        source_type: "s3".to_string(),
        source_config: json!({}),
        bootstrap_pr_url: None,
        last_synced_at: None,
        manifest_override: None,
        published_at: None,
        repo_path: None,
        last_promoted_by: None,
        last_promoted_at: None,
        created_at: Utc::now().fixed_offset(),
        updated_at: Utc::now().fixed_offset(),
    };
    let admitted = with_planes(&db, production.clone(), &app)
        .await
        .expect("production needs no lookup");
    assert_eq!(admitted, production);
}
