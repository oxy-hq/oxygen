use super::*;

const ORG_A: Uuid = Uuid::from_u128(0xA);
const ORG_B: Uuid = Uuid::from_u128(0xB);

fn at(days: i64) -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap() + Duration::days(days)
}

fn shape(kind: StoredKind, all_access: bool, lifetime_days: Option<i64>) -> TokenShape {
    TokenShape {
        kind,
        legacy: kind == StoredKind::LegacyKey,
        all_access,
        created_at: at(0),
        expires_at: lifetime_days.map(at),
    }
}

fn capped(days: i32) -> OrgPolicy {
    OrgPolicy {
        max_lifetime_days: Some(days),
        ..OrgPolicy::default()
    }
}

fn no_all_access() -> OrgPolicy {
    OrgPolicy {
        allow_all_access_tokens: false,
        ..OrgPolicy::default()
    }
}

#[test]
fn the_defaults_block_nothing() {
    let policy = OrgPolicy::default();
    assert_eq!(policy.max_lifetime_days, None);
    assert!(policy.allow_all_access_tokens && policy.require_environment_on_trust_policies);
    assert!(!policy.restricts());
    for token in [
        shape(StoredKind::Personal, true, None),
        shape(StoredKind::ServiceAccount, false, Some(10_000)),
    ] {
        assert_eq!(violation(&token, &policy), None);
    }
    assert_eq!(OrgPolicy::of(None), policy);
}

#[test]
fn a_cap_is_one_to_ten_years_or_none() {
    for ok in [None, Some(1), Some(90), Some(MAX_LIFETIME_DAYS_LIMIT)] {
        let policy = OrgPolicy {
            max_lifetime_days: ok,
            ..OrgPolicy::default()
        };
        assert!(policy.validate().is_ok(), "{ok:?}");
    }
    for bad in [Some(0), Some(-5), Some(MAX_LIFETIME_DAYS_LIMIT + 1)] {
        assert!(capped(bad.unwrap()).validate().is_err(), "{bad:?}");
    }
}

#[test]
fn a_token_outliving_the_cap_or_never_expiring_violates_it() {
    let policy = capped(90);
    let within = shape(StoredKind::Personal, false, Some(90));
    let over = shape(StoredKind::Personal, false, Some(91));
    let forever = shape(StoredKind::ServiceAccount, false, None);
    assert_eq!(
        violation(&within, &policy),
        None,
        "exactly the cap is within it"
    );
    assert_eq!(violation(&over, &policy), Some(Violation::MaxLifetime));
    assert_eq!(violation(&forever, &policy), Some(Violation::MaxLifetime));
}

#[test]
fn lifetime_is_measured_from_creation_not_from_now() {
    // Created long ago, expiring soon: its lifetime is what is capped.
    let token = TokenShape {
        created_at: at(-200),
        ..shape(StoredKind::Personal, false, Some(5))
    };
    assert_eq!(violation(&token, &capped(90)), Some(Violation::MaxLifetime));
}

#[test]
fn the_all_access_rule_is_about_personal_all_access_tokens_only() {
    let policy = no_all_access();
    assert_eq!(
        violation(&shape(StoredKind::Personal, true, Some(30)), &policy),
        Some(Violation::AllAccessDisallowed)
    );
    assert_eq!(
        violation(&shape(StoredKind::Personal, false, Some(30)), &policy),
        None,
        "a grant-bound token still works"
    );
    assert_eq!(
        violation(&shape(StoredKind::ServiceAccount, false, None), &policy),
        None
    );
    // Both broken: the all-access rule is named, since a shorter life would
    // not lift it.
    let both = OrgPolicy {
        max_lifetime_days: Some(1),
        allow_all_access_tokens: false,
        ..OrgPolicy::default()
    };
    assert_eq!(
        violation(&shape(StoredKind::Personal, true, None), &both),
        Some(Violation::AllAccessDisallowed)
    );
}

#[test]
fn a_legacy_key_is_never_judged() {
    let strictest = OrgPolicy {
        max_lifetime_days: Some(1),
        allow_all_access_tokens: false,
        require_environment_on_trust_policies: true,
    };
    let legacy_key = shape(StoredKind::LegacyKey, true, None);
    assert_eq!(violation(&legacy_key, &strictest), None);
    // A token the legacy endpoint minted is stored `personal` but mirrors
    // `api_keys`: just as exempt.
    let legacy_endpoint = TokenShape {
        legacy: true,
        ..shape(StoredKind::Personal, true, None)
    };
    assert_eq!(violation(&legacy_endpoint, &strictest), None);
    assert!(!legacy_endpoint.all_access_personal());
    assert_eq!(check_expiry(&legacy_key, None, Some(1)), Ok(()));
    let policies = HashMap::from([(ORG_A, strictest)]);
    assert!(blocked_in(&legacy_key, &[ORG_A], &policies).is_empty());
}

#[test]
fn blocked_in_names_only_the_orgs_whose_policy_is_broken() {
    let token = shape(StoredKind::Personal, false, Some(120));
    let policies = HashMap::from([(ORG_A, capped(90)), (ORG_B, capped(365))]);
    assert_eq!(
        blocked_in(&token, &[ORG_A, ORG_B, Uuid::from_u128(0xC)], &policies),
        vec![(ORG_A, Violation::MaxLifetime)]
    );
}

#[test]
fn the_tightest_cap_wins() {
    let policies = [OrgPolicy::default(), capped(365), capped(30)];
    assert_eq!(tightest_cap(&policies), Some(30));
    assert_eq!(tightest_cap(&[OrgPolicy::default()]), None);
}

#[test]
fn an_expiry_past_the_cap_is_refused_for_a_grant_bound_token_only() {
    let bound = shape(StoredKind::Personal, false, Some(10));
    assert_eq!(check_expiry(&bound, Some(at(30)), Some(30)), Ok(()));
    assert_eq!(check_expiry(&bound, Some(at(31)), Some(30)), Err(30));
    assert_eq!(check_expiry(&bound, None, Some(30)), Err(30), "no expiry");
    assert_eq!(check_expiry(&bound, None, None), Ok(()), "no cap");
    let account = shape(StoredKind::ServiceAccount, false, Some(10));
    assert_eq!(check_expiry(&account, Some(at(400)), Some(365)), Err(365));
    // An all-access token is not refused: it goes inert where it is capped.
    let all_access = shape(StoredKind::Personal, true, Some(10));
    assert_eq!(check_expiry(&all_access, None, Some(30)), Ok(()));
}

#[test]
fn the_reasons_are_the_wire_strings() {
    assert_eq!(Violation::MaxLifetime.as_str(), "max_lifetime");
    assert_eq!(
        Violation::AllAccessDisallowed.as_str(),
        "all_access_disallowed"
    );
}
