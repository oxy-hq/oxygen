//! GitHub's OIDC signing keys, for this process.
//!
//! One cache behind both exchanges (`oxy_auth::github_oidc`): the legacy
//! trusted-publishing exchange and trusted access. The cache's policy — refresh
//! only on an unknown `kid`, at most once per 30 s, keep the keys held when a
//! refresh fails — is `oxy_auth::github_oidc::keys`; this file is the fetch it
//! is handed, because that crate holds no HTTP client.
//!
//! The URL is a constant. Nothing a request carries reaches it, so there is
//! nothing here for a caller to point elsewhere; redirects are not followed.

use std::time::Duration;

use jsonwebtoken::jwk::JwkSet;
use oxy_auth::github_oidc::keys::{self, FetchFuture};
use oxy_auth::github_oidc::{GITHUB_JWKS_URL, JwksCache};

/// A stalled fetch must not hold an exchange open: the caller is a CI job
/// that will retry.
const FETCH_TIMEOUT: Duration = Duration::from_secs(5);

async fn fetch_jwks() -> Result<JwkSet, reqwest::Error> {
    reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()?
        .get(GITHUB_JWKS_URL)
        .send()
        .await?
        .error_for_status()?
        .json::<JwkSet>()
        .await
}

fn fetch() -> FetchFuture {
    Box::pin(async {
        fetch_jwks().await.map_err(|e| {
            tracing::warn!(error = %e, "github oidc: could not read the JWKS");
        })
    })
}

/// The process's key cache, reading GitHub's JWKS on first use.
pub fn github_keys() -> &'static JwksCache {
    keys::global_or(|| JwksCache::new(fetch))
}

/// Make this process trust exactly `keys`, and never read GitHub's. For
/// integration tests, which sign their own tokens; it must run before the
/// first exchange, and returns `false` when a cache was already in place.
#[doc(hidden)]
pub fn install_keys_for_tests(keys: JwkSet) -> bool {
    keys::install(JwksCache::fixed(keys))
}
