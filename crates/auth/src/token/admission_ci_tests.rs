//! Admission of what Phase 4 added: the `ci` kind, and `app_publish` grants.

use super::*;
use crate::token::credential::source;

const ORG: Uuid = Uuid::from_u128(7);
const ELSEWHERE: Uuid = Uuid::from_u128(8);
const APP: Uuid = Uuid::from_u128(0xA99);

/// An `oxy_ci_` row as the exchange mints it.
fn ci_row() -> api_tokens::Model {
    let now = Utc::now().fixed_offset();
    api_tokens::Model {
        id: Uuid::new_v4(),
        kind: "ci".into(),
        principal_user_id: Uuid::from_u128(1),
        name: "github-oidc:acme/app/.github/workflows/release.yml@refs/heads/main".into(),
        display_prefix: "oxy_ci_Ab3x".into(),
        last_four: "wxyz".into(),
        token_hash: vec![0; 32],
        all_access: false,
        platform: false,
        partner: false,
        expires_at: Some((Utc::now() + chrono::Duration::minutes(15)).fixed_offset()),
        last_used_at: None,
        created_at: now,
        created_by: None,
        revoked_at: None,
        revoked_by: None,
        revoke_reason: None,
        source: source::OIDC.into(),
        legacy_api_key_id: None,
        trust_policy_id: Some(Uuid::from_u128(0x90)),
        oidc_claims: Some(serde_json::json!({ "run_id": "7001" })),
    }
}

fn personal_row() -> api_tokens::Model {
    api_tokens::Model {
        kind: "personal".into(),
        display_prefix: "oxy_pat_Ab3x".into(),
        expires_at: None,
        source: source::UI.into(),
        trust_policy_id: None,
        oidc_claims: None,
        ..ci_row()
    }
}

fn grant(token: Uuid, kind: &str, org_id: Uuid) -> api_token_grants::Model {
    api_token_grants::Model {
        id: Uuid::new_v4(),
        token_id: token,
        kind: kind.into(),
        org_id,
        workspace_id: None,
        role_ceiling: (kind == "workspace").then(|| "member".to_string()),
        app_id: (kind == "app_publish").then_some(APP),
        created_at: Utc::now().fixed_offset(),
        revoked_at: None,
        revoked_by: None,
    }
}

fn active() -> AccountLink {
    AccountLink::Active(AccountStanding {
        org_id: ORG,
        admin: false,
    })
}

fn admit_ci(
    row: &api_tokens::Model,
    grants: &[api_token_grants::Model],
    account: AccountLink,
) -> Result<CredentialContext, Refusal> {
    admit(
        row,
        grants,
        TokenFormat::Ci,
        Links::account(account),
        Utc::now(),
    )
}

fn admit_personal(
    row: &api_tokens::Model,
    grants: &[api_token_grants::Model],
) -> Result<CredentialContext, Refusal> {
    admit(row, grants, TokenFormat::Personal, Links::NONE, Utc::now())
}

#[test]
fn a_ci_token_is_admitted_as_its_account_grant_bound_with_no_standing() {
    let row = ci_row();
    let grants = [grant(row.id, "workspace", ORG)];
    let cred = admit_ci(&row, &grants, active()).expect("admit");
    assert_eq!(cred.kind, StoredKind::Ci);
    assert!(cred.is_service_account() && !cred.is_legacy());
    assert_eq!(
        cred.service_account,
        Some(AccountStanding {
            org_id: ORG,
            admin: false
        })
    );
    let reach = cred.reach().expect("a ci token always narrows");
    assert!(!reach.all_access && !reach.platform && !reach.partner);
    assert!(reach.touches_org(ORG));
    assert!(!reach.touches_org(ELSEWHERE));
}

#[test]
fn the_ci_kind_round_trips_and_acts_as_an_account() {
    assert_eq!(StoredKind::parse("ci"), Some(StoredKind::Ci));
    assert_eq!(StoredKind::Ci.as_str(), "ci");
    assert!(StoredKind::Ci.acts_as_account());
    assert!(StoredKind::ServiceAccount.acts_as_account());
    assert!(!StoredKind::Personal.acts_as_account());
    assert!(!StoredKind::LegacyKey.acts_as_account());
}

#[test]
fn a_ci_row_is_admitted_only_when_presented_as_one() {
    let row = ci_row();
    for presented in [
        TokenFormat::Personal,
        TokenFormat::ServiceAccount,
        TokenFormat::LegacyKey,
    ] {
        let refused = admit(&row, &[], presented, Links::account(active()), Utc::now());
        assert_eq!(refused, Err(Refusal::KindMismatch), "{presented:?}");
    }
    // And an `oxy_ci_` secret never opens another kind's row.
    let refused = admit(
        &personal_row(),
        &[],
        TokenFormat::Ci,
        Links::NONE,
        Utc::now(),
    );
    assert_eq!(refused, Err(Refusal::KindMismatch));
}

#[test]
fn a_ci_row_claiming_what_no_account_holds_is_refused() {
    for (flag, widen) in [
        (
            "all_access",
            (|r| r.all_access = true) as fn(&mut api_tokens::Model),
        ),
        ("platform", |r| r.platform = true),
        ("partner", |r| r.partner = true),
        ("an api_keys mirror", |r| {
            r.legacy_api_key_id = Some(Uuid::new_v4())
        }),
    ] {
        let mut row = ci_row();
        widen(&mut row);
        assert_eq!(
            admit_ci(&row, &[], active()),
            Err(Refusal::Widened(flag)),
            "{flag}"
        );
    }
}

#[test]
fn a_ci_token_stops_with_its_account() {
    let row = ci_row();
    assert_eq!(
        admit_ci(&row, &[], AccountLink::Disabled),
        Err(Refusal::AccountDisabled)
    );
    for gone in [AccountLink::Missing, AccountLink::NotAccount] {
        assert_eq!(admit_ci(&row, &[], gone), Err(Refusal::AccountMissing));
    }
}

#[test]
fn a_ci_token_expires_and_is_revoked_like_any_other() {
    let mut expired = ci_row();
    expired.expires_at = Some((Utc::now() - chrono::Duration::seconds(1)).fixed_offset());
    assert_eq!(admit_ci(&expired, &[], active()), Err(Refusal::Expired));
    let mut revoked = ci_row();
    revoked.revoked_at = Some(Utc::now().fixed_offset());
    assert_eq!(admit_ci(&revoked, &[], active()), Err(Refusal::Revoked));
}

#[test]
fn an_app_publish_grant_is_admitted_and_reaches_no_workspace() {
    let row = ci_row();
    let grants = [grant(row.id, "app_publish", ORG)];
    let cred = admit_ci(&row, &grants, active()).expect("admit");
    assert!(cred.holds_app_publish());
    assert!(cred.publishes_app(APP));
    assert!(!cred.publishes_app(Uuid::from_u128(0xBAD)));
    assert_eq!(
        cred.app_publish,
        vec![AppPublishGrant {
            org_id: ORG,
            app_id: APP
        }]
    );
    // The grant is not reach: the token covers no workspace and no org.
    assert!(cred.grants.is_empty());
    let reach = cred.reach().expect("grant-bound");
    assert!(reach.bound());
    assert!(!reach.touches_org(ORG));
}

#[test]
fn an_app_publish_grant_sits_beside_workspace_grants() {
    let row = ci_row();
    let grants = [
        grant(row.id, "workspace", ORG),
        grant(row.id, "app_publish", ORG),
    ];
    let cred = admit_ci(&row, &grants, active()).expect("admit");
    assert_eq!(cred.grants.len(), 1);
    assert_eq!(cred.app_publish.len(), 1);
}

#[test]
fn an_accounts_app_publish_grant_outside_its_org_is_refused() {
    let row = ci_row();
    let grants = [grant(row.id, "app_publish", ELSEWHERE)];
    assert!(matches!(
        admit_ci(&row, &grants, active()),
        Err(Refusal::UnknownGrant(_))
    ));
}

#[test]
fn an_app_publish_grant_naming_no_app_refuses_the_token() {
    let row = ci_row();
    let mut nameless = grant(row.id, "app_publish", ORG);
    nameless.app_id = None;
    assert_eq!(
        admit_ci(&row, &[nameless], active()),
        Err(Refusal::UnknownGrant(
            "app_publish naming no app".to_string()
        ))
    );
}

#[test]
fn a_revoked_app_publish_grant_no_longer_publishes() {
    let row = ci_row();
    let mut revoked = grant(row.id, "app_publish", ORG);
    revoked.revoked_at = Some(Utc::now().fixed_offset());
    let cred = admit_ci(&row, &[revoked], active()).expect("admit");
    assert!(!cred.holds_app_publish());
}

#[test]
fn a_personal_token_holding_an_app_publish_grant_is_admitted() {
    // The validator refused these until this release could confine them.
    let row = personal_row();
    let grants = [grant(row.id, "app_publish", ORG)];
    let cred = admit_personal(&row, &grants).expect("admit");
    assert_eq!(cred.kind, StoredKind::Personal);
    assert!(cred.publishes_app(APP));
    assert!(cred.grants.is_empty());
    assert!(!cred.is_service_account());
}

#[test]
fn an_all_access_token_holds_no_app_publish_confinement() {
    // "Everything I can reach" is a flag: grant rows beside it are not read.
    let mut row = personal_row();
    row.all_access = true;
    let grants = [grant(row.id, "app_publish", ORG)];
    let cred = admit_personal(&row, &grants).expect("admit");
    assert!(!cred.holds_app_publish());
}

#[test]
fn a_legacy_credential_never_carries_an_app_publish_grant() {
    // A row that mirrors `api_keys` may not be narrowed at all: a grant row
    // beside it refuses the token rather than confining it.
    let mut row = personal_row();
    row.all_access = true;
    row.platform = true;
    row.partner = true;
    row.legacy_api_key_id = Some(row.id);
    let grants = [grant(row.id, "app_publish", ORG)];
    let refused = admit(
        &row,
        &grants,
        TokenFormat::Personal,
        Links::legacy(LegacyLink::Active),
        Utc::now(),
    );
    assert_eq!(refused, Err(Refusal::Narrowed("grants")));
    let admitted = admit(
        &row,
        &[],
        TokenFormat::Personal,
        Links::legacy(LegacyLink::Active),
        Utc::now(),
    )
    .expect("a legacy credential with no grant rows is admitted, as ever");
    assert!(admitted.is_legacy() && !admitted.holds_app_publish());
}
