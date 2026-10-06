use super::*;

fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

const ORG: u128 = 10;
const WS: u128 = 20;
const OTHER_WS: u128 = 21;
const OWNER: u128 = 2;

fn token(kind: &str, all_access: bool) -> api_tokens::Model {
    api_tokens::Model {
        id: id(1),
        kind: kind.to_string(),
        principal_user_id: id(OWNER),
        name: "deploy".into(),
        display_prefix: "oxy_pat_Ab3x".into(),
        last_four: "9f2c".into(),
        token_hash: vec![7; 32],
        all_access,
        platform: false,
        partner: false,
        expires_at: None,
        last_used_at: None,
        created_at: Utc::now().fixed_offset(),
        created_by: Some(id(OWNER)),
        revoked_at: None,
        revoked_by: None,
        revoke_reason: None,
        source: "ui".into(),
        legacy_api_key_id: None,
        trust_policy_id: None,
        oidc_claims: None,
    }
}

fn grant(workspace: Option<u128>, ceiling: &str) -> api_token_grants::Model {
    api_token_grants::Model {
        id: Uuid::new_v4(),
        token_id: id(1),
        kind: api_token_grants::KIND_WORKSPACE.into(),
        org_id: id(ORG),
        workspace_id: workspace.map(id),
        role_ceiling: Some(ceiling.into()),
        app_id: None,
        created_at: Utc::now().fixed_offset(),
        revoked_at: None,
        revoked_by: None,
    }
}

fn here(row: &api_tokens::Model, grants: &[api_token_grants::Model]) -> Option<RoleCeiling> {
    ceiling_here(row, grants, false, id(ORG), id(WS))
}

/// As [`here`], for a token the org's policy makes inert.
fn here_under_a_policy_block(
    row: &api_tokens::Model,
    grants: &[api_token_grants::Model],
) -> Option<RoleCeiling> {
    ceiling_here(row, grants, true, id(ORG), id(WS))
}

#[test]
fn a_token_the_orgs_policy_blocks_does_not_reach_the_workspace() {
    // It used to be listed as active, with its owner's full reach.
    let all_access = token("personal", true);
    assert_eq!(here(&all_access, &[]), Some(RoleCeiling::Owner));
    assert_eq!(here_under_a_policy_block(&all_access, &[]), None);

    // A live grant beside the block reaches nothing either — on the
    // workspace, or on the whole org.
    let narrowed = token("personal", false);
    for grants in [vec![grant(Some(WS), "admin")], vec![grant(None, "owner")]] {
        assert!(here(&narrowed, &grants).is_some());
        assert_eq!(here_under_a_policy_block(&narrowed, &grants), None);
    }
    // The org's own tokens are judged by its policy too (the lifetime cap).
    let account = token("service_account", false);
    assert_eq!(
        here_under_a_policy_block(&account, &[grant(None, "member")]),
        None
    );
}

#[test]
fn an_orgs_revoke_grant_leaves_a_token_out_as_a_policy_block_does() {
    // The other way an org blocks a token: a revoked org-wide grant row.
    let revoked = api_token_grants::Model {
        revoked_at: Some(Utc::now().fixed_offset()),
        ..grant(None, "owner")
    };
    assert_eq!(
        here(&token("personal", true), std::slice::from_ref(&revoked)),
        None
    );
    assert_eq!(
        here_under_a_policy_block(&token("personal", true), &[revoked]),
        None,
        "both at once is still one blocked org"
    );
}

#[test]
fn an_all_access_token_reaches_the_workspace_uncapped() {
    // A legacy API key never gets this far: the inventory lists tokens only.
    assert_eq!(
        here(&token("personal", true), &[]),
        Some(RoleCeiling::Owner)
    );
}

#[test]
fn a_narrowed_token_reaches_it_through_a_grant_at_the_grants_ceiling() {
    let row = token("personal", false);
    assert_eq!(
        here(&row, &[grant(Some(WS), "viewer")]),
        Some(RoleCeiling::Viewer)
    );
    // An org-wide grant covers every workspace in the org.
    assert_eq!(
        here(&row, &[grant(None, "admin")]),
        Some(RoleCeiling::Admin)
    );
    // The highest covering grant is the ceiling here.
    assert_eq!(
        here(&row, &[grant(Some(WS), "viewer"), grant(None, "member")]),
        Some(RoleCeiling::Member)
    );
}

#[test]
fn a_grant_elsewhere_does_not_reach_it() {
    let row = token("personal", false);
    assert_eq!(here(&row, &[grant(Some(OTHER_WS), "owner")]), None);
    assert_eq!(here(&row, &[]), None);
    // Another token's grant is not this token's.
    let someone_elses = api_token_grants::Model {
        token_id: id(99),
        ..grant(Some(WS), "owner")
    };
    assert_eq!(here(&row, &[someone_elses]), None);
}

#[test]
fn a_token_the_validator_would_refuse_is_not_listed() {
    let row = token("personal", false);
    let unreadable = api_token_grants::Model {
        role_ceiling: Some("root".into()),
        ..grant(Some(WS), "owner")
    };
    assert_eq!(here(&row, &[grant(Some(WS), "admin"), unreadable]), None);
}

#[test]
fn a_token_is_listed_only_while_its_owner_can_act_in_the_org() {
    let members = HashSet::from([id(OWNER)]);
    let nobody = HashSet::new();
    let all_access = token("personal", true);
    assert!(owner_reaches(&all_access, &members));
    // The owner left the org: their all-access token no longer reaches it.
    assert!(!owner_reaches(&all_access, &nobody));

    // A narrowed token of a non-member reaches it only through a standing.
    let mut narrowed = token("personal", false);
    assert!(!owner_reaches(&narrowed, &nobody));
    narrowed.partner = true;
    assert!(owner_reaches(&narrowed, &nobody));
}

#[test]
fn a_row_carries_the_fields_the_contract_picks() {
    let row = token("personal", false);
    let out = inventory_token(
        &row,
        RoleCeiling::Admin,
        "ada@example.com".into(),
        false,
        Utc::now(),
    );
    let wire = serde_json::to_value(&out).unwrap();
    let mut keys: Vec<&str> = wire
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "all_access",
            "display_prefix",
            "expires_at",
            "id",
            "kind",
            "last_used_at",
            "name",
            "owner",
            "role_ceiling_here",
            "status",
        ]
    );
    assert_eq!(wire["role_ceiling_here"], "admin");
    assert_eq!(wire["owner"]["label"], "ada@example.com");
    assert_eq!(wire["status"], "active");
}
