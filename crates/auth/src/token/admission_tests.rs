//! Unit tests for the refuse-what-you-can't-enforce admission check.

use super::*;
use crate::token::credential::source;
use chrono::Duration;
use entity::service_accounts;
use oxy_authz::TokenReach;

fn row(kind: &str) -> api_tokens::Model {
    let now = Utc::now().fixed_offset();
    let id = Uuid::new_v4();
    api_tokens::Model {
        id,
        kind: kind.to_string(),
        principal_user_id: Uuid::new_v4(),
        name: "ci".into(),
        display_prefix: "oxy_pat_Ab3x".into(),
        last_four: "wxyz".into(),
        token_hash: vec![0; 32],
        all_access: true,
        platform: true,
        partner: true,
        expires_at: None,
        last_used_at: None,
        created_at: now,
        created_by: None,
        revoked_at: None,
        revoked_by: None,
        revoke_reason: None,
        source: source::LEGACY_ENDPOINT.into(),
        legacy_api_key_id: Some(id),
        trust_policy_id: None,
        oidc_claims: None,
    }
}

/// A personal token minted by the tokens API: it mirrors no `api_keys` row,
/// so it is the one shape this release may narrow.
fn own_row() -> api_tokens::Model {
    api_tokens::Model {
        source: source::UI.into(),
        legacy_api_key_id: None,
        ..row("personal")
    }
}

fn grant_row(token_id: Uuid, kind: &str, ceiling: Option<&str>) -> api_token_grants::Model {
    api_token_grants::Model {
        id: Uuid::new_v4(),
        token_id,
        kind: kind.to_string(),
        org_id: Uuid::from_u128(7),
        workspace_id: None,
        role_ceiling: ceiling.map(str::to_string),
        app_id: None,
        created_at: Utc::now().fixed_offset(),
        revoked_at: None,
        revoked_by: None,
    }
}

fn admit_now(
    row: &api_tokens::Model,
    presented: TokenFormat,
    link: LegacyLink,
) -> Result<CredentialContext, Refusal> {
    admit(row, &[], presented, Links::legacy(link), Utc::now())
}

fn admit_with(
    row: &api_tokens::Model,
    grants: &[api_token_grants::Model],
) -> Result<CredentialContext, Refusal> {
    let (presented, link) = match (row.kind.as_str(), row.legacy_api_key_id) {
        ("legacy_key", _) => (TokenFormat::LegacyKey, LegacyLink::Active),
        (_, Some(_)) => (TokenFormat::Personal, LegacyLink::Active),
        _ => (TokenFormat::Personal, LegacyLink::NotLinked),
    };
    admit(row, grants, presented, Links::legacy(link), Utc::now())
}

#[test]
fn a_live_all_access_personal_token_is_admitted() {
    let r = row("personal");
    let cred = admit_now(&r, TokenFormat::Personal, LegacyLink::Active).expect("admit");
    assert_eq!(cred.token_id, r.id);
    assert_eq!(cred.kind, StoredKind::Personal);
    assert!(cred.all_access && cred.platform && cred.partner);
    assert_eq!(cred.legacy_api_key_id, Some(r.id));
}

#[test]
fn a_live_legacy_key_is_admitted() {
    let r = row("legacy_key");
    assert!(admit_now(&r, TokenFormat::LegacyKey, LegacyLink::Active).is_ok());
}

#[test]
fn a_row_that_mirrors_api_keys_is_never_narrowed() {
    // A legacy key and a legacy-endpoint token are all-access with both
    // standings, always (§3.5). A pod one release back validates them from
    // `api_keys` with full reach, so a narrowed one is refused rather than
    // honoured here and widened there.
    for kind in ["personal", "legacy_key"] {
        for (flag, name) in [
            (0, "all_access=false"),
            (1, "platform=false"),
            (2, "partner=false"),
        ] {
            let mut r = row(kind);
            match flag {
                0 => r.all_access = false,
                1 => r.platform = false,
                _ => r.partner = false,
            }
            assert_eq!(admit_with(&r, &[]), Err(Refusal::Narrowed(name)), "{kind}");
        }
        let r = row(kind);
        let grants = [grant_row(r.id, "workspace", Some("owner"))];
        assert_eq!(
            admit_with(&r, &grants),
            Err(Refusal::Narrowed("grants")),
            "{kind}"
        );
    }
}

#[test]
fn a_personal_token_is_admitted_with_its_narrowing() {
    let mut r = own_row();
    r.all_access = false;
    r.platform = false;
    r.partner = false;
    let mut grant = grant_row(r.id, "workspace", Some("member"));
    grant.workspace_id = Some(Uuid::from_u128(9));
    let cred = admit_with(&r, &[grant]).expect("admit a narrowed token");
    assert!(!cred.all_access && !cred.platform && !cred.partner);
    assert_eq!(
        cred.grants,
        vec![TokenGrant {
            org_id: Uuid::from_u128(7),
            workspace_id: Some(Uuid::from_u128(9)),
            ceiling: RoleCeiling::Member,
        }]
    );
    let reach = cred.reach().expect("a personal token narrows");
    assert!(!reach.all_access && !reach.platform && !reach.partner);
    assert_eq!(reach.grants, cred.grants);
}

#[test]
fn a_personal_token_with_no_grants_is_admitted_and_reaches_nothing() {
    // Its last workspace was deleted, say. It still authenticates — so it can
    // be introspected and revoked — and covers nothing.
    let mut r = own_row();
    r.all_access = false;
    let cred = admit_with(&r, &[]).expect("admit");
    let reach = cred.reach().unwrap();
    assert!(!reach.touches_org(Uuid::from_u128(7)));
}

#[test]
fn a_grant_this_release_cannot_enforce_refuses_the_token() {
    let mut r = own_row();
    r.all_access = false;
    for (kind, ceiling, what) in [
        // An `app_publish` grant is enforced now; one naming no app is not.
        ("app_publish", None, "app_publish naming no app"),
        ("workspace", Some("root"), "ceiling 'root'"),
        ("workspace", None, "ceiling ''"),
        ("org", Some("owner"), "kind 'org'"),
    ] {
        let grants = [
            grant_row(r.id, "workspace", Some("owner")),
            grant_row(r.id, kind, ceiling),
        ];
        assert_eq!(
            admit_with(&r, &grants),
            Err(Refusal::UnknownGrant(what.to_string())),
        );
    }
}

#[test]
fn a_grant_its_org_revoked_no_longer_reaches() {
    let mut r = own_row();
    r.all_access = false;
    let mut revoked = grant_row(r.id, "workspace", Some("owner"));
    revoked.revoked_at = Some(Utc::now().fixed_offset());
    // Revoked first: even an unreadable revoked grant must not matter.
    let mut unreadable = grant_row(r.id, "app_publish", None);
    unreadable.revoked_at = revoked.revoked_at;
    let cred = admit_with(&r, &[revoked, unreadable]).expect("admit");
    assert!(cred.grants.is_empty());
}

#[test]
fn an_all_access_token_does_not_consult_its_grants() {
    let r = own_row();
    let grants = [grant_row(r.id, "app_publish", None)];
    let cred = admit_with(&r, &grants).expect("admit");
    assert!(cred.all_access && cred.grants.is_empty());
}

#[test]
fn a_legacy_credential_narrows_nothing() {
    // The marker a legacy key and a legacy-endpoint token carry: no reach to
    // narrow by, so every decision is the one a session would get.
    for kind in ["personal", "legacy_key"] {
        let cred = admit_with(&row(kind), &[]).expect("admit");
        assert!(cred.is_legacy(), "{kind}");
        assert_eq!(cred.reach(), None, "{kind}");
    }
    let own = admit_with(&own_row(), &[]).expect("admit");
    assert!(!own.is_legacy());
    assert_eq!(own.reach(), Some(TokenReach::unrestricted()));
}

#[test]
fn kinds_this_release_does_not_enforce_are_refused() {
    for kind in ["legacy_publish", "personal_v2", ""] {
        let r = row(kind);
        assert_eq!(
            admit_now(&r, TokenFormat::Personal, LegacyLink::NotLinked),
            Err(Refusal::UnknownKind(kind.to_string())),
            "{kind}"
        );
    }
}

#[test]
fn the_presented_format_must_match_the_stored_kind() {
    let personal = row("personal");
    assert_eq!(
        admit_now(&personal, TokenFormat::LegacyKey, LegacyLink::Active),
        Err(Refusal::KindMismatch)
    );
    let legacy = row("legacy_key");
    assert_eq!(
        admit_now(&legacy, TokenFormat::Personal, LegacyLink::Active),
        Err(Refusal::KindMismatch)
    );
    // An account's prefixes never open a person's row.
    for presented in [TokenFormat::ServiceAccount, TokenFormat::Ci] {
        assert_eq!(
            admit_now(&personal, presented, LegacyLink::Active),
            Err(Refusal::KindMismatch)
        );
    }
}

#[test]
fn a_revoked_row_is_refused() {
    let mut r = row("personal");
    r.revoked_at = Some(Utc::now().fixed_offset());
    assert_eq!(
        admit_now(&r, TokenFormat::Personal, LegacyLink::Active),
        Err(Refusal::Revoked)
    );
}

#[test]
fn an_expired_row_is_refused_and_a_future_one_admitted() {
    let mut r = row("legacy_key");
    r.expires_at = Some((Utc::now() - Duration::seconds(1)).fixed_offset());
    assert_eq!(
        admit_now(&r, TokenFormat::LegacyKey, LegacyLink::Active),
        Err(Refusal::Expired)
    );
    r.expires_at = Some((Utc::now() + Duration::days(1)).fixed_offset());
    assert!(admit_now(&r, TokenFormat::LegacyKey, LegacyLink::Active).is_ok());
}

#[test]
fn an_inactive_or_missing_api_keys_row_is_refused() {
    // A pod one release back revokes by writing api_keys alone; this is what
    // makes that revoke stick on this release.
    let r = row("legacy_key");
    for link in [LegacyLink::Inactive, LegacyLink::Missing] {
        assert_eq!(
            admit_now(&r, TokenFormat::LegacyKey, link),
            Err(Refusal::LegacyKeyInactive)
        );
    }
}

#[test]
fn stored_kind_round_trips() {
    for kind in [
        StoredKind::Personal,
        StoredKind::LegacyKey,
        StoredKind::ServiceAccount,
        StoredKind::Ci,
    ] {
        assert_eq!(StoredKind::parse(kind.as_str()), Some(kind));
    }
}

// ── Service accounts (design §3.3) ───────────────────────────────────────────

const ACCOUNT_ORG: Uuid = Uuid::from_u128(7);

/// An `oxy_sat_` row as the service-account API mints it.
fn account_row() -> api_tokens::Model {
    api_tokens::Model {
        all_access: false,
        platform: false,
        partner: false,
        display_prefix: "oxy_sat_Ab3x".into(),
        ..own_row()
    }
    .with_kind("service_account")
}

trait WithKind {
    fn with_kind(self, kind: &str) -> Self;
}

impl WithKind for api_tokens::Model {
    fn with_kind(mut self, kind: &str) -> Self {
        self.kind = kind.to_string();
        self
    }
}

fn active(admin: bool) -> AccountLink {
    AccountLink::Active(AccountStanding {
        org_id: ACCOUNT_ORG,
        admin,
    })
}

fn admit_account(
    row: &api_tokens::Model,
    grants: &[api_token_grants::Model],
    account: AccountLink,
) -> Result<CredentialContext, Refusal> {
    admit(
        row,
        grants,
        TokenFormat::ServiceAccount,
        Links::account(account),
        Utc::now(),
    )
}

fn account_model(org_role: &str, disabled: bool) -> service_accounts::Model {
    let now = Utc::now().fixed_offset();
    service_accounts::Model {
        user_id: Uuid::new_v4(),
        org_id: ACCOUNT_ORG,
        org_role: org_role.to_string(),
        name: "deploy-bot".into(),
        description: None,
        created_by: None,
        created_at: now,
        disabled_at: disabled.then_some(now),
    }
}

#[test]
fn a_service_account_token_is_admitted_grant_bound_with_no_standing() {
    let row = account_row();
    let grants = [grant_row(row.id, "workspace", Some("member"))];
    let cred = admit_account(&row, &grants, active(false)).expect("admit");
    assert_eq!(cred.kind, StoredKind::ServiceAccount);
    assert!(cred.is_service_account() && !cred.is_legacy());
    assert_eq!(
        cred.service_account,
        Some(AccountStanding {
            org_id: ACCOUNT_ORG,
            admin: false
        })
    );
    let reach = cred
        .reach()
        .expect("a service-account token always narrows");
    assert!(!reach.all_access && !reach.platform && !reach.partner);
    assert_eq!(reach.org_ceiling(ACCOUNT_ORG), Some(RoleCeiling::Member));
    assert_eq!(reach.org_ceiling(Uuid::from_u128(8)), None);
}

#[test]
fn a_service_account_row_claiming_more_than_an_account_holds_is_refused() {
    for (flag, widen) in [
        ("all_access", (true, false, false)),
        ("platform", (false, true, false)),
        ("partner", (false, false, true)),
    ] {
        let mut row = account_row();
        (row.all_access, row.platform, row.partner) = widen;
        assert_eq!(
            admit_account(&row, &[], active(true)),
            Err(Refusal::Widened(flag))
        );
    }
    // Nor may it mirror an `api_keys` row: that would read as a legacy key.
    let mirrored = api_tokens::Model {
        legacy_api_key_id: Some(Uuid::new_v4()),
        ..account_row()
    };
    assert!(matches!(
        admit_account(&mirrored, &[], active(true)),
        Err(Refusal::Widened(_))
    ));
    // And were one ever admitted, its reach would still claim nothing.
    let cred = CredentialContext {
        all_access: true,
        platform: true,
        partner: true,
        ..admit_account(&account_row(), &[], active(true)).unwrap()
    };
    let reach = cred.reach().unwrap();
    assert!(!reach.all_access && !reach.platform && !reach.partner);
}

#[test]
fn a_disabled_or_missing_account_refuses_its_tokens() {
    let row = account_row();
    for (link, refusal) in [
        (AccountLink::Disabled, Refusal::AccountDisabled),
        (AccountLink::Missing, Refusal::AccountMissing),
        (AccountLink::NotAccount, Refusal::AccountMissing),
    ] {
        assert_eq!(admit_account(&row, &[], link), Err(refusal));
    }
}

#[test]
fn a_service_account_grant_outside_its_org_is_refused() {
    let row = account_row();
    let mut stray = grant_row(row.id, "workspace", Some("member"));
    stray.org_id = Uuid::from_u128(8);
    assert!(matches!(
        admit_account(&row, &[stray], active(true)),
        Err(Refusal::UnknownGrant(_))
    ));
}

#[test]
fn a_service_account_token_and_a_personal_one_are_not_interchangeable() {
    // An `oxy_pat_` value cannot resolve a service-account row, nor the reverse.
    let account = account_row();
    assert_eq!(
        admit(
            &account,
            &[],
            TokenFormat::Personal,
            Links::account(active(true)),
            Utc::now()
        ),
        Err(Refusal::KindMismatch)
    );
    assert_eq!(
        admit(
            &own_row(),
            &[],
            TokenFormat::ServiceAccount,
            Links::NONE,
            Utc::now()
        ),
        Err(Refusal::KindMismatch)
    );
}

#[test]
fn the_account_link_reads_the_row_and_refuses_a_role_it_does_not_know() {
    assert_eq!(AccountLink::of(None), AccountLink::Missing);
    assert_eq!(
        AccountLink::of(Some(&account_model("member", false))),
        active(false)
    );
    assert_eq!(
        AccountLink::of(Some(&account_model("admin", false))),
        active(true)
    );
    assert_eq!(
        AccountLink::of(Some(&account_model("admin", true))),
        AccountLink::Disabled
    );
    // `owner` is not a standing an account can have: the row is unreadable.
    assert_eq!(
        AccountLink::of(Some(&account_model("owner", false))),
        AccountLink::Missing
    );
}

// ── An org's block (the inventory's revoke-grant) ────────────────────────────

fn revoked_org_wide(token_id: Uuid) -> api_token_grants::Model {
    api_token_grants::Model {
        revoked_at: Some(Utc::now().fixed_offset()),
        ..grant_row(token_id, "workspace", Some("owner"))
    }
}

#[test]
fn a_revoked_org_wide_grant_blocks_the_org_even_for_an_all_access_token() {
    let row = own_row();
    assert!(row.all_access);
    let cred = admit_with(&row, &[revoked_org_wide(row.id)]).expect("admit");
    assert_eq!(cred.blocked_orgs, vec![Uuid::from_u128(7)]);
    let reach = cred.reach().unwrap();
    assert_eq!(reach.org_ceiling(Uuid::from_u128(7)), None);
    // Everything else the token reaches stands.
    assert_eq!(
        reach.org_ceiling(Uuid::from_u128(8)),
        Some(RoleCeiling::Owner)
    );
}

#[test]
fn a_block_outlives_a_live_grant_in_the_same_org() {
    let row = api_tokens::Model {
        all_access: false,
        ..own_row()
    };
    let mut live = grant_row(row.id, "workspace", Some("admin"));
    live.workspace_id = Some(Uuid::from_u128(71));
    let cred = admit_with(&row, &[revoked_org_wide(row.id), live]).expect("admit");
    let reach = cred.reach().unwrap();
    assert_eq!(
        reach.workspace_ceiling(Uuid::from_u128(7), Uuid::from_u128(71)),
        None
    );
}

#[test]
fn a_revoked_workspace_grant_blocks_only_that_grant() {
    // Only an ORG-WIDE revoked row is a block; a revoked workspace grant is
    // just a grant that no longer reaches.
    let row = api_tokens::Model {
        all_access: false,
        ..own_row()
    };
    let mut revoked = revoked_org_wide(row.id);
    revoked.workspace_id = Some(Uuid::from_u128(71));
    let mut live = grant_row(row.id, "workspace", Some("member"));
    live.workspace_id = Some(Uuid::from_u128(72));
    assert!(blocked_orgs(std::slice::from_ref(&revoked)).is_empty());
    let cred = admit_with(&row, &[revoked, live]).expect("admit");
    let reach = cred.reach().unwrap();
    assert_eq!(
        reach.workspace_ceiling(Uuid::from_u128(7), Uuid::from_u128(72)),
        Some(RoleCeiling::Member)
    );
    assert_eq!(
        reach.workspace_ceiling(Uuid::from_u128(7), Uuid::from_u128(71)),
        None
    );
}

#[test]
fn a_legacy_key_is_never_blocked() {
    // Admission refuses a legacy row with ANY grant beside it rather than
    // honouring a block on it — and the store never loads grants for one, so
    // in practice a legacy key carries no block at all (§3.5).
    let legacy = row("legacy_key");
    let cred = admit_now(&legacy, TokenFormat::LegacyKey, LegacyLink::Active).unwrap();
    assert!(cred.blocked_orgs.is_empty());
    assert!(cred.reach().is_none(), "a legacy key narrows nothing");
}
