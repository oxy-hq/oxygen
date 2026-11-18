use serde_json::json;
use uuid::Uuid;

use super::*;

fn mint(extra: Value) -> Value {
    let mut mint = json!({ "kind": KIND, "org": "acme", "account": "store-ops" });
    if let (Value::Object(mint), Value::Object(extra)) = (&mut mint, extra) {
        mint.extend(extra);
    }
    mint
}

fn refused(body: Value) -> String {
    parse(&body, Some("minted by oxyc on host"))
        .expect_err("refused")
        .0
}

#[test]
fn a_bare_mint_is_a_member_account_token_of_the_default_name_and_lifetime() {
    let parsed = parse(&mint(json!({})), Some("minted by oxyc on host")).unwrap();
    assert_eq!(
        parsed,
        MintRequest {
            org: "acme".into(),
            account: "store-ops".into(),
            role: AccountRole::Member,
            name: "minted by oxyc on host".into(),
            days: DEFAULT_LIFETIME_DAYS,
        }
    );
}

#[test]
fn what_is_asked_for_is_kept() {
    let parsed = parse(
        &mint(json!({ "role": "admin", "name": " tablets ", "expires_in_days": 30 })),
        None,
    )
    .unwrap();
    assert_eq!(parsed.role, AccountRole::Admin);
    assert_eq!(parsed.name, "tablets");
    assert_eq!(parsed.days, 30);
}

#[test]
fn an_owner_or_an_unknown_role_is_refused() {
    assert_eq!(
        refused(mint(json!({ "role": "owner" }))),
        "a service account cannot be an owner"
    );
    assert!(refused(mint(json!({ "role": "viewer" }))).contains("member or admin"));
    assert!(refused(mint(json!({ "role": true }))).contains("member or admin"));
}

#[test]
fn the_account_is_held_to_the_account_name_rules() {
    for bad in [
        "A",
        "Store-Ops",
        "store_ops",
        "-store",
        "store--ops",
        "1store",
    ] {
        let why = refused(mint(json!({ "account": bad })));
        assert!(why.starts_with("'account' must be"), "{bad}: {why}");
    }
    assert!(refused(mint(json!({ "account": 7 }))).contains("'account' must be a string"));
}

#[test]
fn the_org_is_a_slug_or_an_id_and_nothing_else() {
    let id = Uuid::new_v4().to_string();
    let parsed = parse(&mint(json!({ "org": id })), Some("t")).unwrap();
    assert_eq!(parsed.org, id);
    for bad in [
        json!(""),
        json!("   "),
        json!("a\nb"),
        json!("x".repeat(101)),
        json!(3),
    ] {
        assert!(
            refused(mint(json!({ "org": bad.clone() }))).contains("'org'"),
            "{bad}"
        );
    }
}

#[test]
fn the_lifetime_is_one_day_to_ten_years() {
    for days in [0, -1, MAX_LIFETIME_DAYS + 1] {
        assert!(refused(mint(json!({ "expires_in_days": days }))).contains("expires_in_days"));
    }
    assert!(refused(mint(json!({ "expires_in_days": "30" }))).contains("expires_in_days"));
    let longest = parse(
        &mint(json!({ "expires_in_days": MAX_LIFETIME_DAYS })),
        Some("t"),
    );
    assert_eq!(longest.unwrap().days, MAX_LIFETIME_DAYS);
}

#[test]
fn grants_and_other_kinds_fields_are_refused_by_name() {
    assert!(refused(mint(json!({ "grants": [] }))).contains("one org-wide grant"));
    assert!(refused(mint(json!({ "all_access": false }))).contains("one org-wide grant"));
    assert!(refused(mint(json!({ "standing": true }))).contains("no standing"));
    assert!(refused(mint(json!({ "expires_at": null }))).contains("expires_in_days"));
    assert!(refused(mint(json!({ "apps": ["a/b"] }))).contains("not a field"));
}

#[test]
fn a_stored_mint_names_the_org_by_id_and_reads_back_the_same() {
    let org_id = Uuid::new_v4();
    let asked = parse(
        &mint(json!({ "role": "admin" })),
        Some("minted by oxyc on host"),
    )
    .unwrap();
    let stored = asked.stored(org_id);
    assert_eq!(stored["org"], json!(org_id.to_string()));
    assert_eq!(stored["expires_in_days"], json!(DEFAULT_LIFETIME_DAYS));
    let back = parse(&stored, None).unwrap();
    assert_eq!(
        back,
        MintRequest {
            org: org_id.to_string(),
            ..asked
        }
    );
}

#[test]
fn only_its_own_kind_is_read() {
    assert!(asked_for(&mint(json!({}))));
    assert!(!asked_for(&json!({ "kind": "agent" })));
    assert!(!asked_for(&json!({ "apps": ["a/b"] })));
    assert!(refused(json!({ "kind": "agent" })).contains("'kind'"));
    assert!(refused(json!(["service_account"])).contains("JSON object"));
}

#[test]
fn a_stored_mint_must_name_its_token() {
    // A stored mint always carries its resolved name; one without is refused, not defaulted.
    let why = parse(&mint(json!({})), None).expect_err("refused").0;
    assert!(why.contains("'name'"), "{why}");
}

#[test]
fn a_default_name_past_the_limit_asks_for_a_name() {
    let long = default_name(&"h".repeat(255));
    let why = parse(&mint(json!({})), Some(&long)).expect_err("refused").0;
    assert!(why.contains("send 'name'"), "{why}");
}
