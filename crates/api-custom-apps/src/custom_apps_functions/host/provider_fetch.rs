//! What the host reads off a mutating `ctx.fetch` before it is decided, and how
//! a call sent to a provider sandbox is recorded.
//!
//! `EnvPolicy::decide_on_fetch` decides from facts the URL does not show: the
//! API key the request sends (a Stripe test key admits a call to
//! `api.stripe.com`), whether the request carries a value `ctx.env` read
//! through staging's or production's shared fallback (`FallbackSecrets`), and
//! the workspace's registered QuickBooks sandbox companies. This module
//! gathers them ([`fetch_call_facts`], pure) and loads the companies once per
//! invocation, only for a QuickBooks sandbox call.
//!
//! **Nothing here is logged or serialized**: not a header, a body, the query
//! string or a secret value. A call that was sent is recorded as its method,
//! host, path and status only ([`ProjectFunctionHost::note_sent`]).

use std::collections::BTreeSet;

use base64::Engine as _;
use serde_json::{Map, Value};

use super::super::data_audit::{SENT_NOTE, SentCall};
use super::super::env_policy::{Decision, HostOp, Target};
pub(super) use super::fallback_secrets::FallbackSecrets;
use super::*;
use crate::custom_apps_planes::{SandboxAdmits, provider_sandbox};
use reqwest::Url;

/// What the host does with a `ctx.fetch`.
pub(super) enum FetchGate {
    /// Held: not sent; this is what the function is answered.
    Held(Value),
    /// Sent as asked: a read, an allowed upload, or any call in production.
    Send,
    /// Sent to a registered third-party sandbox, and recorded as sent.
    SendToSandbox,
}

/// The facts a call is decided on that its URL does not show.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct CallFacts {
    /// The API key the request sends, if it sends one in `Authorization`.
    pub(super) api_key: Option<String>,
    /// The request carries a value read through a fallback.
    pub(super) carries_fallback_secret: bool,
    /// The request sets a header that names a host of its own.
    pub(super) overrides_host: bool,
    /// The request has more than one `Authorization` header.
    pub(super) duplicate_authorization: bool,
}

/// How many headers are named `Authorization`, whatever the case.
fn authorization_count(headers: &Map<String, Value>) -> usize {
    headers
        .keys()
        .filter(|name| name.eq_ignore_ascii_case("authorization"))
        .count()
}

/// The `Authorization` header's value, when there is exactly one. Two names
/// that differ only in case are two headers on the wire, and which one the
/// provider reads is its choice.
fn authorization(headers: &Map<String, Value>) -> Option<&str> {
    if authorization_count(headers) != 1 {
        return None;
    }
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("authorization"))
        .and_then(|(_, value)| value.as_str())
}

/// The names of the headers that give a request a host of its own.
const HOST_OVERRIDES: [&str; 4] = ["host", ":authority", "x-forwarded-host", "forwarded"];

fn overrides_host(headers: &Map<String, Value>) -> bool {
    headers
        .keys()
        .any(|name| HOST_OVERRIDES.iter().any(|h| name.eq_ignore_ascii_case(h)))
}

/// A key as the checks read it: letters, digits and `_`, nothing else. That
/// rules out a comma, whitespace or a second scheme smuggled into one value.
fn is_plain_key(key: &str) -> bool {
    !key.is_empty() && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// `credentials` of a `<scheme> <credentials>` value, for `scheme`.
fn after_scheme<'a>(value: &'a str, scheme: &str) -> Option<&'a str> {
    let (head, rest) = value.trim().split_once(char::is_whitespace)?;
    head.eq_ignore_ascii_case(scheme).then(|| rest.trim())
}

/// A Basic credential decoded: `user:password`. `None` when it is not base64
/// of UTF-8.
fn basic_decoded(value: &str) -> Option<String> {
    let encoded = after_scheme(value, "basic")?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(encoded))
        .ok()?;
    String::from_utf8(bytes).ok()
}

/// The API key a request sends: an `Authorization: Bearer` token, or the user
/// of an `Authorization: Basic` credential (Stripe takes its key either way).
/// `None` for anything else: no header, more than one, another scheme, or a
/// key that is not [`is_plain_key`].
pub(super) fn api_key_of(headers: &Map<String, Value>) -> Option<String> {
    let value = authorization(headers)?;
    let key = match after_scheme(value, "bearer") {
        Some(token) => token.to_string(),
        None => basic_decoded(value)?
            .split(':')
            .next()
            .unwrap_or_default()
            .to_string(),
    };
    is_plain_key(&key).then_some(key)
}

/// Whether the request carries any of `secrets`: in a header value (as
/// written, percent-decoded, or a decoded Basic credential), the URL (as
/// written and percent-decoded) or a string body (as written, percent-decoded
/// or form-decoded). A match on a substring is deliberate: a false match
/// holds a call, it never sends one.
pub(super) fn carries_any(
    secrets: &FallbackSecrets,
    url: &Url,
    headers: &Map<String, Value>,
    body: Option<&str>,
) -> bool {
    if secrets.is_empty() {
        return false;
    }
    let in_headers = headers.values().filter_map(Value::as_str).any(|value| {
        secrets.found_in_percent_encoded(value)
            || basic_decoded(value).is_some_and(|c| secrets.found_in(&c))
    });
    in_headers
        || secrets.found_in_percent_encoded(url.as_str())
        || body.is_some_and(|b| secrets.found_in_form_encoded(b))
}

/// Everything `decide_on_fetch` needs that the URL alone does not give, read
/// from the `ctx.fetch` `init`. A body sent as base64 (`bodyEncoding`) is
/// checked as written and as the bytes it stands for.
pub(super) fn fetch_call_facts(secrets: &FallbackSecrets, url: &Url, init: &Value) -> CallFacts {
    let empty = Map::new();
    let headers = init
        .get("headers")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let body = init.get("body").and_then(Value::as_str);
    let mut carries = carries_any(secrets, url, headers, body);
    if !carries && !secrets.is_empty() {
        carries = decoded_body(init).is_some_and(|text| secrets.found_in_form_encoded(&text));
    }
    CallFacts {
        api_key: api_key_of(headers),
        carries_fallback_secret: carries,
        overrides_host: overrides_host(headers),
        duplicate_authorization: authorization_count(headers) > 1,
    }
}

/// The body's real bytes as text, when it was sent as base64.
fn decoded_body(init: &Value) -> Option<String> {
    let base64 = init
        .get("bodyEncoding")
        .and_then(Value::as_str)
        .is_some_and(|e| e.trim().eq_ignore_ascii_case("base64"));
    let body = init
        .get("body")
        .and_then(Value::as_str)
        .filter(|_| base64)?;
    super::decode_base64(body)
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

/// The empty set a call that needs no company is decided with.
static NO_COMPANIES: BTreeSet<String> = BTreeSet::new();

/// The registered QuickBooks sandbox companies of a read of the registry. A
/// failed read is the empty set: no company is registered, so the call is
/// held. The error is a database error and never a credential, so it is
/// logged.
pub(super) fn companies_from<E: std::fmt::Display>(
    read: Result<Vec<String>, E>,
) -> BTreeSet<String> {
    match read {
        Ok(realms) => realms.into_iter().collect(),
        Err(e) => {
            tracing::warn!(
                op = HostOp::Fetch.name(),
                error = %e,
                "the registered sandbox companies could not be read; \
                 a QuickBooks sandbox call is held"
            );
            BTreeSet::new()
        }
    }
}

impl ProjectFunctionHost {
    /// The sandbox companies the call is decided with: loaded once per
    /// invocation, and only for a call to a QuickBooks sandbox host that the
    /// environment lists and could send (its op-level decision isolates to a
    /// provider sandbox). Any other call is held or sent without them.
    pub(super) async fn companies_for(&self, url: &Url) -> &BTreeSet<String> {
        let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
        let quickbooks =
            provider_sandbox(&host).is_some_and(|s| s.admits == SandboxAdmits::QuickBooksCompany);
        let listed = self
            .policy
            .planes()
            .is_some_and(|planes| planes.fetch_hosts.contains(&host));
        let wanted = quickbooks
            && listed
            && self.policy.decide(HostOp::Fetch) == Decision::Isolate(Target::ProviderSandbox);
        if !wanted {
            return &NO_COMPANIES;
        }
        self.sandbox_companies
            .get_or_init(|| async {
                let read = oxy_app::surface::previews::list_sources(&self.db, self.project_id)
                    .await
                    .map(|items| items.into_iter().map(|i| i.overrides.realm_id).collect());
                companies_from(read)
            })
            .await
    }

    /// Record one call that was sent to a registered provider sandbox: its
    /// method, host, path and the status it answered (`None`: no response).
    /// Noted in the held-write log — beside the held calls — but not counted
    /// as held. Never the query, a header or the body.
    pub(super) async fn note_sent(&self, method: &str, url: &Url, status: Option<u16>) {
        let host = url.host_str().unwrap_or_default();
        tracing::info!(
            op = HostOp::Fetch.name(),
            environment = %self.policy.environment(),
            home = "provider_sandbox",
            host = host,
            "host call isolated: sent to a registered provider sandbox"
        );
        let record = WriteRecord {
            plane: "fetch",
            namespace: host.to_string(),
            verb: method.to_string(),
            table: String::new(),
            rows: None,
            statements: 1,
            op: Some(HostOp::Fetch.name()),
            note: Some(SENT_NOTE),
            sent: Some(SentCall::new(url.path(), status)),
        };
        let (trace_id, _) = Self::trace_context();
        self.held.note(record, trace_id).await;
    }
}

#[cfg(test)]
#[path = "provider_fetch_tests.rs"]
mod tests;
