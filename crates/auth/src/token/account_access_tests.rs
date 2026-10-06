use chrono::{Duration, TimeZone};

use super::*;

const ORG: Uuid = Uuid::from_u128(0xA);
const OTHER_ORG: Uuid = Uuid::from_u128(0xB);
const WS: Uuid = Uuid::from_u128(0xA1);

fn grant(ceiling: Option<&str>, workspace_id: Option<Uuid>) -> GrantInput {
    GrantInput {
        role_ceiling: ceiling.map(str::to_string),
        workspace_id,
        ..GrantInput::default()
    }
}

#[test]
fn a_name_is_a_slug_of_two_to_forty_characters() {
    for ok in ["ci", "deploy-bot", "a1", "release-2-prod", &"a".repeat(40)] {
        assert_eq!(clean_account_name(ok).as_deref(), Ok(ok), "{ok}");
    }
    for bad in [
        "",
        "a",
        "1ci",
        "-ci",
        "ci-",
        "ci--bot",
        "Deploy",
        "deploy_bot",
        "deploy bot",
        " ci",
        "ci@acme.com",
        "çi",
        &"a".repeat(41),
    ] {
        assert!(clean_account_name(bad).is_err(), "{bad:?} must be refused");
    }
}

#[test]
fn a_name_can_never_look_like_an_address() {
    // The account's name is its display label: it must not contain `@`, so it
    // can never match an email-keyed check.
    assert!(clean_account_name("root@oxy.tech").is_err());
    assert!(!clean_account_name("deploy-bot").unwrap().contains('@'));
}

#[test]
fn an_account_is_a_member_or_an_admin_and_never_an_owner() {
    let body = |role: Option<&str>| CreateAccountBody {
        name: "deploy-bot".into(),
        description: Some("  ships the app  ".into()),
        org_role: role.map(str::to_string),
    };
    let want = body(None).want().unwrap();
    assert_eq!(want.role, AccountRole::Member, "member is the default");
    assert_eq!(want.description.as_deref(), Some("ships the app"));
    assert_eq!(body(Some("admin")).want().unwrap().role, AccountRole::Admin);

    let owner = body(Some("owner")).want().unwrap_err();
    assert!(owner.0.contains("cannot be an owner"), "{}", owner.0);
    assert!(body(Some("viewer")).want().is_err());
    assert_eq!(AccountRole::parse("owner"), None);
}

#[test]
fn an_edit_touches_only_what_the_body_names() {
    let nothing = PatchAccountBody::default().edit().unwrap();
    assert_eq!(nothing, AccountEdit::default());

    let body: PatchAccountBody =
        serde_json::from_str(r#"{"description":null,"org_role":"admin","disabled":true}"#).unwrap();
    assert_eq!(
        body.edit().unwrap(),
        AccountEdit {
            description: Some(None),
            role: Some(AccountRole::Admin),
            disabled: Some(true),
        }
    );

    let owner: PatchAccountBody = serde_json::from_str(r#"{"org_role":"owner"}"#).unwrap();
    assert!(owner.edit().is_err(), "an edit cannot make an owner either");
    let rename: PatchAccountBody = serde_json::from_str(r#"{"name":"other"}"#).unwrap();
    assert!(rename.edit().is_err(), "a rename is refused, not ignored");
    let long = PatchAccountBody {
        description: Some(Some("x".repeat(DESCRIPTION_MAX_CHARS + 1))),
        ..PatchAccountBody::default()
    };
    assert!(long.edit().is_err());
}

#[test]
fn no_grants_means_the_whole_org_at_the_accounts_role() {
    for (role, ceiling) in [
        (AccountRole::Member, RoleCeiling::Member),
        (AccountRole::Admin, RoleCeiling::Admin),
    ] {
        let want = vec![GrantSpec {
            org_id: ORG,
            workspace_id: None,
            ceiling,
        }];
        assert_eq!(account_grants(ORG, role, &[]).unwrap(), want);
    }
}

#[test]
fn a_ceiling_above_the_accounts_role_is_refused() {
    let admin = [grant(Some("admin"), None)];
    assert!(account_grants(ORG, AccountRole::Member, &admin).is_err());
    assert!(account_grants(ORG, AccountRole::Admin, &admin).is_ok());
    // `owner` is never valid, whatever the account is.
    for role in [AccountRole::Member, AccountRole::Admin] {
        let err = account_grants(ORG, role, &[grant(Some("owner"), None)]).unwrap_err();
        assert!(err.0.contains("owner"), "{}", err.0);
    }
    assert!(account_grants(ORG, AccountRole::Admin, &[grant(Some("root"), None)]).is_err());
    // An omitted ceiling is the account's role, not `owner`.
    let got = account_grants(ORG, AccountRole::Member, &[grant(None, Some(WS))]).unwrap();
    assert_eq!(got[0].ceiling, RoleCeiling::Member);
    // Below the role is a narrowing, and fine.
    let got = account_grants(ORG, AccountRole::Admin, &[grant(Some("viewer"), Some(WS))]).unwrap();
    assert_eq!(
        (got[0].workspace_id, got[0].ceiling),
        (Some(WS), RoleCeiling::Viewer)
    );
}

#[test]
fn a_grant_is_in_the_accounts_own_org() {
    let own = GrantInput {
        org_id: Some(ORG),
        ..GrantInput::default()
    };
    assert_eq!(
        account_grants(ORG, AccountRole::Member, &[own]).unwrap()[0].org_id,
        ORG
    );
    let other = GrantInput {
        org_id: Some(OTHER_ORG),
        ..GrantInput::default()
    };
    assert!(account_grants(ORG, AccountRole::Admin, &[other]).is_err());
}

#[test]
fn an_app_publish_grant_is_not_minted_here_and_duplicates_collapse() {
    let publish = GrantInput {
        kind: Some("app_publish".into()),
        app_id: Some(Uuid::from_u128(7)),
        ..GrantInput::default()
    };
    assert!(account_grants(ORG, AccountRole::Admin, &[publish]).is_err());
    let unknown = GrantInput {
        kind: Some("everything".into()),
        ..GrantInput::default()
    };
    assert!(account_grants(ORG, AccountRole::Admin, &[unknown]).is_err());

    let twice = [
        grant(Some("viewer"), Some(WS)),
        grant(Some("admin"), Some(WS)),
    ];
    let got = account_grants(ORG, AccountRole::Admin, &twice).unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].ceiling, RoleCeiling::Admin);
}

#[test]
fn a_token_body_reads_its_name_expiry_and_grants() {
    let now = Utc.with_ymd_and_hms(2026, 10, 2, 0, 0, 0).unwrap();
    let body: CreateAccountTokenBody =
        serde_json::from_str(r#"{"name":" deploy ","expires_in_days":30}"#).unwrap();
    assert_eq!(body.name().unwrap(), "deploy");
    assert_eq!(
        body.expires_at(now).unwrap(),
        Some(now + Duration::days(30))
    );
    assert_eq!(body.grants(ORG, AccountRole::Admin).unwrap().len(), 1);

    let never: CreateAccountTokenBody =
        serde_json::from_str(r#"{"name":"n","expires_at":null,"grants":[]}"#).unwrap();
    assert_eq!(never.expires_at(now).unwrap(), None);
    // An empty list is the same as none: one org-wide grant.
    assert_eq!(
        never.grants(ORG, AccountRole::Member).unwrap()[0].workspace_id,
        None
    );
    let unnamed: CreateAccountTokenBody = serde_json::from_str(r#"{"name":"  "}"#).unwrap();
    assert!(unnamed.name().is_err());
}
