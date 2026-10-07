//! What an agent token's mint may ask for, with no database.

use serde_json::json;
use uuid::Uuid;

use super::*;

const DEFAULT_NAME: &str = "agent on build-box";

fn mint(extra: Value) -> Value {
    let mut mint = json!({ "kind": "agent" });
    if let (Value::Object(mint), Value::Object(extra)) = (&mut mint, extra) {
        mint.extend(extra);
    }
    mint
}

fn refused(mint: &Value) -> String {
    parse(mint, Some(DEFAULT_NAME)).unwrap_err().0
}

#[test]
fn only_kind_agent_asks_for_one() {
    assert!(asked_for(&json!({ "kind": "agent" })));
    for other in [
        json!({ "kind": "sandbox_agent", "apps": [] }),
        json!({ "kind": "personal" }),
        json!({ "kind": null }),
        json!({ "kind": ["agent"] }),
        json!({ "standing": true }),
        json!("agent"),
        json!(null),
    ] {
        assert!(!asked_for(&other), "{other}");
    }
}

#[test]
fn the_defaults_are_eight_hours_no_standing_and_the_hosts_name() {
    let request = parse(&mint(json!({})), Some(DEFAULT_NAME)).unwrap();
    assert_eq!(
        request,
        MintRequest {
            name: DEFAULT_NAME.into(),
            standing: false,
            hours: 8,
        }
    );
    assert_eq!(DEFAULT_HOURS, 8);
    let now = Utc::now();
    assert_eq!(request.expires_at(now), now + Duration::hours(8));

    // An explicit null is the field left out.
    let nulls = mint(json!({ "name": null, "standing": null, "expires_in_hours": null }));
    assert_eq!(parse(&nulls, Some(DEFAULT_NAME)).unwrap(), request);
}

#[test]
fn hours_run_from_one_to_a_week() {
    assert_eq!(MAX_HOURS, 168);
    for hours in [1, 8, 167, MAX_HOURS] {
        let request = parse(
            &mint(json!({ "expires_in_hours": hours })),
            Some(DEFAULT_NAME),
        )
        .unwrap();
        assert_eq!(request.hours, hours);
    }
    for bad in [
        json!(0),
        json!(-1),
        json!(169),
        json!(100_000),
        json!(8.5),
        json!("8"),
        json!(true),
        json!([8]),
    ] {
        let message = refused(&mint(json!({ "expires_in_hours": bad })));
        assert!(message.contains("expires_in_hours"), "{bad}: {message}");
    }
}

#[test]
fn standing_is_a_boolean_and_nothing_looser() {
    for (sent, expected) in [(json!(true), true), (json!(false), false)] {
        let request = parse(&mint(json!({ "standing": sent })), Some(DEFAULT_NAME)).unwrap();
        assert_eq!(request.standing, expected);
    }
    // Never coerced: "true" minting a standing would be more than was typed.
    for bad in [json!("true"), json!(1), json!(["platform"]), json!({})] {
        let message = refused(&mint(json!({ "standing": bad })));
        assert!(message.contains("standing"), "{bad}: {message}");
    }
}

#[test]
fn a_sent_name_is_trimmed_and_held_to_the_token_name_limit() {
    let request = parse(
        &mint(json!({ "name": "  nightly repro  " })),
        Some(DEFAULT_NAME),
    )
    .unwrap();
    assert_eq!(request.name, "nightly repro");

    let longest = "n".repeat(NAME_MAX_CHARS);
    let request = parse(&mint(json!({ "name": longest })), Some(DEFAULT_NAME)).unwrap();
    assert_eq!(request.name.chars().count(), NAME_MAX_CHARS);

    for bad in [
        json!("n".repeat(NAME_MAX_CHARS + 1)),
        json!(""),
        json!("   "),
        json!(7),
        json!(["agent"]),
    ] {
        let message = refused(&mint(json!({ "name": bad })));
        assert!(message.contains("name"), "{bad}: {message}");
    }
}

#[test]
fn a_mint_with_no_name_needs_a_default_that_fits() {
    // A stored mint always carries its name: with no default, one is required.
    let message = parse(&mint(json!({})), None).unwrap_err().0;
    assert!(message.contains("'name'"), "{message}");

    // A 255-character hostname makes a default no token may be named.
    let default = format!("agent on {}", "h".repeat(255));
    let message = parse(&mint(json!({})), Some(&default)).unwrap_err().0;
    assert!(message.contains("send 'name'"), "{message}");
    let named = parse(&mint(json!({ "name": "agent" })), Some(&default)).unwrap();
    assert_eq!(named.name, "agent");
}

#[test]
fn a_field_of_another_kind_is_refused_and_says_whose_it_is() {
    for field in PERSONAL_FIELDS {
        let message = refused(&mint(json!({ field: true })));
        assert!(
            message.contains(field) && message.contains("does not apply to an agent token"),
            "{field}: {message}"
        );
    }
    let message = refused(&mint(
        json!({ "apps": ["0199f3a0-0000-7000-8000-00000000000a"] }),
    ));
    assert!(
        message.contains("'apps'") && message.contains("sandbox_agent"),
        "{message}"
    );
}

#[test]
fn an_unknown_field_is_refused_rather_than_ignored() {
    for field in ["read_only", "orgs", "org_id", "scope", "Standing", "hours"] {
        let message = refused(&mint(json!({ field: true })));
        assert!(
            message.contains(field) && message.contains("not a field"),
            "{field}: {message}"
        );
    }
}

#[test]
fn only_an_object_of_kind_agent_is_parsed() {
    for not_a_mint in [json!("agent"), json!(["agent"]), json!(null), json!(8)] {
        let message = refused(&not_a_mint);
        assert!(message.contains("JSON object"), "{not_a_mint}: {message}");
    }
    for other in [
        json!({}),
        json!({ "kind": "sandbox_agent" }),
        json!({ "kind": 1 }),
    ] {
        let message = refused(&other);
        assert!(message.contains("'kind'"), "{other}: {message}");
    }
}

#[test]
fn what_a_code_stores_parses_back_to_the_same_request() {
    let asked = mint(json!({ "standing": true, "expires_in_hours": 24 }));
    let request = parse(&asked, Some(DEFAULT_NAME)).unwrap();
    let stored = request.stored();
    assert_eq!(
        stored,
        json!({
            "kind": "agent",
            "name": DEFAULT_NAME,
            "standing": true,
            "expires_in_hours": 24,
        })
    );
    // Read back with no default: the name it was approved under is in it.
    assert_eq!(parse(&stored, None).unwrap(), request);
    // And it is a mint the sandbox agent parser refuses, which is what a
    // binary one release back makes of a code it does not know.
    assert!(asked_for(&stored));
    assert!(crate::token::sandbox::parse(&stored, None).is_err());
}

fn row(kind: &str, source: &str) -> api_tokens::Model {
    api_tokens::Model {
        id: Uuid::new_v4(),
        kind: kind.into(),
        principal_user_id: Uuid::from_u128(1),
        name: "agent on build-box".into(),
        display_prefix: "oxy_pat_Ab3x".into(),
        last_four: "wxyz".into(),
        token_hash: vec![0; 32],
        all_access: true,
        platform: false,
        partner: false,
        expires_at: Some((Utc::now() + Duration::hours(8)).fixed_offset()),
        last_used_at: None,
        created_at: Utc::now().fixed_offset(),
        created_by: None,
        revoked_at: None,
        revoked_by: None,
        revoke_reason: None,
        source: source.into(),
        legacy_api_key_id: None,
        trust_policy_id: None,
        oidc_claims: None,
    }
}

#[test]
fn an_agent_token_is_a_personal_token_of_its_own_source() {
    assert!(is_agent_token(&row("personal", source::OXYC_AGENT)));
    for other in [
        source::UI,
        source::OXYC_LOGIN,
        source::OXYC,
        source::LEGACY_ENDPOINT,
    ] {
        assert!(!is_agent_token(&row("personal", other)), "{other}");
    }
    // The source alone does not make another kind one.
    for kind in ["sandbox_agent", "service_account", "ci", "legacy_key"] {
        assert!(!is_agent_token(&row(kind, source::OXYC_AGENT)), "{kind}");
    }
}
