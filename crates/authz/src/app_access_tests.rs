//! An app's credential in the model: the app's service account, an admin of
//! its one org, narrowed to the actions an org admin approved for the app.
//!
//! The account's standing alone would reach the whole `OrgAdmin` ring. These
//! tests prove the approval is what confines it: an approved action passes
//! where it was approved and nowhere else, and every other action refuses,
//! including the ones that share its ring.

use uuid::Uuid;

use super::{AppActionGrant, AppReach};
use crate::{
    Action, PrincipalFacts, Resource, RoleCeiling, ServiceAccountStanding, TokenGrant, TokenReach,
    allows,
};

const ORG: Uuid = Uuid::from_u128(0xA);
const OTHER_ORG: Uuid = Uuid::from_u128(0xB);
const WS: Uuid = Uuid::from_u128(0xA1);
const OTHER_WS: Uuid = Uuid::from_u128(0xA2);
const APP: Uuid = Uuid::from_u128(0xAA);
const ACCOUNT: Uuid = Uuid::from_u128(0x5A);

fn approval(action: Action) -> AppActionGrant {
    AppActionGrant {
        action,
        org_id: ORG,
    }
}

/// The credential the host builds for an app: one org-wide admin grant per
/// approved action, and the actions themselves.
fn app_token(approvals: Vec<AppActionGrant>) -> TokenReach {
    let grants = approvals
        .iter()
        .map(|a| TokenGrant {
            org_id: a.org_id,
            workspace_id: None,
            ceiling: RoleCeiling::Admin,
        })
        .collect();
    TokenReach {
        all_access: false,
        platform: false,
        partner: false,
        grants,
        blocked_orgs: Vec::new(),
        sandbox_agent: None,
        app: Some(AppReach {
            app_id: APP,
            grants: approvals,
        }),
        ci_run: false,
    }
}

/// The app's service account in `ORG`, as the loader builds it, narrowed by
/// `token` as the loader narrows every credential.
fn app_account(token: &TokenReach) -> PrincipalFacts {
    PrincipalFacts {
        user_id: ACCOUNT,
        service_account: Some(ServiceAccountStanding {
            org_id: ORG,
            admin: true,
        }),
        ..Default::default()
    }
    .narrowed_by(token)
}

#[test]
fn an_approved_org_action_passes_in_its_org_only() {
    let facts = app_account(&app_token(vec![approval(Action::FrontlineDeviceManage)]));
    assert!(allows(
        &facts,
        Action::FrontlineDeviceManage,
        &Resource::org(ORG)
    ));
    assert!(!allows(
        &facts,
        Action::FrontlineDeviceManage,
        &Resource::org(OTHER_ORG)
    ));
}

#[test]
fn the_rest_of_the_ring_is_refused() {
    let facts = app_account(&app_token(vec![approval(Action::FrontlineDeviceManage)]));
    let org = Resource::org(ORG);
    for action in Action::ALL {
        if action == Action::FrontlineDeviceManage {
            continue;
        }
        for resource in [
            org.clone(),
            Resource::workspace(WS, ORG),
            Resource::app(APP, ORG),
            Resource::platform(),
        ] {
            assert!(
                !allows(&facts, action, &resource),
                "an app approved for kiosks must not reach {action:?} on {resource:?}"
            );
        }
    }
}

#[test]
fn an_upload_approval_reaches_the_orgs_workspaces_and_nothing_else() {
    let facts = app_account(&app_token(vec![approval(Action::SourceUploadReport)]));
    // No workspace is named: the approval is the org's, and the host op says
    // which pipeline's landing zone a call may reach.
    for ws in [WS, OTHER_WS] {
        assert!(allows(
            &facts,
            Action::SourceUploadReport,
            &Resource::workspace(ws, ORG)
        ));
        assert!(!allows(
            &facts,
            Action::WorkspaceEdit,
            &Resource::workspace(ws, ORG)
        ));
    }
    assert!(!allows(
        &facts,
        Action::SourceUploadReport,
        &Resource::workspace(WS, OTHER_ORG)
    ));
    assert!(!allows(
        &facts,
        Action::SourceUploadReport,
        &Resource::app(APP, ORG)
    ));
}

#[test]
fn no_approval_reaches_nothing() {
    let facts = app_account(&app_token(Vec::new()));
    for action in Action::ALL {
        assert!(!allows(&facts, action, &Resource::org(ORG)));
        assert!(!allows(&facts, action, &Resource::workspace(WS, ORG)));
    }
}

#[test]
fn an_approval_needs_the_standing_too() {
    // The reach only subtracts: a plain-member account approved for kiosks
    // still cannot manage them, because the ring wants an admin.
    let token = app_token(vec![approval(Action::FrontlineDeviceManage)]);
    let member = PrincipalFacts {
        user_id: ACCOUNT,
        service_account: Some(ServiceAccountStanding {
            org_id: ORG,
            admin: false,
        }),
        ..Default::default()
    }
    .narrowed_by(&token);
    assert!(!allows(
        &member,
        Action::FrontlineDeviceManage,
        &Resource::org(ORG)
    ));
}

#[test]
fn a_person_holding_an_app_reach_keeps_none_of_their_standing() {
    // Never built by the host, which names the app's account. If it ever were,
    // the person's memberships are dropped rather than carried.
    let token = app_token(vec![approval(Action::FrontlineDeviceManage)]);
    let person = PrincipalFacts {
        user_id: Uuid::from_u128(0x1),
        member_orgs: vec![ORG],
        admin_orgs: vec![ORG],
        owned_orgs: vec![ORG],
        ..Default::default()
    }
    .narrowed_by(&token);
    assert!(!allows(
        &person,
        Action::FrontlineDeviceManage,
        &Resource::org(ORG)
    ));
    assert!(!allows(&person, Action::OrgRead, &Resource::org(ORG)));
}

#[test]
fn only_app_actions_have_an_id() {
    for action in Action::APP_ACTIONS {
        let id = action.app_action_id().expect("an app action has an id");
        assert_eq!(Action::from_app_action_id(id), Some(action));
    }
    for action in Action::ALL {
        if !Action::APP_ACTIONS.contains(&action) {
            assert_eq!(
                action.app_action_id(),
                None,
                "{action:?} is not an app action"
            );
            assert_eq!(Action::from_app_action_id(action.as_str()), None);
        }
    }
    assert_eq!(Action::from_app_action_id("member_set_role"), None);
    assert_eq!(Action::from_app_action_id(""), None);
}

#[test]
fn the_workspace_actions_are_the_ones_decided_on_a_workspace() {
    assert!(Action::SourceUploadReport.app_action_on_workspace());
    assert!(!Action::FrontlineDeviceManage.app_action_on_workspace());
}
