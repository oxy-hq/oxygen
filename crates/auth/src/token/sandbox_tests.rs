//! What a sandbox agent mint may ask for, with no database.

use serde_json::json;

use super::*;

const APP_A: &str = "0199f3a0-0000-7000-8000-00000000000a";
const APP_B: &str = "0199f3a0-0000-7000-8000-00000000000b";

fn body(extra: serde_json::Value) -> Value {
    let mut body = json!({ "name": "agent", "kind": "sandbox_agent", "apps": [APP_A] });
    if let (Value::Object(body), Value::Object(extra)) = (&mut body, extra) {
        body.extend(extra);
    }
    body
}

fn refused(body: &Value) -> String {
    parse(body, None).unwrap_err().0
}

#[test]
fn a_body_with_no_kind_or_personal_is_a_personal_tokens() {
    assert_eq!(asked_for(&json!({ "name": "laptop" })), Ok(false));
    assert_eq!(asked_for(&json!({ "kind": null })), Ok(false));
    assert_eq!(asked_for(&json!({ "kind": "personal" })), Ok(false));
    assert_eq!(asked_for(&json!({ "kind": "sandbox_agent" })), Ok(true));
}

#[test]
fn any_other_kind_is_refused_rather_than_minted_as_personal() {
    for kind in [json!("service_account"), json!("ci"), json!(7), json!([])] {
        assert!(asked_for(&json!({ "kind": kind })).is_err(), "{kind}");
    }
}

#[test]
fn the_defaults_are_eight_hours_and_the_apps_as_sent() {
    let mint = parse(&body(json!({})), None).unwrap();
    assert_eq!(mint.name, "agent");
    assert_eq!(mint.apps, vec![APP_A.to_string()]);
    assert_eq!(mint.hours, DEFAULT_HOURS);
    let now = Utc::now();
    assert_eq!(mint.expires_at(now), now + Duration::hours(8));
}

#[test]
fn hours_run_from_one_to_a_week() {
    for hours in [1, 8, MAX_HOURS] {
        let mint = parse(&body(json!({ "expires_in_hours": hours })), None).unwrap();
        assert_eq!(mint.hours, hours);
    }
    for bad in [json!(0), json!(-1), json!(169), json!(8.5), json!("8")] {
        let message = refused(&body(json!({ "expires_in_hours": bad })));
        assert!(message.contains("expires_in_hours"), "{message}");
    }
}

#[test]
fn apps_are_one_to_five_and_distinct() {
    let five: Vec<String> = (0..5)
        .map(|n| format!("0199f3a0-0000-7000-8000-00000000001{n}"))
        .collect();
    assert_eq!(
        parse(&body(json!({ "apps": five })), None).unwrap().apps,
        five
    );

    let six: Vec<String> = (0..6)
        .map(|n| format!("0199f3a0-0000-7000-8000-00000000001{n}"))
        .collect();
    for bad in [
        json!([]),
        json!(six),
        json!([APP_A, APP_A]),
        // The same id in another case is the same app.
        json!([APP_A, APP_A.to_uppercase()]),
        json!([7]),
        json!("not a list"),
    ] {
        let message = refused(&body(json!({ "apps": bad })));
        assert!(message.contains("apps"), "{message}");
    }
    let mut no_apps = body(json!({}));
    no_apps.as_object_mut().unwrap().remove("apps");
    assert!(refused(&no_apps).contains("apps"));
}

#[test]
fn an_id_that_is_not_a_uuid_is_kept_for_the_handler_to_refuse() {
    // It names no app: the handler answers it as an app the caller cannot
    // reach, so a mint cannot tell a malformed id from an unknown one.
    let mint = parse(&body(json!({ "apps": ["acme/store", APP_B] })), None).unwrap();
    assert_eq!(mint.apps, vec!["acme/store".to_string(), APP_B.to_string()]);
}

#[test]
fn a_personal_tokens_field_is_refused_not_ignored() {
    for field in PERSONAL_FIELDS {
        let mut extra = serde_json::Map::new();
        extra.insert(field.to_string(), json!(true));
        let message = refused(&body(Value::Object(extra)));
        assert!(message.contains(field), "{field}: {message}");
    }
}

#[test]
fn the_name_is_required_unless_the_caller_has_a_default() {
    let mut unnamed = body(json!({}));
    unnamed.as_object_mut().unwrap().remove("name");
    assert!(refused(&unnamed).contains("name"));
    assert_eq!(
        parse(&unnamed, Some("sandbox agent on laptop"))
            .unwrap()
            .name,
        "sandbox agent on laptop"
    );
    // A name that is sent wins, and is cleaned like any token's.
    assert_eq!(
        parse(&body(json!({ "name": "  ci bot " })), Some("default"))
            .unwrap()
            .name,
        "ci bot"
    );
    assert!(refused(&body(json!({ "name": "" }))).contains("name"));
    assert!(refused(&body(json!({ "name": 7 }))).contains("name"));
}

#[test]
fn only_a_sandbox_body_parses() {
    assert!(parse(&json!({ "name": "x", "apps": [APP_A] }), None).is_err());
    assert!(
        parse(
            &json!({ "name": "x", "kind": "personal", "apps": [APP_A] }),
            None
        )
        .is_err()
    );
    assert!(parse(&json!([]), None).is_err());
}

#[test]
fn a_stored_mint_reads_back_as_the_same_request() {
    let mint = parse(&body(json!({ "expires_in_hours": 24 })), None).unwrap();
    let app = Uuid::parse_str(APP_A).unwrap();
    let stored = mint.stored(&[app]);
    assert_eq!(parse(&stored, None), Ok(mint));
}
