//! `ctx.crypto`: HMAC vectors, junk signatures, webhooks.

use super::*;

/// The published HMAC-SHA256 vector for key "key" over "The quick brown fox
/// jumps over the lazy dog". Using a known-outside vector rather than
/// round-tripping our own output means a wrong implementation cannot agree
/// with itself and pass.
const FOX_HMAC_HEX: &str = "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crypto_hmac_matches_the_published_vector_and_verifies() {
    let (result, _) = run_with_mock(&format!(
            r#"export default async function (req, ctx) {{
                 const key = "key";
                 const data = "The quick brown fox jumps over the lazy dog";
                 return {{ status: 200, body: JSON.stringify({{
                   hex: ctx.crypto.hmac({{ key, data }}),
                   b64: ctx.crypto.hmac({{ key, data, encoding: "base64" }}),
                   good: ctx.crypto.verifyHmac({{ key, data, signature: "{FOX_HMAC_HEX}" }}),
                   tampered: ctx.crypto.verifyHmac({{ key, data, signature: "{tampered}" }}),
                   wrongKey: ctx.crypto.verifyHmac({{ key: "kex", data, signature: "{FOX_HMAC_HEX}" }}),
                 }}) }};
               }}"#,
            FOX_HMAC_HEX = FOX_HMAC_HEX,
            // Same length, one nibble changed — so a length check cannot be
            // what rejects it.
            tampered = format!("e{}", &FOX_HMAC_HEX[1..]),
        ))
        .await;
    let parsed: serde_json::Value =
        serde_json::from_str(&result.expect("handler must complete").body).unwrap();
    assert_eq!(parsed["hex"], FOX_HMAC_HEX);
    // base64 of the SAME published digest, derived from the hex vector
    // independently — not copied from what this code emitted.
    assert_eq!(
        parsed["b64"],
        "97yD9DBThCSxMpjmqm+xQ+9NWaFJRhdZl0edvC0aPNg="
    );
    assert_eq!(parsed["good"], true);
    assert_eq!(parsed["tampered"], false);
    assert_eq!(parsed["wrongKey"], false);
}

/// The security-relevant split: a malformed signature is attacker-controlled
/// and must reject cleanly, while a bad algorithm is an author bug and must
/// throw. Getting this backwards turns a forged request into a 500.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crypto_rejects_junk_signatures_but_throws_on_author_error() {
    let (result, _) = run_with_mock(
        r#"export default async function (req, ctx) {
                 const base = { key: "k", data: "d" };
                 let threw = false;
                 try { ctx.crypto.verifyHmac({ ...base, algorithm: "md5", signature: "aa" }); }
                 catch { threw = true; }
                 return { status: 200, body: JSON.stringify({
                   notHex: ctx.crypto.verifyHmac({ ...base, signature: "zzzz" }),
                   empty: ctx.crypto.verifyHmac({ ...base, signature: "" }),
                   badAlgorithmThrew: threw,
                   eqSame: ctx.crypto.timingSafeEqual("s3cret", "s3cret"),
                   eqDiff: ctx.crypto.timingSafeEqual("s3cret", "s3cres"),
                   eqLen: ctx.crypto.timingSafeEqual("s3cret", "s3cre"),
                 }) };
               }"#,
    )
    .await;
    let parsed: serde_json::Value =
        serde_json::from_str(&result.expect("handler must complete").body).unwrap();
    assert_eq!(
        parsed["notHex"], false,
        "undecodable signature must not throw"
    );
    assert_eq!(parsed["empty"], false);
    assert_eq!(parsed["badAlgorithmThrew"], true);
    assert_eq!(parsed["eqSame"], true);
    assert_eq!(parsed["eqDiff"], false);
    assert_eq!(parsed["eqLen"], false);
}

/// Regression: an unset secret must not authorize. The documented pattern is
/// `timingSafeEqual(req.headers[...], ctx.env.SECRET)`; if the env var was
/// never set and the attacker simply omits the header, both sides coerced to
/// "" and an empty-vs-empty compare returned TRUE. Reported in review.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unset_secret_never_authorizes() {
    let (result, _) = run_with_mock(
            r#"export default async function (req, ctx) {
                 const out = { threw: [] };
                 const t = (name, fn) => { try { out[name] = fn(); } catch { out.threw.push(name); } };

                 // The documented shared-secret pattern, with NOTHING configured
                 // and NOTHING sent. Must not come back true.
                 t("unsetVsMissing", () =>
                   ctx.crypto.timingSafeEqual(req.headers["x-shared-secret"], ctx.env.NOPE));
                 t("bothEmpty", () => ctx.crypto.timingSafeEqual("", ""));
                 t("emptyVsReal", () => ctx.crypto.timingSafeEqual("", "s3cret"));

                 // A missing key must not silently become the literal "undefined",
                 // which is a publicly known key anyone can sign with.
                 t("verifyNoKey", () =>
                   ctx.crypto.verifyHmac({ key: ctx.env.NOPE, data: "d", signature: "aa" }));
                 t("verifyEmptyKey", () =>
                   ctx.crypto.verifyHmac({ key: "", data: "d", signature: "aa" }));
                 t("hmacNoKey", () => ctx.crypto.hmac({ key: ctx.env.NOPE, data: "d" }));

                 return { status: 200, body: JSON.stringify(out) };
               }"#,
        )
        .await;
    let parsed: serde_json::Value =
        serde_json::from_str(&result.expect("handler must complete").body).unwrap();

    assert_eq!(
        parsed["unsetVsMissing"], false,
        "an unset secret + an absent header must NOT authorize"
    );
    assert_eq!(parsed["bothEmpty"], false);
    assert_eq!(parsed["emptyVsReal"], false);

    // A missing/empty key is an author error, so it throws rather than
    // signing with a guessable key.
    let threw: Vec<String> = serde_json::from_value(parsed["threw"].clone()).unwrap();
    for case in ["verifyNoKey", "verifyEmptyKey", "hmacNoKey"] {
        assert!(
            threw.contains(&case.to_string()),
            "{case} must throw: {parsed}"
        );
    }
}

/// The whole point of this and the `req.headers` work together: a GitHub
/// webhook can be verified inside a function, which was impossible before.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_github_style_webhook_can_be_verified_end_to_end() {
    let secret = "It's a Secret to Everybody";
    let body = "Hello, World!";
    let (_cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
    let host = Arc::new(MockHost::default());
    let result = run(
        r#"export default async function (req, ctx) {
                 const header = req.headers["x-hub-signature-256"] || "";
                 const sig = header.replace(/^sha256=/, "");
                 return { status: 200, body: JSON.stringify({
                   ok: ctx.crypto.verifyHmac({
                     key: ctx.env.WEBHOOK_SECRET, data: req.body, signature: sig,
                   }),
                 }) };
               }"#
        .to_string(),
        uuid::Uuid::new_v4(),
        {
            let mut c = test_ctx();
            c.env
                .insert("WEBHOOK_SECRET".to_string(), secret.to_string());
            c
        },
        FnRequest {
            method: "POST".to_string(),
            headers: std::collections::BTreeMap::from([(
                "x-hub-signature-256".to_string(),
                format!(
                    "sha256={}",
                    hex::encode(super::super::ops::hmac_digest("sha256", secret, body).unwrap())
                ),
            )]),
            body: body.as_bytes().to_vec(),
        },
        host.clone(),
        cancel_rx,
        std::time::Duration::from_secs(10),
        Arc::new(std::sync::Mutex::new(Vec::new())),
        InvocationMeters::start(),
        tracing::Span::none(),
    )
    .await
    .expect("handler must complete");
    let parsed: serde_json::Value = serde_json::from_str(&result.body).unwrap();
    assert_eq!(parsed["ok"], true, "a valid GitHub signature must verify");
}
