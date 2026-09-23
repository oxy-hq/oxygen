//! A published function that catches a failed `ctx.*` call still pages.
//!
//! A handler that wraps a host call in `try/catch` and answers 200 looks like a
//! success everywhere the caller can see. Before `note_host_call_failure`, its
//! invocation row carried no `failure_fingerprint`, so the ops pager, which
//! groups rows by that column, never saw it. The broker now notes the failure
//! on `ProjectFunctionHost` before the isolate gets the rejection. Finalization
//! then writes a `host_call` fingerprint over `host_call <op> <kind>
//! <normalized message>` — the message so that a new break on an op the
//! function already catches is still new to the pager, normalized so that a
//! fingerprint carries none of the message's data — and, for a `fetch` whose
//! message names no host, ` @ <host>`, so that two vendors' failures in one
//! function are two patterns.
//!
//! The runtime unit tests prove the broker makes that note, over a `MockHost`.
//! These prove the rest of the path: the real host keeps the note, with the
//! message it normalized, and the invocation row a published app writes
//! carries the fingerprint over all three. Both failures are deterministic:
//!
//! - `ctx.fetch` to an `http://` URL is refused by `is_safe_outbound` before
//!   any DNS lookup or connection. Its message contains "blocked", which
//!   classifies as `not_allowed`, a kind that pages.
//! - `ctx.storage.copy` from a key that was never written fails with
//!   `StorageError::NotFound` — "storage object not found: …" — on both asset
//!   stores: the filesystem one (`test_db` points `OXY_STATE_DIR` at a
//!   tempdir), and the object store when `OXY_CUSTOMER_APPS_STORAGE_S3_BUCKET`
//!   is set, where `s3::copy` reads the `NoSuchKey` code off `CopyObject`'s
//!   error. That classifies as `not_found`, ordinary control flow that must not
//!   page. The test runs against whichever store the environment configures —
//!   the filesystem locally, minio in CI — so the exemption is proven on the
//!   store the deployment uses, not only on the one that happens to be simplest.
//!
//! Needs Postgres, like `custom_app_functions_e2e`.

use axum::http::StatusCode;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{
    FunctionSpec, call_function, invocations, publish_app, seeded_tenant,
};

const APP: &str = "fn-host-failures";

/// Falls back when the live rate lookup fails. Only the catch branch can
/// answer `fallback`: the fetch never succeeds, and the try branch would say
/// `live`.
const QUOTE_RATES_JS: &str = r#"
export default async (req, ctx) => {
  try {
    await ctx.fetch("http://rates.example.com/v1/usd");
    return Response.json({ rates: "live" });
  } catch (e) {
    return Response.json({ rates: "fallback", reason: e.message });
  }
};
"#;

/// Copies a report that may not exist yet, and treats its absence as normal.
/// The key is a full silo key, so the test sends the app id it only learns at
/// publish.
const ARCHIVE_REPORT_JS: &str = r#"
export default async (req, ctx) => {
  const { key } = JSON.parse(req.body);
  try {
    await ctx.storage.copy(key, "archive/report.csv");
    return Response.json({ archived: true });
  } catch (e) {
    return Response.json({ archived: false, reason: e.message });
  }
};
"#;

fn functions() -> Vec<FunctionSpec> {
    vec![
        FunctionSpec {
            name: "quote-rates",
            manifest: json!({ "route": true }),
            js: QUOTE_RATES_JS,
        },
        FunctionSpec {
            name: "archive-report",
            manifest: json!({ "route": true, "storage": { "read": true, "write": true } }),
            js: ARCHIVE_REPORT_JS,
        },
    ]
}

/// The message `ProjectFunctionHost::fetch` refuses `quote-rates`' URL with —
/// the text the broker hands `note_host_call_failure`.
const FETCH_REFUSAL: &str = "fetch to 'http://rates.example.com/v1/usd' blocked by SSRF allowlist";
/// `FETCH_REFUSAL` as `failure_signal::normalize` writes it: the quoted URL is
/// one `?`. Written out by hand, so this test computes the fingerprint from
/// the formula rather than from the crate's own function; the unit test
/// `a_locator_the_host_writes_bare_does_not_reach_the_fingerprint_input`
/// pins the same line from the other side.
///
/// No host survives in the message, and that is the rule rather than an
/// oversight. A *bare* URL keeps its host (`for url (api.example.com/?)`); this
/// URL is *quoted*: the host wrote it as a value, the quote rule takes it whole
/// before the run rule sees it, and that rule is `main`'s — changing it would
/// re-fingerprint every stored failure that quotes a URL. The host reaches the
/// fingerprint another way: the broker reads it off the call as the target
/// (`runtime::host_call_target`), and a `fetch` whose message does not name
/// its host carries the target after ` @ ` (`HostCallFailure::fingerprint`),
/// so a refused `rates.example.com` and a refused `other.example.com` in one
/// function are two patterns.
const FETCH_REFUSAL_NORMALIZED: &str = "fetch to ? blocked by SSRF allowlist";

/// The host `quote-rates` fetches, as `url_shape::fetch_target` reads it off
/// the URL and `failure_signal::normalize` keeps it (no digits, so unchanged).
const FETCH_REFUSAL_TARGET: &str = "rates.example.com";

/// The fingerprint `failure_signal::Failure::of` gives a caught host-call
/// failure: 16 hex chars of SHA-256 over `host_call <op> <kind> <normalized
/// message>`, then ` @ <target>` when the fingerprint carries one. Recomputed
/// here because `failure_signal` is a private module of the crate.
fn host_call_fingerprint(
    op: &str,
    kind: &str,
    normalized_message: &str,
    target: Option<&str>,
) -> String {
    let mut input = format!("host_call {op} {kind} {normalized_message}");
    if let Some(target) = target {
        input.push_str(" @ ");
        input.push_str(target);
    }
    let digest = Sha256::digest(input.as_bytes());
    hex::encode(&digest[..8])
}

#[tokio::test]
async fn a_caught_fetch_refusal_answers_200_and_still_writes_a_host_call_fingerprint() {
    let t = seeded_tenant().await;
    let published = publish_app(&t, APP, demo_workspace_id(), &functions()).await;

    let call = call_function(APP, "quote-rates", json!({})).await;

    assert_eq!(call.status, StatusCode::OK, "stream: {}", call.raw);
    let data = call
        .frame("data")
        .unwrap_or_else(|| panic!("the handler answers: {}", call.raw));
    assert_eq!(
        data["rates"], "fallback",
        "only the catch branch answers fallback; stream: {}",
        call.raw
    );
    let reason = data["reason"].as_str().expect("the caught message");
    // The isolate prefixes the surface (`ctx.fetch: …`); the host's own text,
    // which is what the fingerprint normalizes, is the rest.
    assert!(
        reason.ends_with(FETCH_REFUSAL),
        "the fetch failed on the SSRF check, not on the network, and said so in the \
         words the fingerprint below assumes: {reason}"
    );
    assert_eq!(call.frame("done"), Some(&json!({ "status": 200 })));

    let rows = invocations(&t.db, published.app_id, "quote-rates").await;
    let seen: Vec<_> = rows
        .iter()
        .map(|r| (r.mode.as_str(), r.status.as_str(), r.error.as_deref()))
        .collect();
    assert_eq!(
        seen,
        vec![("route", "success", None)],
        "a caught failure is still a successful invocation"
    );
    assert_eq!(
        rows[0].failure_fingerprint.as_deref(),
        Some(
            host_call_fingerprint(
                "fetch",
                "not_allowed",
                FETCH_REFUSAL_NORMALIZED,
                Some(FETCH_REFUSAL_TARGET),
            )
            .as_str()
        ),
        "the caught fetch refusal must leave the fingerprint the pager groups by: op, kind, \
         the normalized message and — since the message names no host — the target"
    );
    assert_ne!(
        rows[0].failure_fingerprint.as_deref(),
        Some(
            host_call_fingerprint("fetch", "not_allowed", FETCH_REFUSAL_NORMALIZED, None).as_str()
        ),
        "the message alone no longer names the failure: a second refused host would be the \
         same pattern"
    );
    assert_ne!(
        rows[0].failure_fingerprint.as_deref(),
        Some(host_call_fingerprint("fetch", "not_allowed", "", None).as_str()),
        "op and kind alone no longer name the failure"
    );
}

#[tokio::test]
async fn a_caught_not_found_host_call_leaves_no_fingerprint() {
    let t = seeded_tenant().await;
    let published = publish_app(&t, APP, demo_workspace_id(), &functions()).await;
    let missing = format!(
        "customer-app-storage/{}/reports/never-written.csv",
        published.app_id
    );

    let call = call_function(APP, "archive-report", json!({ "key": missing })).await;

    assert_eq!(call.status, StatusCode::OK, "stream: {}", call.raw);
    let data = call
        .frame("data")
        .unwrap_or_else(|| panic!("the handler answers: {}", call.raw));
    assert_eq!(data["archived"], false, "stream: {}", call.raw);
    let reason = data["reason"].as_str().expect("the caught message");
    // The call did fail, and in the shape both stores raise for a missing
    // source. Without this, a NULL fingerprint could mean the host was never
    // reached, or that the store failed some other way.
    assert!(
        reason.contains("storage object not found: copy source '")
            && reason.contains("never-written.csv' does not exist"),
        "the copy failed on the missing source: {reason}"
    );

    let rows = invocations(&t.db, published.app_id, "archive-report").await;
    let seen: Vec<_> = rows
        .iter()
        .map(|r| (r.mode.as_str(), r.status.as_str()))
        .collect();
    assert_eq!(seen, vec![("route", "success")]);
    assert_eq!(
        rows[0].failure_fingerprint, None,
        "a not_found host call is control flow and pages nobody"
    );
}
