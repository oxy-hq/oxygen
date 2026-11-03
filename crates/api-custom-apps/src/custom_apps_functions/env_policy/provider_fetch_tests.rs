//! `provider_refusal`: each check of a call to a provider's sandbox, in the
//! order the module docs give. Pure — a call is a URL and three facts.

use std::collections::BTreeSet;

use super::*;
use crate::custom_apps_planes::PROVIDER_SANDBOXES;

const SQUARE: &str = "https://connect.squareupsandbox.com/v2/payments";
const REALM: &str = "4620816365000000";

fn all_listed() -> BTreeSet<String> {
    PROVIDER_SANDBOXES
        .iter()
        .map(|sandbox| sandbox.host.to_string())
        .collect()
}

fn companies(realms: &[&str]) -> BTreeSet<String> {
    realms.iter().map(|realm| realm.to_string()).collect()
}

/// A POST of `url` with the facts a test varies; everything else is clean.
fn refusal(
    listed: &BTreeSet<String>,
    url: &str,
    api_key: Option<&str>,
    registered: &BTreeSet<String>,
    carries_fallback_secret: bool,
) -> Option<&'static str> {
    let url = url::Url::parse(url).expect("a URL");
    let call = FetchCall {
        method: "POST",
        url: &url,
        minted: &[],
        carries_fallback_secret,
        api_key,
        overrides_host: false,
        duplicate_authorization: false,
        sandbox_companies: registered,
    };
    provider_refusal(listed, &call)
}

fn refusal_of(url: &str) -> Option<&'static str> {
    refusal(&all_listed(), url, None, &BTreeSet::new(), false)
}

#[test]
fn a_clean_call_to_a_sandbox_only_host_is_sent() {
    assert_eq!(refusal_of(SQUARE), None);
    assert_eq!(
        refusal_of("https://Connect.SquareUpSandbox.com/v2/payments"),
        None,
        "the host is compared lowercased"
    );
}

#[test]
fn a_call_that_is_not_plain_https_is_held() {
    for url in [
        "http://connect.squareupsandbox.com/v2/payments",
        "https://connect.squareupsandbox.com:8443/v2/payments",
        "https://user@connect.squareupsandbox.com/v2/payments",
        "https://user:pw@connect.squareupsandbox.com/v2/payments",
    ] {
        assert_eq!(refusal_of(url), Some(NOTE_NOT_PLAIN_HTTPS), "{url}");
    }
}

#[test]
fn a_host_the_environment_did_not_list_is_held() {
    assert_eq!(
        refusal_of("https://api.squareup.com/v2/payments"),
        Some(NOTE_NOT_LISTED)
    );
    let only_stripe = BTreeSet::from(["api.stripe.com".to_string()]);
    assert_eq!(
        refusal(&only_stripe, SQUARE, None, &BTreeSet::new(), false),
        Some(NOTE_NOT_LISTED)
    );
    assert_eq!(
        refusal(&BTreeSet::new(), SQUARE, None, &BTreeSet::new(), false),
        Some(NOTE_NOT_LISTED),
        "an empty list sends nothing"
    );
}

#[test]
fn a_listed_host_no_longer_in_the_registry_is_held() {
    let listed = BTreeSet::from(["example.com".to_string()]);
    assert_eq!(
        refusal(
            &listed,
            "https://example.com/x",
            None,
            &BTreeSet::new(),
            false
        ),
        Some(NOTE_NOT_REGISTERED)
    );
}

#[test]
fn stripe_is_sent_only_with_a_test_key() {
    let url = "https://api.stripe.com/v1/charges";
    let none = BTreeSet::new();
    for key in ["sk_test_abc", "rk_test_abc"] {
        assert_eq!(refusal(&all_listed(), url, Some(key), &none, false), None);
    }
    for key in [
        Some("sk_live_abc"),
        Some("rk_live_abc"),
        Some("pk_test_abc"),
        Some(""),
        None,
    ] {
        assert_eq!(
            refusal(&all_listed(), url, key, &none, false),
            Some(NOTE_NO_TEST_KEY),
            "{key:?}"
        );
    }
}

#[test]
fn quickbooks_is_sent_only_to_a_registered_sandbox_company() {
    let host = "https://sandbox-quickbooks.api.intuit.com";
    let registered = companies(&[REALM]);
    let on = |path: &str, registered: &BTreeSet<String>| {
        refusal(
            &all_listed(),
            &format!("{host}{path}"),
            None,
            registered,
            false,
        )
    };
    assert_eq!(
        on(&format!("/v3/company/{REALM}/invoice"), &registered),
        None
    );
    assert_eq!(on(&format!("/v3/company/{REALM}"), &registered), None);
    for path in [
        "/v3/company/9999/invoice",
        "/v3/companyinfo",
        "/v3/company",
        "/v3/company/",
        "/v2/company/4620816365000000/invoice",
        "/",
    ] {
        assert_eq!(
            on(path, &registered),
            Some(NOTE_UNREGISTERED_COMPANY),
            "{path}"
        );
    }
    assert_eq!(
        on(&format!("/v3/company/{REALM}/invoice"), &BTreeSet::new()),
        Some(NOTE_UNREGISTERED_COMPANY),
        "a registry read that failed is an empty set"
    );
}

#[test]
fn a_call_carrying_a_fallback_secret_is_held_last() {
    assert_eq!(
        refusal(&all_listed(), SQUARE, None, &BTreeSet::new(), true),
        Some(NOTE_FALLBACK_CREDENTIAL)
    );
    // An earlier check names its own note even when the secret is there too.
    assert_eq!(
        refusal(
            &all_listed(),
            "https://api.stripe.com/v1/charges",
            Some("sk_live_abc"),
            &BTreeSet::new(),
            true
        ),
        Some(NOTE_NO_TEST_KEY)
    );
}

#[test]
fn a_path_or_host_that_only_looks_like_a_sandbox_is_held() {
    let qb = "https://sandbox-quickbooks.api.intuit.com";
    let registered = companies(&[REALM]);
    let on = |url: String| refusal(&all_listed(), &url, None, &registered, false);
    for path in [
        // `..` is resolved by the URL parser, so the realm is `other`.
        format!("/v3/company/{REALM}/../other/x"),
        // An encoded segment is not the registered realm.
        "/v3/company/%34620816365000000/invoice".to_string(),
        format!("/v3//company/{REALM}"),
    ] {
        assert_eq!(
            on(format!("{qb}{path}")),
            Some(NOTE_UNREGISTERED_COMPANY),
            "{path}"
        );
    }
    assert_eq!(
        refusal_of("https://connect.squareupsandbox.com./v2/payments"),
        Some(NOTE_NOT_LISTED),
        "a trailing dot is another name"
    );
    assert_eq!(
        refusal_of("https://[::1]/v2/payments"),
        Some(NOTE_NOT_LISTED),
        "an IP literal is no registered host"
    );
}

#[test]
fn the_default_port_written_out_is_the_default_port() {
    assert_eq!(
        refusal_of("https://connect.squareupsandbox.com:443/v2/payments"),
        None
    );
}

#[test]
fn a_test_key_prefix_alone_is_admitted() {
    // Harmless: Stripe rejects a key that is only the prefix; the host is
    // still the sandbox-or-test one and the request carries no live value.
    assert_eq!(
        refusal(
            &all_listed(),
            "https://api.stripe.com/v1/charges",
            Some("sk_test_"),
            &BTreeSet::new(),
            false
        ),
        None
    );
}

/// A POST to `url` with the two header facts; everything else is clean.
fn refusal_with_headers(
    url: &str,
    api_key: Option<&str>,
    overrides_host: bool,
    duplicate_authorization: bool,
) -> Option<&'static str> {
    let url = url::Url::parse(url).expect("a URL");
    let none = BTreeSet::new();
    let call = FetchCall {
        method: "POST",
        url: &url,
        minted: &[],
        carries_fallback_secret: false,
        api_key,
        overrides_host,
        duplicate_authorization,
        sandbox_companies: &none,
    };
    provider_refusal(&all_listed(), &call)
}

#[test]
fn a_call_that_sets_its_own_host_is_held() {
    assert_eq!(
        refusal_with_headers(SQUARE, None, true, false),
        Some(NOTE_HOST_OVERRIDE)
    );
    assert_eq!(
        refusal_with_headers(
            "https://api.stripe.com/v1/charges",
            Some("sk_test_a"),
            true,
            false
        ),
        Some(NOTE_HOST_OVERRIDE),
        "a test key does not excuse it"
    );
    assert_eq!(refusal_with_headers(SQUARE, None, false, false), None);
}

#[test]
fn a_second_authorization_header_holds_whatever_the_host() {
    assert_eq!(
        refusal_with_headers(SQUARE, None, false, true),
        Some(NOTE_DUPLICATE_AUTHORIZATION)
    );
    // Stripe has no key then, so its own check names the note first.
    assert_eq!(
        refusal_with_headers("https://api.stripe.com/v1/charges", None, false, true),
        Some(NOTE_NO_TEST_KEY)
    );
}
