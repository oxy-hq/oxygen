//! Unit tests for `environment_scope`: the `?environment=` error table, on a
//! disconnected connection so no answer here can have cost a query.

use entity::users::UserStatus;
use futures::FutureExt;

use super::*;

fn app_row() -> apps::Model {
    let now = chrono::Utc::now().fixed_offset();
    apps::Model {
        visibility: "org".to_string(),
        id: Uuid::new_v4(),
        slug: "x".to_string(),
        name: "X".to_string(),
        org_id: Uuid::new_v4(),
        project_id: Uuid::nil(),
        branch: "main".to_string(),
        source_repo: String::new(),
        status: "created".to_string(),
        source_type: "s3".to_string(),
        source_config: serde_json::json!({}),
        bootstrap_pr_url: None,
        last_synced_at: None,
        manifest_override: None,
        published_at: None,
        repo_path: None,
        draft_build_id: None,
        published_build_id: None,
        last_promoted_by: None,
        last_promoted_at: None,
        created_at: now,
        updated_at: now,
    }
}

fn caller() -> AuthenticatedUser {
    AuthenticatedUser {
        id: Uuid::new_v4(),
        email: Some("someone@customer.example".to_string()),
        name: "Someone".to_string(),
        picture: None,
        status: UserStatus::Active,
    }
}

fn token() -> AppPublishTokenAuth {
    AppPublishTokenAuth {
        token_id: Uuid::new_v4(),
        app_id: None,
        machine_identity: None,
    }
}

/// Every case here runs against a disconnected connection, on which any
/// statement panics: none of these answers may cost a query.
async fn resolved(
    marker: Option<&AppPublishTokenAuth>,
    raw: Option<&str>,
) -> Result<AppEnvironment, ScopeError> {
    let db = DatabaseConnection::default();
    resolve(&db, &app_row(), &caller(), marker, raw).await
}

#[tokio::test]
async fn environment_scope_reads_absent_blank_and_production_as_production() {
    let token = token();
    for raw in [None, Some(""), Some("  "), Some("production")] {
        for marker in [None, Some(&token)] {
            assert_eq!(
                resolved(marker, raw).await.expect("production passes"),
                AppEnvironment::Production,
                "{raw:?}: production needs no reach, with or without a publish token"
            );
        }
    }
}

/// A name that is not exactly an environment is a 400 — never production
/// by default, which would run or read the live app on a typo.
#[tokio::test]
async fn environment_scope_refuses_a_name_that_is_not_an_environment() {
    for raw in [
        "Staging",
        "prod",
        "dev",
        "dev-",
        "dev-UPPER",
        "dev-a--b",
        "dev-thirteenchars",
        "staging ; drop",
    ] {
        let e = resolved(None, Some(raw))
            .await
            .expect_err("not an environment");
        assert_eq!(
            (e.status, e.code),
            (StatusCode::BAD_REQUEST, "invalid_environment"),
            "{raw}"
        );
    }
}

/// A publish token is refused on every non-production environment, before
/// the reach lookup: whoever minted it, and whatever they may open.
#[tokio::test]
async fn environment_scope_refuses_a_publish_token_outside_production() {
    let token = token();
    for raw in ["staging", "dev-a1"] {
        let e = resolved(Some(&token), Some(raw))
            .await
            .expect_err("a publish token never acts outside production");
        assert_eq!(
            (e.status, e.code),
            (StatusCode::FORBIDDEN, "publish_token_refused"),
            "{raw}"
        );
    }
}

/// Without a token the reach decision is what stands between a caller and
/// a non-production environment, and it is never skipped: with no
/// database to decide from, the caller is not admitted.
#[tokio::test]
async fn environment_scope_admits_nobody_outside_production_without_a_reach_decision() {
    for raw in ["staging", "dev-a1"] {
        let outcome = std::panic::AssertUnwindSafe(resolved(None, Some(raw)))
            .catch_unwind()
            .await;
        assert!(
            !matches!(outcome, Ok(Ok(_))),
            "{raw}: admitted with no reach decision"
        );
    }
}

#[test]
fn environment_scope_answers_an_error_code_beside_its_message() {
    let response = ScopeError::new(StatusCode::FORBIDDEN, "not_a_check", "no").into_response();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let e: ScopeError = TriggerError::NotACheck {
        function: "f".into(),
        environment: AppEnvironment::Staging,
    }
    .into();
    assert_eq!((e.status, e.code), (StatusCode::FORBIDDEN, "not_a_check"));
    let e: ScopeError = TriggerError::NoBuild(AppEnvironment::Staging).into();
    assert_eq!(
        (e.status, e.code),
        (StatusCode::NOT_FOUND, "environment_has_no_build")
    );
    let e: ScopeError = TriggerError::FunctionNotFound("f".into()).into();
    assert_eq!(
        (e.status, e.code),
        (StatusCode::NOT_FOUND, "function_not_found")
    );
    let e: ScopeError = TriggerError::AppNotFound.into();
    assert_eq!((e.status, e.code), (StatusCode::NOT_FOUND, "app_not_found"));
    // A server-side failure never leaks its cause.
    let e: ScopeError = TriggerError::Db("connection refused at 10.0.0.4".into()).into();
    assert_eq!(
        (e.status, e.code),
        (StatusCode::INTERNAL_SERVER_ERROR, "internal")
    );
    assert!(!e.message.contains("10.0.0.4"), "{}", e.message);
}
