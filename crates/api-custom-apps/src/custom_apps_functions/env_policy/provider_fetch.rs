//! The `ctx.fetch` a non-production run sends to a third party's sandbox: the
//! per-call checks behind `Isolate(Target::ProviderSandbox)`.
//!
//! An environment whose `planes.fetch` is `allow` lists provider sandbox hosts
//! (`custom_apps_planes::PROVIDER_SANDBOXES`). The list says which hosts a
//! call *may* reach, not that every call to one does: a provider can serve
//! test and live traffic from one host, and a function can put a production
//! secret in a request it sends to a sandbox. So [`provider_refusal`] decides
//! each call from the call itself, and a call that fails a check is held with
//! the check's note. In this order:
//!
//! - **plain HTTPS** — scheme `https`, the default port, no userinfo. The
//!   listed host is what the request really reaches, and `http://`, `:8443`
//!   or `user@host` are other places by another route;
//! - **listed** — the host is in the environment's allow list, compared
//!   lowercased. Nothing a function names is sent that the environment did not;
//! - **still registered** — the host is in the static registry *now*. A host
//!   dropped from it since the setting was saved stops being sent to;
//! - **its own Host** — the request sets no `Host`, `:authority`,
//!   `x-forwarded-host` or `forwarded` header. The listed host is what the
//!   decision is made on, so a header that names another one would send the
//!   call somewhere the checks never saw;
//! - **sandbox, not live** — what the registry row says the call must show:
//!   a Stripe test key (`sk_test_` / `rk_test_`; one host serves both modes),
//!   or a QuickBooks company registered for workspace previews (a path of
//!   `/v3/company/<realm>/…` whose realm is a sandbox company); a host that
//!   serves no production traffic needs nothing more;
//! - **one Authorization** — at most one `Authorization` header, so the key
//!   the checks read is the key the provider reads;
//! - **its own credential** — the request carries no value `ctx.env` read
//!   through staging's or production's shared keys. A sandbox call made with
//!   production's secret is production's, whoever it is sent to.
//!
//! This only ever narrows: `EnvPolicy::decide_on_fetch` keeps an op-level
//! `Isolate(ProviderSandbox)` when the call passes and holds it otherwise, and
//! never answers `Allow`.

use std::collections::BTreeSet;

use super::MintedUpload;
use crate::custom_apps_planes::{SandboxAdmits, provider_sandbox};

/// The call is not `https` on the default port, or names userinfo.
pub const NOTE_NOT_PLAIN_HTTPS: &str =
    "the request is not plain HTTPS on the default port without a username or password";
/// The host is not in the environment's `fetch` allow list.
pub const NOTE_NOT_LISTED: &str = "the host is not in this environment's fetch allow-list";
/// The listed host is no longer in the registry of provider sandboxes.
pub const NOTE_NOT_REGISTERED: &str = "the host is no longer a registered provider sandbox";
/// A Stripe call whose key is not a test key.
pub const NOTE_NO_TEST_KEY: &str =
    "the request does not use a Stripe test key (sk_test_ or rk_test_)";
/// A QuickBooks call that does not name a registered sandbox company.
pub const NOTE_UNREGISTERED_COMPANY: &str =
    "the QuickBooks company is not a sandbox company registered for workspace previews";
/// The request sets a header that names a host of its own.
pub const NOTE_HOST_OVERRIDE: &str = "a sandbox call may not set its own Host header";
/// The request has more than one `Authorization` header.
pub const NOTE_DUPLICATE_AUTHORIZATION: &str =
    "a sandbox call may carry only one Authorization header";
/// The request carries a value read through a fallback.
pub const NOTE_FALLBACK_CREDENTIAL: &str = "the request carries a secret this environment read \
     from staging's or production's shared keys; give the environment its own value";

/// The prefixes of a Stripe test-mode secret or restricted key.
const STRIPE_TEST_KEYS: [&str; 2] = ["sk_test_", "rk_test_"];

/// One `ctx.fetch` as the policy decides it: the request, what this
/// invocation minted, and the facts the host read from the request that the
/// URL does not show.
#[derive(Clone, Copy, Debug)]
pub struct FetchCall<'a> {
    pub method: &'a str,
    pub url: &'a url::Url,
    pub minted: &'a [MintedUpload],
    /// The request carries a value `ctx.env` read through a fallback.
    pub carries_fallback_secret: bool,
    /// The API key the request sends: an `Authorization: Bearer` token, or a
    /// Basic credential's user. `None` when there is more than one
    /// `Authorization` header or the token is not a plain one.
    pub api_key: Option<&'a str>,
    /// The request sets a `Host`, `:authority`, `x-forwarded-host` or
    /// `forwarded` header.
    pub overrides_host: bool,
    /// The request has more than one `Authorization` header.
    pub duplicate_authorization: bool,
    /// Realm ids of the workspace's registered QuickBooks sandbox companies
    /// (`previews::sources`).
    pub sandbox_companies: &'a BTreeSet<String>,
}

/// `None` when `call` may be sent to a provider sandbox of `listed` hosts,
/// else the note it is held with. See the module docs for each check.
pub fn provider_refusal(listed: &BTreeSet<String>, call: &FetchCall) -> Option<&'static str> {
    let url = call.url;
    if url.scheme() != "https"
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Some(NOTE_NOT_PLAIN_HTTPS);
    }
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    if !listed.contains(&host) {
        return Some(NOTE_NOT_LISTED);
    }
    let Some(sandbox) = provider_sandbox(&host) else {
        return Some(NOTE_NOT_REGISTERED);
    };
    if call.overrides_host {
        return Some(NOTE_HOST_OVERRIDE);
    }
    let admitted = match sandbox.admits {
        SandboxAdmits::Host => None,
        SandboxAdmits::StripeTestKey => stripe_refusal(call.api_key),
        SandboxAdmits::QuickBooksCompany => company_refusal(call),
    };
    admitted
        .or(call
            .duplicate_authorization
            .then_some(NOTE_DUPLICATE_AUTHORIZATION))
        .or(call
            .carries_fallback_secret
            .then_some(NOTE_FALLBACK_CREDENTIAL))
}

fn stripe_refusal(api_key: Option<&str>) -> Option<&'static str> {
    let test = api_key.is_some_and(|key| STRIPE_TEST_KEYS.iter().any(|p| key.starts_with(p)));
    (!test).then_some(NOTE_NO_TEST_KEY)
}

/// The path must be `/v3/company/<realm>/…` with `<realm>` registered.
fn company_refusal(call: &FetchCall) -> Option<&'static str> {
    let Some(mut segments) = call.url.path_segments() else {
        return Some(NOTE_UNREGISTERED_COMPANY);
    };
    let registered = match (segments.next(), segments.next(), segments.next()) {
        (Some("v3"), Some("company"), Some(realm)) => call.sandbox_companies.contains(realm),
        _ => false,
    };
    (!registered).then_some(NOTE_UNREGISTERED_COMPANY)
}

#[cfg(test)]
#[path = "provider_fetch_tests.rs"]
mod tests;
