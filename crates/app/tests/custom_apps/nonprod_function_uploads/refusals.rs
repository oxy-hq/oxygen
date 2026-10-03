//! Every mutating `ctx.fetch` outside production that is **not** this
//! invocation's own upload still answers `409` unsent — one test per way of
//! getting it wrong.
//!
//! Each test's invocation has minted an upload of its own and PUT it (sent):
//! the control that says the refused call beside it was refused for what it
//! named, not because nothing is ever sent. The foreign URLs are correctly
//! signed ones (`fixture::signed_for`) wherever a signer would issue them, so
//! "held" never rests on a bad signature the object store would catch anyway.

use std::sync::Arc;

use oxy_app::server::api::custom_apps_functions::runtime::FunctionHost;
use oxy_app::server::api::custom_apps_storage::Silo;
use oxy_app_core::custom_app_environment::AppEnvironment;
use uuid::Uuid;

use super::fixture::{
    BUCKET, Minted, Rig, STORE_HOST, assert_held, assert_sent, mint, put, sandbox, send, signed_for,
};

/// An invocation in `environment` that has minted an upload and sent it.
async fn uploading(rig: &Rig, environment: AppEnvironment) -> (Arc<dyn FunctionHost>, Minted) {
    let host = rig.invocation(environment);
    let own = mint(&*host, "uploads/report.txt").await;
    assert_sent(&put(&*host, &own.url).await, "the invocation's own upload");
    (host, own)
}

async fn staging() -> (Rig, Arc<dyn FunctionHost>, Minted) {
    let rig = Rig::offline().await;
    let (host, own) = uploading(&rig, AppEnvironment::Staging).await;
    (rig, host, own)
}

#[tokio::test]
async fn a_put_into_productions_silo_is_held() {
    let (rig, host, own) = staging().await;
    let production = signed_for(&Silo::production(rig.app_id), "uploads/report.txt").await;
    assert!(
        production.contains(&format!("/customer-app-storage/{}/uploads/", rig.app_id)),
        "{production}"
    );
    assert_held(
        &put(&*host, &production).await,
        "a URL signed for production's silo",
    );
    let renamed = own.url.replace("~staging/", "/");
    assert_ne!(renamed, own.url);
    assert_held(
        &put(&*host, &renamed).await,
        "its own URL with the environment suffix removed",
    );
}

#[tokio::test]
async fn a_put_into_another_environments_silo_is_held() {
    let (rig, host, own) = staging().await;
    let a1 = Silo::for_environment(rig.app_id, &sandbox("a1"));
    assert_held(
        &put(&*host, &signed_for(&a1, "uploads/report.txt").await).await,
        "staging's PUT to a URL signed for dev-a1's silo",
    );
    assert_held(
        &put(&*host, &own.url.replace("~staging/", "~dev-a1/")).await,
        "its own URL renamed into dev-a1's silo",
    );

    let (sandboxed, _) = uploading(&rig, sandbox("a1")).await;
    assert_held(
        &put(&*sandboxed, &own.url).await,
        "dev-a1's PUT to the URL a staging invocation minted",
    );
}

#[tokio::test]
async fn a_put_into_another_apps_silo_is_held() {
    let (rig, host, own) = staging().await;
    let other_app = Uuid::new_v4();
    let theirs = Silo::for_environment(other_app, &AppEnvironment::Staging);
    assert_held(
        &put(&*host, &signed_for(&theirs, "uploads/report.txt").await).await,
        "a URL signed for another app's staging silo",
    );

    // And the other way: another app's staging invocation, handed this one's.
    let foreign = rig.invocation_of(other_app, AppEnvironment::Staging);
    let their_own = mint(&*foreign, "uploads/report.txt").await;
    assert_sent(&put(&*foreign, &their_own.url).await, "their own upload");
    assert_held(
        &put(&*foreign, &own.url).await,
        "another app's PUT to this app's URL",
    );
}

#[tokio::test]
async fn a_put_to_another_host_with_the_same_path_is_held() {
    let (_rig, host, own) = staging().await;
    let elsewhere = [
        ("another host", "objects.elsewhere.invalid".to_string()),
        (
            "the store's name as a subdomain of another host",
            format!("{STORE_HOST}.elsewhere.invalid"),
        ),
        (
            "the store's name as the userinfo of another host",
            format!("{STORE_HOST}@elsewhere.invalid"),
        ),
        ("another port of the store", format!("{STORE_HOST}:8443")),
    ];
    for (what, authority) in elsewhere {
        let url = own.url.replacen(STORE_HOST, &authority, 1);
        assert_ne!(url, own.url);
        assert_held(&put(&*host, &url).await, what);
    }
}

#[tokio::test]
async fn a_traversal_out_of_the_silo_is_held() {
    let (rig, host, own) = staging().await;
    let app = rig.app_id;
    let out_of_the_silo = [
        ("`..` into production's silo", format!("~staging/../{app}/")),
        (
            "an encoded `..` into production's silo",
            format!("~staging/%2e%2e/{app}/"),
        ),
        (
            "`..` with an encoded slash",
            format!("~staging/..%2f{app}%2f"),
        ),
        (
            "`..` out of the bucket and back",
            format!("~staging/../../{BUCKET}/customer-app-storage/{app}/"),
        ),
    ];
    for (what, replacement) in out_of_the_silo {
        let url = own.url.replacen("~staging/", &replacement, 1);
        assert_ne!(url, own.url);
        assert_held(&put(&*host, &url).await, what);
    }
}

#[tokio::test]
async fn a_put_to_another_bucket_or_with_another_query_is_held() {
    let (_rig, host, own) = staging().await;
    let (path, query) = own.url.split_once('?').expect("a signed query");
    let another_bucket = own
        .url
        .replacen(&format!("/{BUCKET}/"), "/another-bucket/", 1);
    assert_ne!(another_bucket, own.url);
    let tampered = [
        ("another bucket on the same endpoint", another_bucket),
        (
            "a query parameter added",
            format!("{}&x-amz-acl=public-read", own.url),
        ),
        ("the signature dropped", path.to_string()),
        (
            "a second key in the query",
            format!("{path}?key=other&{query}"),
        ),
        ("a fragment added", format!("{}#part", own.url)),
    ];
    for (what, url) in tampered {
        assert_held(&put(&*host, &url).await, what);
    }
}

#[tokio::test]
async fn any_method_but_put_to_the_minted_url_is_held() {
    let (_rig, host, own) = staging().await;
    for method in ["POST", "PATCH", "DELETE"] {
        assert_held(&send(&*host, method, &own.url).await, method);
    }
    // A GET carrying a body is a write (`staging_functions`), minted URL or not.
    assert_held(
        &send(&*host, "GET", &own.url).await,
        "a GET carrying a body",
    );
    assert_sent(
        &put(&*host, &own.url).await,
        "the PUT is still sent after them",
    );
}

/// The rule is per invocation: what one run minted, another run of the same
/// environment cannot send — its host never saw the URL signed.
#[tokio::test]
async fn a_url_an_earlier_invocation_minted_is_held() {
    let (rig, _first, earlier) = staging().await;
    let (second, _) = uploading(&rig, AppEnvironment::Staging).await;
    assert_held(
        &put(&*second, &earlier.url).await,
        "a PUT to a URL another staging invocation minted",
    );
}
