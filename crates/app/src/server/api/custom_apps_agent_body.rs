//! The one body a **sandbox agent token** (`oxy_sbx_`) is refused with.
//!
//! Whatever route refuses the token, the answer is `application/json`:
//!
//! ```json
//! { "code": "<stable code>", "error": "<the same code>", "message": "<one sentence>" }
//! ```
//!
//! `error` repeats `code` for a client that reads that field. The status is
//! the route's own. A field the route's JSON carried beside these is kept.
//!
//! ## Why it is given here, after the route has answered
//!
//! The token's routes are shared with every other credential, and each family
//! refuses in its own shape: `{error, message}` on the console and sandbox
//! routes, plain text on secrets and publish, a bare status elsewhere. Those
//! shapes are other clients' contracts, and they do not change. So the token's
//! shape is given in one place, to the token alone:
//!
//! * [`shape_for_agent`] is a layer of the `/api` stack, inside
//!   authentication. It does nothing unless the request's credential is a
//!   sandbox agent token, so every other credential's response passes through
//!   untouched, byte for byte.
//! * [`shaped`] is the same step for `/logs`, which authenticates in its
//!   handler.
//! * [`Refusal`] is answered directly where the token is refused before a
//!   route answers: the serve tree's fence, and `/fn` up to its gate.
//!
//! ## Codes
//!
//! A code the route gave is kept: `environment_not_found`,
//! `token_sandbox_limit`, `sandbox_token_refused`, `credential_shaped_value`
//! and the rest. A refusal that carries none is named after its status
//! ([`code_of`]): `not_found`, `bad_request`, `conflict`.
//!
//! A `404` never says whether the thing exists. Where the app is named by id,
//! the route allow-list answers `not_found` for an id the token is not
//! granted and for one that does not exist alike. Where it is named by slugs
//! (`/fn`, `/logs`), every `404` is `not_found`. A sandbox that is not the
//! token's own is `environment_not_found`, as a missing one is.
//!
//! ## What is not shaped
//!
//! * A `401`. The request has no valid credential, so there is no token to
//!   answer in a shape: it is the status every credential gets.
//! * What `/fn` answers once the call is admitted into the token's own
//!   sandbox. That is the function-call contract, the same for every caller:
//!   `{error, message}` under a PascalCase name for a missing function or a
//!   sandbox with no build, and the function's own event stream.

use std::borrow::Cow;

use axum::Json;
use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use oxy_auth::token::CredentialContext;
use serde_json::{Map, Value, json};

/// The most of a refused response that is read back. A refusal is a sentence;
/// a longer body is answered by its status alone.
const MAX_BODY_BYTES: usize = 64 * 1024;

/// What a `404` with no sentence of its own says: the same whether the thing
/// is missing or outside the token's reach.
const NOT_FOUND: &str = "not found, or outside what this token reaches";

/// One refusal of a sandbox agent token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    status: StatusCode,
    code: Cow<'static, str>,
    message: String,
    /// What the route's own JSON carried beside the three fields.
    extra: Map<String, Value>,
}

/// The code of a refusal the route gave none: its status's reason phrase in
/// snake case (`not_found`, `bad_request`, `unprocessable_entity`), and
/// `internal` for a `500`, the code the coded routes already answer.
pub fn code_of(status: StatusCode) -> Cow<'static, str> {
    if status == StatusCode::INTERNAL_SERVER_ERROR {
        return Cow::Borrowed("internal");
    }
    match status.canonical_reason() {
        Some(reason) => Cow::Owned(snake_case(reason)),
        None => Cow::Owned(format!("http_{}", status.as_u16())),
    }
}

fn snake_case(reason: &str) -> String {
    let lower = |c: char| {
        if c.is_ascii_alphanumeric() {
            c.to_ascii_lowercase()
        } else {
            '_'
        }
    };
    reason.chars().map(lower).collect()
}

/// The sentence of a refusal the route gave none.
fn sentence_of(status: StatusCode) -> String {
    if status == StatusCode::NOT_FOUND {
        return NOT_FOUND.to_string();
    }
    status
        .canonical_reason()
        .map_or_else(|| "request refused".to_string(), str::to_ascii_lowercase)
}

/// Whether `value` is a machine code: snake case, 2 to 64 characters. The
/// rule `oxyc` reads a code by (`sdk/cli/src/apps/sandbox-token.ts`).
fn is_code(value: &str) -> bool {
    let mut chars = value.chars();
    let lead = chars.next().is_some_and(|c| c.is_ascii_lowercase());
    let rest = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_';
    lead && (2..=64).contains(&value.len()) && chars.all(rest)
}

/// A non-empty string field of a JSON refusal.
fn text_of(fields: &Map<String, Value>, key: &str) -> Option<String> {
    let text = fields.get(key)?.as_str()?.trim();
    (!text.is_empty()).then(|| text.to_string())
}

impl Refusal {
    /// A refusal with a code of its own.
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code: Cow::Borrowed(code),
            message: message.into(),
            extra: Map::new(),
        }
    }

    /// A refusal that says no more than its status.
    pub fn of_status(status: StatusCode) -> Self {
        Self::said(status, None, None)
    }

    /// `404 not_found`: missing, or outside the token's reach.
    pub fn not_found() -> Self {
        Self::of_status(StatusCode::NOT_FOUND)
    }

    fn said(status: StatusCode, code: Option<String>, message: Option<String>) -> Self {
        Self {
            status,
            code: code.map_or_else(|| code_of(status), Cow::Owned),
            message: message.unwrap_or_else(|| sentence_of(status)),
            extra: Map::new(),
        }
    }

    /// The refusal as a route whose errors are plain text states it:
    /// `<code>: <message>`. [`Self::read`] reads it back.
    pub fn into_text(self) -> (StatusCode, String) {
        (self.status, format!("{}: {}", self.code, self.message))
    }

    /// What a route answered with `status` and `body`, as a refusal.
    ///
    /// * A JSON object: its `code`, or its `error` when that is a code; its
    ///   `message`, or its `error` when that is a sentence; its other fields.
    /// * Text: `<code>: <sentence>` gives both, anything else is the sentence.
    /// * Nothing, or a page of HTML: the status alone.
    pub fn read(status: StatusCode, body: &[u8]) -> Self {
        let text = String::from_utf8_lossy(body);
        let text = text.trim();
        match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(fields)) => Self::from_json(status, fields),
            Ok(Value::String(sentence)) => Self::from_text(status, sentence.trim()),
            Ok(_) => Self::of_status(status),
            Err(_) => Self::from_text(status, text),
        }
    }

    fn from_json(status: StatusCode, mut fields: Map<String, Value>) -> Self {
        let named = text_of(&fields, "code").filter(|code| is_code(code));
        let error = text_of(&fields, "error");
        let code = named.or_else(|| error.clone().filter(|error| is_code(error)));
        let sentence = error.filter(|error| Some(error) != code.as_ref());
        let message = text_of(&fields, "message").or(sentence);
        for key in ["code", "error", "message"] {
            fields.remove(key);
        }
        Self {
            extra: fields,
            ..Self::said(status, code, message)
        }
    }

    fn from_text(status: StatusCode, text: &str) -> Self {
        if text.is_empty() || text.starts_with('<') {
            return Self::of_status(status);
        }
        // `<code>: <sentence>`. An underscore is asked of the code so that an
        // ordinary sentence opening with a word and a colon is not one.
        match text.split_once(": ") {
            Some((code, sentence))
                if is_code(code) && code.contains('_') && !sentence.trim().is_empty() =>
            {
                let sentence = sentence.trim().to_string();
                Self::said(status, Some(code.to_string()), Some(sentence))
            }
            _ => Self::said(status, None, Some(text.to_string())),
        }
    }

    /// The JSON body: the three fields, over whatever else the route sent.
    fn body(&self) -> Value {
        let mut body = self.extra.clone();
        body.insert("code".into(), json!(self.code));
        body.insert("error".into(), json!(self.code));
        body.insert("message".into(), json!(self.message));
        Value::Object(body)
    }
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        (self.status, Json(self.body())).into_response()
    }
}

/// `response` in the one shape, when it is a refusal. A success, a redirect
/// and a `401` are returned as they are. The status and every header but the
/// body's own type and length are kept.
pub async fn shaped(response: Response) -> Response {
    let status = response.status();
    let refused = status.is_client_error() || status.is_server_error();
    if !refused || status == StatusCode::UNAUTHORIZED {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let refusal = match axum::body::to_bytes(body, MAX_BODY_BYTES).await {
        Ok(bytes) => Refusal::read(status, &bytes),
        Err(_) => Refusal::of_status(status),
    };
    let body = refusal.body().to_string().into_bytes();
    let json = HeaderValue::from_static("application/json");
    parts.headers.insert(header::CONTENT_TYPE, json);
    parts
        .headers
        .insert(header::CONTENT_LENGTH, HeaderValue::from(body.len()));
    Response::from_parts(parts, Body::from(body))
}

/// The `/api` layer: every refusal answered to a request that authenticated
/// with a sandbox agent token is given the one shape. Mounted inside
/// authentication, which is what attaches the credential it reads; for any
/// other credential, and for none, the response is passed through untouched.
pub async fn shape_for_agent(request: Request, next: Next) -> Response {
    let agent = request
        .extensions()
        .get::<CredentialContext>()
        .is_some_and(CredentialContext::is_sandbox_agent);
    let response = next.run(request).await;
    if agent {
        shaped(response).await
    } else {
        response
    }
}

/// Whether a response of `status`, answered by a route that authenticates the
/// request's `headers` itself, was answered to a sandbox agent token.
///
/// A presented `oxy_sbx_` token is the only credential such a route reads
/// (`oxy_auth::token::authenticate_request` takes a new-format token before a
/// session or a legacy key), so the request either authenticated with it or
/// was answered `401`.
fn answered_to_agent(headers: &HeaderMap, status: StatusCode) -> bool {
    status != StatusCode::UNAUTHORIZED && oxy_auth::token::presents_sandbox_agent(headers)
}

/// `status`, as a route that authenticates in its handler refuses with it
/// (`/fn`): the one shape for a sandbox agent token, the bare status for
/// anyone else.
pub fn status_response(headers: &HeaderMap, status: StatusCode) -> Response {
    if answered_to_agent(headers, status) {
        Refusal::of_status(status).into_response()
    } else {
        status.into_response()
    }
}

#[cfg(test)]
#[path = "custom_apps_agent_body_tests.rs"]
mod tests;
