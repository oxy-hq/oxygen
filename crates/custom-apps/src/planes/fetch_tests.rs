use oxy_app_core::custom_app_environment::AppEnvironment;
use serde_json::json;

use super::{
    FetchMode, FetchSetting, PROVIDER_SANDBOXES, PlaneSpec, provider_sandbox, validate_fetch,
};

fn setting(value: serde_json::Value) -> FetchSetting {
    PlaneSpec::parse(&json!({ "fetch": value }))
        .expect("a parseable fetch setting")
        .fetch
        .expect("fetch is set")
}

fn allow(hosts: &[&str]) -> FetchSetting {
    setting(json!({"mode": "allow", "hosts": hosts}))
}

#[test]
fn bare_mode_string_parses() {
    let parsed = setting(json!("held-writes"));
    assert_eq!(parsed, FetchSetting::Mode(FetchMode::HeldWrites));
    assert_eq!(parsed.mode(), FetchMode::HeldWrites);
    assert!(parsed.hosts().is_empty());
}

#[test]
fn object_form_parses() {
    let parsed = allow(&["Sandbox-QuickBooks.api.intuit.com"]);
    assert_eq!(parsed.mode(), FetchMode::Allow);
    assert_eq!(parsed.hosts(), ["Sandbox-QuickBooks.api.intuit.com"]);
}

#[test]
fn unknown_key_and_unknown_mode_are_refused() {
    // The control: the same object without the stray key parses, so the
    // refusal below is the key's and not something else's.
    assert!(PlaneSpec::parse(&json!({"fetch": {"mode": "allow", "hosts": []}})).is_ok());
    let extra = json!({"fetch": {"mode": "allow", "hosts": [], "x": 1}});
    let error = PlaneSpec::parse(&extra).expect_err("an unknown key is refused");
    assert!(error.contains("`x`"), "the error names the key: {error}");
    assert!(PlaneSpec::parse(&json!({"fetch": "everything"})).is_err());
    assert!(PlaneSpec::parse(&json!({"fetch": {"hosts": []}})).is_err());
}

#[test]
fn resolve_lowercases_hosts_and_defaults_to_held_writes() {
    let spec = PlaneSpec::parse(&json!({
        "fetch": {"mode": "allow", "hosts": ["Sandbox-QuickBooks.api.intuit.com"]}
    }))
    .expect("parses");
    let resolved = spec
        .resolve(&AppEnvironment::Staging)
        .expect("staging has planes");
    assert_eq!(resolved.fetch, FetchMode::Allow);
    assert!(
        resolved
            .fetch_hosts
            .contains("sandbox-quickbooks.api.intuit.com")
    );
    assert_eq!(resolved.fetch_hosts.len(), 1);

    let default = PlaneSpec::default()
        .resolve(&AppEnvironment::Staging)
        .expect("staging has planes");
    assert_eq!(default.fetch, FetchMode::HeldWrites);
    assert!(default.fetch_hosts.is_empty());

    let held = PlaneSpec::parse(&json!({"fetch": "held-writes"}))
        .expect("parses")
        .resolve(&AppEnvironment::Staging)
        .expect("staging has planes");
    assert!(held.fetch_hosts.is_empty());
}

#[test]
fn every_registered_host_validates_in_any_case() {
    for sandbox in PROVIDER_SANDBOXES {
        validate_fetch(&allow(&[sandbox.host])).expect(sandbox.host);
        validate_fetch(&allow(&[&sandbox.host.to_uppercase()])).expect(sandbox.host);
        assert_eq!(
            provider_sandbox(&sandbox.host.to_uppercase()).map(|s| s.host),
            Some(sandbox.host)
        );
    }
    let all: Vec<&str> = PROVIDER_SANDBOXES.iter().map(|s| s.host).collect();
    validate_fetch(&allow(&all)).expect("every host at once");
}

#[test]
fn registry_hosts_are_lowercase_and_unique() {
    let mut seen = std::collections::BTreeSet::new();
    for sandbox in PROVIDER_SANDBOXES {
        assert_eq!(sandbox.host, sandbox.host.to_lowercase());
        assert!(seen.insert(sandbox.host), "{} twice", sandbox.host);
    }
}

#[test]
fn allow_needs_hosts_and_held_writes_takes_none() {
    assert!(validate_fetch(&allow(&[])).is_err());
    assert!(validate_fetch(&setting(json!({"mode": "allow"}))).is_err());
    assert!(validate_fetch(&setting(json!("allow"))).is_err());
    let held_with_hosts = setting(json!({"mode": "held-writes", "hosts": ["api.stripe.com"]}));
    assert!(validate_fetch(&held_with_hosts).is_err());
    validate_fetch(&setting(json!("held-writes"))).expect("the default");
    validate_fetch(&setting(json!({"mode": "held-writes"}))).expect("object without hosts");
}

#[test]
fn unregistered_and_non_bare_hosts_are_refused() {
    for host in [
        "api.intuit.com",
        "quickbooks.api.intuit.com",
        "https://api.stripe.com",
        "api.stripe.com:443",
        "api.stripe.com/v1",
        "user@api.stripe.com",
        "api.stripe.com ",
        "",
    ] {
        assert!(validate_fetch(&allow(&[host])).is_err(), "{host:?}");
    }
}

#[test]
fn more_than_sixteen_hosts_and_duplicates_are_refused() {
    let many: Vec<String> = (0..17).map(|i| format!("h{i}.example.com")).collect();
    let refs: Vec<&str> = many.iter().map(String::as_str).collect();
    let error = validate_fetch(&allow(&refs)).expect_err("17 hosts");
    assert!(error.contains("16"), "{error}");

    let error =
        validate_fetch(&allow(&["api.stripe.com", "API.stripe.com"])).expect_err("a repeated host");
    assert!(error.contains("twice"), "{error}");
}
