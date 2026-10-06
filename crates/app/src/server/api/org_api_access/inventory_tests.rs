use chrono::Utc;

use super::*;

const ORG: Uuid = Uuid::from_u128(0xA);
const MEMBER: Uuid = Uuid::from_u128(1);
const OUTSIDER: Uuid = Uuid::from_u128(2);
const ACCOUNT: Uuid = Uuid::from_u128(3);
const WS_A: Uuid = Uuid::from_u128(0xA1);
const WS_B: Uuid = Uuid::from_u128(0xA2);

fn token(kind: &str, principal: Uuid, all_access: bool) -> api_tokens::Model {
    api_tokens::Model {
        id: Uuid::new_v4(),
        kind: kind.to_string(),
        principal_user_id: principal,
        name: "t".into(),
        display_prefix: "oxy_pat_Ab3x".into(),
        last_four: "wxyz".into(),
        token_hash: vec![0; 32],
        all_access,
        platform: false,
        partner: false,
        expires_at: None,
        last_used_at: None,
        created_at: Utc::now().fixed_offset(),
        created_by: None,
        revoked_at: None,
        revoked_by: None,
        revoke_reason: None,
        source: "ui".into(),
        legacy_api_key_id: None,
        trust_policy_id: None,
        oidc_claims: None,
    }
}

fn grant(
    token: &api_tokens::Model,
    workspace_id: Option<Uuid>,
    revoked: bool,
) -> api_token_grants::Model {
    let now = Utc::now().fixed_offset();
    api_token_grants::Model {
        id: Uuid::new_v4(),
        token_id: token.id,
        kind: api_token_grants::KIND_WORKSPACE.to_string(),
        org_id: ORG,
        workspace_id,
        role_ceiling: Some("member".into()),
        app_id: None,
        created_at: now,
        revoked_at: revoked.then_some(now),
        revoked_by: None,
    }
}

fn org(grants: Vec<api_token_grants::Model>) -> OrgSide {
    let now = Utc::now().fixed_offset();
    let account = service_accounts::Model {
        user_id: ACCOUNT,
        org_id: ORG,
        org_role: "member".into(),
        name: "deploy-bot".into(),
        description: None,
        created_by: None,
        created_at: now,
        disabled_at: None,
    };
    OrgSide {
        org_id: ORG,
        members: HashSet::from([MEMBER]),
        accounts: HashMap::from([(ACCOUNT, account)]),
        grants,
    }
}

#[test]
fn an_all_access_token_or_a_legacy_key_is_listed_for_a_member_only() {
    let side = org(Vec::new());
    assert!(side.lists(&token("personal", MEMBER, true)));
    assert!(side.lists(&token("legacy_key", MEMBER, true)));
    assert!(!side.lists(&token("personal", OUTSIDER, true)));
    assert!(!side.lists(&token("legacy_key", OUTSIDER, true)));
    // A legacy-endpoint token is a legacy key whatever it is stored as.
    let minted_by_legacy_endpoint = api_tokens::Model {
        legacy_api_key_id: Some(Uuid::new_v4()),
        ..token("personal", MEMBER, true)
    };
    assert!(side.lists(&minted_by_legacy_endpoint));
}

#[test]
fn a_service_account_token_is_listed_for_its_own_org() {
    let side = org(Vec::new());
    assert!(side.lists(&token("service_account", ACCOUNT, false)));
    // Another org's account is not ours, whatever grant row might name us.
    let foreign = token("service_account", OUTSIDER, false);
    let side = org(vec![grant(&foreign, None, false)]);
    assert!(!side.lists(&foreign));
}

#[test]
fn a_narrowed_token_is_listed_by_its_grant_here() {
    let held = token("personal", MEMBER, false);
    assert!(!org(Vec::new()).lists(&held), "no grant here: not ours");
    assert!(org(vec![grant(&held, Some(WS_A), false)]).lists(&held));

    // Its owner left the org: the grant reaches nothing, so it is not listed —
    // unless the token carries a standing that reaches without membership.
    let left = token("personal", OUTSIDER, false);
    assert!(!org(vec![grant(&left, None, false)]).lists(&left));
    let staff = api_tokens::Model {
        platform: true,
        ..token("personal", OUTSIDER, false)
    };
    assert!(org(vec![grant(&staff, None, false)]).lists(&staff));

    // Reach the org already ended stays listed, as history.
    assert!(org(vec![grant(&left, None, true)]).lists(&left));
}

#[test]
fn the_workspace_filter_asks_the_reach_model() {
    let legacy = token("legacy_key", MEMBER, true);
    let all = token("personal", MEMBER, true);
    let one = token("personal", MEMBER, false);
    let wide = token("personal", MEMBER, false);
    let blocked = token("personal", MEMBER, true);
    let side = org(vec![
        grant(&one, Some(WS_A), false),
        grant(&wide, None, false),
        grant(&blocked, None, true),
    ]);
    for (row, a, b) in [
        (&legacy, true, true),
        (&all, true, true),
        (&one, true, false),
        (&wide, true, true),
        // The org ended an all-access token's reach: it reaches no workspace.
        (&blocked, false, false),
    ] {
        assert_eq!(side.reaches_workspace(row, WS_A), a, "{}", row.kind);
        assert_eq!(side.reaches_workspace(row, WS_B), b, "{}", row.kind);
    }
    // A service-account token is never all-access, whatever its row says.
    let account = api_tokens::Model {
        all_access: true,
        ..token("service_account", ACCOUNT, false)
    };
    assert!(!org(Vec::new()).reaches_workspace(&account, WS_A));
}

#[test]
fn filters_are_read_leniently_and_a_bad_id_matches_nothing() {
    let none = InventoryQuery::default().filters().unwrap();
    assert_eq!(none, Filters::default());
    let blank = InventoryQuery {
        kind: Some("  ".into()),
        owner: Some(String::new()),
        workspace_id: None,
    };
    assert_eq!(blank.filters().unwrap(), Filters::default());
    let set = InventoryQuery {
        kind: Some("legacy_key".into()),
        owner: Some(MEMBER.to_string()),
        workspace_id: Some(WS_A.to_string()),
    };
    assert_eq!(
        set.filters().unwrap(),
        Filters {
            kind: Some("legacy_key".into()),
            owner: Some(MEMBER),
            workspace_id: Some(WS_A),
        }
    );
    let bad = InventoryQuery {
        owner: Some("ada@acme.com".into()),
        ..InventoryQuery::default()
    };
    assert!(bad.filters().is_err());
}
