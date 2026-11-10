//! Where a GitHub import lands for an org.

use super::*;

#[test]
fn an_org_without_a_workspace_gets_a_new_one() {
    assert_eq!(target_for(None).unwrap(), ImportTarget::New);
}

/// The blank `Default` every org is created with is the one the import
/// connects, so the org keeps one workspace and its id.
#[test]
fn a_workspace_without_a_repository_is_adopted() {
    let id = Uuid::new_v4();
    assert_eq!(
        target_for(Some((id, true))).unwrap(),
        ImportTarget::Adopt(id)
    );
}

/// A second import is not a way to switch repositories, and never makes a
/// second workspace.
#[test]
fn a_workspace_with_a_repository_refuses_another_import() {
    let refused = target_for(Some((Uuid::new_v4(), false))).unwrap_err();
    assert!(matches!(refused, ProvisionError::OrgHasWorkspace));
    assert_eq!(refused.status_code(), axum::http::StatusCode::CONFLICT);
    assert!(refused.to_string().starts_with("org_has_workspace:"));
}

/// A workspace whose import failed is adoptable again, so a typo in the branch
/// or subdirectory can be corrected by importing once more; so is one whose
/// clone went stale (its pod died mid-clone). One that is cloning now, or
/// working, is not.
#[test]
fn only_a_failed_import_can_be_taken_over() {
    let now = chrono::Utc::now().fixed_offset();
    let ok = |remote, status: WorkspaceStatus, age: chrono::Duration| {
        adoptable_state(remote, &status, now - age, now)
    };
    let fresh = chrono::Duration::minutes(5);
    let stale = STALE_CLONE + chrono::Duration::minutes(1);
    assert!(ok(false, WorkspaceStatus::Ready, fresh));
    assert!(ok(true, WorkspaceStatus::Failed, fresh));
    assert!(ok(true, WorkspaceStatus::NotOxyProject, fresh));
    assert!(ok(true, WorkspaceStatus::Cloning, stale));
    assert!(!ok(true, WorkspaceStatus::Cloning, fresh));
    assert!(!ok(true, WorkspaceStatus::Ready, stale));
}
