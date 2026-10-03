//! `EnvPolicy::decide_on_fetch`: the upload a non-production run sends, and
//! every request beside it that is still held. Pure — URLs are built by hand
//! in both S3 addressing styles, with a stand-in for the signature.

use oxy_app_core::custom_app_environment::AppEnvironment;
use uuid::Uuid;

use super::MintedUpload;
use crate::server::api::custom_apps_functions::env_policy::{Decision, EnvPolicy, HostOp, Target};
use crate::server::api::custom_apps_storage::Silo;

const SENT: Decision = Decision::Isolate(Target::StorageSilo);
const SIGNATURE: &str = "X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Signature=abc123&x-id=PutObject";

fn app() -> Uuid {
    Uuid::from_u128(7)
}

fn staging() -> EnvPolicy {
    EnvPolicy::for_environment(AppEnvironment::Staging)
}

fn sandbox(handle: &str) -> EnvPolicy {
    EnvPolicy::for_environment(AppEnvironment::Dev {
        handle: handle.into(),
    })
}

fn key_in(silo: &Silo) -> String {
    format!("{}uploads/report-x1.txt", silo.prefix())
}

/// Path-style, as an `AWS_ENDPOINT_URL` store (MinIO, R2) is addressed.
fn path_style(key: &str) -> String {
    format!("https://objects.example.com/assets/{key}?{SIGNATURE}")
}

/// Virtual-host style, as AWS S3 itself is addressed.
fn virtual_host(key: &str) -> String {
    format!("https://assets.s3.us-east-1.amazonaws.com/{key}?{SIGNATURE}")
}

fn minted(url: &str, key: &str) -> MintedUpload {
    MintedUpload::new(url, key).expect("a URL")
}

/// The upload `policy`'s own `getUploadUrl` mints for [`app`], path-style.
fn own(policy: &EnvPolicy) -> (String, MintedUpload) {
    let key = key_in(&policy.storage_silo(app()));
    let url = path_style(&key);
    (url.clone(), minted(&url, &key))
}

fn decide(policy: &EnvPolicy, method: &str, url: &str, minted: &[MintedUpload]) -> Decision {
    let url = url::Url::parse(url).expect("a URL");
    policy.decide_on_fetch(app(), method, &url, minted)
}

#[test]
fn a_put_to_a_url_this_invocation_minted_is_sent_to_the_environments_silo() {
    for policy in [staging(), sandbox("a1")] {
        let (url, upload) = own(&policy);
        assert_eq!(decide(&policy, "PUT", &url, &[upload.clone()]), SENT);
        assert_eq!(decide(&policy, "put", &url, &[upload]), SENT);
        assert_eq!(policy.decide(HostOp::Fetch), Decision::Hold, "the op alone");
    }
    // Either addressing style: the URL is compared whole, never taken apart.
    let key = key_in(&staging().storage_silo(app()));
    let url = virtual_host(&key);
    assert_eq!(decide(&staging(), "PUT", &url, &[minted(&url, &key)]), SENT);
}

#[test]
fn a_url_nobody_minted_this_invocation_is_held() {
    let (url, upload) = own(&staging());
    assert_eq!(decide(&staging(), "PUT", &url, &[]), Decision::Hold);
    let other = path_style(&format!(
        "{}uploads/other.txt",
        staging().storage_silo(app()).prefix()
    ));
    assert_eq!(
        decide(&staging(), "PUT", &other, &[upload]),
        Decision::Hold,
        "another key of the same silo, never minted"
    );
}

#[test]
fn any_method_but_put_is_held() {
    let (url, upload) = own(&staging());
    for method in ["POST", "PATCH", "DELETE", "GET", "HEAD", "PUTS", ""] {
        assert_eq!(
            decide(&staging(), method, &url, std::slice::from_ref(&upload)),
            Decision::Hold,
            "{method}"
        );
    }
}

/// Every URL that is not the minted one, with the minted one on the list.
#[test]
fn a_url_that_differs_from_the_minted_one_is_held() {
    let policy = staging();
    let (url, upload) = own(&policy);
    let app = app();
    let swap = |from: &str, to: &str| {
        assert!(url.contains(from), "{from} not in {url}");
        url.replacen(from, to, 1)
    };
    let stg = "~staging/";
    let differing = [
        ("production's silo", swap(stg, "/")),
        ("another environment's silo", swap(stg, "~dev-a1/")),
        (
            "another app's silo",
            swap(&app.to_string(), &Uuid::from_u128(8).to_string()),
        ),
        (
            "another host",
            swap("objects.example.com", "objects.example.net"),
        ),
        (
            "the store as a subdomain of another host",
            swap("objects.example.com", "objects.example.com.evil.net"),
        ),
        (
            "the store as another host's userinfo",
            swap("objects.example.com", "objects.example.com@evil.net"),
        ),
        (
            "another port",
            swap("objects.example.com", "objects.example.com:8443"),
        ),
        ("plain http", swap("https://", "http://")),
        ("another bucket", swap("/assets/", "/other-bucket/")),
        (
            "`..` into production's silo",
            swap(stg, &format!("~staging/../{app}/")),
        ),
        (
            "an encoded `..` into production's silo",
            swap(stg, &format!("~staging/%2e%2e/{app}/")),
        ),
        ("an encoded slash", swap(stg, "~staging%2f")),
        ("an encoded tilde", swap("~staging", "%7Estaging")),
        ("a parameter added", format!("{url}&x-amz-acl=public-read")),
        ("the signature changed", swap("abc123", "abc124")),
        (
            "the query dropped",
            url.split('?').next().unwrap().to_string(),
        ),
        ("a fragment added", format!("{url}#part")),
    ];
    for (what, other) in differing {
        assert_ne!(other, url, "{what}");
        assert_eq!(
            decide(&policy, "PUT", &other, std::slice::from_ref(&upload)),
            Decision::Hold,
            "{what}: {other}"
        );
    }
}

/// The request is compared as parsed — the value it is sent to. A spelling
/// that parses to the minted URL is the minted URL.
#[test]
fn a_spelling_that_parses_to_the_minted_url_is_the_minted_url() {
    let (url, upload) = own(&staging());
    let respelled = [
        url.replacen("objects.example.com", "OBJECTS.example.com:443", 1),
        url.replacen("/uploads/", "/./uploads/", 1),
        url.replacen("/uploads/", "/x/../uploads/", 1),
    ];
    for spelling in respelled {
        assert_ne!(spelling, url);
        assert_eq!(
            decide(&staging(), "PUT", &spelling, std::slice::from_ref(&upload)),
            SENT,
            "{spelling}"
        );
    }
}

/// Belt and braces: a record whose key is in any silo but this environment's
/// own is never an upload here, even with its URL matched exactly.
#[test]
fn a_minted_record_outside_the_environments_own_silo_is_held() {
    let other_silos = [
        ("production's", Silo::production(app())),
        (
            "another environment's",
            Silo::for_environment(
                app(),
                &AppEnvironment::Dev {
                    handle: "a1".into(),
                },
            ),
        ),
        (
            "another app's",
            Silo::for_environment(Uuid::from_u128(8), &AppEnvironment::Staging),
        ),
    ];
    for (what, silo) in other_silos {
        let key = key_in(&silo);
        let url = path_style(&key);
        assert_eq!(
            decide(&staging(), "PUT", &url, &[minted(&url, &key)]),
            Decision::Hold,
            "{what}"
        );
    }
    // And one sandbox's upload is not another's, nor staging's.
    let (url, upload) = own(&sandbox("a1"));
    for policy in [sandbox("b2"), sandbox("a"), staging()] {
        assert_eq!(
            decide(&policy, "PUT", &url, std::slice::from_ref(&upload)),
            Decision::Hold
        );
    }
}

#[test]
fn production_allows_every_fetch_and_a_preview_holds_every_mutating_one() {
    let (url, upload) = own(&staging());
    let production = EnvPolicy::production();
    for (method, target) in [("PUT", url.as_str()), ("POST", "https://api.example.com/x")] {
        assert_eq!(
            decide(&production, method, target, std::slice::from_ref(&upload)),
            Decision::Allow
        );
        assert_eq!(decide(&production, method, target, &[]), Decision::Allow);
    }
    // A production run reading a branch has no silo of its own: even a record
    // in production's silo, matched exactly, is held.
    let preview = EnvPolicy::production().within_scope_pin(Some(Uuid::from_u128(9)));
    let key = key_in(&Silo::production(app()));
    let own_url = path_style(&key);
    for record in [upload, minted(&own_url, &key)] {
        assert_eq!(
            decide(&preview, "PUT", &url, std::slice::from_ref(&record)),
            Decision::Hold
        );
        assert_eq!(
            decide(&preview, "PUT", &own_url, std::slice::from_ref(&record)),
            Decision::Hold
        );
    }
}

#[test]
fn a_url_that_does_not_parse_is_never_kept() {
    assert_eq!(MintedUpload::new("not a url", "k"), None);
}
