//! A function that uploads through a presigned URL can be exercised outside
//! production (`internal-docs/customer-apps-functions.md` → Staging).
//!
//! `ctx.storage.getUploadUrl` in staging or a sandbox mints a URL into that
//! environment's own silo, and the function's `ctx.fetch` PUT to it used to be
//! held like any other mutating fetch — so an app that uploads from a function
//! could not be tested there. The policy now sends that one PUT
//! (`EnvPolicy::decide_on_fetch`): to a URL **this invocation's own**
//! `getUploadUrl` minted, into the silo the storage policy isolates. Every
//! other mutating fetch is held exactly as before — `refusals` has one test
//! per way of getting it wrong.
//!
//! - a staging invocation's PUT to the URL it minted is sent, and is not in
//!   its held-write row; the third-party write beside it still is;
//! - a sandbox's is sent too, and a URL another sandbox minted is held;
//! - production sends what it always sent and holds nothing.
//!
//! No V8 and no object store: see `fixture` for why none can answer.

pub(crate) mod fixture;
mod refusals;

use oxy_app_core::custom_app_environment::AppEnvironment;
use serde_json::Value;

use self::fixture::{Rig, STORE_HOST, assert_held, assert_sent, mint, put, sandbox, send};
use crate::custom_app_functions_fixture::seeded_tenant;
use crate::staging_functions::held_rows;

/// `(op, host, method)` of every call a held row lists.
fn held_calls(row: &entity::audit_events::Model) -> Vec<(String, String, String)> {
    let text = |write: &Value, field: &str| write[field].as_str().unwrap_or_default().to_string();
    row.metadata["writes"]
        .as_array()
        .expect("writes")
        .iter()
        .map(|w| (text(w, "op"), text(w, "namespace"), text(w, "verb")))
        .collect()
}

fn fetch_of(host: &str, method: &str) -> (String, String, String) {
    ("fetch".to_string(), host.to_string(), method.to_string())
}

#[tokio::test]
async fn a_staging_put_to_the_url_its_own_invocation_minted_is_sent_and_not_listed_as_held() {
    let t = seeded_tenant().await;
    let rig = Rig::on(&t).await;
    let host = rig.invocation(AppEnvironment::Staging);

    let upload = mint(&*host, "uploads/report.txt").await;
    let silo = format!("customer-app-storage/{}~staging/uploads/", rig.app_id);
    assert!(upload.key.starts_with(&silo), "{}", upload.key);
    assert!(upload.url.contains(&upload.key), "{}", upload.url);

    assert_sent(
        &put(&*host, &upload.url).await,
        "a PUT to the URL this invocation minted",
    );
    assert_sent(&put(&*host, &upload.url).await, "the same PUT, retried");
    assert_held(
        &send(&*host, "POST", "https://api.example.com/orders").await,
        "a write to a third party",
    );

    host.end_of_invocation().await;
    let held = held_rows(&t).await;
    assert_eq!(held.len(), 1, "one held row for the invocation");
    assert_eq!(held[0].environment, "staging");
    assert_eq!(
        held_calls(&held[0]),
        vec![fetch_of("api.example.com", "POST")],
        "the upload is not a held write: only the third-party one is listed, never {STORE_HOST}"
    );
}

#[tokio::test]
async fn a_sandbox_put_to_its_own_minted_url_is_sent_and_another_sandboxes_is_held() {
    let t = seeded_tenant().await;
    let rig = Rig::on(&t).await;
    let (a, b) = (rig.invocation(sandbox("a1")), rig.invocation(sandbox("b2")));

    let in_a = mint(&*a, "uploads/a.txt").await;
    let in_b = mint(&*b, "uploads/b.txt").await;
    let silo = |handle: &str| format!("customer-app-storage/{}~dev-{handle}/", rig.app_id);
    assert!(in_a.key.starts_with(&silo("a1")), "{}", in_a.key);
    assert!(in_b.key.starts_with(&silo("b2")), "{}", in_b.key);

    assert_sent(&put(&*a, &in_a.url).await, "dev-a1's PUT to its own URL");
    assert_sent(&put(&*b, &in_b.url).await, "dev-b2's PUT to its own URL");
    assert_held(&put(&*b, &in_a.url).await, "dev-b2's PUT to dev-a1's URL");

    a.end_of_invocation().await;
    b.end_of_invocation().await;
    let held = held_rows(&t).await;
    assert_eq!(held.len(), 1, "dev-a1 held nothing");
    assert_eq!(held[0].environment, "dev-b2");
    assert_eq!(held_calls(&held[0]), vec![fetch_of(STORE_HOST, "PUT")]);
}

/// Production allows every op, as before environments existed: its own
/// upload, any other PUT, and a write to a third party are all sent, and it
/// writes no held row.
#[tokio::test]
async fn production_sends_every_fetch_as_it_always_did() {
    let t = seeded_tenant().await;
    let rig = Rig::on(&t).await;
    let host = rig.invocation(AppEnvironment::Production);

    let upload = mint(&*host, "uploads/report.txt").await;
    let silo = format!("customer-app-storage/{}/uploads/", rig.app_id);
    assert!(upload.key.starts_with(&silo), "{}", upload.key);

    assert_sent(&put(&*host, &upload.url).await, "production's own upload");
    let elsewhere = format!("https://{STORE_HOST}/another-bucket/key.txt");
    assert_sent(&put(&*host, &elsewhere).await, "a PUT anywhere else");
    assert_sent(
        &send(&*host, "POST", "https://production-probe.invalid/orders").await,
        "a write to a third party",
    );

    host.end_of_invocation().await;
    assert!(held_rows(&t).await.is_empty(), "production holds nothing");
}
