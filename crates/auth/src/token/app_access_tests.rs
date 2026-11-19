//! The credential an app's host op decides with, from its rows. The database
//! half (approve, revoke, `credential_for`) is driven by the server's
//! integration group; this is the part with decisions in it.

use chrono::Utc;
use entity::{api_tokens, app_platform_grants, apps, service_accounts};
use oxy_authz::{Action, AppActionGrant, RoleCeiling};
use uuid::Uuid;

use super::{DISPLAY_PREFIX, approved_targets, credential_of, effective_grants};
use crate::token::account_access::AccountRole;
use crate::token::credential::StoredKind;

const ORG: Uuid = Uuid::from_u128(0xA);
const OTHER_ORG: Uuid = Uuid::from_u128(0xB);
const APP: Uuid = Uuid::from_u128(0xAA);
const WS: Uuid = Uuid::from_u128(0xA1);
const OTHER_WS: Uuid = Uuid::from_u128(0xA2);
const ACCOUNT: Uuid = Uuid::from_u128(0x5A);
const TOKEN: Uuid = Uuid::from_u128(0x70);

fn app(workspace: Uuid) -> apps::Model {
    let now = Utc::now().fixed_offset();
    apps::Model {
        id: APP,
        slug: "store-ops".into(),
        name: "Store Ops".into(),
        org_id: ORG,
        project_id: workspace,
        branch: "main".into(),
        source_repo: String::new(),
        status: "active".into(),
        source_type: "bundle".into(),
        source_config: serde_json::json!({}),
        last_synced_at: None,
        manifest_override: None,
        bootstrap_pr_url: None,
        published_at: None,
        repo_path: None,
        last_promoted_by: None,
        last_promoted_at: None,
        visibility: "org".into(),
        require_second_person_to_promote: false,
        created_at: now,
        updated_at: now,
    }
}

fn grant(action: &str) -> app_platform_grants::Model {
    app_platform_grants::Model {
        id: Uuid::new_v4(),
        token_id: TOKEN,
        app_id: APP,
        org_id: ORG,
        action: action.into(),
        target: None,
        created_at: Utc::now().fixed_offset(),
        created_by: None,
        revoked_at: None,
        revoked_by: None,
    }
}

fn account() -> service_accounts::Model {
    service_accounts::Model {
        user_id: ACCOUNT,
        org_id: ORG,
        org_role: "admin".into(),
        name: "app-store-ops".into(),
        description: None,
        created_by: None,
        created_at: Utc::now().fixed_offset(),
        disabled_at: None,
    }
}

fn token() -> api_tokens::Model {
    api_tokens::Model {
        id: TOKEN,
        kind: StoredKind::App.as_str().into(),
        principal_user_id: ACCOUNT,
        name: "app:store-ops".into(),
        display_prefix: DISPLAY_PREFIX.into(),
        last_four: String::new(),
        token_hash: vec![0; 32],
        all_access: false,
        platform: false,
        partner: false,
        expires_at: None,
        last_used_at: None,
        created_at: Utc::now().fixed_offset(),
        created_by: None,
        revoked_at: None,
        revoked_by: None,
        revoke_reason: None,
        source: "app_approval".into(),
        legacy_api_key_id: None,
        trust_policy_id: None,
        oidc_claims: None,
    }
}

#[test]
fn an_action_is_approved_across_the_org() {
    let got = effective_grants(&app(WS), &[grant("frontline_device_manage")]);
    assert_eq!(
        got,
        vec![AppActionGrant {
            action: Action::FrontlineDeviceManage,
            org_id: ORG,
        }]
    );
    // Wherever the app is published from: approvals name no workspace.
    let rows = [grant("source_upload_report")];
    assert_eq!(
        effective_grants(&app(WS), &rows),
        effective_grants(&app(OTHER_WS), &rows)
    );
}

#[test]
fn rows_that_are_not_live_app_actions_grant_nothing() {
    let mut revoked = grant("frontline_device_manage");
    revoked.revoked_at = Some(Utc::now().fixed_offset());
    let mut elsewhere = grant("frontline_device_manage");
    elsewhere.org_id = OTHER_ORG;
    let rows = [
        grant("member_set_role"),
        grant("workspace_edit"),
        grant(""),
        revoked,
        elsewhere,
    ];
    assert!(effective_grants(&app(WS), &rows).is_empty());
}

#[test]
fn the_credential_is_the_apps_account_confined_to_its_approvals() {
    let rows = [
        grant("frontline_device_manage"),
        grant("source_upload_report"),
    ];
    let cred = credential_of(&app(WS), &account(), AccountRole::Admin, &token(), &rows);
    assert_eq!(cred.kind, StoredKind::App);
    assert_eq!(cred.token_id, TOKEN);
    assert_eq!(cred.principal_user_id, ACCOUNT);
    assert!(cred.is_service_account());
    assert!(!cred.all_access && !cred.platform && !cred.partner);
    assert_eq!(
        cred.grants.len(),
        1,
        "one org-wide grant, whatever is approved"
    );
    assert!(
        cred.grants.iter().all(|g| g.ceiling == RoleCeiling::Admin
            && g.workspace_id.is_none()
            && g.org_id == ORG)
    );
    let reach = cred.reach().expect("a credential always narrows");
    assert!(!reach.all_access && !reach.platform && !reach.partner);
    let app_reach = reach.app.expect("an app credential carries the app fact");
    assert_eq!(app_reach.app_id, APP);
    assert_eq!(app_reach.grants.len(), 2);
    assert!(reach.sandbox_agent.is_none());
}

#[test]
fn an_app_credential_with_nothing_read_still_carries_an_empty_app_fact() {
    let mut cred = credential_of(&app(WS), &account(), AccountRole::Admin, &token(), &[]);
    cred.app_access = None;
    let app_reach = cred.reach().and_then(|r| r.app).expect("fail closed");
    assert!(app_reach.grants.is_empty());
}

#[test]
fn no_other_kind_carries_an_app_fact() {
    let mut cred = credential_of(
        &app(WS),
        &account(),
        AccountRole::Admin,
        &token(),
        &[grant("frontline_device_manage")],
    );
    cred.kind = StoredKind::ServiceAccount;
    assert!(cred.reach().and_then(|r| r.app).is_none());
}

#[test]
fn an_upload_is_approved_per_pipeline() {
    let app = app(Uuid::new_v4());
    let on = |name: &str| app_platform_grants::Model {
        target: Some(name.into()),
        ..grant("source_upload_report")
    };
    let mut revoked = on("toast_pos");
    revoked.revoked_at = Some(Utc::now().fixed_offset());
    let elsewhere = app_platform_grants::Model {
        org_id: Uuid::new_v4(),
        ..on("other_org")
    };
    let grants = [
        on("ubereats_transactions"),
        on("ubereats_transactions"),
        revoked,
        elsewhere,
        grant("frontline_device_manage"),
    ];
    let got = approved_targets(&app, &grants, Action::SourceUploadReport);
    assert_eq!(
        got.into_iter().collect::<Vec<_>>(),
        ["ubereats_transactions"],
        "live rows of this app and org only, each pipeline once"
    );
    assert!(
        approved_targets(&app, &grants, Action::FrontlineDeviceManage).is_empty(),
        "kiosks are approved on the whole org, with no target"
    );
}
