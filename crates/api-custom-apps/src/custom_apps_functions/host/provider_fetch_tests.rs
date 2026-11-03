//! The facts the host reads off a `ctx.fetch` (`provider_fetch`): pure.

use super::*;

fn headers(pairs: &[(&str, &str)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), Value::String(v.to_string())))
        .collect()
}

fn basic(user_and_password: &str) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(user_and_password);
    format!("Basic {encoded}")
}

fn secrets(values: &[&str]) -> FallbackSecrets {
    FallbackSecrets::new(values.iter().map(|v| v.to_string()).collect())
}

fn url(s: &str) -> Url {
    Url::parse(s).expect("a URL")
}

#[test]
fn api_key_of_reads_bearer_and_basic() {
    let bearer = headers(&[("Authorization", "Bearer sk_test_a")]);
    assert_eq!(api_key_of(&bearer).as_deref(), Some("sk_test_a"));
    let b = headers(&[("Authorization", &basic("sk_test_x:"))]);
    assert_eq!(api_key_of(&b).as_deref(), Some("sk_test_x"));
    let with_password = headers(&[("Authorization", &basic("rk_test_y:pw"))]);
    assert_eq!(api_key_of(&with_password).as_deref(), Some("rk_test_y"));
}

#[test]
fn api_key_of_ignores_the_header_name_case_and_anything_else() {
    let lower = headers(&[("authorization", "bearer sk_live_z")]);
    assert_eq!(api_key_of(&lower).as_deref(), Some("sk_live_z"));
    assert_eq!(api_key_of(&headers(&[])), None);
    assert_eq!(api_key_of(&headers(&[("x-api-key", "sk_test_a")])), None);
    assert_eq!(
        api_key_of(&headers(&[("Authorization", "Token abc")])),
        None
    );
    assert_eq!(
        api_key_of(&headers(&[("Authorization", "Basic !!!")])),
        None
    );
    assert_eq!(api_key_of(&headers(&[("Authorization", "Bearer ")])), None);
}

#[test]
fn carries_any_finds_a_value_in_a_header() {
    let s = secrets(&["prod-secret-1"]);
    let h = headers(&[("x-token", "pre-prod-secret-1-post")]);
    assert!(carries_any(&s, &url("https://a.example/p"), &h, None));
}

#[test]
fn carries_any_finds_a_value_in_a_basic_credential() {
    let s = secrets(&["prod-secret-1"]);
    let h = headers(&[("Authorization", &basic("prod-secret-1:"))]);
    assert!(carries_any(&s, &url("https://a.example/p"), &h, None));
    let password = headers(&[("authorization", &basic("user:prod-secret-1"))]);
    assert!(carries_any(
        &s,
        &url("https://a.example/p"),
        &password,
        None
    ));
}

#[test]
fn carries_any_finds_a_value_in_the_url_query_even_percent_encoded() {
    let s = secrets(&["a b&c"]);
    let none = headers(&[]);
    assert!(carries_any(
        &s,
        &url("https://a.example/p?k=a%20b%26c"),
        &none,
        None
    ));
    let plain = secrets(&["prod-secret-1"]);
    assert!(carries_any(
        &plain,
        &url("https://a.example/p?k=prod-secret-1"),
        &none,
        None
    ));
    assert!(carries_any(
        &plain,
        &url("https://a.example/prod-secret-1/x"),
        &none,
        None
    ));
}

#[test]
fn carries_any_finds_a_value_in_a_string_body() {
    let s = secrets(&["prod-secret-1"]);
    let none = headers(&[]);
    let u = url("https://a.example/p");
    assert!(carries_any(&s, &u, &none, Some("key=prod-secret-1&x=2")));
}

#[test]
fn carries_any_is_false_when_absent_and_an_empty_value_never_matches() {
    let s = secrets(&["prod-secret-1"]);
    let h = headers(&[("Authorization", "Bearer sk_test_a"), ("x", "y")]);
    let u = url("https://a.example/p?q=1");
    assert!(!carries_any(&s, &u, &h, Some("hello")));
    let empty = secrets(&["", ""]);
    assert!(empty.is_empty());
    assert!(!carries_any(&empty, &u, &h, Some("hello")));
    assert!(!carries_any(&FallbackSecrets::default(), &u, &h, None));
}

#[test]
fn debug_prints_a_count_and_never_a_value() {
    let s = secrets(&["prod-secret-1", "prod-secret-2"]);
    let printed = format!("{s:?}");
    assert_eq!(printed, "FallbackSecrets(2 values)");
    assert!(!printed.contains("prod-secret"));
}

#[test]
fn facts_read_the_key_and_the_carried_secret_from_the_init() {
    let s = secrets(&["prod-secret-1"]);
    let u = url("https://api.stripe.com/v1/charges");
    let init = serde_json::json!({
        "method": "POST",
        "headers": { "authorization": "Bearer sk_test_a" },
        "body": "note=hello",
    });
    let facts = fetch_call_facts(&s, &u, &init);
    assert_eq!(facts.api_key.as_deref(), Some("sk_test_a"));
    assert!(!facts.carries_fallback_secret);

    let leaking = serde_json::json!({
        "headers": { "Authorization": basic("prod-secret-1:") },
    });
    let facts = fetch_call_facts(&s, &u, &leaking);
    assert_eq!(facts.api_key, None, "a hyphen is not in a plain key");
    assert!(facts.carries_fallback_secret);
}

#[test]
fn facts_see_a_secret_in_a_base64_body() {
    let s = secrets(&["prod-secret-1"]);
    let u = url("https://a.example/p");
    let body = base64::engine::general_purpose::STANDARD.encode("k=prod-secret-1");
    let init = serde_json::json!({ "body": body, "bodyEncoding": "base64" });
    assert!(fetch_call_facts(&s, &u, &init).carries_fallback_secret);
    let clean = base64::engine::general_purpose::STANDARD.encode("k=nothing");
    let init = serde_json::json!({ "body": clean, "bodyEncoding": "base64" });
    assert!(!fetch_call_facts(&s, &u, &init).carries_fallback_secret);
}

#[test]
fn facts_with_no_init_fields_are_blank() {
    let facts = fetch_call_facts(
        &secrets(&["zzz-secret"]),
        &url("https://a.example/"),
        &Value::Null,
    );
    assert_eq!(facts, CallFacts::default());
}

#[test]
fn two_authorization_headers_have_no_api_key() {
    // Both are sent; which one the provider reads is its choice.
    let two = headers(&[
        ("Authorization", "Bearer sk_test_a"),
        ("authorization", "Bearer sk_live_b"),
    ]);
    assert_eq!(api_key_of(&two), None);
    assert_eq!(authorization_count(&two), 2);
    // The control: the first alone is admitted.
    let one = headers(&[("Authorization", "Bearer sk_test_a")]);
    assert_eq!(api_key_of(&one).as_deref(), Some("sk_test_a"));
    assert_eq!(authorization_count(&one), 1);
}

#[test]
fn a_value_that_is_not_one_plain_token_has_no_api_key() {
    for value in [
        "Bearer sk_test_a, Bearer sk_live_b",
        "Bearer sk_test_a,sk_live_b",
        "Bearer sk_test_a sk_live_b",
        "Bearer sk_test_a;x",
        "Bearer sk-test-a",
        "Bearer sk_test_a\u{e9}",
    ] {
        let h = headers(&[("Authorization", value)]);
        assert_eq!(api_key_of(&h), None, "{value}");
    }
    let basic_two = headers(&[("Authorization", &basic("sk_test_a,sk_live_b:"))]);
    assert_eq!(api_key_of(&basic_two), None);
}

#[test]
fn facts_flag_a_second_authorization_and_a_host_override() {
    let u = url("https://api.stripe.com/v1/charges");
    let none = FallbackSecrets::default();
    let two = serde_json::json!({
        "headers": { "Authorization": "Bearer sk_test_a", "authorization": "Bearer sk_live_b" },
    });
    let facts = fetch_call_facts(&none, &u, &two);
    assert!(facts.duplicate_authorization);
    assert_eq!(facts.api_key, None);
    let one = serde_json::json!({ "headers": { "Authorization": "Bearer sk_test_a" } });
    let facts = fetch_call_facts(&none, &u, &one);
    assert!(!facts.duplicate_authorization);
    assert!(!facts.overrides_host);
    assert_eq!(facts.api_key.as_deref(), Some("sk_test_a"));
}

#[test]
fn each_host_naming_header_is_an_override_in_any_case() {
    let u = url("https://connect.squareupsandbox.com/v2/payments");
    let none = FallbackSecrets::default();
    for name in [
        "host",
        "Host",
        "HOST",
        ":authority",
        ":AUTHORITY",
        "x-forwarded-host",
        "X-Forwarded-Host",
        "forwarded",
        "FORWARDED",
    ] {
        let init = serde_json::json!({ "headers": { name: "connect.squareup.com" } });
        assert!(fetch_call_facts(&none, &u, &init).overrides_host, "{name}");
    }
    let plain = serde_json::json!({ "headers": { "x-forward": "a", "hosts": "b" } });
    assert!(!fetch_call_facts(&none, &u, &plain).overrides_host);
}

/// A fallback value with every character a form encoder rewrites.
const AWKWARD: &str = "a+b/c=d@e:f g";

#[test]
fn a_fallback_secret_in_a_percent_encoded_body_is_found() {
    let s = secrets(&[AWKWARD]);
    let u = url("https://api.stripe.com/v1/charges");
    let body = format!("note={}", urlencoding::encode(AWKWARD));
    assert!(!body.contains(AWKWARD), "the fixture must be encoded");
    let init = serde_json::json!({ "body": body });
    assert!(fetch_call_facts(&s, &u, &init).carries_fallback_secret);
}

#[test]
fn a_fallback_secret_in_a_form_encoded_body_is_found() {
    let s = secrets(&[AWKWARD]);
    let u = url("https://api.stripe.com/v1/charges");
    // `URLSearchParams` writes a space as `+` and a plus as `%2B`.
    let body = format!("note={}", urlencoding::encode(AWKWARD).replace("%20", "+"));
    assert!(body.contains('+') && !body.contains(AWKWARD));
    let init = serde_json::json!({ "body": body });
    assert!(fetch_call_facts(&s, &u, &init).carries_fallback_secret);
    // The control: a body without it is clean.
    let clean = serde_json::json!({ "body": "note=a+b%2Fc" });
    assert!(!fetch_call_facts(&s, &u, &clean).carries_fallback_secret);
}

#[test]
fn a_fallback_secret_in_a_percent_encoded_header_is_found() {
    let s = secrets(&[AWKWARD]);
    let u = url("https://api.stripe.com/v1/charges");
    let encoded = urlencoding::encode(AWKWARD).into_owned();
    assert!(!encoded.contains(AWKWARD));
    let init = serde_json::json!({ "headers": { "x-note": encoded } });
    assert!(fetch_call_facts(&s, &u, &init).carries_fallback_secret);
}

#[test]
fn a_failed_registry_read_is_no_companies_so_the_call_holds() {
    let failed: Result<Vec<String>, String> = Err("connection reset".to_string());
    assert!(companies_from(failed).is_empty());
    let read: Result<Vec<String>, String> = Ok(vec!["111".to_string(), "222".to_string()]);
    let companies = companies_from(read);
    assert_eq!(
        companies.into_iter().collect::<Vec<_>>(),
        vec!["111".to_string(), "222".to_string()]
    );
}
