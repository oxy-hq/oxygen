use super::*;
use axum::http::StatusCode;
use entity::org_members::OrgRole;
use entity::workspace_members::WorkspaceRole;
use uuid::Uuid;

fn make_org_member(role: OrgRole) -> entity::org_members::Model {
    let now = chrono::Utc::now().fixed_offset();
    entity::org_members::Model {
        id: Uuid::new_v4(),
        org_id: Uuid::new_v4(),
        user_id: Uuid::new_v4(),
        role,
        created_at: now,
        updated_at: now,
    }
}

// ---- validate_role_override ----

#[test]
fn validate_role_override_admin_cannot_target_owner() {
    let caller = make_org_member(OrgRole::Admin);
    let target = make_org_member(OrgRole::Owner);
    let err = validate_role_override(&caller, &target, &WorkspaceRole::Viewer).unwrap_err();
    assert_eq!(err, StatusCode::FORBIDDEN);
}

#[test]
fn validate_role_override_admin_cannot_target_admin() {
    let caller = make_org_member(OrgRole::Admin);
    let target = make_org_member(OrgRole::Admin);
    let err = validate_role_override(&caller, &target, &WorkspaceRole::Viewer).unwrap_err();
    assert_eq!(err, StatusCode::FORBIDDEN);
}

#[test]
fn validate_role_override_owner_can_target_admin() {
    let caller = make_org_member(OrgRole::Owner);
    let target = make_org_member(OrgRole::Admin);
    assert!(validate_role_override(&caller, &target, &WorkspaceRole::Viewer).is_ok());
}

#[test]
fn validate_role_override_owner_can_grant_owner() {
    let caller = make_org_member(OrgRole::Owner);
    let target = make_org_member(OrgRole::Member);
    assert!(validate_role_override(&caller, &target, &WorkspaceRole::Owner).is_ok());
}

#[test]
fn validate_role_override_admin_cannot_grant_owner() {
    let caller = make_org_member(OrgRole::Admin);
    let target = make_org_member(OrgRole::Member);
    let err = validate_role_override(&caller, &target, &WorkspaceRole::Owner).unwrap_err();
    assert_eq!(err, StatusCode::FORBIDDEN);
}

#[test]
fn validate_role_override_admin_can_target_member() {
    let caller = make_org_member(OrgRole::Admin);
    let target = make_org_member(OrgRole::Member);
    assert!(validate_role_override(&caller, &target, &WorkspaceRole::Admin).is_ok());
}

// ---- map_org_role_to_workspace ----

#[test]
fn maps_owner_to_owner() {
    assert_eq!(
        map_org_role_to_workspace(&OrgRole::Owner),
        WorkspaceRole::Owner
    );
}

#[test]
fn maps_admin_to_admin() {
    assert_eq!(
        map_org_role_to_workspace(&OrgRole::Admin),
        WorkspaceRole::Admin
    );
}

#[test]
fn maps_member_to_member() {
    assert_eq!(
        map_org_role_to_workspace(&OrgRole::Member),
        WorkspaceRole::Member
    );
}
