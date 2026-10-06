//! Admission of the sandbox agent token: the `sandbox_agent` kind, the
//! `app_sandbox` grant, and the workspace grant each one stands for.

use super::*;
use crate::token::credential::{SandboxAppHome, place_sandbox_apps, source};

const ORG: Uuid = Uuid::from_u128(7);
const ELSEWHERE: Uuid = Uuid::from_u128(8);
const APP: Uuid = Uuid::from_u128(0xA99);
const OTHER_APP: Uuid = Uuid::from_u128(0xA9A);
const WORKSPACE: Uuid = Uuid::from_u128(0x77);
const MINTER: Uuid = Uuid::from_u128(1);

/// An `oxy_sbx_` row as the mint writes it.
fn sandbox_row() -> api_tokens::Model {
    let now = Utc::now().fixed_offset();
    api_tokens::Model {
        id: Uuid::from_u128(0x5B),
        kind: "sandbox_agent".into(),
        principal_user_id: MINTER,
        name: "agent".into(),
        display_prefix: "oxy_sbx_Ab3x".into(),
        last_four: "wxyz".into(),
        token_hash: vec![0; 32],
        all_access: false,
        platform: true,
        partner: false,
        expires_at: Some((Utc::now() + chrono::Duration::hours(8)).fixed_offset()),
        last_used_at: None,
        created_at: now,
        created_by: Some(MINTER),
        revoked_at: None,
        revoked_by: None,
        revoke_reason: None,
        source: source::UI.into(),
        legacy_api_key_id: None,
        trust_policy_id: None,
        oidc_claims: None,
    }
}

fn personal_row() -> api_tokens::Model {
    api_tokens::Model {
        kind: "personal".into(),
        display_prefix: "oxy_pat_Ab3x".into(),
        ..sandbox_row()
    }
}

fn grant(kind: &str, org_id: Uuid, app_id: Option<Uuid>) -> api_token_grants::Model {
    api_token_grants::Model {
        id: Uuid::new_v4(),
        token_id: sandbox_row().id,
        kind: kind.into(),
        org_id,
        workspace_id: None,
        role_ceiling: (kind == "workspace").then(|| "admin".to_string()),
        app_id,
        created_at: Utc::now().fixed_offset(),
        revoked_at: None,
        revoked_by: None,
    }
}

fn app_sandbox(org_id: Uuid, app_id: Uuid) -> api_token_grants::Model {
    grant("app_sandbox", org_id, Some(app_id))
}

fn admit_sandbox(
    row: &api_tokens::Model,
    grants: &[api_token_grants::Model],
) -> Result<CredentialContext, Refusal> {
    admit(
        row,
        grants,
        TokenFormat::SandboxAgent,
        Links::NONE,
        Utc::now(),
    )
}

fn home(app_id: Uuid, org_id: Uuid) -> SandboxAppHome {
    SandboxAppHome {
        app_id,
        org_id,
        workspace_id: WORKSPACE,
    }
}

#[test]
fn a_sandbox_row_is_admitted_with_its_apps_and_no_workspace_grant_yet() {
    let cred = admit_sandbox(&sandbox_row(), &[app_sandbox(ORG, APP)]).unwrap();
    assert_eq!(cred.kind, StoredKind::SandboxAgent);
    assert!(cred.is_sandbox_agent() && !cred.is_legacy() && !cred.is_service_account());
    assert_eq!(
        cred.app_sandbox,
        vec![AppSandboxGrant {
            org_id: ORG,
            app_id: APP
        }]
    );
    assert!(cred.sandboxes_app(APP) && !cred.sandboxes_app(OTHER_APP));
    // The workspace grant names where the app lives now, which only the store
    // can say: until it has, the token reaches no workspace at all.
    assert!(cred.grants.is_empty() && cred.app_publish.is_empty());
    assert!(cred.service_account.is_none());
}

#[test]
fn each_app_becomes_a_workspace_grant_at_admin_where_the_app_lives_now() {
    let mut cred = admit_sandbox(
        &sandbox_row(),
        &[app_sandbox(ORG, APP), app_sandbox(ORG, OTHER_APP)],
    )
    .unwrap();
    place_sandbox_apps(&mut cred, &[home(APP, ORG), home(OTHER_APP, ORG)]);
    // Two apps from one workspace: one grant between them.
    assert_eq!(
        cred.grants,
        vec![TokenGrant {
            org_id: ORG,
            workspace_id: Some(WORKSPACE),
            ceiling: RoleCeiling::Admin,
        }]
    );
    assert_eq!(cred.app_sandbox.len(), 2);

    let reach = cred.reach().unwrap();
    assert!(!reach.all_access && reach.platform && !reach.partner);
    assert_eq!(
        reach.workspace_ceiling(ORG, WORKSPACE),
        Some(RoleCeiling::Admin)
    );
    let sandbox = reach.sandbox_agent.unwrap();
    assert_eq!(sandbox.token_id, cred.token_id);
    assert_eq!(sandbox.apps.len(), 2);
}

#[test]
fn a_grant_whose_app_is_gone_or_moved_org_reaches_nothing() {
    let mut cred = admit_sandbox(
        &sandbox_row(),
        &[app_sandbox(ORG, APP), app_sandbox(ORG, OTHER_APP)],
    )
    .unwrap();
    // `APP` is gone; `OTHER_APP` now belongs to another org.
    place_sandbox_apps(&mut cred, &[home(OTHER_APP, ELSEWHERE)]);
    assert!(cred.app_sandbox.is_empty() && cred.grants.is_empty());
    // It is still a sandbox agent's reach — with no app — never a plain
    // grant-bound token's.
    let reach = cred.reach().unwrap();
    assert_eq!(reach.sandbox_agent.map(|s| s.apps.len()), Some(0));
}

#[test]
fn a_revoked_grant_is_skipped_and_the_others_stand() {
    let mut revoked = app_sandbox(ORG, APP);
    revoked.revoked_at = Some(Utc::now().fixed_offset());
    let cred = admit_sandbox(&sandbox_row(), &[revoked, app_sandbox(ORG, OTHER_APP)]).unwrap();
    assert_eq!(
        cred.app_sandbox,
        vec![AppSandboxGrant {
            org_id: ORG,
            app_id: OTHER_APP
        }]
    );
}

#[test]
fn a_revoked_or_expired_sandbox_token_is_refused() {
    let grants = [app_sandbox(ORG, APP)];
    let mut revoked = sandbox_row();
    revoked.revoked_at = Some(Utc::now().fixed_offset());
    assert_eq!(admit_sandbox(&revoked, &grants), Err(Refusal::Revoked));

    let mut expired = sandbox_row();
    expired.expires_at = Some((Utc::now() - chrono::Duration::seconds(1)).fixed_offset());
    assert_eq!(admit_sandbox(&expired, &grants), Err(Refusal::Expired));
}

#[test]
fn a_sandbox_row_claiming_more_than_the_kind_holds_is_refused() {
    let grants = [app_sandbox(ORG, APP)];
    let widened = |edit: fn(&mut api_tokens::Model)| {
        let mut row = sandbox_row();
        edit(&mut row);
        admit_sandbox(&row, &grants)
    };
    assert_eq!(
        widened(|r| r.all_access = true),
        Err(Refusal::SandboxWidened("all_access"))
    );
    assert_eq!(
        widened(|r| r.partner = true),
        Err(Refusal::SandboxWidened("partner"))
    );
    assert_eq!(
        widened(|r| r.expires_at = None),
        Err(Refusal::SandboxWidened("no expiry"))
    );
    assert_eq!(
        widened(|r| r.legacy_api_key_id = Some(Uuid::from_u128(3))),
        Err(Refusal::SandboxWidened("an api_keys mirror"))
    );
}

#[test]
fn a_sandbox_token_holds_no_other_kind_of_grant() {
    for other in [
        grant("workspace", ORG, None),
        grant("app_publish", ORG, Some(APP)),
    ] {
        let refused = admit_sandbox(&sandbox_row(), &[app_sandbox(ORG, APP), other]);
        assert!(
            matches!(refused, Err(Refusal::UnknownGrant(_))),
            "{refused:?}"
        );
    }
    // And a grant that names no app is one this release cannot read.
    let nameless = grant("app_sandbox", ORG, None);
    assert!(matches!(
        admit_sandbox(&sandbox_row(), &[nameless]),
        Err(Refusal::UnknownGrant(_))
    ));
}

#[test]
fn an_app_sandbox_grant_on_any_other_kind_is_refused() {
    // Nothing enforces the grant on a personal token, so honouring the token
    // would be honouring a restriction that is not applied.
    let refused = admit(
        &personal_row(),
        &[app_sandbox(ORG, APP)],
        TokenFormat::Personal,
        Links::NONE,
        Utc::now(),
    );
    assert!(
        matches!(refused, Err(Refusal::UnknownGrant(_))),
        "{refused:?}"
    );
}

#[test]
fn the_format_must_match_the_stored_kind() {
    let grants = [app_sandbox(ORG, APP)];
    // An `oxy_sbx_` row presented as a personal token, and the reverse.
    assert_eq!(
        admit(
            &sandbox_row(),
            &grants,
            TokenFormat::Personal,
            Links::NONE,
            Utc::now()
        ),
        Err(Refusal::KindMismatch)
    );
    let mut personal = personal_row();
    personal.all_access = true;
    assert_eq!(
        admit_sandbox(&personal, &[]),
        Err(Refusal::KindMismatch),
        "a personal row never resolves from an oxy_sbx_ secret"
    );
}

#[test]
fn placing_apps_changes_no_other_kind() {
    let mut personal = personal_row();
    personal.all_access = true;
    let mut cred = admit(
        &personal,
        &[],
        TokenFormat::Personal,
        Links::NONE,
        Utc::now(),
    )
    .unwrap();
    let before = cred.clone();
    place_sandbox_apps(&mut cred, &[home(APP, ORG)]);
    assert_eq!(cred, before);
    assert_eq!(cred.reach().unwrap().sandbox_agent, None);
}

#[test]
fn the_workspace_reader_reads_an_app_sandbox_grant_as_none() {
    // The inventories read workspace grants through this; a sandbox agent
    // token's grant names an app, and reaches no workspace on its own.
    assert_eq!(readable_grants(&[app_sandbox(ORG, APP)]), Ok(Vec::new()));
    assert!(blocked_orgs(&[app_sandbox(ORG, APP)]).is_empty());
}
