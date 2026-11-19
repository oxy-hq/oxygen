//! Reading a registered token issuer's discovery document and keys.
//!
//! The URLs come from an org admin: the issuer (or discovery URL) they typed,
//! the `jwks_uri` its discovery document named, or a `jwks_uri` they gave. So every request goes through
//! `oxy_shared::http::public_only` — HTTPS, public addresses only (checked
//! again on every DNS answer), no redirects — with a short timeout and a cap
//! on what is read. A registered issuer cannot be pointed at the metadata
//! service, a pod in the cluster, or a loopback port.
//!
//! Two callers: registration (the discovery document, then the keys, once)
//! and the exchange's per-URL key caches
//! (`oxy_auth::workload_identity::keys`), which refresh only on an unknown
//! `kid` and at most once every 30 seconds. An issuer registered with its keys
//! inline is never fetched for at all.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use jsonwebtoken::jwk::JwkSet;
use oxy_auth::github_oidc::keys::FetchFuture;
use oxy_auth::workload_identity::keys;
use oxy_shared::http::public_only;
use serde_json::Value;

/// A stalled issuer must not hold a request open.
const FETCH_TIMEOUT: Duration = Duration::from_secs(5);

/// The most a discovery document or key set may be. EKS's are under 2 KB.
pub const MAX_BODY_BYTES: usize = 256 * 1024;

type Stub = Box<dyn Fn(&str) -> Option<Value> + Send + Sync>;

static STUB: OnceLock<Stub> = OnceLock::new();

/// Answer every issuer fetch from `stub` instead of the network: `Some` is
/// the JSON at that URL, `None` an unreachable one. For integration tests,
/// which stand a locally generated issuer in for a cluster. First call wins.
#[doc(hidden)]
pub fn stub_for_tests(stub: impl Fn(&str) -> Option<Value> + Send + Sync + 'static) -> bool {
    STUB.set(Box::new(stub)).is_ok()
}

fn client() -> Option<&'static reqwest::Client> {
    static CLIENT: OnceLock<Option<reqwest::Client>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            public_only::builder()
                .timeout(FETCH_TIMEOUT)
                .build()
                .map_err(|e| tracing::error!(error = %e, "oidc issuer: no http client"))
                .ok()
        })
        .as_ref()
}

/// The JSON at `url`, or why not — in words an org admin can act on.
pub async fn fetch_json(url: &str) -> Result<Value, String> {
    if let Some(stub) = STUB.get() {
        return stub(url).ok_or_else(|| format!("{url} could not be read"));
    }
    let parsed = reqwest::Url::parse(url).map_err(|_| format!("{url} is not a URL"))?;
    if !public_only::is_safe_outbound(&parsed) {
        return Err(format!("{url} is not a public HTTPS address"));
    }
    let client = client().ok_or_else(|| "no http client".to_string())?;
    let unreadable = |e: reqwest::Error| format!("{url} could not be read: {e}");
    let mut response = client
        .get(parsed)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(unreadable)?
        .error_for_status()
        .map_err(unreadable)?;
    let mut body: Vec<u8> = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(unreadable)? {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_BODY_BYTES {
            return Err(format!("{url} answered more than {MAX_BODY_BYTES} bytes"));
        }
    }
    serde_json::from_slice(&body).map_err(|_| format!("{url} did not answer JSON"))
}

/// The key set at `jwks_uri`, for a key cache. Every failure is the same
/// "unavailable" to the cache, and logged here.
fn fetch_keys(jwks_uri: String) -> FetchFuture {
    Box::pin(async move {
        let value = fetch_json(&jwks_uri).await.map_err(|reason| {
            tracing::warn!(%reason, "oidc issuer: could not read an issuer's keys");
        })?;
        serde_json::from_value::<JwkSet>(value).map_err(|e| {
            tracing::warn!(error = %e, %jwks_uri, "oidc issuer: not a key set");
        })
    })
}

/// Make the exchange's key caches read through [`fetch_json`]. Idempotent:
/// the first install in a process wins.
pub fn install_key_fetcher() {
    keys::install_fetcher(Arc::new(fetch_keys));
}
