//! The compile check's guard against the editor guard it replaced on the staging
//! compile routes, from request extensions alone. The facts are pre-inserted where
//! `request_facts` memoizes them, so no database is read — what is under test is the
//! guard's own arithmetic: the shipped term, the model, and their conjunction.

use super::*;
use crate::role_guards::{ExperimentPricer, WorkspaceEditor};
use axum::body::Body;
use axum::http::Request;
use oxy_auth::token::{AccountStanding, CredentialContext, StoredKind};
use oxy_auth::types::AuthenticatedUser;
use oxy_authz::{PrincipalFacts, RoleCeiling, ServiceAccountStanding, TokenGrant};
use uuid::Uuid;

const ORG: Uuid = Uuid::from_u128(0x7);
const WS: Uuid = Uuid::from_u128(0x71);
const ACCOUNT: Uuid = Uuid::from_u128(0x5A);

fn workspace(org_id: Option<Uuid>) -> entity::workspaces::Model {
    let now = chrono::Utc::now().fixed_offset();
    entity::workspaces::Model {
        id: WS,
        name: "ws".into(),
        git_namespace_id: None,
        git_remote_url: None,
        created_at: now,
        updated_at: now,
        path: None,
        last_opened_at: None,
        created_by: None,
        org_id,
        status: entity::workspaces::WorkspaceStatus::Ready,
        error: None,
        monthly_vlm_budget_micros: None,
        current_revision_id: None,
        default_branch: None,
        repo_subdir: None,
        default_branch_attempted_at: None,
    }
}

/// The org's member service account, through a token of `kind` holding `viewer`
/// on `WS` — what a trust policy for the CI gate grants its run.
fn account_credential(kind: StoredKind) -> CredentialContext {
    CredentialContext {
        token_id: Uuid::from_u128(0x9),
        kind,
        principal_user_id: ACCOUNT,
        all_access: false,
        platform: false,
        partner: false,
        name: "ci".into(),
        display_prefix: "oxy_ci_Ab3x".into(),
        blocked_orgs: Vec::new(),
        expires_at: None,
        service_account: Some(AccountStanding {
            org_id: ORG,
            admin: false,
        }),
        grants: vec![TokenGrant {
            org_id: ORG,
            workspace_id: Some(WS),
            ceiling: RoleCeiling::Viewer,
        }],
        app_publish: Vec::new(),
        app_sandbox: Vec::new(),
        app_access: None,
    }
}

/// A request as the workspace middleware leaves it for the account: its role
/// there is Viewer (Member capped at the grant's ceiling), and the facts the
/// loader would load — the account's standing, narrowed by the token.
fn account_request(kind: StoredKind) -> Parts {
    let credential = account_credential(kind);
    let user = AuthenticatedUser {
        id: ACCOUNT,
        email: None,
        name: "ci".into(),
        picture: None,
        status: entity::users::UserStatus::Active,
        credential: Some(credential.clone()),
    };
    let reach = credential.reach().expect("a token narrows");
    let facts = PrincipalFacts {
        user_id: ACCOUNT,
        service_account: Some(ServiceAccountStanding {
            org_id: ORG,
            admin: false,
        }),
        ..Default::default()
    }
    .narrowed_by(&reach);
    let mut req = Request::builder().body(Body::empty()).unwrap();
    let ext = req.extensions_mut();
    ext.insert(EffectiveWorkspaceRole(WorkspaceRole::Viewer));
    ext.insert(workspace(Some(ORG)));
    ext.insert(user);
    ext.insert(credential);
    ext.insert(facts);
    req.into_parts().0
}

fn role_only(role: WorkspaceRole) -> Parts {
    let mut req = Request::builder().body(Body::empty()).unwrap();
    req.extensions_mut().insert(EffectiveWorkspaceRole(role));
    req.into_parts().0
}

#[tokio::test]
async fn the_orgs_account_through_a_ci_run_token_checks_a_compile_at_viewer() {
    let mut parts = account_request(StoredKind::Ci);
    let CompileChecker(role) = CompileChecker::from_request_parts(&mut parts, &())
        .await
        .expect("the CI gate's account is admitted");
    assert_eq!(role, WorkspaceRole::Viewer);
}

/// The proof the account gains nothing else: the very request the compile check
/// admits is refused by the editor guard, and by a contributor guard beside it.
#[tokio::test]
async fn the_same_request_is_still_refused_by_every_editor_guard() {
    let mut parts = account_request(StoredKind::Ci);
    assert_eq!(
        WorkspaceEditor::from_request_parts(&mut parts, &())
            .await
            .unwrap_err(),
        StatusCode::FORBIDDEN
    );
    let mut parts = account_request(StoredKind::Ci);
    assert_eq!(
        ExperimentPricer::from_request_parts(&mut parts, &())
            .await
            .unwrap_err(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn the_accounts_own_long_lived_token_is_refused_at_viewer() {
    let mut parts = account_request(StoredKind::ServiceAccount);
    assert_eq!(
        CompileChecker::from_request_parts(&mut parts, &())
            .await
            .unwrap_err(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn a_person_keeps_the_editor_guards_answer() {
    // No workspace org to decide in: the shipped term alone, as `WorkspaceEditor`.
    let mut viewer = role_only(WorkspaceRole::Viewer);
    assert_eq!(
        CompileChecker::from_request_parts(&mut viewer, &())
            .await
            .unwrap_err(),
        StatusCode::FORBIDDEN
    );
    let mut member = role_only(WorkspaceRole::Member);
    assert!(
        CompileChecker::from_request_parts(&mut member, &())
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn a_missing_role_is_a_wiring_bug() {
    let mut parts = Request::builder()
        .body(Body::empty())
        .unwrap()
        .into_parts()
        .0;
    assert_eq!(
        CompileChecker::from_request_parts(&mut parts, &())
            .await
            .unwrap_err(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
}
