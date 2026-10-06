use super::*;
use serde_json::{Value, json};

fn create(body: Value) -> CreateBody {
    serde_json::from_value(body).expect("a well-formed create body")
}

fn patch(body: Value) -> PatchBody {
    serde_json::from_value(body).expect("a well-formed patch body")
}

fn org(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

fn stored(all_access: bool) -> Settings {
    Settings {
        name: "ci".into(),
        all_access,
        platform: false,
        partner: false,
    }
}

fn workspace(org_id: Uuid, workspace_id: Option<Uuid>, ceiling: RoleCeiling) -> GrantWant {
    GrantWant::Workspace {
        org_id,
        workspace_id,
        ceiling,
    }
}

#[test]
fn a_new_token_is_all_access_with_no_standing_unless_it_says_otherwise() {
    let access = create(json!({ "name": "laptop" })).access().unwrap();
    assert_eq!(
        access,
        Access {
            all_access: true,
            platform: false,
            partner: false,
            grants: vec![],
        }
    );
    // What the dialog sends for the default: every field, explicitly.
    let explicit = create(json!({
        "name": "laptop", "all_access": true, "platform": false, "partner": false, "grants": []
    }));
    assert_eq!(explicit.access().unwrap(), access);
}

#[test]
fn the_standing_flags_are_carried_as_asked() {
    let access = create(json!({ "name": "ops", "platform": true, "partner": true }))
        .access()
        .unwrap();
    assert!(access.platform && access.partner && access.all_access);
}

#[test]
fn a_narrowed_token_names_its_grants() {
    let access = create(json!({
        "name": "ci",
        "all_access": false,
        "grants": [
            { "org_id": org(1), "workspace_id": org(10), "role_ceiling": "viewer" },
            { "kind": "workspace", "org_id": org(2), "workspace_id": null }
        ]
    }))
    .access()
    .unwrap();
    assert!(!access.all_access);
    assert_eq!(
        access.grants,
        vec![
            workspace(org(1), Some(org(10)), RoleCeiling::Viewer),
            // No ceiling named is no cap; no workspace named is the whole org.
            workspace(org(2), None, RoleCeiling::Owner),
        ]
    );
}

#[test]
fn grants_beside_all_access_are_refused_not_dropped() {
    // The default is all-access, so a caller who only sent `grants` would
    // otherwise be handed a token wider than the one they described.
    for body in [
        json!({ "name": "ci", "grants": [{ "org_id": org(1) }] }),
        json!({ "name": "ci", "all_access": true, "grants": [{ "org_id": org(1) }] }),
    ] {
        assert!(create(body.clone()).access().is_err(), "{body}");
    }
}

#[test]
fn a_narrowed_token_with_no_grants_is_refused() {
    for body in [
        json!({ "name": "ci", "all_access": false }),
        json!({ "name": "ci", "all_access": false, "grants": [] }),
    ] {
        assert!(create(body.clone()).access().is_err(), "{body}");
    }
}

#[test]
fn a_grant_that_cannot_be_read_is_refused() {
    for grant in [
        json!({}),
        json!({ "workspace_id": org(10) }),
        json!({ "org_id": org(1), "role_ceiling": "root" }),
        json!({ "kind": "everything", "org_id": org(1) }),
        json!({ "kind": "app_publish", "org_id": org(1) }),
    ] {
        let body = json!({ "name": "ci", "all_access": false, "grants": [grant] });
        assert!(create(body.clone()).access().is_err(), "{body}");
    }
    // A malformed id never gets as far as a grant.
    let bad_id = json!({ "name": "ci", "all_access": false, "grants": [{ "org_id": "acme" }] });
    assert!(serde_json::from_value::<CreateBody>(bad_id).is_err());
}

#[test]
fn naming_a_target_twice_keeps_the_higher_ceiling() {
    let grants = parse_grants(&[
        GrantInput {
            org_id: Some(org(1)),
            role_ceiling: Some("viewer".into()),
            ..Default::default()
        },
        GrantInput {
            org_id: Some(org(1)),
            role_ceiling: Some("admin".into()),
            ..Default::default()
        },
        GrantInput {
            org_id: Some(org(1)),
            workspace_id: Some(org(10)),
            role_ceiling: Some("member".into()),
            ..Default::default()
        },
        GrantInput {
            kind: Some("app_publish".into()),
            app_id: Some(org(50)),
            ..Default::default()
        },
        GrantInput {
            kind: Some("app_publish".into()),
            app_id: Some(org(50)),
            ..Default::default()
        },
    ])
    .unwrap();
    assert_eq!(
        grants,
        vec![
            workspace(org(1), None, RoleCeiling::Admin),
            // The org-wide grant and the workspace's own are different targets.
            workspace(org(1), Some(org(10)), RoleCeiling::Member),
            GrantWant::AppPublish { app_id: org(50) },
        ]
    );
}

#[test]
fn expiry_is_days_a_date_never_or_ninety_days() {
    let now = Utc::now();
    let days = |n: i64| Some(now + Duration::days(n));
    assert_eq!(expiry(None, None, now), Ok(days(DEFAULT_LIFETIME_DAYS)));
    assert_eq!(expiry(Some(30), None, now), Ok(days(30)));
    assert_eq!(expiry(None, Some(None), now), Ok(None));
    let at = "2099-01-31T00:00:00Z";
    assert_eq!(
        expiry(None, Some(Some(at)), now),
        Ok(Some(at.parse::<DateTime<Utc>>().unwrap()))
    );
}

#[test]
fn a_bad_expiry_is_refused() {
    let now = Utc::now();
    let past = (now - Duration::days(1)).to_rfc3339();
    for (days, at) in [
        (Some(0), None),
        (Some(-1), None),
        (Some(MAX_LIFETIME_DAYS + 1), None),
        (Some(30), Some(None)),
        (Some(30), Some(Some("2099-01-31T00:00:00Z"))),
        (None, Some(Some("tomorrow"))),
        (None, Some(Some(past.as_str()))),
    ] {
        assert!(expiry(days, at, now).is_err(), "{days:?} {at:?}");
    }
}

#[test]
fn the_body_tells_no_expiry_from_an_omitted_one() {
    let now = Utc::now();
    let never = create(json!({ "name": "n", "expires_at": null }));
    assert_eq!(never.expires_at(now), Ok(None));
    let omitted = create(json!({ "name": "n" }));
    assert_eq!(
        omitted.expires_at(now),
        Ok(Some(now + Duration::days(DEFAULT_LIFETIME_DAYS)))
    );
    let in_days = create(json!({ "name": "n", "expires_in_days": 7 }));
    assert_eq!(in_days.expires_at(now), Ok(Some(now + Duration::days(7))));
}

#[test]
fn a_name_is_trimmed_and_one_to_a_hundred_characters() {
    assert_eq!(clean_name("  laptop "), Ok("laptop".to_string()));
    assert_eq!(
        clean_name(&"é".repeat(100)).map(|n| n.chars().count()),
        Ok(100)
    );
    assert!(clean_name("").is_err());
    assert!(clean_name("   ").is_err());
    assert!(clean_name(&"x".repeat(101)).is_err());
}

#[test]
fn an_edit_that_names_nothing_changes_nothing() {
    for current in [stored(true), stored(false)] {
        let edit = patch(json!({})).edit(&current).unwrap();
        assert_eq!(edit.settings, current);
        assert!(!edit.asks_platform && !edit.asks_partner);
        // An all-access token has no live grants to keep, so clearing is a no-op.
        let expected = if current.all_access {
            GrantsEdit::Clear
        } else {
            GrantsEdit::Keep
        };
        assert_eq!(edit.grants, expected);
    }
}

#[test]
fn a_rename_keeps_the_access() {
    let edit = patch(json!({ "name": " deploy " }))
        .edit(&stored(false))
        .unwrap();
    assert_eq!(edit.settings.name, "deploy");
    assert_eq!(edit.grants, GrantsEdit::Keep);
    assert!(patch(json!({ "name": "" })).edit(&stored(false)).is_err());
}

#[test]
fn all_access_with_empty_grants_clears_them() {
    // What the dialog sends when a narrowed token is switched back.
    let edit = patch(json!({
        "all_access": true, "platform": false, "partner": false, "grants": []
    }))
    .edit(&stored(false))
    .unwrap();
    assert!(edit.settings.all_access);
    assert_eq!(edit.grants, GrantsEdit::Clear);
}

#[test]
fn grants_replace_the_set() {
    let body = patch(json!({
        "all_access": false, "platform": false, "partner": false,
        "grants": [{ "org_id": org(1), "role_ceiling": "member" }]
    }));
    let replace = GrantsEdit::Replace(vec![workspace(org(1), None, RoleCeiling::Member)]);
    // Narrowing an all-access token, and re-scoping a narrowed one.
    for current in [stored(true), stored(false)] {
        let edit = body.edit(&current).unwrap();
        assert!(!edit.settings.all_access);
        assert_eq!(edit.grants, replace);
    }
}

#[test]
fn an_edit_cannot_leave_a_narrowed_token_without_grants() {
    // Emptying the set, and narrowing without saying to what.
    assert!(patch(json!({ "grants": [] })).edit(&stored(false)).is_err());
    assert!(
        patch(json!({ "all_access": false }))
            .edit(&stored(true))
            .is_err()
    );
    // Grants beside all-access, stored or asked for.
    let grants = json!([{ "org_id": org(1) }]);
    assert!(
        patch(json!({ "grants": grants }))
            .edit(&stored(true))
            .is_err()
    );
    assert!(
        patch(json!({ "all_access": true, "grants": grants }))
            .edit(&stored(false))
            .is_err()
    );
}

#[test]
fn asking_for_a_standing_is_told_apart_from_keeping_one() {
    let carrying = Settings {
        platform: true,
        partner: true,
        ..stored(true)
    };
    // Untouched flags stay as stored and ask for nothing.
    let kept = patch(json!({ "name": "x" })).edit(&carrying).unwrap();
    assert!(kept.settings.platform && kept.settings.partner);
    assert!(!kept.asks_platform && !kept.asks_partner);
    // Sent as on, they are asked for — even when already stored.
    let asked = patch(json!({ "platform": true, "partner": true }))
        .edit(&carrying)
        .unwrap();
    assert!(asked.asks_platform && asked.asks_partner);
    // Sent as off, they are dropped without needing any standing.
    let dropped = patch(json!({ "platform": false, "partner": false }))
        .edit(&carrying)
        .unwrap();
    assert!(!dropped.settings.platform && !dropped.settings.partner);
    assert!(!dropped.asks_platform && !dropped.asks_partner);
}
