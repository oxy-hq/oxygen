use super::*;
use chrono::Utc;

fn account() -> service_accounts::Model {
    service_accounts::Model {
        user_id: Uuid::from_u128(7),
        org_id: Uuid::from_u128(0xA),
        org_role: "member".into(),
        name: "deployer".into(),
        description: None,
        created_by: None,
        created_at: Utc::now().fixed_offset(),
        disabled_at: None,
    }
}

fn policy() -> oidc_trust_policies::Model {
    oidc_trust_policies::Model {
        id: Uuid::from_u128(1),
        org_id: Uuid::from_u128(0xA),
        service_account_id: Uuid::from_u128(7),
        provider: "github_actions".into(),
        repository_owner_id: 42,
        repository_id: 987,
        repository: "acme/app".into(),
        workflow_path: ".github/workflows/release.yml".into(),
        environment: Some("production".into()),
        ref_pattern: None,
        allow_self_hosted: false,
        created_by: None,
        created_at: Utc::now().fixed_offset(),
        last_used_at: None,
        disabled_at: None,
    }
}

#[test]
fn the_audit_row_is_in_the_policys_org_and_names_the_policy() {
    let actor = RequestActor::session(oxy_auth::types::AuthenticatedUser {
        id: Uuid::from_u128(1),
        email: Some("ada@acme.com".into()),
        name: "Ada".into(),
        picture: None,
        status: entity::users::UserStatus::Active,
        credential: None,
    });
    let row = policy();
    let entry = entry(&actor, CREATED, &row, summary(&account(), &row, &[]));
    assert_eq!(entry.org_id, Some(row.org_id));
    assert_eq!(entry.target_type.as_deref(), Some(TARGET_TYPE));
    assert_eq!(
        entry.target_id.as_deref(),
        Some(row.id.to_string().as_str())
    );
}

#[test]
fn the_summary_changes_with_anything_a_run_is_matched_or_granted_by() {
    let base = summary(&account(), &policy(), &[]);
    let mut edited = policy();
    edited.allow_self_hosted = true;
    assert_ne!(base, summary(&account(), &edited, &[]));
    let mut disabled = policy();
    disabled.disabled_at = Some(Utc::now().fixed_offset());
    assert_ne!(base, summary(&account(), &disabled, &[]));
    // Being used is not a change to the policy.
    let mut used = policy();
    used.last_used_at = Some(Utc::now().fixed_offset());
    assert_eq!(base, summary(&account(), &used, &[]));
}

fn grant(ceiling: &str, workspace: Option<u128>) -> GrantRow {
    oidc_trust_policy_grants::Model {
        id: Uuid::new_v4(),
        policy_id: Uuid::from_u128(1),
        kind: "workspace".into(),
        org_id: Uuid::from_u128(0xA),
        workspace_id: workspace.map(Uuid::from_u128),
        role_ceiling: Some(ceiling.into()),
        app_id: None,
        created_at: Utc::now().fixed_offset(),
    }
}

#[test]
fn replacing_the_grants_with_an_equal_set_is_no_change() {
    // A PATCH re-inserts the rows, under new ids and in any order.
    let before = [grant("viewer", Some(1)), grant("member", Some(2))];
    let after = [grant("member", Some(2)), grant("viewer", Some(1))];
    assert_eq!(grants_summary(&before), grants_summary(&after));
    let narrowed = [grant("viewer", Some(1))];
    assert_ne!(grants_summary(&before), grants_summary(&narrowed));
    let raised = [grant("member", Some(1)), grant("member", Some(2))];
    assert_ne!(grants_summary(&before), grants_summary(&raised));
}
