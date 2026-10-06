//! The one body, and who is given it: a sandbox agent token, and nobody else.

use axum::Router;
use axum::http::Request as HttpRequest;
use axum::routing::get;
use oxy_auth::token::StoredKind;
use tower::ServiceExt;
use uuid::Uuid;

use super::*;
use crate::server::api::custom_apps_agent_fixture as fixture;

fn read(status: StatusCode, body: &str) -> Value {
    Refusal::read(status, body.as_bytes()).body()
}

/// What a response carries: status, content type, and its body as sent.
async fn sent(response: Response) -> (StatusCode, Option<String>, Vec<u8>) {
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = axum::body::to_bytes(response.into_body(), MAX_BODY_BYTES)
        .await
        .expect("read the body");
    (status, content_type, bytes.to_vec())
}

fn json_of(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).expect("a JSON body")
}

/// Exactly the three fields: the code under both names, and a sentence.
fn assert_the_shape(body: &Value, code: &str) {
    let fields = body.as_object().expect("an object");
    assert_eq!(fields.len(), 3, "{body}");
    assert_eq!(body["code"], code, "{body}");
    assert_eq!(body["error"], code, "{body}");
    let message = body["message"].as_str().expect("a message");
    assert!(!message.trim().is_empty(), "{body}");
}

#[test]
fn a_refusal_with_no_code_is_named_after_its_status() {
    let cases = [
        (StatusCode::BAD_REQUEST, "bad_request"),
        (StatusCode::FORBIDDEN, "forbidden"),
        (StatusCode::NOT_FOUND, "not_found"),
        (StatusCode::METHOD_NOT_ALLOWED, "method_not_allowed"),
        (StatusCode::CONFLICT, "conflict"),
        (StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large"),
        (StatusCode::UNPROCESSABLE_ENTITY, "unprocessable_entity"),
        (StatusCode::TOO_MANY_REQUESTS, "too_many_requests"),
        (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        (StatusCode::BAD_GATEWAY, "bad_gateway"),
        (StatusCode::SERVICE_UNAVAILABLE, "service_unavailable"),
    ];
    for (status, code) in cases {
        assert_eq!(code_of(status), code, "{status}");
        assert!(is_code(&code_of(status)), "{status}");
        assert_the_shape(&Refusal::of_status(status).body(), code);
    }
}

/// Every shape a route of the loop answers a refusal in today, read into the
/// one shape with the code it already had.
#[test]
fn every_shape_a_route_answers_is_read_into_the_one() {
    let not_found = StatusCode::NOT_FOUND;
    // The console and sandbox routes: `{error: <code>, message}`.
    let console =
        r#"{"error":"environment_not_found","message":"this app has no such environment"}"#;
    let body = read(not_found, console);
    assert_the_shape(&body, "environment_not_found");
    assert_eq!(body["message"], "this app has no such environment");

    // A refused publish and a refused write: already the shape.
    let coded =
        r#"{"code":"sandbox_token_refused","error":"sandbox_token_refused","message":"no"}"#;
    assert_the_shape(&read(StatusCode::FORBIDDEN, coded), "sandbox_token_refused");

    // The token routes: `{code, error: <sentence>}`.
    let token = r#"{"error":"a sandbox agent token is fixed","code":"sandbox_token_fixed"}"#;
    let body = read(StatusCode::CONFLICT, token);
    assert_the_shape(&body, "sandbox_token_fixed");
    assert_eq!(body["message"], "a sandbox agent token is fixed");

    // Logs: `{error: <sentence>}`.
    let body = read(not_found, r#"{"error":"not permitted"}"#);
    assert_the_shape(&body, "not_found");
    assert_eq!(body["message"], "not permitted");

    // Secrets: `<code>: <sentence>`, and a plain sentence.
    let body = read(
        StatusCode::BAD_REQUEST,
        "credential_shaped_value: not a value this token may store",
    );
    assert_the_shape(&body, "credential_shaped_value");
    assert_eq!(body["message"], "not a value this token may store");
    let body = read(StatusCode::BAD_REQUEST, "missing bundle");
    assert_the_shape(&body, "bad_request");
    assert_eq!(body["message"], "missing bundle");

    // A bare status, and a proxy's page: the status alone.
    for nothing in ["", "  \n", "<html><body>502</body></html>", "[1,2]", "7"] {
        let body = read(not_found, nothing);
        assert_the_shape(&body, "not_found");
        assert_eq!(body["message"], NOT_FOUND, "{nothing:?}");
    }
}

#[test]
fn a_sentence_that_opens_with_a_word_and_a_colon_is_not_a_code() {
    for sentence in [
        "invalid bundle: the archive is empty",
        "error: something broke",
        "Multipart: bad boundary",
        "a_b:no space after the colon",
        "under_score: ",
    ] {
        let body = read(StatusCode::BAD_REQUEST, sentence);
        assert_the_shape(&body, "bad_request");
    }
    assert!(!is_code("a") && !is_code("Not_Found") && !is_code("9lives") && !is_code(""));
    assert!(!is_code(&"x".repeat(65)) && is_code(&"x".repeat(64)) && is_code("not_found"));
}

/// A field the route sent beside the three is kept, and a camel-case `error`
/// is a sentence's stand-in, never the code.
#[test]
fn a_routes_own_fields_are_kept() {
    let sent =
        r#"{"error":"exceeds the org's policy","code":"exceeds_policy","max_lifetime_days":30}"#;
    let body = read(StatusCode::BAD_REQUEST, sent);
    assert_eq!(body["code"], "exceeds_policy");
    assert_eq!(body["error"], "exceeds_policy");
    assert_eq!(body["message"], "exceeds the org's policy");
    assert_eq!(body["max_lifetime_days"], 30);

    let camel = r#"{"error":"FunctionNotFound","message":"no function named x"}"#;
    let body = read(StatusCode::NOT_FOUND, camel);
    assert_the_shape(&body, "not_found");
    assert_eq!(body["message"], "no function named x");
}

/// Reading a body that is already the shape changes nothing, so a response
/// shaped on one replica and proxied through another is shaped once.
#[test]
fn reading_the_shape_is_idempotent() {
    for first in [
        Refusal::new(StatusCode::CONFLICT, "token_sandbox_limit", "three already"),
        Refusal::not_found(),
        Refusal::read(StatusCode::BAD_REQUEST, b"missing bundle"),
    ] {
        let again = Refusal::read(first.status, first.body().to_string().as_bytes());
        assert_eq!(again, first);
    }
}

/// A refusal that travels as a route's plain text comes back whole.
#[test]
fn a_refusal_stated_as_text_is_read_back() {
    let refusal = Refusal::new(
        StatusCode::NOT_FOUND,
        "environment_not_found",
        "this app has no environment dev-a",
    );
    let (status, text) = refusal.clone().into_text();
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        text,
        "environment_not_found: this app has no environment dev-a"
    );
    assert_eq!(Refusal::read(status, text.as_bytes()), refusal);
}

#[tokio::test]
async fn a_refusal_answers_json_with_its_status() {
    let refusal = Refusal::new(StatusCode::FORBIDDEN, "sandbox_token_refused", "no");
    let (status, content_type, bytes) = sent(refusal.into_response()).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(content_type.as_deref(), Some("application/json"));
    assert_the_shape(&json_of(&bytes), "sandbox_token_refused");
}

/// A plain-text refusal with headers of its own.
fn text_refusal() -> Response {
    let mut response = (StatusCode::CONFLICT, "still being deleted").into_response();
    let retry = HeaderValue::from_static("30");
    response.headers_mut().insert(header::RETRY_AFTER, retry);
    response
}

#[tokio::test]
async fn shaping_keeps_the_status_and_the_routes_headers() {
    let response = shaped(text_refusal()).await;
    assert_eq!(response.headers()[header::RETRY_AFTER], "30");
    let length = response.headers()[header::CONTENT_LENGTH].clone();
    let (status, content_type, bytes) = sent(response).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(content_type.as_deref(), Some("application/json"));
    assert_eq!(length, bytes.len().to_string().as_str());
    let body = json_of(&bytes);
    assert_the_shape(&body, "conflict");
    assert_eq!(body["message"], "still being deleted");
}

/// A success, a redirect and a `401` are not refusals of the token.
#[tokio::test]
async fn what_is_not_a_refusal_is_left_as_it_is() {
    for status in [
        StatusCode::OK,
        StatusCode::NO_CONTENT,
        StatusCode::SEE_OTHER,
        StatusCode::UNAUTHORIZED,
    ] {
        let untouched = shaped((status, "as it was").into_response()).await;
        let (after, content_type, bytes) = sent(untouched).await;
        assert_eq!(after, status);
        assert!(content_type.is_some_and(|t| t.starts_with("text/plain")));
        assert_eq!(bytes, b"as it was", "{status}");
    }
}

/// A body too long to be a refusal is answered by its status.
#[tokio::test]
async fn an_oversized_body_is_answered_by_its_status() {
    let long = "x".repeat(MAX_BODY_BYTES + 1);
    let (status, _, bytes) =
        sent(shaped((StatusCode::BAD_REQUEST, long).into_response()).await).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_the_shape(&json_of(&bytes), "bad_request");
}

// ── The layer: who is given the shape ───────────────────────────────────────

/// A route that refuses in each of the three shapes the loop's routes use.
fn routes() -> Router {
    let text = || async { (StatusCode::NOT_FOUND, "this app has no environment dev-a") };
    let coded = || async {
        let body = json!({ "error": "environment_not_found", "message": "no such environment" });
        (StatusCode::NOT_FOUND, Json(body))
    };
    Router::new()
        .route("/text", get(text))
        .route("/coded", get(coded))
        .route("/bare", get(|| async { StatusCode::NOT_FOUND }))
        .route("/fine", get(|| async { Json(json!({ "ok": true })) }))
}

fn token(kind: StoredKind) -> CredentialContext {
    let mut credential =
        fixture::credential(Uuid::from_u128(0x70), Uuid::nil(), Uuid::nil(), Uuid::nil());
    credential.kind = kind;
    credential
}

/// `GET path` through the layer, authenticated with `credential`.
async fn through_the_layer(
    path: &str,
    credential: Option<CredentialContext>,
) -> (StatusCode, Option<String>, Vec<u8>) {
    let mut request = HttpRequest::get(path).body(Body::empty()).expect("request");
    if let Some(credential) = credential {
        request.extensions_mut().insert(credential);
    }
    let layered = routes().layer(axum::middleware::from_fn(shape_for_agent));
    sent(layered.oneshot(request).await.expect("oneshot")).await
}

/// The same request with no layer at all: what the route itself answers.
async fn unlayered(path: &str) -> (StatusCode, Option<String>, Vec<u8>) {
    let request = HttpRequest::get(path).body(Body::empty()).expect("request");
    sent(routes().oneshot(request).await.expect("oneshot")).await
}

#[tokio::test]
async fn a_sandbox_agent_token_is_answered_in_the_one_shape() {
    let agent = || Some(token(StoredKind::SandboxAgent));
    for (path, code, message) in [
        ("/text", "not_found", "this app has no environment dev-a"),
        ("/coded", "environment_not_found", "no such environment"),
        ("/bare", "not_found", NOT_FOUND),
    ] {
        let (status, content_type, bytes) = through_the_layer(path, agent()).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        assert_eq!(content_type.as_deref(), Some("application/json"), "{path}");
        let body = json_of(&bytes);
        assert_the_shape(&body, code);
        assert_eq!(body["message"], message, "{path}");
    }
    // What is not a refusal is the route's own answer.
    let (status, _, bytes) = through_the_layer("/fine", agent()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_of(&bytes), json!({ "ok": true }));
}

/// A session, a request nothing authenticated, and every other kind of key
/// or token get the route's own answer, byte for byte.
#[tokio::test]
async fn no_other_credential_is_given_the_shape() {
    // `None` is every request with no `CredentialContext`: a browser session,
    // an `oxypublish_` token (it carries a marker, not a credential), nobody.
    let mut callers = vec![None];
    for kind in [
        StoredKind::Personal,
        StoredKind::LegacyKey,
        StoredKind::ServiceAccount,
        StoredKind::Ci,
    ] {
        callers.push(Some(token(kind)));
    }
    for path in ["/text", "/coded", "/bare", "/fine"] {
        let as_routed = unlayered(path).await;
        for credential in callers.clone() {
            let kind = credential.as_ref().map(|credential| credential.kind);
            let answered = through_the_layer(path, credential).await;
            assert_eq!(answered, as_routed, "{path} as {kind:?}");
        }
    }
}

/// A route that authenticates in its handler: the shape goes to a request
/// that presents the token and was not answered `401`, and to nobody else.
#[test]
fn a_handler_that_authenticates_itself_knows_the_token_by_what_it_presents() {
    let presenting = |value: &str| {
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, value.parse().expect("a header"));
        headers
    };
    let agent = presenting(&format!(
        "Bearer {}",
        oxy_auth::token::generate_sandbox_agent().plaintext
    ));
    let personal = presenting(&format!(
        "Bearer {}",
        oxy_auth::token::generate_personal().plaintext
    ));
    let not_found = StatusCode::NOT_FOUND;
    assert!(answered_to_agent(&agent, not_found));
    assert!(!answered_to_agent(&agent, StatusCode::UNAUTHORIZED));
    assert!(!answered_to_agent(&personal, not_found));
    assert!(!answered_to_agent(&HeaderMap::new(), not_found));
}

#[tokio::test]
async fn a_bare_status_is_shaped_for_the_token_alone() {
    let mut agent = HeaderMap::new();
    let bearer = format!(
        "Bearer {}",
        oxy_auth::token::generate_sandbox_agent().plaintext
    );
    agent.insert(header::AUTHORIZATION, bearer.parse().expect("a header"));

    let (status, content_type, bytes) = sent(status_response(&agent, StatusCode::NOT_FOUND)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(content_type.as_deref(), Some("application/json"));
    // The same bytes as the fence and the gate answer: one `404`, not three.
    let fence = sent(Refusal::not_found().into_response()).await;
    assert_eq!((status, content_type, bytes.clone()), fence);
    assert_the_shape(&json_of(&bytes), "not_found");

    for (headers, status) in [
        (HeaderMap::new(), StatusCode::NOT_FOUND),
        (agent, StatusCode::UNAUTHORIZED),
    ] {
        let (after, content_type, bytes) = sent(status_response(&headers, status)).await;
        assert_eq!((after, content_type, bytes), (status, None, Vec::new()));
    }
}
