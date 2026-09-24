//! Custom-app webhook receiver — an unauthenticated POST from a third party,
//! verified by the platform, dispatched to an Oxy Function.
//!
//! ## Why the platform verifies, not the function
//!
//! A function *could*: `x-*` headers reach app code, and `req.body` is the raw
//! request bytes (`String::from_utf8_lossy`, no parse/re-serialize), so an HMAC
//! computed in JS would match. But then an unverified request has already
//! reached app code, and a forgotten check is silent. Verifying here means it
//! cannot be forgotten — the same call `webhooks::toast` makes.
//!
//! ## Fail closed, twice
//!
//! No `webhook:` block in the manifest → **404**, as if the endpoint did not
//! exist. No resolvable secret → **401**. The first distinction matters because
//! this route is anonymous: answering "wrong signature" for an undeclared
//! function would turn it into a directory of every function an app has.
//!
//! ## Why it enqueues rather than runs
//!
//! Providers retry on non-2xx — Uber ~7 times with exponential backoff — so the
//! answer has to be fast and must not depend on the work succeeding. The job is
//! durable (`trigger_function_job`), so it survives instance death and inherits
//! the queue's retry, and the caller gets `202` as soon as the sender is proven.
//!
//! ## What the function can and cannot do
//!
//! It runs the SYSTEM path: no invoking user, so no workspace role, so an
//! `airhouse_managed` credential is minted **Reader** (see
//! `custom_apps_functions::should_resolve_role`). A webhook function therefore
//! cannot write airhouse — but it CAN write `ctx.storage`, which is app-scoped
//! and consults no role. Land the payload there and let an Automation, which
//! does resolve a role, do the warehouse write.

use axum::body::Bytes;
use axum::extract::Path;
use axum::http::{HeaderMap, StatusCode};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

use crate::server::api::custom_apps_functions::function_webhook_contract;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};

type HmacSha256 = Hmac<Sha256>;

/// `POST /api/webhooks/apps/{org_slug}/{app_slug}/{function_name}`
#[tracing::instrument(skip_all, fields(org = %org_slug, app = %app_slug, function = %function_name))]
pub async fn app_function_webhook(
    Path((org_slug, app_slug, function_name)): Path<(String, String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, (StatusCode, String)> {
    let db = oxy::database::client::establish_connection()
        .await
        .map_err(|e| {
            tracing::error!("db connection failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "database unavailable".to_string(),
            )
        })?;

    let (app, (secret_var, signature_header, encoding)) =
        resolve_webhook_target(&db, &org_slug, &app_slug, &function_name).await?;

    // Past this point the endpoint is admitted to exist, so failures are 401 —
    // the caller has proven nothing yet.
    let keys = resolve_signing_keys(&app, &secret_var).await?;
    verify_signature(&headers, &body, &signature_header, &encoding, &keys)?;

    enqueue(&db, &app, &function_name, &body).await
}

/// The signing key(s) a declared `secretVar` resolves to.
///
/// A declared-but-unresolvable secret is the dangerous case — an anonymous route
/// with nothing to verify against — so it is a 401, never an open endpoint. A
/// secret that exists but is blank counts as unresolvable for the same reason:
/// an empty key would otherwise be a *valid* HMAC key that any caller could
/// compute with.
async fn resolve_signing_keys(
    app: &entity::apps::Model,
    secret_var: &str,
) -> Result<String, (StatusCode, String)> {
    resolve_app_secret(app.project_id, app.id, secret_var)
        .await
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            tracing::warn!(
                app = %app.id, secret = %secret_var,
                "rejecting app webhook: declared secret does not resolve",
            );
            (
                StatusCode::UNAUTHORIZED,
                "webhook signing secret is not configured".to_string(),
            )
        })
}

/// Hand the verified body to the function as a durable job and answer at once.
///
/// The enqueue failing is OUR fault, not the sender's, so it is a 5xx — which is
/// also what makes the provider retry, and a retry is exactly the recovery we
/// want for a job we failed to record.
async fn enqueue(
    db: &DatabaseConnection,
    app: &entity::apps::Model,
    function_name: &str,
    body: &[u8],
) -> Result<StatusCode, (StatusCode, String)> {
    // Not every provider sends JSON. A non-JSON body still reaches the function,
    // as a string, rather than being dropped for failing to parse something
    // nobody promised.
    let payload = serde_json::from_slice::<serde_json::Value>(body)
        .unwrap_or_else(|_| serde_json::Value::String(String::from_utf8_lossy(body).into_owned()));

    match crate::server::api::custom_apps_functions::trigger_function_job(
        db,
        app.id,
        function_name,
        Some(payload),
        crate::server::api::custom_apps_functions::FunctionJobTrigger::Webhook,
    )
    .await
    {
        Ok(run_id) => {
            tracing::info!(app = %app.id, function = %function_name, run = %run_id, "app webhook accepted");
            Ok(StatusCode::ACCEPTED)
        }
        Err(e) => {
            tracing::error!("app webhook enqueue failed: {e}");
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not enqueue the webhook job".to_string(),
            ))
        }
    }
}

/// The app and the webhook contract its published manifest declares.
///
/// Every failure here is the SAME 404. Resolving the org, the app, the published
/// build, the function and its `webhook:` block are all "does this endpoint
/// exist" questions, and to a stranger the honest answer to all of them is no —
/// distinguishing them would make the route a directory of every function an app
/// has, and of which apps exist at all.
async fn resolve_webhook_target(
    db: &DatabaseConnection,
    org_slug: &str,
    app_slug: &str,
    function_name: &str,
) -> Result<(entity::apps::Model, (String, String, String)), (StatusCode, String)> {
    let not_found = || (StatusCode::NOT_FOUND, "no such webhook".to_string());

    let app = resolve_app(db, org_slug, app_slug)
        .await
        .ok_or_else(not_found)?;
    let manifest = function_manifest(db, &app, function_name)
        .await
        .ok_or_else(not_found)?;
    let contract = function_webhook_contract(&manifest).ok_or_else(not_found)?;
    Ok((app, contract))
}

/// HMAC-SHA256 over the RAW body, compared against every configured key.
///
/// Comma-separated keys are a rotation pair, not a list of tenants: providers
/// that issue two live signing keys (Uber's `BASIC_HMAC` does) sign with either
/// during a rotation, so accepting only one drops half the events for as long as
/// the overlap lasts.
///
/// The comparison is `Mac::verify_slice`, which is constant-time. A byte-wise
/// `==` on a signature leaks its prefix through timing.
fn verify_signature(
    headers: &HeaderMap,
    body: &[u8],
    signature_header: &str,
    encoding: &str,
    keys: &str,
) -> Result<(), (StatusCode, String)> {
    let provided = headers
        .get(signature_header)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .ok_or((
            StatusCode::UNAUTHORIZED,
            format!("missing `{signature_header}` header"),
        ))?;

    let provided_bytes = match encoding {
        // Uber sends LOWERCASE hex; accept either case rather than reject a
        // provider for shouting.
        "hex" => hex::decode(provided.to_ascii_lowercase())
            .map_err(|_| (StatusCode::UNAUTHORIZED, "signature is not hex".to_string()))?,
        "base64" => BASE64.decode(provided).map_err(|_| {
            (
                StatusCode::UNAUTHORIZED,
                "signature is not base64".to_string(),
            )
        })?,
        other => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("unknown webhook signature encoding '{other}'"),
            ));
        }
    };

    for key in keys.split(',').map(str::trim).filter(|k| !k.is_empty()) {
        let mut mac = match HmacSha256::new_from_slice(key.as_bytes()) {
            Ok(m) => m,
            Err(_) => continue,
        };
        mac.update(body);
        if mac.verify_slice(&provided_bytes).is_ok() {
            return Ok(());
        }
    }
    Err((StatusCode::UNAUTHORIZED, "signature mismatch".to_string()))
}

/// The app behind `(org_slug, app_slug)`, or `None`.
async fn resolve_app(
    db: &DatabaseConnection,
    org_slug: &str,
    app_slug: &str,
) -> Option<entity::apps::Model> {
    let org = entity::organizations::Entity::find()
        .filter(entity::organizations::Column::Slug.eq(org_slug))
        .one(db)
        .await
        .ok()??;
    entity::apps::Entity::find()
        .filter(entity::apps::Column::OrgId.eq(org.id))
        .filter(entity::apps::Column::Slug.eq(app_slug))
        .one(db)
        .await
        .ok()?
}

/// A function's `manifest_json` from the app's PUBLISHED build.
///
/// Published only — never the draft. A draft is unreviewed by definition, and
/// this route is reachable by anyone on the internet: letting a draft declare a
/// webhook would make "publish" stop being the thing that decides what the
/// world can reach.
async fn function_manifest(
    db: &DatabaseConnection,
    app: &entity::apps::Model,
    function_name: &str,
) -> Option<serde_json::Value> {
    let build_id = app.published_build_id?;
    entity::app_functions::Entity::find()
        .filter(entity::app_functions::Column::BuildId.eq(build_id))
        .filter(entity::app_functions::Column::Name.eq(function_name))
        .one(db)
        .await
        .ok()?
        .and_then(|f| f.manifest_json)
}

/// One app-scoped secret, by the bare key a manifest names.
///
/// Same namespace `ctx.env` reads (`apps/<app_id>/<KEY>`), so an author sets the
/// signing key exactly as they set every other app secret, and a manifest cannot
/// reach the project's secrets or another app's.
async fn resolve_app_secret(
    project_id: uuid::Uuid,
    app_id: uuid::Uuid,
    key: &str,
) -> Option<String> {
    use oxy::service::secret_manager::SecretManagerService;
    SecretManagerService::new(project_id)
        .get_secret(&format!("apps/{app_id}/{key}"))
        .await
}

#[cfg(test)]
mod tests {
    use super::{HmacSha256, verify_signature};
    use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD as BASE64;
    use hmac::{KeyInit, Mac};

    const HEADER: &str = "x-uber-signature";

    /// The digest a provider would send: HMAC-SHA256 over the raw body.
    fn digest(key: &str, body: &[u8]) -> Vec<u8> {
        let mut mac = HmacSha256::new_from_slice(key.as_bytes()).unwrap();
        mac.update(body);
        mac.finalize().into_bytes().to_vec()
    }

    fn signed_hex(key: &str, body: &[u8]) -> String {
        hex::encode(digest(key, body))
    }

    fn headers(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(
            HeaderName::from_static(HEADER),
            HeaderValue::from_str(value).unwrap(),
        );
        h
    }

    fn status(r: Result<(), (StatusCode, String)>) -> StatusCode {
        r.expect_err("expected a rejection").0
    }

    #[test]
    fn accepts_a_correct_hex_signature() {
        let body = br#"{"event":"eats.report.success"}"#;
        let h = headers(&signed_hex("k1", body));
        assert!(verify_signature(&h, body, HEADER, "hex", "k1").is_ok());
    }

    /// The rotation case this whole comma-separated shape exists for: a provider
    /// mid-rotation signs with EITHER live key, so a receiver that accepts only
    /// the first drops half the events for the length of the overlap.
    #[test]
    fn accepts_either_key_of_a_rotation_pair() {
        let body = b"payload";
        for key in ["old-key", "new-key"] {
            let h = headers(&signed_hex(key, body));
            assert!(
                verify_signature(&h, body, HEADER, "hex", "old-key, new-key").is_ok(),
                "rotation pair rejected a signature made with {key}"
            );
        }
    }

    #[test]
    fn rejects_a_key_outside_the_rotation_pair() {
        let body = b"payload";
        let h = headers(&signed_hex("retired-key", body));
        assert_eq!(
            status(verify_signature(&h, body, HEADER, "hex", "old-key,new-key")),
            StatusCode::UNAUTHORIZED
        );
    }

    /// The forgery case: right key, but the body was altered in flight. The MAC
    /// is over the RAW bytes, so any mutation must fail — this is what stops a
    /// replayed-envelope-with-swapped-payload attack.
    #[test]
    fn rejects_a_tampered_body() {
        let signed = b"{\"amount\":1}";
        let h = headers(&signed_hex("k1", signed));
        let tampered = b"{\"amount\":9}";
        assert_eq!(
            status(verify_signature(&h, tampered, HEADER, "hex", "k1")),
            StatusCode::UNAUTHORIZED
        );
    }

    /// A truncated signature must NOT pass on its matching prefix. `verify_slice`
    /// compares length first; a hand-rolled `starts_with` would accept this and
    /// reduce the forgery cost to brute-forcing one byte.
    #[test]
    fn rejects_a_truncated_signature() {
        let body = b"payload";
        let full = signed_hex("k1", body);
        let h = headers(&full[..8]);
        assert_eq!(
            status(verify_signature(&h, body, HEADER, "hex", "k1")),
            StatusCode::UNAUTHORIZED
        );
    }

    #[test]
    fn accepts_uppercase_hex() {
        let body = b"payload";
        let h = headers(&signed_hex("k1", body).to_ascii_uppercase());
        assert!(verify_signature(&h, body, HEADER, "hex", "k1").is_ok());
    }

    #[test]
    fn accepts_base64_when_the_manifest_says_so() {
        let body = b"payload";
        let h = headers(&BASE64.encode(digest("k1", body)));
        assert!(verify_signature(&h, body, HEADER, "base64", "k1").is_ok());
    }

    /// Encoding is not sniffed: a base64 digest presented to a `hex` contract is
    /// a mismatch, not a lucky parse.
    #[test]
    fn does_not_sniff_the_encoding() {
        let body = b"payload";
        let h = headers(&BASE64.encode(digest("k1", body)));
        assert_eq!(
            status(verify_signature(&h, body, HEADER, "hex", "k1")),
            StatusCode::UNAUTHORIZED
        );
    }

    #[test]
    fn rejects_a_missing_signature_header() {
        assert_eq!(
            status(verify_signature(
                &HeaderMap::new(),
                b"payload",
                HEADER,
                "hex",
                "k1"
            )),
            StatusCode::UNAUTHORIZED
        );
    }

    #[test]
    fn rejects_an_undecodable_signature() {
        let h = headers("not-a-digest");
        assert_eq!(
            status(verify_signature(&h, b"payload", HEADER, "hex", "k1")),
            StatusCode::UNAUTHORIZED
        );
    }

    /// A signature is trimmed, not rejected, for the whitespace a proxy adds.
    #[test]
    fn tolerates_surrounding_whitespace() {
        let body = b"payload";
        let h = headers(&format!("  {}  ", signed_hex("k1", body)));
        assert!(verify_signature(&h, body, HEADER, "hex", "k1").is_ok());
    }

    /// A secret of only separators leaves nothing to verify against, so it must
    /// reject rather than fall out of the loop as success.
    #[test]
    fn rejects_when_the_secret_holds_no_usable_key() {
        let body = b"payload";
        let h = headers(&signed_hex("k1", body));
        assert_eq!(
            status(verify_signature(&h, body, HEADER, "hex", " , ,")),
            StatusCode::UNAUTHORIZED
        );
    }

    /// An unknown encoding is OUR misconfiguration, not the caller's failure to
    /// authenticate — answering 401 would send a provider into a retry loop over
    /// a manifest typo it can do nothing about.
    /// ⚠️ CROSS-REPO CONTRACT — an external uptime monitor asserts on this exact
    /// body, so it is not free text.
    ///
    /// `allquiet/uptime/monitors.tf` in oxy-hq/infrastructure probes a real
    /// custom app by POSTing a DELIBERATELY WRONG signature and asserting
    /// `401` + this string. That is the whole design: reaching "signature
    /// mismatch" means the request got past `resolve_webhook_target` (so
    /// Postgres answered org → app → published build → the declared webhook)
    /// and past `resolve_signing_keys` (so the secret manager read a non-empty
    /// key), then actually ran the HMAC. It proves more resolution than a 202
    /// does, and — the point — it needs no valid signature, so no signing
    /// material has to live in a monitoring vendor, in SSM, or in a
    /// `TF_VAR_*`, and rotating the app's secret cannot break the probe.
    ///
    /// The other 401 bodies are what make it specific: "webhook signing secret
    /// is not configured" means the secret vanished, and "signature is not hex"
    /// means the monitor's header is malformed. Both are real failures the
    /// monitor should catch, and it only catches them because this string is
    /// distinct from them. Changing any of these three words changes what that
    /// monitor means; change them together.
    #[test]
    fn a_wrong_signature_reports_mismatch_verbatim() {
        let body = b"payload";
        // Valid lowercase hex, 32 bytes — the right SHAPE, the wrong value. A
        // malformed header would stop earlier at "signature is not hex" and the
        // monitor would be asserting the wrong thing entirely.
        let wrong = "0".repeat(64);
        let err = verify_signature(&headers(&wrong), body, HEADER, "hex", "the-real-key")
            .expect_err("a zero signature must not verify");

        assert_eq!(err.0, StatusCode::UNAUTHORIZED);
        assert_eq!(
            err.1, "signature mismatch",
            "an external monitor asserts this string; see the doc comment above"
        );
    }

    #[test]
    fn an_unknown_encoding_is_a_server_error_not_a_401() {
        let body = b"payload";
        let h = headers(&signed_hex("k1", body));
        assert_eq!(
            status(verify_signature(&h, body, HEADER, "base32", "k1")),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}
