//! The mock's project and role behaviour, which every provisioner test stands on.
use super::*;

fn req(name: &str) -> CreateProjectRequest {
    CreateProjectRequest {
        name: name.to_string(),
        region_id: "aws-us-east-2".into(),
        // Deliberately NOT `DEFAULT_PG_VERSION`: this asserts the value
        // round-trips, which a number equal to the default could satisfy by
        // coincidence. The literals in the provider test doubles are meant
        // to differ from it — only `config.rs` carries the shipped default.
        pg_version: 17,
    }
}

#[tokio::test]
async fn create_discloses_the_owner_password_exactly_once() {
    let p = MockProvider::new();
    let created = p.create_project(req("acme")).await.unwrap();
    assert!(created.owner_role.password.is_some());

    let refetched = p.get_project(&created.id).await.unwrap().unwrap();
    assert!(
        refetched.owner_role.password.is_none(),
        "re-reading a project must not re-disclose the owner password"
    );
}

#[tokio::test]
async fn duplicate_name_is_rejected_not_adopted() {
    let p = MockProvider::new();
    p.create_project(req("acme")).await.unwrap();
    let err = p.create_project(req("acme")).await.unwrap_err();
    assert!(matches!(err, ProviderError::ProjectNameTaken(n) if n == "acme"));
    assert_eq!(
        p.project_count(),
        1,
        "the second create must not have landed"
    );
}

#[tokio::test]
async fn delete_project_is_idempotent_and_frees_the_name() {
    let p = MockProvider::new();
    let proj = p.create_project(req("acme")).await.unwrap();
    p.delete_project(&proj.id).await.unwrap();
    p.delete_project(&proj.id).await.unwrap();
    assert_eq!(p.project_count(), 0);
    // Name is reusable once the project is gone.
    p.create_project(req("acme")).await.unwrap();
}

#[tokio::test]
async fn deleting_a_project_takes_its_roles_with_it() {
    let p = MockProvider::new();
    let proj = p.create_project(req("acme")).await.unwrap();
    p.create_role(&proj.id, &proj.branch.id, "app_x_rw")
        .await
        .unwrap();
    p.delete_project(&proj.id).await.unwrap();
    assert!(p.role_names(&proj.id, &proj.branch.id).is_empty());
}

#[tokio::test]
async fn get_role_never_returns_a_password() {
    let p = MockProvider::new();
    let proj = p.create_project(req("acme")).await.unwrap();
    p.create_role(&proj.id, &proj.branch.id, "app_x_rw")
        .await
        .unwrap();
    let role = p
        .get_role(&proj.id, &proj.branch.id, "app_x_rw")
        .await
        .unwrap()
        .unwrap();
    assert!(role.password.is_none());
}

#[tokio::test]
async fn reset_changes_the_password() {
    let p = MockProvider::new();
    let proj = p.create_project(req("acme")).await.unwrap();
    let before = p
        .create_role(&proj.id, &proj.branch.id, "app_x_rw")
        .await
        .unwrap()
        .password
        .unwrap();
    let after = p
        .reset_role_password(&proj.id, &proj.branch.id, "app_x_rw")
        .await
        .unwrap()
        .password
        .unwrap();
    assert_ne!(before, after);
    assert_eq!(
        p.peek_password(&proj.id, &proj.branch.id, "app_x_rw"),
        Some(after)
    );
}

#[tokio::test]
async fn reset_on_a_missing_role_errors_rather_than_creating_one() {
    let p = MockProvider::new();
    let proj = p.create_project(req("acme")).await.unwrap();
    let err = p
        .reset_role_password(&proj.id, &proj.branch.id, "nope")
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::RoleNotFound(..)));
}

#[tokio::test]
async fn create_role_on_a_missing_project_errors() {
    let p = MockProvider::new();
    let err = p.create_role("proj-nope", "br-1", "r").await.unwrap_err();
    assert!(matches!(err, ProviderError::ProjectNotFound(_)));
}

#[tokio::test]
async fn delete_role_is_idempotent() {
    let p = MockProvider::new();
    let proj = p.create_project(req("acme")).await.unwrap();
    p.delete_role(&proj.id, &proj.branch.id, "ghost")
        .await
        .unwrap();
}

#[tokio::test]
async fn injected_faults_pop_in_order() {
    let p = MockProvider::new();
    p.push_fault(ProviderError::RateLimited);
    p.push_fault(ProviderError::Transport("boom".into()));

    assert!(matches!(
        p.create_project(req("acme")).await.unwrap_err(),
        ProviderError::RateLimited
    ));
    assert!(matches!(
        p.create_project(req("acme")).await.unwrap_err(),
        ProviderError::Transport(_)
    ));
    // Queue drained — the third call succeeds.
    p.create_project(req("acme")).await.unwrap();
}

#[test]
fn retryable_classification_matches_intent() {
    assert!(ProviderError::RateLimited.is_retryable());
    assert!(ProviderError::Transport("x".into()).is_retryable());
    assert!(
        ProviderError::Api {
            status: 503,
            message: "x".into()
        }
        .is_retryable()
    );
    assert!(!ProviderError::ProjectNameTaken("x".into()).is_retryable());
    assert!(
        !ProviderError::Api {
            status: 400,
            message: "x".into()
        }
        .is_retryable()
    );
}
