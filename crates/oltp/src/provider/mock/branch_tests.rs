//! The mock's branch operations, which the provisioner's branch tests stand on —
//! so the Neon-shaped behaviour they assume (an own endpoint, an own owner
//! password, adoption by name) is pinned here rather than taken on trust.

use super::*;

async fn project(p: &MockProvider) -> Project {
    p.create_project(CreateProjectRequest {
        name: "oxy-org-acme".into(),
        region_id: "aws-us-east-2".into(),
        pg_version: 17,
    })
    .await
    .unwrap()
}

fn staging(project: &Project) -> BranchRequest {
    BranchRequest {
        project_id: project.id.clone(),
        parent_branch_id: project.branch.id.clone(),
        name: "oxy-staging".into(),
        database_name: project.database.name.clone(),
        owner_role: project.owner_role.name.clone(),
    }
}

#[tokio::test]
async fn a_branch_gets_its_own_endpoint_and_owner_password() {
    let p = MockProvider::new();
    let proj = project(&p).await;
    let branch = p.create_branch(&staging(&proj)).await.unwrap();

    assert_ne!(branch.id, proj.branch.id);
    assert_ne!(
        branch.host, proj.host,
        "a branch is reached on its own endpoint"
    );
    assert_eq!(branch.parent_id, proj.branch.id);
    assert_eq!(branch.database.name, proj.database.name);
    let pw = branch
        .owner_role
        .password
        .expect("disclosed once, at create");
    assert_ne!(Some(pw), proj.owner_role.password);
}

#[tokio::test]
async fn creating_a_branch_that_exists_adopts_it_with_a_fresh_password() {
    let p = MockProvider::new();
    let proj = project(&p).await;
    let first = p.create_branch(&staging(&proj)).await.unwrap();
    let again = p.create_branch(&staging(&proj)).await.unwrap();

    assert_eq!(p.branch_count(), 1, "adopted, not duplicated");
    assert_eq!(again.id, first.id);
    assert_ne!(
        again.owner_role.password, first.owner_role.password,
        "the lost password is unrecoverable, so adoption re-mints it"
    );
}

#[tokio::test]
async fn a_branch_of_a_missing_project_is_refused() {
    let p = MockProvider::new();
    let proj = project(&p).await;
    let mut req = staging(&proj);
    req.project_id = "proj-nope".into();
    let err = p.create_branch(&req).await.unwrap_err();
    assert!(matches!(err, ProviderError::ProjectNotFound(_)));
}

#[tokio::test]
async fn reset_re_mints_the_owner_and_counts() {
    let p = MockProvider::new();
    let proj = project(&p).await;
    let branch = p.create_branch(&staging(&proj)).await.unwrap();
    let reset = p.reset_branch(&staging(&proj), &branch.id).await.unwrap();

    assert_eq!(reset.id, branch.id, "a reset keeps the branch");
    assert_eq!(reset.host, branch.host, "and its endpoint");
    assert_ne!(reset.owner_role.password, branch.owner_role.password);
    assert_eq!(p.branch_resets(&branch.id), 1);
}

#[tokio::test]
async fn resetting_a_missing_branch_says_so() {
    let p = MockProvider::new();
    let proj = project(&p).await;
    let err = p
        .reset_branch(&staging(&proj), "br-gone")
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::BranchNotFound(id) if id == "br-gone"));
}

#[tokio::test]
async fn delete_is_idempotent_and_never_takes_the_default_branch() {
    let p = MockProvider::new();
    let proj = project(&p).await;
    let branch = p.create_branch(&staging(&proj)).await.unwrap();

    p.delete_branch(&staging(&proj), &branch.id).await.unwrap();
    p.delete_branch(&staging(&proj), &branch.id).await.unwrap();
    assert_eq!(p.branch_count(), 0);

    let err = p
        .delete_branch(&staging(&proj), &proj.branch.id)
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::BranchIsProduction(_)));
    assert_eq!(
        p.branch_delete_attempts().last(),
        Some(&proj.branch.id),
        "the attempt is recorded even though it was refused"
    );
    assert!(
        p.get_project(&proj.id).await.unwrap().is_some(),
        "production survives a delete aimed at it"
    );
}

#[tokio::test]
async fn deleting_the_project_takes_its_branches() {
    let p = MockProvider::new();
    let proj = project(&p).await;
    p.create_branch(&staging(&proj)).await.unwrap();
    p.delete_project(&proj.id).await.unwrap();
    assert_eq!(p.branch_count(), 0);
}
